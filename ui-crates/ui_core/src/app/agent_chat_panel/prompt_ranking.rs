use agent_chat_core::{ChatMessage, ChatRole, ToolCall};

#[derive(Clone, Debug)]
struct PromptBlock {
    messages: Vec<ChatMessage>,
    start_index: usize,
    chars: usize,
    priority: i32,
    is_mandatory: bool,
}

fn message_chars(message: &ChatMessage) -> usize {
    message.content.chars().count()
}

fn block_has_tool_chain(messages: &[ChatMessage]) -> bool {
    messages
        .iter()
        .any(|message| !message.tool_calls.is_empty() || message.role == ChatRole::Tool)
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
        .find(|message| !message.content.trim().is_empty())
        .map(|message| message.role)
        .unwrap_or(ChatRole::Assistant);

    priority += match first_role {
        ChatRole::System => 900,
        ChatRole::User => 600,
        ChatRole::Assistant => 450,
        ChatRole::Tool => 500,
    };

    if block_has_tool_chain(&block.messages) {
        priority += 180;
    }

    if block
        .messages
        .iter()
        .any(|message| message.role == ChatRole::Assistant && !message.tool_calls.is_empty())
    {
        priority += 120;
    }

    if block
        .messages
        .iter()
        .any(|message| message.content.contains("Subagent"))
    {
        priority += 80;
    }

    priority + (block.chars.min(4_000) as i32 / 40)
}

fn split_into_blocks(messages: Vec<ChatMessage>) -> Vec<PromptBlock> {
    let mut blocks = Vec::new();
    let mut current_messages: Vec<ChatMessage> = Vec::new();
    let mut current_start_index = 0usize;

    for (index, message) in messages.into_iter().enumerate() {
        let starts_new_block = match message.role {
            ChatRole::System => !current_messages.is_empty(),
            ChatRole::User => !current_messages.is_empty(),
            _ => current_messages.is_empty() && !blocks.is_empty(),
        };

        if starts_new_block {
            let chars = current_messages.iter().map(message_chars).sum();
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

        current_messages.push(message);
    }

    if !current_messages.is_empty() {
        let chars = current_messages.iter().map(message_chars).sum();
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
    messages: &[ChatMessage],
    max_chars: usize,
) -> (Vec<ChatMessage>, Option<Vec<ChatMessage>>) {
    let total_chars: usize = messages.iter().map(message_chars).sum();
    if total_chars <= max_chars {
        return (messages.to_vec(), None);
    }

    let mut kept_reversed = Vec::new();
    let mut kept_chars = 0usize;
    let mut skip_until_assistant_with_calls = false;

    for message in messages.iter().rev() {
        let len = message_chars(message);
        let fits = kept_reversed.is_empty() || kept_chars + len <= max_chars;

        if message.role == ChatRole::Tool {
            skip_until_assistant_with_calls = true;
            kept_chars += len;
            kept_reversed.push(message.clone());
        } else if message.role == ChatRole::Assistant && !message.tool_calls.is_empty() {
            skip_until_assistant_with_calls = false;
            kept_chars += len;
            kept_reversed.push(message.clone());
        } else if skip_until_assistant_with_calls {
            kept_chars += len;
            kept_reversed.push(message.clone());
        } else if fits {
            kept_chars += len;
            kept_reversed.push(message.clone());
        } else {
            break;
        }
    }

    kept_reversed.reverse();
    let dropped_count = messages.len().saturating_sub(kept_reversed.len());
    (kept_reversed, Some(messages[..dropped_count].to_vec()))
}

pub(crate) fn compact_messages(
    messages: Vec<ChatMessage>,
    max_chars: usize,
    summary_budget: usize,
) -> (Vec<ChatMessage>, Option<Vec<ChatMessage>>) {
    let total_chars: usize = messages.iter().map(message_chars).sum();
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
        if block
            .messages
            .iter()
            .all(|message| message.role == ChatRole::System)
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

    let system_messages: Vec<ChatMessage> = system_blocks
        .iter()
        .flat_map(|block| block.messages.clone())
        .collect();

    if system_chars >= max_chars || dialog_blocks.is_empty() {
        return (system_messages, None);
    }

    let dialog_block_count = dialog_blocks.len();
    let last_index = dialog_block_count - 1;
    for index in 0..dialog_block_count {
        let is_mandatory = index == last_index;
        let priority = {
            let block = &dialog_blocks[index];
            block_priority(block, index, dialog_block_count)
        };
        let block = &mut dialog_blocks[index];
        block.is_mandatory = is_mandatory;
        block.priority = priority;
    }

    let mandatory_block = dialog_blocks.pop().unwrap();
    let (mandatory_messages, mandatory_dropped) =
        shrink_suffix_to_budget(&mandatory_block.messages, dialog_budget);
    let mandatory_chars: usize = mandatory_messages.iter().map(message_chars).sum();

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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn keeps_recent_turns_and_prefers_tool_heavy_context() {
        let messages = vec![
            msg(ChatRole::System, "system prompt"),
            msg(ChatRole::User, "old user question"),
            msg(ChatRole::Assistant, "old answer"),
            ChatMessage {
                role: ChatRole::Assistant,
                content: "spawned a subagent".to_string(),
                tool_call_id: None,
                tool_calls: vec![tool_call()],
            },
            ChatMessage {
                role: ChatRole::Tool,
                content: "{\"ok\":true}".to_string(),
                tool_call_id: Some("call-1".to_string()),
                tool_calls: vec![],
            },
            msg(ChatRole::Assistant, "subagent result summarized here"),
            msg(ChatRole::User, "current request that must stay"),
            msg(ChatRole::Assistant, "current answer"),
        ];

        let (compacted, dropped) = compact_messages(messages, 160, 40);
        assert!(dropped.is_some());
        assert!(compacted.iter().any(|m| m.content.contains("current request")));
        assert!(compacted.iter().any(|m| m.content.contains("subagent result")));
        assert!(!compacted.iter().any(|m| m.content.contains("old user question")));
    }
}