//! One undo/redo system for every editor (Pulsar-Native#931).
//!
//! History is **per document**: a level, a Blueprint, a script, a texture,
//! not per panel or per editor kind. The editor of a document registers its
//! history under the document's id with the host's [`UndoService`] (handed
//! to it in [`crate::EditorServices`], never a process global: a plugin
//! library has its own copy of those), so the host can find it. A second
//! editor of the same file (another window on the project, with its own
//! copy of the data) keeps its own history: it must never replay the
//! other's edits ([`UndoService::adopt`]).
//!
//! The pieces:
//! - [`History`]: the undo and redo stacks of one document, generic over an
//!   editor's own entry type (its commands or deltas). Capped, with
//!   optional merging of consecutive entries (a drag, typing) and a save
//!   point for dirty tracking. It never applies an entry itself; the editor
//!   does, given the entry ([`History::undo`] / [`History::redo`]).
//! - [`UndoService`]: the host's registry of histories by [`DocumentId`].
//! - [`Undo`] and [`Redo`]: the one pair of actions, bound to the usual keys
//!   by the host ([`bind_undo_keys`]). An editor handles them for the
//!   document it shows; a text field's own, more specific bindings still win
//!   inside it.
//! - [`crate::EditorMenu::undo_redo`]: the Edit menu's two items, labelled
//!   with what they undo and greyed out when there is nothing to undo.

use std::any::Any;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, Weak};

/// Undo the last change in the focused editor's document.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Deserialize,
    schemars::JsonSchema,
    gpui::Action,
)]
#[action(namespace = pulsar_app)]
pub struct Undo;

/// Redo the last undone change in the focused editor's document.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Deserialize,
    schemars::JsonSchema,
    gpui::Action,
)]
#[action(namespace = pulsar_app)]
pub struct Redo;

/// Bind [`Undo`] and [`Redo`] to the platform's keys. The host calls this
/// once; editors only handle the actions.
pub fn bind_undo_keys(cx: &mut gpui::App) {
    cx.bind_keys([
        gpui::KeyBinding::new("secondary-z", Undo, None),
        gpui::KeyBinding::new("secondary-shift-z", Redo, None),
        #[cfg(not(target_os = "macos"))]
        gpui::KeyBinding::new("ctrl-y", Redo, None),
    ]);
}

/// A document's identity: what its history is keyed by. For a file-backed
/// document, its path relative to the project (so two views of one file
/// share a history); for an unsaved one, any id unique to it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DocumentId(Arc<str>);

impl DocumentId {
    pub fn new(id: impl AsRef<str>) -> Self {
        Self(Arc::from(id.as_ref()))
    }

