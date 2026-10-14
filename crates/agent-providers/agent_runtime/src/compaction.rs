//! Context compaction. The ranking logic is the one that used to live in the
//! panel (`prompt_ranking.rs`), now working on projected messages that
//! remember which ledger entries they came from.

use std::sync::Arc;

use agent_chat_core::{ChatMessage, ChatProvider, ChatRequest, ChatRole};

use crate::ledger::{Ledger, LedgerEntry};
use crate::profile::ModelProfile;
use crate::project::project_indexed;

/// A message plus the index it had before compaction.
#[derive(Clone, Debug)]
struct Tagged {
    origin: usize,
    message: ChatMessage,
}

#[derive(Clone, Debug)]
struct PromptBlock {
    messages: Vec<Tagged>,
    start_index: usize,
    chars: usize,
    priority: i32,
    is_mandatory: bool,
}

fn message_chars(message: &ChatMessage) -> usize {
    message.content.chars().count()
}

fn tagged_chars(item: &Tagged) -> usize {
    message_chars(&item.message)
}

fn block_has_tool_chain(messages: &[Tagged]) -> bool {
    messages
        .iter()
        .any(|item| !item.message.tool_calls.is_empty() || item.message.role == ChatRole::Tool)
}

fn block_priority(block: &PromptBlock, block_index: usize, block_count: usize) -> i32 {
    let mut priority = 0i32;

    if block.is_mandatory {
        priority += 1_000;
    }

    let recency_bonus = if block_count <= 1 {
        200
    } else {
        ((block_index as i32) * 200) / ((block_count as i32) - 1)
    };
    priority += recency_bonus;

    let first_role = block
        .messages
        .iter()
        .find(|item| !item.message.content.trim().is_empty())
        .map(|item| item.message.role)
        .unwrap_or(ChatRole::Assistant);

    priority += match first_role {
        ChatRole::System => 900,
        ChatRole::User => 600,
        ChatRole::Tool => 500,
        ChatRole::Assistant => 450,
    };

    if block_has_tool_chain(&block.messages) {
        priority += 180;
    }

    if block
        .messages
        .iter()
        .any(|item| item.message.role == ChatRole::Assistant && !item.message.tool_calls.is_empty())
    {
        priority += 120;
    }

    // Sub-agent reports (see `project::report_envelope`) outrank plain turns.
    if block.messages.iter().any(|item| {
        item.message.content.contains("<subagent_report") || item.message.content.contains("Subagent")
    }) {
        priority += 80;
    }

    priority + (block.chars.min(4_000) as i32 / 40)
}

fn split_into_blocks(messages: Vec<Tagged>) -> Vec<PromptBlock> {
    let mut blocks = Vec::new();
    let mut current_messages: Vec<Tagged> = Vec::new();
    let mut current_start_index = 0usize;

    for (index, item) in messages.into_iter().enumerate() {
        let starts_new_block = match item.message.role {
            ChatRole::System | ChatRole::User => !current_messages.is_empty(),
            _ => current_messages.is_empty() && !blocks.is_empty(),
        };

        if starts_new_block {
            let chars = current_messages.iter().map(tagged_chars).sum();
            blocks.push(PromptBlock {
                messages: std::mem::take(&mut current_messages),
                start_index: current_start_index,
                chars,
                priority: 0,
                is_mandatory: false,
            });
        }

        if current_messages.is_empty() {
            current_start_index = index;
        }

        current_messages.push(item);
    }

    if !current_messages.is_empty() {
        let chars = current_messages.iter().map(tagged_chars).sum();
        blocks.push(PromptBlock {
            messages: current_messages,
            start_index: current_start_index,
            chars,
            priority: 0,
            is_mandatory: false,
        });
    }

    blocks
}

fn shrink_suffix_to_budget(
    messages: &[Tagged],
    max_chars: usize,
) -> (Vec<Tagged>, Option<Vec<Tagged>>) {
    let total_chars: usize = messages.iter().map(tagged_chars).sum();
    if total_chars <= max_chars {
        return (messages.to_vec(), None);
    }

    let mut kept_reversed = Vec::new();
    let mut kept_chars = 0usize;
    let mut skip_until_assistant_with_calls = false;

    for item in messages.iter().rev() {
        let len = tagged_chars(item);
        let fits = kept_reversed.is_empty() || kept_chars + len <= max_chars;

        if item.message.role == ChatRole::Tool {
            skip_until_assistant_with_calls = true;
            kept_chars += len;
            kept_reversed.push(item.clone());
        } else if item.message.role == ChatRole::Assistant && !item.message.tool_calls.is_empty() {
            skip_until_assistant_with_calls = false;
            kept_chars += len;
            kept_reversed.push(item.clone());
        } else if skip_until_assistant_with_calls {
            kept_chars += len;
            kept_reversed.push(item.clone());
        } else if fits {
            kept_chars += len;
            kept_reversed.push(item.clone());
        } else {
            break;
        }
    }

    kept_reversed.reverse();
    let dropped_count = messages.len().saturating_sub(kept_reversed.len());
    (kept_reversed, Some(messages[..dropped_count].to_vec()))
}

