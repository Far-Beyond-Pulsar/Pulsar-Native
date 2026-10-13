//! The document behind an editor: one asset file, its edit history and its
//! saved/unsaved state.
//!
//! Every view of the file (the panels of one editor, a second editor window)
//! shares one `Entity<Document<A>>`. It notifies only when the asset changes,
//! so panels can read it in `render` without rebuilding on unrelated events.
//!
//! Editing is snapshot based: an edit stores the asset as it was, and undo and
//! redo swap it with the current one. Widgets are a few hundred kilobytes at
//! most, so this is simpler and safer than inverse commands. Consecutive edits
//! that share a merge key (a slider drag, typing in a field) collapse into one
//! undo step.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpui::*;
use plugin_editor_api::undo::lock;
use plugin_editor_api::{DocumentId, SharedHistory, UndoEntry, UndoService};

use crate::model::Asset;

/// One undoable change: the asset on the other side of it.
pub struct AssetEdit<A> {
    other: A,
    label: &'static str,
    key: Option<&'static str>,
    /// When the latest edit folded into this entry happened.
    at: Instant,
}

/// Edits closer together than this, with the same key, are one gesture.
const MERGE_WINDOW: Duration = Duration::from_millis(900);

impl<A: Send + 'static> UndoEntry for AssetEdit<A> {
    fn label(&self) -> String {
        self.label.to_string()
    }

    /// Edits with the same key in quick succession fold into the earliest,
    /// whose `other` (the state before the whole run) is the one that matters.
    fn merge(&mut self, next: &Self) -> bool {
        let same_gesture = self.key.is_some()
            && self.key == next.key
            && next.at.saturating_duration_since(self.at) < MERGE_WINDOW;
        if same_gesture {
            self.at = next.at;
        }
        same_gesture
    }
}

/// How the last file operation went, for the status bar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Saved,
    Reverted,
    SaveFailed(String),
}

/// Tells every open editor that some file was saved, so the ones showing
/// other files that refer to it can look again.
pub struct SaveBus {
    pub revision: u64,
}

struct GlobalSaveBus(Entity<SaveBus>);

impl Global for GlobalSaveBus {}

impl SaveBus {
    /// The shared bus, made on first use.
    pub fn get(cx: &mut App) -> Entity<SaveBus> {
        if let Some(bus) = cx.try_global::<GlobalSaveBus>() {
            return bus.0.clone();
        }
        let bus = cx.new(|_| SaveBus { revision: 0 });
        cx.set_global(GlobalSaveBus(bus.clone()));
        bus
    }

    pub fn announce(cx: &mut App) {
        let bus = Self::get(cx);
        bus.update(cx, |bus, cx| {
            bus.revision += 1;
            cx.notify();
        });
    }
}

pub struct Document<A: Asset> {
    path: Option<PathBuf>,
    asset: A,
    history: SharedHistory<AssetEdit<A>>,
    /// Counts every change to the asset, so caches can tell when to rebuild.
    revision: u64,
    /// Things repaired while loading, shown once.
    load_notes: Vec<String>,
    /// Set when the file existed but couldn't be read as this asset. The file
    /// is left alone until the user chooses to start over.
    unreadable: Option<String>,
    status: Option<Status>,
}

impl<A: Asset> Document<A> {
    /// A document for the file at `path`. A file that can't be read doesn't
    /// stop the editor opening: it shows the problem and refuses to save
    /// over it until [`Self::start_fresh`].
    pub fn open(path: Option<PathBuf>) -> Self {
        let mut doc = Self {
            path,
            asset: A::new_default(),
            history: Default::default(),
            revision: 0,
            load_notes: Vec::new(),
            unreadable: None,
            status: None,
        };
        doc.load_from_disk();
        doc
    }

    fn load_from_disk(&mut self) {
        let Some(path) = &self.path else { return };
        match std::fs::read_to_string(path) {
            Ok(text) => match A::parse(&text) {
                Ok((mut asset, notes)) => {
                    // A file that had no id has been given one; it has to be
                    // on disk before anything can refer to it.
                    if asset.stamp() {
                        if let Err(error) = std::fs::write(path, asset.to_text()) {
                            tracing::error!("couldn't write the new id into {}: {error}", path.display());
                        }
                    }
                    self.asset = asset;
                    self.load_notes = notes;
                    self.unreadable = None;
                }
                Err(error) => {
                    self.unreadable = Some(error);
                }
            },
            // A file that doesn't exist yet is a new asset.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => self.unreadable = Some(error.to_string()),
        }
        self.revision += 1;
    }

