//! Scene Domain — the actual level data (objects, hierarchy, play-mode snapshots)
//!
//! It holds the [`SharedScene`] (the `pulsar_scenedb::SceneDb` shared with the
//! renderer) plus the editor-only state around it: mode, file path, undo history.
//!
//! The `revision` counter is bumped on every mutation so that observer tasks
//! (running on the GPUI main thread) can detect changes made by background
//! threads (AI tools, asset import, etc.) and trigger a re-render.

use crate::level_editor::scene_edit::{
    self, ObjectId, SceneHistoryDelta, SceneHistorySnapshot, SceneObjectData,
};
use engine_backend::scene::SharedScene;
use parking_lot::{
    MappedRwLockReadGuard, MappedRwLockWriteGuard, RwLock, RwLockReadGuard, RwLockWriteGuard,
};
use pulsar_scenedb::World;
use std::path::PathBuf;
use std::sync::Arc;

/// Undo/redo history depth cap (Pulsar-Native#554). Each entry is a full
/// scene snapshot (`scene_edit::history::capture_history_snapshot`'s cost is
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
/// - `scene` — the SceneDB scene shared with the renderer; read/edit its world directly.
/// - `editor_mode` — `Edit` or `Play`.
/// - `current_scene` — path to the currently open `.level` file on disk.
/// - `has_unsaved_changes` — set by every mutation, cleared on save.
/// - `revision` — monotonic counter bumped on every mutation.
/// - `snapshot` — play-mode snapshot captured on `enter_play_mode`.
#[derive(Clone)]
pub struct SceneDomain {
    /// The scene — single source of truth for all scene data. Panels read and
    /// edit its `World` directly (see [`Self::world`] / [`Self::world_mut`]).
    pub scene: SharedScene,
    /// Bumped whenever the whole scene is rebuilt in place (undo/redo, leaving
    /// play mode). Subscriptions die with the entities they watched, so anything
    /// caching against them must re-arm when this moves.
    pub rebuild_epoch: u64,
    /// Snapshot of scene state when entering play mode (for reset on stop).
    /// Immutable snapshot captured before PIE; it carries parent links and
    /// component instances atomically.
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
    undo_stack: Vec<SceneHistoryDelta>,
    /// Redo history — populated by [`Self::undo`], cleared by every new
    /// mutating command (standard undo/redo semantics: once you make a new
    /// change, the old "future" you undid past is gone).
    redo_stack: Vec<SceneHistoryDelta>,
}

impl Default for SceneDomain {
    fn default() -> Self {
        Self {
            scene: Arc::new(RwLock::new(engine_backend::scene::new_scene())),
            rebuild_epoch: 0,
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
    // ── The world ─────────────────────────────────────────────────────────

    /// Read access to the scene's `World`. Holds the scene lock for as long as
    /// the guard lives; do not hold it across a call that also locks the scene.
    pub fn world(&self) -> MappedRwLockReadGuard<'_, World> {
        RwLockReadGuard::map(self.scene.read(), |scene| &scene.world)
    }

    /// Write access to the scene's `World`. Same locking rule as [`Self::world`].
    pub fn world_mut(&self) -> MappedRwLockWriteGuard<'_, World> {
        RwLockWriteGuard::map(self.scene.write(), |scene| &mut scene.world)
    }

    /// Monotonic count of every mutation the world has recorded -- what panel
    /// frame pumps compare to notice the scene changed.
    pub fn world_revision(&self) -> u64 {
        self.scene.read().world.revision()
    }

    /// The shared scene handle, for consumers that must hold the same world the
    /// editor mutates (the PIE host handing its world to the guest, the renderer).
    pub fn shared_scene(&self) -> SharedScene {
        Arc::clone(&self.scene)
    }

    /// Rebuild generation; see [`Self::rebuild_epoch`]. Subscriptions armed against an
    /// older generation are dead and must be re-armed.
    pub fn subscriptions_epoch(&self) -> u64 {
        self.rebuild_epoch
    }

