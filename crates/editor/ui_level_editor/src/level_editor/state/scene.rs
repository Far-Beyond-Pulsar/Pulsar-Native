//! Scene Domain — the actual level data (objects, hierarchy, play-mode snapshots)
//!
//! This is the only domain that directly wraps [`SceneDatabase`] and is therefore
//! the bridge to the `RwLock<WorldSceneStore>` shared with the renderer.
//!
//! The `revision` counter is bumped on every mutation so that observer tasks
//! (running on the GPUI main thread) can detect changes made by background
//! threads (AI tools, asset import, etc.) and trigger a re-render.

use crate::level_editor::scene_database::{ObjectId, SceneHistorySnapshot, SceneObjectData};
use crate::level_editor::SceneDatabase;
use std::path::PathBuf;

/// Undo/redo history depth cap (Pulsar-Native#554). Each entry is a full
/// scene snapshot (`SceneDatabase::capture_history_snapshot`'s cost is
/// O(scene size) -- see that method's doc), so this bounds memory rather
/// than letting an unbounded session-long history grow forever. Not tuned
/// against a real project yet; a reasonable starting point for a v1.
const MAX_UNDO_HISTORY: usize = 100;

// ── Editor mode ────────────────────────────────────────────────────────────

/// Editor mode — either editing the scene or playing it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorMode {
    /// Editing mode — gizmos active, game thread paused.
    Edit,
    /// Play mode — game running, gizmos hidden.
    Play,
}

// ── Scene domain ──────────────────────────────────────────────────────────

/// Scene-level state — the authoritative source for all scene object data.
///
/// Fields:
/// - `database` — the `SceneDatabase` (wraps `Arc<RwLock<WorldSceneStore>>` + `SceneMetadataDb`).
/// - `editor_mode` — `Edit` or `Play`.
/// - `current_scene` — path to the currently open `.level` file on disk.
/// - `has_unsaved_changes` — set by every mutation, cleared on save.
/// - `revision` — monotonic counter bumped on every mutation.
/// - `snapshot` — play-mode snapshot captured on `enter_play_mode`.
#[derive(Clone)]
pub struct SceneDomain {
    /// Scene database — single source of truth for all scene data.
    pub database: SceneDatabase,
    /// Snapshot of scene state when entering play mode (for reset on stop).
    /// Immutable SceneDB snapshot captured before PIE.  Keep the database's
    /// native snapshot here instead of a second object-shaped store; it
    /// carries parent links and component metadata atomically.
    pub snapshot: Option<SceneHistorySnapshot>,
    /// Current editor mode.
    pub editor_mode: EditorMode,
    /// Currently open scene file path.
    pub current_scene: Option<PathBuf>,
    /// Whether the scene has unsaved changes.
    pub has_unsaved_changes: bool,
    /// Monotonic revision counter — bumped on every mutation so pollers
    /// (and the observer system) can detect external changes.
    pub revision: u64,
    /// Undo history (Pulsar-Native#554) — one entry per mutating
    /// `SceneCommand` (`commands.rs::execute_command` pushes onto this),
    /// oldest first. Bounded at [`MAX_UNDO_HISTORY`].
    undo_stack: Vec<SceneHistorySnapshot>,
    /// Redo history — populated by [`Self::undo`], cleared by every new
    /// mutating command (standard undo/redo semantics: once you make a new
    /// change, the old "future" you undid past is gone).
    redo_stack: Vec<SceneHistorySnapshot>,
}

impl Default for SceneDomain {
    fn default() -> Self {
        let database = SceneDatabase::new();
        Self {
            database,
            snapshot: None,
            editor_mode: EditorMode::Edit,
            current_scene: None,
            has_unsaved_changes: false,
            revision: 0,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }
}

impl SceneDomain {
    // ── Selection ─────────────────────────────────────────────────────────

    pub fn selected_object(&self) -> Option<ObjectId> {
        self.database.get_selected_object_id()
    }

    pub fn select_object(&mut self, object_id: Option<ObjectId>) {
        self.database.select_object(object_id);
    }

    pub fn get_selected_object(&self) -> Option<SceneObjectData> {
        self.database.get_selected_object()
    }

    // ── Scene traversal ───────────────────────────────────────────────────

    pub fn scene_objects(&self) -> Vec<SceneObjectData> {
        self.database.get_root_objects()
    }