    /// The id of the document stored at `path`: its normalised form, so the
    /// same file reached two ways is one document.
    pub fn for_path(path: &std::path::Path) -> Self {
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        Self::new(path.to_string_lossy().replace('\\', "/"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for DocumentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One undoable change, in an editor's own terms.
pub trait UndoEntry: Send + 'static {
    /// What the change was, for "Undo <label>" (empty for no label).
    fn label(&self) -> String {
        String::new()
    }

    /// Fold `next`, recorded right after `self`, into `self`, so one undo
    /// reverts both (a drag's moves, a word's keystrokes). Returns whether
    /// it did; `next` is pushed as its own entry otherwise.
    fn merge(&mut self, _next: &Self) -> bool {
        false
    }
}

/// The undo and redo stacks of one document.
pub struct History<E> {
    undo: VecDeque<E>,
    redo: Vec<E>,
    capacity: usize,
    /// The undo depth the document was last saved at; `None` once that
    /// state can no longer be reached (it was dropped by the cap, or a new
    /// change was made after undoing past it).
    saved_at: Option<usize>,
    /// Whether the next push may merge into the last entry.
    merging: bool,
    revision: u64,
}

impl<E: UndoEntry> Default for History<E> {
    fn default() -> Self {
        Self::with_capacity(History::<E>::DEFAULT_CAPACITY)
    }
}

impl<E: UndoEntry> History<E> {
    /// How many entries a history keeps by default.
    pub const DEFAULT_CAPACITY: usize = 100;

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            undo: VecDeque::new(),
            redo: Vec::new(),
            capacity: capacity.max(1),
            saved_at: Some(0),
            merging: false,
            revision: 0,
        }
    }

    /// Record a change that was just applied. Clears the redo stack.
    pub fn push(&mut self, entry: E) {
        self.revision += 1;
        if !self.redo.is_empty() {
            self.redo.clear();
            if self.saved_at.is_some_and(|saved| saved > self.undo.len()) {
                self.saved_at = None;
            }
        }
        if self.merging && self.saved_at != Some(self.undo.len()) {
            if let Some(last) = self.undo.back_mut() {
                if last.merge(&entry) {
                    return;
                }
            }
        }
        self.undo.push_back(entry);
        self.merging = true;
        if self.undo.len() > self.capacity {
            self.undo.pop_front();
            self.saved_at = match self.saved_at {
                Some(0) | None => None,
                Some(saved) => Some(saved - 1),
            };
        }
    }

    /// End the run of entries that may merge: the next push starts a new
    /// entry (a drag or a burst of typing ended).
    pub fn seal(&mut self) {
        self.merging = false;
    }

    /// Undo the last entry: `apply` reverts it in the document and returns
    /// whether it did. A reverted entry moves to the redo stack; one that
    /// could not be reverted is dropped, with the redo stack (its effect is
    /// gone or no longer reachable). Returns the entry's label when one was
    /// undone.
    pub fn undo(&mut self, apply: impl FnOnce(&mut E) -> bool) -> Option<String> {
        let mut entry = self.undo.pop_back()?;
        self.revision += 1;
        self.merging = false;
        if apply(&mut entry) {
            let label = entry.label();
            self.redo.push(entry);
            Some(label)
        } else {
            self.redo.clear();
            self.saved_at = None;
            None
        }
    }

    /// Redo the last undone entry, the same way as [`Self::undo`].
    pub fn redo(&mut self, apply: impl FnOnce(&mut E) -> bool) -> Option<String> {
        let mut entry = self.redo.pop()?;
        self.revision += 1;
        self.merging = false;
        if apply(&mut entry) {
            let label = entry.label();
            self.undo.push_back(entry);
            Some(label)
        } else {
            self.redo.clear();
            self.saved_at = None;
            None
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// The label of what [`Self::undo`] would revert.
    pub fn undo_label(&self) -> Option<String> {
        self.undo.back().map(UndoEntry::label)
    }

    /// The label of what [`Self::redo`] would reapply.
    pub fn redo_label(&self) -> Option<String> {
        self.redo.last().map(UndoEntry::label)
    }

    /// The entries that can be undone, oldest first.
    pub fn undo_entries(&self) -> impl DoubleEndedIterator<Item = &E> {
        self.undo.iter()
    }

    /// The entries that can be undone, oldest first, mutably (an editor
    /// rewriting stored state, e.g. after a class definition changed).
    pub fn undo_entries_mut(&mut self) -> impl DoubleEndedIterator<Item = &mut E> {
        self.undo.iter_mut()
    }

    /// The entries that can be redone, next first, mutably.
    pub fn redo_entries_mut(&mut self) -> impl DoubleEndedIterator<Item = &mut E> {
        self.redo.iter_mut().rev()
    }

    /// Forget every entry (the document was reloaded from disk).
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.saved_at = Some(0);
        self.merging = false;
        self.revision += 1;
    }

    /// The document was just saved: its current state is the clean one.
    pub fn mark_saved(&mut self) {
        self.saved_at = Some(self.undo.len());
        self.merging = false;
        self.revision += 1;
    }

    /// Whether the document differs from its last saved state.
    pub fn is_dirty(&self) -> bool {
        self.saved_at != Some(self.undo.len())
    }

    /// A number that changes whenever anything above may have: for menus
    /// and other views that cache what they show.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn len(&self) -> usize {
        self.undo.len()
    }

    pub fn is_empty(&self) -> bool {
        self.undo.is_empty()
    }
}

/// A document's history as the host sees it, whatever its entry type.
pub trait HistoryView: Any + Send + Sync {
    fn can_undo(&self) -> bool;
    fn can_redo(&self) -> bool;
    fn undo_label(&self) -> Option<String>;
    fn redo_label(&self) -> Option<String>;
    fn is_dirty(&self) -> bool;
    fn revision(&self) -> u64;
}

/// A document's history, shared by the editor's own parts (panels, menus)
/// and registered with the host.
pub type SharedHistory<E> = Arc<Mutex<History<E>>>;

impl<E: UndoEntry> HistoryView for Mutex<History<E>> {
    fn can_undo(&self) -> bool {
        lock(self).can_undo()
    }
    fn can_redo(&self) -> bool {
        lock(self).can_redo()
    }
    fn undo_label(&self) -> Option<String> {
        lock(self).undo_label()
    }
    fn redo_label(&self) -> Option<String> {
        lock(self).redo_label()
    }
    fn is_dirty(&self) -> bool {
        lock(self).is_dirty()
    }
    fn revision(&self) -> u64 {
        lock(self).revision()
    }
}

/// A history's lock, recovered when a panic poisoned it: the stacks stay
/// consistent between calls, so the data is still usable.
pub fn lock<E>(history: &Mutex<History<E>>) -> std::sync::MutexGuard<'_, History<E>> {
    history
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The host's histories, one per open document.
///
/// A history lives while any view of its document holds it, and is
/// forgotten once the last one closes.
#[derive(Clone, Default)]
pub struct UndoService {
    histories: Arc<Mutex<HashMap<DocumentId, Weak<dyn HistoryView>>>>,
}

impl UndoService {
    pub fn new() -> Self {
        Self::default()
    }

