//! Folding compacted entries into a single running summary.
//!
//! `compact_ledger` leaves summaries alone (they are system content), so
//! without merging every compaction would add another one and a small window
//! would slowly fill with summaries. [`summarize_into_ledger`] folds the old
//! summaries into the new one.
//!
//! The pinned ledger is not summarised: it stays verbatim in the system prompt
//! (see `project`), compaction never drops it, and the structured summary is
//! told about it only so it does not contradict or restate it.

use std::sync::Arc;

use agent_chat_core::{ChatMessage, ChatProvider, ChatRequest, ChatRole};

use crate::compaction::Compaction;
use crate::ledger::{EntryKind, Ledger, LedgerEntry};

/// How the summary is written.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SummaryStyle {
    /// The prose summary the panel always used.
    #[default]
    Plain,
    /// Task / State / Next step, plus a list of what was dropped. Used when
    /// the steward is on.
    Structured,
}

/// Summarise `compaction.dropped` together with any earlier summaries, replace
/// those summaries with the new one and return its text. `max_chars` bounds
/// the stored summary.
pub fn summarize_into_ledger(
    ledger: &mut Ledger,
    compaction: &Compaction,
    provider: &Arc<dyn ChatProvider>,
    compact_model: &str,
    max_chars: Option<usize>,
) -> String {
    summarize_into_ledger_styled(
        ledger,
        compaction,
        provider,
        compact_model,
        max_chars,
        SummaryStyle::Plain,
    )
}