    // ── Selection ─────────────────────────────────────────────────────────

    pub fn selected_object(&self) -> Option<ObjectId> {
        scene_edit::objects::get_selected_object_id(&self.world())
    }

    pub fn select_object(&mut self, object_id: Option<ObjectId>) {
        scene_edit::objects::select_object(&mut self.world_mut(), object_id.as_deref());
    }

    pub fn get_selected_object(&self) -> Option<SceneObjectData> {
        scene_edit::objects::get_selected_object(&self.world())
    }

    // ── Scene traversal ───────────────────────────────────────────────────

    pub fn scene_objects(&self) -> Vec<SceneObjectData> {
        scene_edit::objects::get_root_objects(&self.world())
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
        scene_edit::history::capture_history_snapshot(&self.world())
    }

    /// Commit a previously captured pre-state onto the undo stack and clear
    /// the redo stack. Called by `execute_command` only when the command it
    /// preceded actually changed something.
    pub fn commit_undo_checkpoint(
        &mut self,
        pre_state: SceneHistorySnapshot,
        post_state: SceneHistorySnapshot,
    ) {
        self.undo_stack.push(SceneHistoryDelta {
            before: pre_state,
            after: post_state,
        });
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
        let Some(delta) = self.undo_stack.pop() else {
            return false;
        };
        let ids = delta.ids();
        let current = scene_edit::history::capture_history_subset(&self.world(), &ids);
        if !self.restore_delta(&delta.before, &ids) {
            self.undo_stack.push(delta);
            return false;
        }
        self.redo_stack.push(SceneHistoryDelta {
            before: current,
            after: delta.after,
        });
        true
    }

    /// Redo the last undone command. See [`Self::undo`]'s doc for the
    /// caller's responsibilities on success.
    pub fn redo(&mut self) -> bool {
        let Some(delta) = self.redo_stack.pop() else {
            return false;
        };
        let ids = delta.ids();
        let current = scene_edit::history::capture_history_subset(&self.world(), &ids);
        if !self.restore_delta(&delta.after, &ids) {
            self.redo_stack.push(delta);
            return false;
        }
        self.undo_stack.push(SceneHistoryDelta {
            before: delta.before,
            after: current,
        });
        true
    }

    /// Rebuild the scene from `snapshot` in place. Bumps [`Self::rebuild_epoch`]
    /// on success; on failure the live scene is untouched.
    fn restore(&mut self, snapshot: &SceneHistorySnapshot) -> bool {
        let result = scene_edit::history::restore_history_snapshot(&mut self.world_mut(), snapshot);
        match result {
            Ok(()) => {
                self.rebuild_epoch = self.rebuild_epoch.wrapping_add(1);
                true
            }
            Err(error) => {
                tracing::error!(%error, "scene restore rejected");
                false
            }
        }
    }

    fn restore_delta(&mut self, snapshot: &SceneHistorySnapshot, ids: &[ObjectId]) -> bool {
        match scene_edit::history::restore_history_delta(&mut self.world_mut(), snapshot, ids) {
            Ok(()) => true,
            Err(error) => {
                tracing::error!(%error, "scoped scene restore rejected");
                false
            }
        }
    }

    // ── Play mode ─────────────────────────────────────────────────────────

    /// Enter play mode — snapshot scene and start game thread.
    pub fn enter_play_mode(&mut self) {
        self.snapshot = Some(self.capture_history_snapshot());
        self.editor_mode = EditorMode::Play;
    }

    /// Exit play mode — restore scene state from snapshot.
    pub fn exit_play_mode(&mut self) {
        if let Some(snapshot) = self.snapshot.take() {
            // The restore rebuilds the complete hierarchy and rehydrates registered
            // components together with the object data.
            if !self.restore(&snapshot) {
                tracing::error!("failed to restore the editor scene after play mode");
            }
        }
        self.editor_mode = EditorMode::Edit;
    }
}