    /// The history of `document` with entries of type `E`: the one already
    /// registered, or a new one. For parts that edit the same copy of the
    /// document; an editor with its own copy uses [`Self::adopt`] instead.
    /// `None` when the document's history holds another entry type (two
    /// editors disagree on what the document is).
    pub fn history<E: UndoEntry>(&self, document: &DocumentId) -> Option<SharedHistory<E>> {
        let mut histories = self.histories.lock().unwrap_or_else(|p| p.into_inner());
        histories.retain(|_, history| history.strong_count() > 0);
        if let Some(existing) = histories.get(document).and_then(Weak::upgrade) {
            let any: Arc<dyn Any + Send + Sync> = existing;
            return any.downcast::<Mutex<History<E>>>().ok();
        }
        let history: SharedHistory<E> = Arc::new(Mutex::new(History::default()));
        let view: Arc<dyn HistoryView> = history.clone();
        histories.insert(document.clone(), Arc::downgrade(&view));
        Some(history)
    }

    /// Register `history` as `document`'s, keeping what it already
    /// recorded (a view that learned its document late). When another live
    /// view already holds the document's history, `history` stays this
    /// view's own: each editor holds its own copy of the document's data (a
    /// second window on the project opens the file again), so one view must
    /// never replay another's edits. Returns whether `history` is the one
    /// the host keeps for `document`.
    pub fn adopt<E: UndoEntry>(&self, document: &DocumentId, history: &SharedHistory<E>) -> bool {
        let mut histories = self.histories.lock().unwrap_or_else(|p| p.into_inner());
        histories.retain(|_, history| history.strong_count() > 0);
        if let Some(existing) = histories.get(document).and_then(Weak::upgrade) {
            let any: Arc<dyn Any + Send + Sync> = existing;
            return any
                .downcast::<Mutex<History<E>>>()
                .is_ok_and(|existing| Arc::ptr_eq(&existing, history));
        }
        let view: Arc<dyn HistoryView> = history.clone();
        histories.insert(document.clone(), Arc::downgrade(&view));
        true
    }

    /// `document`'s history as the host sees it, while any view holds it.
    pub fn view(&self, document: &DocumentId) -> Option<Arc<dyn HistoryView>> {
        let histories = self.histories.lock().unwrap_or_else(|p| p.into_inner());
        histories.get(document).and_then(Weak::upgrade)
    }

    /// The documents with a live history.
    pub fn documents(&self) -> Vec<DocumentId> {
        let histories = self.histories.lock().unwrap_or_else(|p| p.into_inner());
        let mut documents: Vec<_> = histories
            .iter()
            .filter(|(_, history)| history.strong_count() > 0)
            .map(|(document, _)| document.clone())
            .collect();
        documents.sort();
        documents
    }
}

impl std::fmt::Debug for UndoService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UndoService")
            .field("documents", &self.documents())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test document: a list of words; entries insert or remove one.
    #[derive(Debug, PartialEq)]
    enum Edit {
        Push(String),
        Typed(String),
    }

    impl UndoEntry for Edit {
        fn label(&self) -> String {
            match self {
                Edit::Push(word) => format!("add {word}"),
                Edit::Typed(text) => format!("type {text}"),
            }
        }
        fn merge(&mut self, next: &Self) -> bool {
            match (self, next) {
                (Edit::Typed(text), Edit::Typed(more)) => {
                    text.push_str(more);
                    true
                }
                _ => false,
            }
        }
    }

