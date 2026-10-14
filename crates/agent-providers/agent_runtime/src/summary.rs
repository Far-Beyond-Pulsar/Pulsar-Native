//! Folding compacted entries into a single running summary.
//!
//! `compact_ledger` leaves summaries alone (they are system content), so
//! without merging every compaction would add another one and a small window
//! would slowly fill with summaries. [`summarize_into_ledger`] folds the old
//! summaries into the new one.

use std::sync::Arc;

use agent_chat_core::{ChatMessage, ChatProvider, ChatRole};

use crate::compaction::{summarize_dropped, Compaction};
use crate::ledger::{EntryKind, Ledger, LedgerEntry};

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
        });
    }
    input.extend(compaction.dropped.iter().cloned());

    let mut summary = summarize_dropped(&input, provider, compact_model);
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