    // ── Editor mode helpers ──────────────────────────────────────────────

    pub fn is_edit_mode(&self) -> bool {
        self.editor_mode == EditorMode::Edit
    }

    pub fn is_play_mode(&self) -> bool {
        self.editor_mode == EditorMode::Play
    }

    // ── Revision tracking ────────────────────────────────────────────────

    /// Bump the revision counter and optionally mark the scene as unsaved.
    pub fn bump_revision(&mut self, marks_unsaved: bool) {
        self.revision = self.revision.saturating_add(1);
        if marks_unsaved {
            self.has_unsaved_changes = true;
        }
    }

    // ── Undo/redo (Pulsar-Native#554) ────────────────────────────────────
    //
    // v1, correctness-first: whole-scene snapshot per step, no per-command
    // diffing. `commands.rs::execute_command` is the sole call site that
    // pushes onto `undo_stack` -- it captures the pre-state before running a
    // mutating command and commits it here only if the command actually
    // changed something, so no-op commands and selection changes never
    // clutter the history. `undo`/`redo` themselves are not run through
    // `execute_command` -- they have their own pre/post semantics (push onto
    // the *other* stack) that don't fit that flow.
    //
    // Note for callers driving the renderer (`panel.rs`'s `on_undo`/
    // `on_redo`): a successful restore replaces `WorldSceneStore` wholesale,
    // which the renderer's delta-sync path (`HelioRenderer::sync_scene_delta`)
    // can't correctly diff against its own `known_ids`/cache state -- it was
    // never told about entities that silently stopped existing because the
    // whole store swapped rather than being individually despawned. Callers
    // MUST force a full resync afterward (`GpuRenderer::force_full_resync`)
    // or removed objects can be left behind in the Helio scene. See that
    // method's doc.

    /// Capture the scene's current state for later restore. Exposed so
    /// `execute_command` can capture *before* running a command (the state
    /// undo should return to), not after.
    pub fn capture_history_snapshot(&self) -> SceneHistorySnapshot {
        self.database.capture_history_snapshot()
    }

    /// Commit a previously captured pre-state onto the undo stack and clear
    /// the redo stack. Called by `execute_command` only when the command it
    /// preceded actually changed something.
    pub fn commit_undo_checkpoint(&mut self, pre_state: SceneHistorySnapshot) {
        self.undo_stack.push(pre_state);
        if self.undo_stack.len() > MAX_UNDO_HISTORY {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    /// Undo the last mutating command. Returns `true` if something was
    /// undone (the caller should then force a renderer resync -- see this
    /// section's top doc -- and bump the revision/mark unsaved, which this
    /// method deliberately leaves to the caller since it has no `cx` to
    /// notify with here).
    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.undo_stack.pop() else {
            return false;
        };
        let current = self.database.capture_history_snapshot();
        if self.database.restore_history_snapshot(&previous).is_err() {
            // Restore failed (malformed snapshot) -- put it back so the
            // entry isn't silently lost, and leave the redo stack alone.
            self.undo_stack.push(previous);
            return false;
        }
        self.redo_stack.push(current);
        true
    }

    /// Redo the last undone command. See [`Self::undo`]'s doc for the
    /// caller's responsibilities on success.
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo_stack.pop() else {
            return false;
        };
        let current = self.database.capture_history_snapshot();
        if self.database.restore_history_snapshot(&next).is_err() {
            self.redo_stack.push(next);
            return false;
        }
        self.undo_stack.push(current);
        true
    }

    // ── Play mode ─────────────────────────────────────────────────────────

    /// Enter play mode — snapshot scene and start game thread.
    pub fn enter_play_mode(&mut self) {
        self.snapshot = Some(self.database.capture_history_snapshot());
        self.editor_mode = EditorMode::Play;
    }

    /// Exit play mode — restore scene state from snapshot.
    pub fn exit_play_mode(&mut self) {
        if let Some(snapshot) = self.snapshot.take() {
            // WorldSceneStore restores the complete hierarchy in one
            // transaction and rehydrates registered components together with
            // the object data. This avoids the old clear-and-readd loop,
            // which turned every object into a root and silently regenerated
            // IDs when cleanup was incomplete.
            if let Err(error) = self.database.restore_history_snapshot(&snapshot) {
                tracing::error!(%error, "failed to restore the editor scene after play mode");
            }
        }
        self.editor_mode = EditorMode::Edit;
    }
}