    fn apply_undo(doc: &mut Vec<String>) -> impl FnOnce(&mut Edit) -> bool + '_ {
        move |_| doc.pop().is_some()
    }

    #[test]
    fn undo_and_redo_walk_the_stacks_and_report_labels() {
        let mut doc = vec!["a".to_string()];
        let mut history = History::default();
        history.push(Edit::Push("a".into()));
        assert_eq!(history.undo_label().as_deref(), Some("add a"));
        assert_eq!(history.undo(apply_undo(&mut doc)).as_deref(), Some("add a"));
        assert!(doc.is_empty());
        assert!(!history.can_undo() && history.can_redo());
        assert_eq!(
            history.redo(|entry| {
                if let Edit::Push(word) = entry {
                    doc.push(word.clone());
                }
                true
            }),
            Some("add a".into())
        );
        assert_eq!(doc, ["a"]);
        assert!(history.can_undo() && !history.can_redo());
    }

    #[test]
    fn a_new_change_clears_redo() {
        let mut history = History::default();
        history.push(Edit::Push("a".into()));
        history.undo(|_| true);
        history.push(Edit::Push("b".into()));
        assert!(!history.can_redo());
        assert_eq!(history.undo_label().as_deref(), Some("add b"));
    }

    #[test]
    fn consecutive_entries_merge_until_sealed() {
        let mut history = History::default();
        history.push(Edit::Typed("he".into()));
        history.push(Edit::Typed("llo".into()));
        assert_eq!(history.len(), 1);
        assert_eq!(history.undo_label().as_deref(), Some("type hello"));
        history.seal();
        history.push(Edit::Typed(" world".into()));
        assert_eq!(history.len(), 2);
        // An undo seals too.
        history.undo(|_| true);
        history.push(Edit::Typed("!".into()));
        assert_eq!(history.len(), 2);
    }

    #[test]
    fn the_cap_drops_the_oldest() {
        let mut history = History::with_capacity(2);
        for word in ["a", "b", "c"] {
            history.push(Edit::Push(word.into()));
            history.seal();
        }
        assert_eq!(history.len(), 2);
        let labels: Vec<_> = history.undo_entries().map(UndoEntry::label).collect();
        assert_eq!(labels, ["add b", "add c"]);
    }

    #[test]
    fn an_entry_that_cannot_be_reverted_is_dropped_with_redo() {
        let mut history = History::default();
        history.push(Edit::Push("a".into()));
        history.push(Edit::Push("b".into()));
        history.undo(|_| true);
        assert!(history.can_redo());
        assert_eq!(history.undo(|_| false), None);
        assert!(!history.can_undo() && !history.can_redo());
    }

    #[test]
    fn the_save_point_tracks_dirtiness() {
        let mut history = History::default();
        assert!(!history.is_dirty());
        history.push(Edit::Push("a".into()));
        assert!(history.is_dirty());
        history.mark_saved();
        assert!(!history.is_dirty());
        // A typed entry after a save does not merge into the saved one.
        history.push(Edit::Typed("x".into()));
        history.push(Edit::Typed("y".into()));
        assert_eq!(history.len(), 2);
        history.undo(|_| true);
        assert!(!history.is_dirty(), "back at the saved state");
        history.undo(|_| true);
        assert!(history.is_dirty());
        // A new change while undone past the save point: it is unreachable.
        history.push(Edit::Push("b".into()));
        assert!(history.is_dirty());
        history.undo(|_| true);
        assert!(history.is_dirty());
    }

    #[test]
    fn views_of_one_document_share_its_history() {
        let service = UndoService::new();
        let level = DocumentId::new("levels/main.level");
        let first = service.history::<Edit>(&level).unwrap();
        let second = service.history::<Edit>(&level).unwrap();
        lock(&first).push(Edit::Push("a".into()));
        assert!(lock(&second).can_undo(), "one history per document");
        let view = service.view(&level).unwrap();
        assert_eq!(view.undo_label().as_deref(), Some("add a"));
        assert_eq!(service.documents(), [level.clone()]);

        // Another document has its own.
        let other = service.history::<Edit>(&DocumentId::new("b")).unwrap();
        assert!(!lock(&other).can_undo());

        // An editor disagreeing on the entry type gets none.
        assert!(service.history::<u8>(&level).is_none());

        // Closing every view forgets it.
        drop((first, second, view));
        assert!(service.view(&level).is_none());
        assert!(!lock(&service.history::<Edit>(&level).unwrap()).can_undo());
    }

    #[test]
    fn a_view_adopting_its_document_keeps_its_edits_and_never_takes_another_views() {
        let service = UndoService::new();
        let doc = DocumentId::new("graph");
        let early: SharedHistory<Edit> = Arc::default();
        lock(&early).push(Edit::Push("a".into()));
        assert!(service.adopt(&doc, &early));
        assert!(service.adopt(&doc, &early), "adopting again is a no-op");
        assert_eq!(
            service.view(&doc).unwrap().undo_label().as_deref(),
            Some("add a")
        );

        // A second view of the document (its own copy of the data) keeps
        // its own history: undoing there must not replay the first's edits.
        let late: SharedHistory<Edit> = Arc::default();
        lock(&late).push(Edit::Push("b".into()));
        assert!(!service.adopt(&doc, &late));
        assert_eq!(lock(&late).undo_label().as_deref(), Some("add b"));
        assert_eq!(lock(&early).undo_label().as_deref(), Some("add a"));

        // Once the first closes, the next view becomes the document's.
        drop(early);
        assert!(service.adopt(&doc, &late));

        let wrong: SharedHistory<u8> = Arc::default();
        assert!(!service.adopt(&doc, &wrong));
    }

    impl UndoEntry for u8 {}
}
