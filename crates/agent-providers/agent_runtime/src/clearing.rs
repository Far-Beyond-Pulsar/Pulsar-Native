//! Tool-result clearing: before the loop compacts, old large tool outputs are
//! swapped for a one-line stub. The full text goes to the [`OutputStore`], so
//! the model can read it back with `fetch_output`.

use crate::ledger::{EntryKind, Ledger};
use crate::output_cap::OutputStore;

/// Prefix of every stub.
pub const CLEARED_PREFIX: &str = "[result cleared: ref ";

#[derive(Clone, Copy, Debug)]
pub struct ClearOptions {
    /// The newest this-many tool exchanges are never cleared.
    pub keep_recent: usize,
    /// Results shorter than this (characters) are not worth a stub.
    pub min_chars: usize,
}

impl Default for ClearOptions {
    fn default() -> Self {
        Self {
            keep_recent: 4,
            min_chars: 1_500,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClearReport {
    pub cleared: usize,
    pub freed_chars: usize,
}

pub fn is_cleared(content: &str) -> bool {
    content.starts_with(CLEARED_PREFIX)
}

pub fn cleared_stub(reference: &str, chars: usize) -> String {
    let kb = chars.div_ceil(1024).max(1);
    format!("{CLEARED_PREFIX}{reference}, {kb} KB]")
}

/// The ref an already-capped result points at, if the store still has it.
fn existing_ref(content: &str, store: &OutputStore) -> Option<String> {
    let marker = "fetch_output ref=\"";
    let start = content.find(marker)? + marker.len();
    let end = content[start..].find('"')? + start;
    let reference = &content[start..end];
    store.get(reference).map(|_| reference.to_string())
}

/// Clear old, large tool results oldest first. `needs_more` is asked before
/// each clear; return `false` once the context fits. `None` clears every
/// candidate.
pub fn clear_old_tool_results(
    ledger: &mut Ledger,
    store: &OutputStore,
    opts: ClearOptions,
    needs_more: &mut dyn FnMut(&Ledger) -> bool,
) -> ClearReport {
    let exchanges: Vec<usize> = ledger
        .entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| matches!(entry.kind, EntryKind::ToolExchange(_)))
        .map(|(ix, _)| ix)
        .collect();
    let eligible = exchanges.len().saturating_sub(opts.keep_recent);
    let mut report = ClearReport::default();
    for &ix in &exchanges[..eligible] {
        let EntryKind::ToolExchange(exchange) = &ledger.entries[ix].kind else {
            continue;
        };
        let content = &exchange.result.content;
        let chars = content.chars().count();
        if is_cleared(content) || chars < opts.min_chars {
            continue;
        }
        if !needs_more(ledger) {
            break;
        }
        // Prefer the ref of the uncut original over a new one for the
        // already-cut text.
        let (reference, total) = match existing_ref(content, store) {
            Some(reference) => {
                let total = store.get(&reference).map_or(chars, |t| t.chars().count());
                (reference, total)
            }
            None => (store.put(content), chars),
        };
        let stub = cleared_stub(&reference, total);
        report.freed_chars += chars.saturating_sub(stub.chars().count());
        report.cleared += 1;
        if let EntryKind::ToolExchange(exchange) = &mut ledger.entries[ix].kind {
            exchange.result.content = stub;
        }
    }
    report
}
