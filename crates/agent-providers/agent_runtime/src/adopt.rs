//! Folding a finished run's entries back into the chat's own ledger.
//!
//! The chat panel keeps a ledger with one streaming placeholder for the
//! assistant's reply; the run works on a copy that grows with every assistant
//! turn and tool exchange. [`adopt_run_tail`] swaps the placeholder for that
//! tail, so tool exchanges (and foreground sub-agent results) are saved with
//! the chat and the chat can be replayed after a reload.
//!
//! The run may have compacted its copy, so entries are matched by an anchor
//! (the last entry before the placeholder) and not by index.

use crate::ledger::{EntryKind, Ledger, LedgerEntry};

/// Identity of an entry for anchoring; `None` for kinds that never anchor.
fn anchor_key(entry: &LedgerEntry) -> Option<(u8, &str)> {
    match &entry.kind {
        EntryKind::UserTurn { text } => Some((0, text.as_str())),
        EntryKind::AssistantTurn { text } => Some((1, text.as_str())),
        EntryKind::EngineNotice { text } => Some((2, text.as_str())),
        EntryKind::ToolExchange(exchange) => Some((3, exchange.call.id.as_str())),
        EntryKind::SubagentReport(report) => Some((4, report.run_id.as_str())),
        EntryKind::System { .. } | EntryKind::Summary { .. } | EntryKind::Pinned { .. } => None,
    }
}

/// Replace `panel[placeholder_ix..]` with what `run` added after the entries
/// before the placeholder. Pins stay with the panel (it edits them while the
/// run goes). Returns the index of the last assistant turn that was adopted,
/// or `None` when nothing was adopted (the panel ledger is untouched).
pub fn adopt_run_tail(panel: &mut Ledger, placeholder_ix: usize, run: &Ledger) -> Option<usize> {
    if placeholder_ix > panel.entries.len() {
        return None;
    }
    let anchor = panel.entries[..placeholder_ix]
        .iter()
        .rev()
        .find_map(anchor_key)?;
    let run_anchor = run
        .entries
        .iter()
        .rposition(|entry| anchor_key(entry) == Some(anchor))?;

    let mut tail: Vec<LedgerEntry> = run.entries[run_anchor + 1..]
        .iter()
        .filter(|entry| !matches!(entry.kind, EntryKind::Pinned { .. } | EntryKind::Summary { .. }))
        .cloned()
        .collect();
    // The panel's empty streaming placeholder is the first thing after the anchor.
    if matches!(
        tail.first().map(|entry| &entry.kind),
        Some(EntryKind::AssistantTurn { text }) if text.is_empty()
    ) {
        tail.remove(0);
    }
    if tail.is_empty() {
        return None;
    }

    let panel_pins: Vec<LedgerEntry> = panel.entries[placeholder_ix..]
        .iter()
        .filter(|entry| matches!(entry.kind, EntryKind::Pinned { .. }))
        .cloned()
        .collect();
    panel.entries.truncate(placeholder_ix);
    panel.entries.extend(tail);
    let last_assistant = panel
        .entries
        .iter()
        .rposition(|entry| matches!(entry.kind, EntryKind::AssistantTurn { .. }));
    panel.entries.extend(panel_pins);
    last_assistant.filter(|ix| *ix >= placeholder_ix)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::{PinKind, ToolResult};
    use agent_chat_core::ToolCall;

    fn exchange(id: &str) -> LedgerEntry {
        LedgerEntry::tool_exchange(
            ToolCall {
                id: id.into(),
                name: "read_file".into(),
                arguments_json: serde_json::json!({"path": "a.rs"}),
            },
            ToolResult {
                content: "fn main() {}".into(),
                is_error: false,
            },
        )
    }

    #[test]
    fn tool_exchanges_replace_the_placeholder() {
        let mut panel = Ledger::with_system_prompt("sys");
        panel.push(LedgerEntry::user("look"));
        let placeholder = panel.push(LedgerEntry::assistant("streamed text"));
        panel.push(LedgerEntry::pinned(PinKind::FileTouched, "a.rs"));

        let mut run = Ledger::with_system_prompt("sys");
        run.push(LedgerEntry::user("look"));
        run.push(LedgerEntry::assistant(""));
        run.push(LedgerEntry::pinned(PinKind::LiveState, "tasks"));
        run.push(LedgerEntry::assistant("reading"));
        run.push(exchange("c1"));
        run.push(LedgerEntry::assistant("done"));

        let last = adopt_run_tail(&mut panel, placeholder, &run);
        assert_eq!(last, Some(4));
        assert_eq!(panel.len(), 6);
        assert!(matches!(panel.entries[3].kind, EntryKind::ToolExchange(_)));
        // The live-state pin stays out; the panel's own pin stays in.
        assert!(matches!(&panel.entries[5].kind, EntryKind::Pinned { kind: PinKind::FileTouched, .. }));
    }

    #[test]
    fn compaction_in_the_run_does_not_confuse_the_anchor() {
        let mut panel = Ledger::with_system_prompt("sys");
        panel.push(LedgerEntry::user("old"));
        panel.push(LedgerEntry::assistant("old reply"));
        panel.push(LedgerEntry::user("new"));
        let placeholder = panel.push(LedgerEntry::assistant(""));

        let mut run = Ledger::with_system_prompt("sys");
        run.push(LedgerEntry::summary("earlier", 1..3));
        run.push(LedgerEntry::user("new"));
        run.push(LedgerEntry::assistant(""));
        run.push(LedgerEntry::assistant("answer"));

        assert_eq!(adopt_run_tail(&mut panel, placeholder, &run), Some(4));
        assert_eq!(panel.len(), 5);
        assert_eq!(panel.entries[0].char_len(), 3);
    }

    #[test]
    fn nothing_new_leaves_the_panel_ledger_alone() {
        let mut panel = Ledger::with_system_prompt("sys");
        panel.push(LedgerEntry::user("hi"));
        let placeholder = panel.push(LedgerEntry::assistant("partial"));
        let mut run = Ledger::with_system_prompt("sys");
        run.push(LedgerEntry::user("hi"));
        run.push(LedgerEntry::assistant(""));
        assert_eq!(adopt_run_tail(&mut panel, placeholder, &run), None);
        assert_eq!(panel.len(), 3);
    }
}