/// Returns `(kept, dropped)`; `dropped` is `None` when everything fit.
fn compact_tagged(
    messages: Vec<Tagged>,
    max_chars: usize,
    summary_budget: usize,
) -> (Vec<Tagged>, Option<Vec<Tagged>>) {
    let total_chars: usize = messages.iter().map(tagged_chars).sum();
    if total_chars <= max_chars {
        return (messages, None);
    }

    let mut blocks = split_into_blocks(messages);
    if blocks.is_empty() {
        return (Vec::new(), None);
    }

    let mut system_blocks = Vec::new();
    let mut dialog_blocks = Vec::new();
    for block in blocks.drain(..) {
        // Pure System blocks are pinned and never dropped.
        if block
            .messages
            .iter()
            .all(|item| item.message.role == ChatRole::System)
        {
            system_blocks.push(block);
        } else {
            dialog_blocks.push(block);
        }
    }

    let system_chars: usize = system_blocks.iter().map(|block| block.chars).sum();
    let dialog_budget = max_chars
        .saturating_sub(system_chars)
        .saturating_sub(summary_budget)
        .max(1_500);

    let system_messages: Vec<Tagged> = system_blocks
        .iter()
        .flat_map(|block| block.messages.clone())
        .collect();

    if system_chars >= max_chars || dialog_blocks.is_empty() {
        return (system_messages, None);
    }

    let dialog_block_count = dialog_blocks.len();
    let last_index = dialog_block_count - 1;
    // The first block that contains a user message is the original goal — protect it
    // from compaction so the agent never loses track of what was asked.
    let first_user_block = dialog_blocks
        .iter()
        .position(|b| b.messages.iter().any(|m| m.message.role == ChatRole::User));
    for index in 0..dialog_block_count {
        let is_mandatory = index == last_index || Some(index) == first_user_block;
        let block = &mut dialog_blocks[index];
        block.is_mandatory = is_mandatory;
        // Mandatory status is part of the ranking score. Set it before
        // computing priority so the original user goal receives the same
        // protection as the latest request.
        block.priority = block_priority(block, index, dialog_block_count);
    }

    let mandatory_block = dialog_blocks.pop().unwrap();
    let (mandatory_messages, mandatory_dropped) =
        shrink_suffix_to_budget(&mandatory_block.messages, dialog_budget);
    let mandatory_chars: usize = mandatory_messages.iter().map(tagged_chars).sum();

    let mut selected_blocks: Vec<PromptBlock> = vec![PromptBlock {
        messages: mandatory_messages,
        start_index: mandatory_block.start_index,
        chars: mandatory_chars,
        priority: mandatory_block.priority,
        is_mandatory: true,
    }];
    let mut selected_dialog_chars = mandatory_chars;
    let mut dropped_messages = mandatory_dropped.unwrap_or_default();

    let mut ranked_remaining = dialog_blocks;
    ranked_remaining.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then_with(|| b.start_index.cmp(&a.start_index))
    });

    for block in ranked_remaining {
        if selected_dialog_chars + block.chars <= dialog_budget {
            selected_dialog_chars += block.chars;
            selected_blocks.push(block);
        } else {
            dropped_messages.extend(block.messages);
        }
    }

    selected_blocks.sort_by(|a, b| a.start_index.cmp(&b.start_index));

    let mut compacted = system_messages;
    for block in selected_blocks {
        compacted.extend(block.messages);
    }

    if dropped_messages.is_empty() {
        (compacted, None)
    } else {
        (compacted, Some(dropped_messages))
    }
}

/// Compact a flat message list. Kept for callers that have no ledger.
pub fn compact_messages(
    messages: Vec<ChatMessage>,
    max_chars: usize,
    summary_budget: usize,
) -> (Vec<ChatMessage>, Option<Vec<ChatMessage>>) {
    let tagged = messages
        .into_iter()
        .enumerate()
        .map(|(origin, message)| Tagged { origin, message })
        .collect();
    let (kept, dropped) = compact_tagged(tagged, max_chars, summary_budget);
    (
        kept.into_iter().map(|item| item.message).collect(),
        dropped.map(|items| items.into_iter().map(|item| item.message).collect()),
    )
}

/// What `compact_ledger` removed.
pub struct Compaction {
    /// The projected messages that were dropped, for summarising.
    pub dropped: Vec<ChatMessage>,
    /// Span of ledger indexes the dropped messages came from.
    pub covers: std::ops::Range<usize>,
}