    // ── Reading ────────────────────────────────────────────────────────────

    pub fn asset(&self) -> &A {
        &self.asset
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn file_name(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map_or_else(|| format!("New {}", A::KIND), |n| n.to_string_lossy().into_owned())
    }

    pub fn load_notes(&self) -> &[String] {
        &self.load_notes
    }

    pub fn dismiss_load_notes(&mut self, cx: &mut Context<Self>) {
        self.load_notes.clear();
        cx.notify();
    }

    pub fn unreadable(&self) -> Option<&str> {
        self.unreadable.as_deref()
    }

    pub fn status(&self) -> Option<&Status> {
        self.status.as_ref()
    }

    pub fn is_dirty(&self) -> bool {
        lock(&self.history).is_dirty()
    }

    pub fn can_undo(&self) -> bool {
        lock(&self.history).can_undo()
    }

    pub fn can_redo(&self) -> bool {
        lock(&self.history).can_redo()
    }

    pub fn undo_label(&self) -> Option<String> {
        lock(&self.history).undo_label()
    }

    pub fn redo_label(&self) -> Option<String> {
        lock(&self.history).redo_label()
    }

    pub fn can_save(&self) -> bool {
        self.path.is_some() && self.unreadable.is_none()
    }

    /// Share this file's history with every other view of it.
    pub fn adopt_undo(&mut self, service: &UndoService) {
        if let Some(path) = &self.path {
            service.adopt(&DocumentId::for_path(path), &mut self.history);
        }
    }

    // ── Editing ────────────────────────────────────────────────────────────

    /// Change the asset with `change`. Does nothing, and records nothing,
    /// when the asset comes out the same.
    pub fn edit(
        &mut self,
        label: &'static str,
        merge_key: Option<&'static str>,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut A),
    ) {
        let before = self.asset.clone();
        change(&mut self.asset);
        if self.asset == before {
            return;
        }
        self.status = None;
        self.revision += 1;
        lock(&self.history).push(AssetEdit { other: before, label, key: merge_key, at: Instant::now() });
        cx.notify();
    }


    // ── Gestures ───────────────────────────────────────────────────────────
    //
    // A drag changes the asset many times but should undo as one step. The
    // gesture takes a snapshot first, changes the asset live without
    // recording anything, and records one undo step at the end (or puts the
    // snapshot back if cancelled).

    /// The asset as it is now, to hand to [`Self::commit_gesture`] or
    /// [`Self::cancel_gesture`].
    pub fn snapshot(&self) -> A {
        self.asset.clone()
    }

    /// Change the asset without recording history.
    pub fn live(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut A)) {
        change(&mut self.asset);
        self.revision += 1;
        cx.notify();
    }

    /// Record the change since `before` as one undo step.
    pub fn commit_gesture(&mut self, before: A, label: &'static str, cx: &mut Context<Self>) {
        if self.asset == before {
            return;
        }
        self.status = None;
        lock(&self.history).push(AssetEdit { other: before, label, key: None, at: Instant::now() });
        cx.notify();
    }

    /// Put the asset back as it was at the start of the gesture.
    pub fn cancel_gesture(&mut self, before: A, cx: &mut Context<Self>) {
        self.asset = before;
        self.revision += 1;
        cx.notify();
    }

    pub fn undo(&mut self, cx: &mut Context<Self>) {
        let history = self.history.clone();
        let asset = &mut self.asset;
        let done = lock(&history)
            .undo(|edit| {
                std::mem::swap(asset, &mut edit.other);
                true
            })
            .is_some();
        if done {
            self.status = None;
            self.revision += 1;
            cx.notify();
        }
    }

    pub fn redo(&mut self, cx: &mut Context<Self>) {
        let history = self.history.clone();
        let asset = &mut self.asset;
        let done = lock(&history)
            .redo(|edit| {
                std::mem::swap(asset, &mut edit.other);
                true
            })
            .is_some();
        if done {
            self.status = None;
            self.revision += 1;
            cx.notify();
        }
    }

    /// Replace the unreadable file's contents with a fresh asset (the user
    /// chose to give up on what was there).
    pub fn start_fresh(&mut self, cx: &mut Context<Self>) {
        self.unreadable = None;
        self.asset = A::new_default();
        self.revision += 1;
        lock(&self.history).clear();
        // Differs from the (unreadable) file on disk, so it's unsaved.
        lock(&self.history).push(AssetEdit {
            other: A::new_default(),
            label: "Start over",
            key: None,
            at: Instant::now(),
        });
        cx.notify();
    }