pub fn summarize_into_ledger_styled(
    ledger: &mut Ledger,
    compaction: &Compaction,
    provider: &Arc<dyn ChatProvider>,
    compact_model: &str,
    max_chars: Option<usize>,
    style: SummaryStyle,
) -> String {
    let earlier: Vec<String> = ledger
        .entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            EntryKind::Summary { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();

    let mut input: Vec<ChatMessage> = Vec::with_capacity(compaction.dropped.len() + 1);
    if !earlier.is_empty() {
        input.push(ChatMessage {
            role: ChatRole::System,
            content: format!("Earlier summary: {}", earlier.join("\n")),
            tool_call_id: None,
            tool_calls: vec![],
            images: vec![],
        });
    }
    input.extend(compaction.dropped.iter().cloned());

    let pins = pins_text(ledger);
    let mut summary = summarize_styled(&input, provider, compact_model, style, &pins);
    if let Some(limit) = max_chars {
        if summary.chars().count() > limit {
            let cut: String = summary.chars().take(limit.saturating_sub(1)).collect();
            summary = format!("{}…", cut.trim_end());
        }
    }

    ledger
        .entries
        .retain(|entry| !matches!(entry.kind, EntryKind::Summary { .. }));
    ledger
        .entries
        .push(LedgerEntry::summary(summary.clone(), compaction.covers.clone()));
    summary
}

fn pins_text(ledger: &Ledger) -> String {
    ledger
        .pins()
        .iter()
        .map(|pin| format!("- {}: {}", pin.kind.label(), pin.text))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Ask `compact_model` for a short summary of `dropped`; falls back to the
/// structured excerpt when the call fails or returns nothing.
pub fn summarize_dropped(
    dropped: &[ChatMessage],
    provider: &Arc<dyn ChatProvider>,
    compact_model: &str,
) -> String {
    summarize_styled(dropped, provider, compact_model, SummaryStyle::Plain, "")
}

fn role_label(role: ChatRole) -> &'static str {
    match role {
        ChatRole::User => "User",
        ChatRole::Assistant => "AI",
        ChatRole::Tool => "Tool",
        ChatRole::System => "System",
    }
}

fn summarize_styled(
    dropped: &[ChatMessage],
    provider: &Arc<dyn ChatProvider>,
    compact_model: &str,
    style: SummaryStyle,
    pins: &str,
) -> String {
    let formatted: String = dropped
        .iter()
        .take(40)
        .map(|m| {
            let snippet: String = m.content.chars().take(600).collect();
            format!("[{}]: {snippet}", role_label(m.role))
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    let (system, user) = match style {
        SummaryStyle::Plain => (
            "You are a conversation summarizer. Given a list of messages, \
             produce a concise summary (max 250 words) that captures: \
             key decisions, important findings, files or tools used, \
             and any critical context needed to continue the conversation. \
             Write in past tense. Be specific, not vague."
                .to_string(),
            format!("Summarize these earlier conversation messages:\n\n{formatted}"),
        ),
        SummaryStyle::Structured => (
            "You are a conversation summarizer. Write exactly three labelled parts, \
             at most 200 words in total. \"Task:\" what the user is trying to achieve. \
             \"State:\" what has been done, found and decided so far, with file names. \
             \"Next step:\" what should happen next. The pinned context is kept \
             separately and word for word; do not restate or contradict it."
                .to_string(),
            if pins.is_empty() {
                format!("Summarize these earlier conversation messages:\n\n{formatted}")
            } else {
                format!(
                    "Pinned context (kept verbatim elsewhere):\n{pins}\n\nSummarize these earlier conversation messages:\n\n{formatted}"
                )
            },
        ),
    };

    let request = ChatRequest {
        model: compact_model.to_string(),
        messages: vec![
            ChatMessage {
                role: ChatRole::System,
                content: system,
                tool_call_id: None,
                tool_calls: vec![],
                images: vec![],
            },
            ChatMessage {
                role: ChatRole::User,
                content: user,
                tool_call_id: None,
                tool_calls: vec![],
                images: vec![],
            },
        ],
        enable_tool_calls: false,
        tools: vec![],
        temperature: Some(0.3),
        top_p: Some(1.0),
        max_tokens: Some(350),
        response_format: None,
    };

    let generated = provider
        .chat(request)
        .ok()
        .and_then(|r| r.assistant_message)
        .filter(|text| !text.trim().is_empty());
    match (generated, style) {
        (Some(text), SummaryStyle::Plain) => text,
        // The model cannot be trusted to list what it left out; the engine
        // does it.
        (Some(text), SummaryStyle::Structured) => {
            format!("{}\n{}", text.trim(), dropped_listing(dropped))
        }
        (None, _) => structured_fallback(dropped),
    }
}

fn snippet(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        let cut: String = flat.chars().take(max.saturating_sub(1)).collect();
        format!("{}…", cut.trim_end())
    }
}

/// "Dropped: ..." line: counts and the user requests that were folded in.
pub fn dropped_listing(dropped: &[ChatMessage]) -> String {
    let count = |role: ChatRole| dropped.iter().filter(|m| m.role == role).count();
    let mut out = format!(
        "Dropped: {} messages ({} user, {} assistant, {} tool results).",
        dropped.len(),
        count(ChatRole::User),
        count(ChatRole::Assistant),
        count(ChatRole::Tool)
    );
    let asks: Vec<String> = dropped
        .iter()
        .filter(|m| m.role == ChatRole::User)
        .take(5)
        .map(|m| format!("\"{}\"", snippet(&m.content, 70)))
        .collect();
    if !asks.is_empty() {
        out.push_str(&format!(" User asks folded in: {}.", asks.join("; ")));
    }
    out
}

/// A deterministic Task / State / Next step summary for when no model is
/// available (or it failed).
pub fn structured_fallback(dropped: &[ChatMessage]) -> String {
    let first_user = dropped
        .iter()
        .find(|m| m.role == ChatRole::User)
        .map(|m| snippet(&m.content, 200));
    let last_user = dropped
        .iter()
        .rev()
        .find(|m| m.role == ChatRole::User)
        .map(|m| snippet(&m.content, 200));
    let last_assistant = dropped
        .iter()
        .rev()
        .find(|m| m.role == ChatRole::Assistant && !m.content.trim().is_empty())
        .map(|m| snippet(&m.content, 240));
    let mut tools: Vec<String> = Vec::new();
    for call in dropped.iter().flat_map(|m| m.tool_calls.iter()) {
        if !tools.contains(&call.name) {
            tools.push(call.name.clone());
        }
    }

    let mut state = format!("{} earlier messages were compacted.", dropped.len());
    if !tools.is_empty() {
        state.push_str(&format!(" Tools used: {}.", tools.join(", ")));
    }
    if let Some(text) = last_assistant {
        state.push_str(&format!(" Last assistant message: {text}"));
    }
    format!(
        "Task: {}\nState: {}\nNext step: {}\n{}",
        first_user.unwrap_or_else(|| "(see the earlier summary and the pinned context)".into()),
        state,
        last_user
            .map(|text| format!("continue from the user's latest request in the dropped part: {text}"))
            .unwrap_or_else(|| "continue from the latest request below".into()),
        dropped_listing(dropped)
    )
}