/// Drop entries from `ledger` until its projection fits `max_chars`.
/// Returns `None` when it already fits or when there is nothing to summarise.
pub fn compact_ledger(
    ledger: &mut Ledger,
    profile: &ModelProfile,
    max_chars: usize,
    summary_budget: usize,
) -> Option<Compaction> {
    let projected = project_indexed(ledger, profile);
    let entry_lists: Vec<Vec<usize>> = projected.iter().map(|p| p.entries.clone()).collect();
    let tagged: Vec<Tagged> = projected
        .into_iter()
        .enumerate()
        .map(|(origin, p)| Tagged {
            origin,
            message: p.message,
        })
        .collect();
    let total_messages = tagged.len();

    let (kept, dropped) = compact_tagged(tagged, max_chars, summary_budget);
    let kept_everything = kept.len() == total_messages && dropped.is_none();
    if kept_everything {
        return None;
    }

    // A kept message keeps all its entries; every other entry that came
    // from a projected message is dropped.
    let mut keep_entry = vec![false; ledger.entries.len()];
    let mut mapped = vec![false; ledger.entries.len()];
    for entries in &entry_lists {
        for &ix in entries {
            mapped[ix] = true;
        }
    }
    for item in &kept {
        for &ix in &entry_lists[item.origin] {
            keep_entry[ix] = true;
        }
    }
    // Entries that never reached the model (empty turns) stay as they were.
    for (ix, was_mapped) in mapped.iter().enumerate() {
        if !was_mapped {
            keep_entry[ix] = true;
        }
    }

    let dropped_entries: Vec<usize> = (0..keep_entry.len())
        .filter(|&ix| !keep_entry[ix])
        .collect();

    let mut index = 0usize;
    ledger.entries.retain(|_| {
        let keep = keep_entry[index];
        index += 1;
        keep
    });

    // When only the system block survives, the ledger is cut without a
    // summary, exactly as the panel did.
    let dropped = dropped?;
    let covers = match (dropped_entries.first(), dropped_entries.last()) {
        (Some(&first), Some(&last)) => first..last + 1,
        _ => 0..0,
    };
    Some(Compaction {
        dropped: dropped.into_iter().map(|item| item.message).collect(),
        covers,
    })
}

/// Fold a compaction into the ledger as a `Summary` entry.
pub fn summarize_compaction(
    ledger: &mut Ledger,
    compaction: &Compaction,
    provider: &Arc<dyn ChatProvider>,
    compact_model: &str,
) -> String {
    let summary = summarize_dropped(&compaction.dropped, provider, compact_model);
    ledger.entries.push(LedgerEntry::summary(
        summary.clone(),
        compaction.covers.clone(),
    ));
    summary
}

