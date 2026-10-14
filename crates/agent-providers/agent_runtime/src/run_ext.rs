//! Extension points for domain crates that act outside the file system.
//!
//! The runtime knows how to checkpoint and revert files. A crate whose tools
//! change something else (the scene tools edit the live level) plugs in here
//! instead of teaching the runtime about its types:
//!
//! - [`RunExtension::classify`] says which of its tools only read and which
//!   write, so confirmation, taint and read-only harness trimming apply.
//! - [`RunExtension::has_undo`] / [`RunExtension::revert`] join the run's
//!   one-click revert: reverting a run (and every run below it) also undoes
//!   what the extension recorded for it.
//! - [`RunExtension::report_sections`] adds sections to a finished child run's
//!   report (a sandboxed run returns a diff there).
//! - [`RunExtension::pending_reviews`] / [`RunExtension::resolve_review`] let
//!   the panel show items that wait for the user (a sandbox diff) with Apply
//!   and Discard.
//!
//! Extensions register once at start-up with [`register_run_extension`]; the
//! runtime never names a concrete one.

use std::sync::{Arc, RwLock};

use crate::taint::ToolClass;

/// An item waiting for the user's decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Review {
    /// Passed back to [`resolve_review`].
    pub id: String,
    /// Run that produced it (a checkpoint id).
    pub run_id: String,
    pub title: String,
    /// Human-readable body, e.g. a diff.
    pub summary: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewDecision {
    Apply,
    Discard,
}

/// A domain crate's hooks into the run loop. Every method has a neutral
/// default, so an extension implements only what it needs.
pub trait RunExtension: Send + Sync {
    /// Unique name; registering the same name again replaces the earlier one.
    fn name(&self) -> &'static str;

    /// Class of one of this extension's tools, `None` for tools it does not
    /// own.
    fn classify(&self, _tool: &str) -> Option<ToolClass> {
        None
    }

    /// Does `run_id`, or any run below it, have something to revert?
    fn has_undo(&self, _run_id: &str) -> bool {
        false
    }

    /// Undo what `run_id` and the runs below it did, newest first. Returns
    /// one note per thing undone for the revert summary.
    fn revert(&self, _run_id: &str) -> Result<Vec<String>, String> {
        Ok(Vec::new())
    }

    /// `(section name, body)` pairs appended to the report of the finished
    /// child run `run_id`.
    fn report_sections(&self, _run_id: &str) -> Vec<(String, String)> {
        Vec::new()
    }

    fn pending_reviews(&self) -> Vec<Review> {
        Vec::new()
    }

    fn resolve_review(&self, id: &str, _decision: ReviewDecision) -> Result<String, String> {
        Err(format!("No pending review '{id}'."))
    }
}

static EXTENSIONS: RwLock<Vec<Arc<dyn RunExtension>>> = RwLock::new(Vec::new());

fn snapshot() -> Vec<Arc<dyn RunExtension>> {
    EXTENSIONS
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// Add `extension`, replacing a registered one of the same name.
pub fn register_run_extension(extension: Arc<dyn RunExtension>) {
    let mut all = EXTENSIONS
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match all.iter_mut().find(|e| e.name() == extension.name()) {
        Some(existing) => *existing = extension,
        None => all.push(extension),
    }
}

/// Is `run_id` the checkpoint id `root` or one of the runs below it?
/// Child checkpoint ids are `<parent>.<child>`.
pub fn run_in_tree(run_id: &str, root: &str) -> bool {
    run_id == root
        || run_id
            .strip_prefix(root)
            .is_some_and(|rest| rest.starts_with('.'))
}

pub(crate) fn classify(tool: &str) -> Option<ToolClass> {
    snapshot().iter().find_map(|e| e.classify(tool))
}

/// Does any extension hold something to revert for this run tree?
pub fn extensions_have_undo(run_id: &str) -> bool {
    snapshot().iter().any(|e| e.has_undo(run_id))
}

/// Revert the run tree in every extension. Returns the notes of what was
/// undone and the errors, so one failing extension does not hide the rest.
pub fn revert_extensions(run_id: &str) -> (Vec<String>, Vec<String>) {
    let mut notes = Vec::new();
    let mut errors = Vec::new();
    for extension in snapshot() {
        if !extension.has_undo(run_id) {
            continue;
        }
        match extension.revert(run_id) {
            Ok(mut part) => notes.append(&mut part),
            Err(err) => errors.push(format!("{}: {err}", extension.name())),
        }
    }
    (notes, errors)
}

/// Report sections every extension adds for the finished child `run_id`.
pub fn extension_report_sections(run_id: &str) -> Vec<(String, String)> {
    snapshot()
        .iter()
        .flat_map(|e| e.report_sections(run_id))
        .collect()
}

/// Everything waiting for the user's decision.
pub fn pending_reviews() -> Vec<Review> {
    snapshot().iter().flat_map(|e| e.pending_reviews()).collect()
}

/// Apply or discard one pending review.
pub fn resolve_review(id: &str, decision: ReviewDecision) -> Result<String, String> {
    for extension in snapshot() {
        if extension.pending_reviews().iter().any(|r| r.id == id) {
            return extension.resolve_review(id, decision);
        }
    }
    Err(format!("No pending review '{id}'."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_membership_uses_the_dot_separator() {
        assert!(run_in_tree("run-1", "run-1"));
        assert!(run_in_tree("run-1.child-2", "run-1"));
        assert!(run_in_tree("run-1.a.b", "run-1"));
        assert!(!run_in_tree("run-10", "run-1"));
        assert!(!run_in_tree("run-1", "run-1.child"));
    }
}