    // ── Files ──────────────────────────────────────────────────────────────

    pub fn save(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.path.clone() else { return };
        if self.unreadable.is_some() {
            return;
        }
        match std::fs::write(&path, self.asset.to_text()) {
            Ok(()) => {
                lock(&self.history).mark_saved();
                self.status = Some(Status::Saved);
                SaveBus::announce(cx);
            }
            Err(error) => {
                tracing::error!("couldn't save {}: {error}", path.display());
                self.status = Some(Status::SaveFailed(error.to_string()));
            }
        }
        cx.notify();
    }

    /// Throw away unsaved changes and read the file again.
    pub fn revert(&mut self, cx: &mut Context<Self>) {
        self.load_from_disk();
        lock(&self.history).clear();
        self.status = Some(Status::Reverted);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    // Explicit imports: `super::*` would pull in `gpui::*`, whose own `test`
    // macro shadows the built-in `#[test]` attribute.
    use super::{AssetEdit, Document, Instant, MERGE_WINDOW};
    use crate::model::WidgetAsset;
    use plugin_editor_api::undo::lock;
    use plugin_editor_api::SharedHistory;
    use std::time::Duration;

    fn swap_back(history: &SharedHistory<AssetEdit<WidgetAsset>>, asset: &mut WidgetAsset) {
        lock(history).undo(|edit| {
            std::mem::swap(asset, &mut edit.other);
            true
        });
    }

    /// The history logic without GPUI: the same operations on the pieces.
    #[test]
    fn edits_undo_and_merge_by_key() {
        let history: SharedHistory<AssetEdit<WidgetAsset>> = Default::default();
        let mut asset = WidgetAsset::new();
        let push = |asset: &mut WidgetAsset, key: Option<&'static str>, width: f32| {
            let before = asset.clone();
            asset.design_size.x = width;
            lock(&history).push(AssetEdit { other: before, label: "size", key, at: Instant::now() });
        };
        push(&mut asset, Some("w"), 1000.0);
        push(&mut asset, Some("w"), 1100.0);
        push(&mut asset, Some("w"), 1200.0);
        assert_eq!(lock(&history).len(), 1, "a drag is one undo step");
        std::thread::sleep(MERGE_WINDOW + Duration::from_millis(50));
        push(&mut asset, Some("w"), 1300.0);
        assert_eq!(lock(&history).len(), 2);
        swap_back(&history, &mut asset);
        assert_eq!(asset.design_size.x, 1200.0);
        swap_back(&history, &mut asset);
        assert_eq!(asset.design_size.x, WidgetAsset::DEFAULT_SIZE.x, "undo restores the state before the whole drag");
    }

    #[test]
    fn a_missing_file_is_new_a_broken_one_is_protected_and_an_idless_one_is_stamped() {
        let dir = std::env::temp_dir().join(format!("ui_editor_doc_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let missing = Document::<WidgetAsset>::open(Some(dir.join("nope.widget")));
        assert!(missing.unreadable().is_none());
        assert!(missing.can_save());

        let broken = dir.join("broken.widget");
        std::fs::write(&broken, "{ definitely not json").unwrap();
        let doc = Document::<WidgetAsset>::open(Some(broken.clone()));
        assert!(doc.unreadable().is_some());
        assert!(!doc.can_save(), "must not overwrite a file it couldn't read");
        assert_eq!(std::fs::read_to_string(&broken).unwrap(), "{ definitely not json");

        // A file created from the template has no id; opening it writes one.
        let fresh = dir.join("fresh.widget");
        std::fs::write(&fresh, "").unwrap();
        let doc = Document::<WidgetAsset>::open(Some(fresh.clone()));
        let on_disk = WidgetAsset::peek_id(&std::fs::read_to_string(&fresh).unwrap());
        assert_eq!(on_disk.as_ref(), Some(&doc.asset().id), "the id is written when the file is opened");
        assert!(!doc.is_dirty(), "stamping an id is not an unsaved change");

        // Opening it again keeps the id.
        let again = Document::<WidgetAsset>::open(Some(fresh));
        assert_eq!(again.asset().id, doc.asset().id);
        std::fs::remove_dir_all(&dir).ok();
    }
}