/// Ask `compact_model` for a short summary of `dropped`; falls back to a
/// bulleted excerpt when the call fails.
pub fn summarize_dropped(
    dropped: &[ChatMessage],
    provider: &Arc<dyn ChatProvider>,
    compact_model: &str,
) -> String {
    let formatted: String = dropped
        .iter()
        .take(40)
        .map(|m| {
            let role = match m.role {
                ChatRole::User => "User",
                ChatRole::Assistant => "AI",
                ChatRole::Tool => "Tool",
                ChatRole::System => "System",
            };
            let snippet: String = m.content.chars().take(600).collect();
            format!("[{role}]: {snippet}")
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    let summary_request = ChatRequest {
        model: compact_model.to_string(),
        messages: vec![
            ChatMessage {
                role: ChatRole::System,
                content: "You are a conversation summarizer. Given a list of messages, \
                          produce a concise summary (max 250 words) that captures: \
                          key decisions, important findings, files or tools used, \
                          and any critical context needed to continue the conversation. \
                          Write in past tense. Be specific, not vague."
                    .to_string(),
                tool_call_id: None,
                tool_calls: vec![],
            },
            ChatMessage {
                role: ChatRole::User,
                content: format!("Summarize these earlier conversation messages:\n\n{formatted}"),
                tool_call_id: None,
                tool_calls: vec![],
            },
        ],
        enable_tool_calls: false,
        tools: vec![],
        temperature: Some(0.3),
        top_p: Some(1.0),
        max_tokens: Some(350),
    };

    provider
        .chat(summary_request)
        .ok()
        .and_then(|r| r.assistant_message)
        .unwrap_or_else(|| {
            dropped
                .iter()
                .take(12)
                .map(|m| {
                    let role = match m.role {
                        ChatRole::User => "user",
                        ChatRole::Assistant => "assistant",
                        _ => "system",
                    };
                    let snippet: String = m.content.replace('\n', " ").chars().take(180).collect();
                    format!("- {role}: {snippet}")
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::EntryKind;
    use agent_chat_core::ToolCall;

    fn msg(role: ChatRole, content: &str) -> ChatMessage {
        ChatMessage {
            role,
            content: content.to_string(),
            tool_call_id: None,
            tool_calls: vec![],
        }
    }

    fn tool_call() -> ToolCall {
        ToolCall {
            id: "call-1".to_string(),
            name: "get_subagent_result".to_string(),
            arguments_json: serde_json::json!({"subagent_id": "subagent-1"}),
        }
    }

    #[test]
    fn protects_goals_and_prefers_tool_heavy_context() {
        let messages = vec![
            msg(ChatRole::System, "system prompt"),
            msg(
                ChatRole::User,
                &format!("ORIGINAL_GOAL: {}", "g".repeat(350)),
            ),
            msg(
                ChatRole::Assistant,
                &format!("ORIGINAL_RESPONSE: {}", "r".repeat(150)),
            ),
            msg(
                ChatRole::User,
                &format!("ORDINARY_MIDDLE_TURN: {}", "m".repeat(100)),
            ),
            msg(
                ChatRole::Assistant,
                &format!("ORDINARY_MIDDLE_RESPONSE: {}", "n".repeat(800)),
            ),
            msg(
                ChatRole::User,
                &format!(
                    "<subagent_report id=\"s\" status=\"success\" trust=\"data\">Subagent result: {}</subagent_report>",
                    "s".repeat(350)
                ),
            ),
            ChatMessage {
                role: ChatRole::Assistant,
                content: "tool-backed agent synthesis".to_string(),
                tool_call_id: None,
                tool_calls: vec![tool_call()],
            },
            ChatMessage {
                role: ChatRole::Tool,
                content: "{\"ok\":true}".to_string(),
                tool_call_id: Some("call-1".to_string()),
                tool_calls: vec![],
            },
            msg(ChatRole::Assistant, "tool result incorporated"),
            msg(
                ChatRole::User,
                &format!("CURRENT_REQUEST: {}", "c".repeat(150)),
            ),
            msg(
                ChatRole::Assistant,
                &format!("CURRENT_RESPONSE: {}", "a".repeat(100)),
            ),
        ];

        let (compacted, dropped) = compact_messages(messages, 2_200, 200);
        let dropped = dropped.expect("the ordinary middle turn should exceed the budget");

        assert!(compacted
            .iter()
            .any(|message| message.content.contains("ORIGINAL_GOAL")));
        assert!(compacted
            .iter()
            .any(|message| message.content.contains("CURRENT_REQUEST")));
        assert!(compacted
            .iter()
            .any(|message| message.content.contains("Subagent result")));
        assert!(!compacted
            .iter()
            .any(|message| message.content.contains("ORDINARY_MIDDLE_TURN")));
        assert!(dropped
            .iter()
            .any(|message| message.content.contains("ORDINARY_MIDDLE_TURN")));
    }

    #[test]
    fn compact_ledger_drops_whole_entries_and_keeps_tool_chains_intact() {
        let mut ledger = Ledger::with_system_prompt("system prompt");
        ledger.push(LedgerEntry::user(format!("GOAL {}", "g".repeat(200))));
        ledger.push(LedgerEntry::assistant("ok"));
        ledger.push(LedgerEntry::user(format!("MIDDLE {}", "m".repeat(2_000))));
        ledger.push(LedgerEntry::assistant(format!("MIDDLE_REPLY {}", "n".repeat(2_000))));
        ledger.push(LedgerEntry::user("CURRENT"));
        ledger.push(LedgerEntry::assistant("calling"));
        ledger.push(LedgerEntry::tool_exchange(
            tool_call(),
            crate::ledger::ToolResult {
                content: "{\"ok\":true}".to_string(),
                is_error: false,
            },
        ));

        let profile = ModelProfile::default();
        let compaction = compact_ledger(&mut ledger, &profile, 2_000, 200)
            .expect("the middle turn does not fit");
        assert!(compaction
            .dropped
            .iter()
            .any(|m| m.content.contains("MIDDLE")));

        let texts: Vec<String> = ledger
            .entries
            .iter()
            .filter_map(|e| match &e.kind {
                EntryKind::UserTurn { text } | EntryKind::AssistantTurn { text } => {
                    Some(text.clone())
                }
                _ => None,
            })
            .collect();
        assert!(texts.iter().any(|t| t.starts_with("GOAL")));
        assert!(texts.iter().any(|t| t == "CURRENT"));
        assert!(!texts.iter().any(|t| t.starts_with("MIDDLE")));
        // The tool exchange survives together with the assistant turn that
        // carries its call.
        let projected = crate::project::project(&ledger, &profile);
        let call_msg = projected
            .iter()
            .position(|m| !m.tool_calls.is_empty())
            .expect("assistant message with the call");
        assert_eq!(projected[call_msg + 1].role, ChatRole::Tool);
    }
}
