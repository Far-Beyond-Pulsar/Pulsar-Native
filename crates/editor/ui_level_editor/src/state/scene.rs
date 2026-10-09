//! Scene Domain — the actual level data (objects, hierarchy, play-mode snapshots)
//!
//! It holds the [`SharedScene`] (the `pulsar_scenedb::SceneDb` shared with the
//! renderer) plus the editor-only state around it: mode, file path, undo history.
//!
//! The `revision` counter is bumped on every mutation so that observer tasks
//! (running on the GPUI main thread) can detect changes made by background
//! threads (AI tools, asset import, etc.) and trigger a re-render.

use crate::scene_edit::history::VoxelEditJournal;
use crate::scene_edit::{self, ObjectId, SceneHistoryDelta, SceneHistorySnapshot, SceneObjectData};
use crate::world_settings_data::WorldSettingsData;
use engine_backend::scene::SceneWorldExt;
use engine_backend::scene::SharedScene;
use parking_lot::{
    MappedRwLockReadGuard, MappedRwLockWriteGuard, RwLock, RwLockReadGuard, RwLockWriteGuard,
};
use pulsar_scenedb::{Entity, World};
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

/// Undo/redo history depth cap (Pulsar-Native#554). Each entry is a full
/// scene snapshot (`scene_edit::history::capture_history_snapshot`'s cost is
/// O(scene size) -- see that method's doc), so this bounds memory rather
/// than letting an unbounded session-long history grow forever. Not tuned
/// against a real project yet; a reasonable starting point for a v1.
/// Hard cap on retained undo/redo checkpoints. These contain scene snapshots,
/// so this is a memory and worst-case restore-cost limit, not just a UI limit.
const MAX_UNDO_HISTORY: usize = 32;

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
    /// Settings persisted alongside the current level's scene data.
    pub world_settings: WorldSettingsData,
    /// Bumped whenever the whole scene is rebuilt in place (leaving play
    /// mode). Objects are respawned, so anything caching entities must rebind
    /// when this moves.
    pub rebuild_epoch: u64,
    /// Snapshot of scene state when entering play mode (for reset on stop).
    /// Immutable snapshot captured before PIE; it carries parent links and
    /// component instances atomically.
    pub snapshot: Option<SceneHistorySnapshot>,
    /// Every entity alive when the snapshot was taken. Stop despawns the
    /// rest, including entities spawned during Play without a StableId
    /// (which the snapshot restore can't see).
    play_entities: Option<HashSet<Entity>>,
    /// Current editor mode.
    pub editor_mode: EditorMode,
    /// Bumped when a class update rebuilt placed instances (#935), from
    /// whatever thread published it; the level editor redraws on a change.
    pub class_updates: u64,
    /// Currently open scene file path.
    pub current_scene: Option<PathBuf>,
    /// Whether the scene has unsaved changes.
    pub has_unsaved_changes: bool,
    /// Monotonic revision counter — bumped on every mutation so pollers
    /// (and the observer system) can detect external changes.
    pub revision: u64,
    /// Set by callers without a renderer handle (the AI tools) after undo/redo,
    /// which may change the selection; the panel poller takes it and points
    /// the gizmo at the restored selection.
    pub pending_selection_sync: bool,
    /// Undo history (Pulsar-Native#554) — one entry per mutating
    /// `SceneCommand` (`commands.rs::execute_command` pushes onto this),
    /// oldest first. Bounded at [`MAX_UNDO_HISTORY`].
    undo_stack: VecDeque<SceneHistoryDelta>,
    /// Redo history — populated by [`Self::undo`], cleared by every new
    /// mutating command (standard undo/redo semantics: once you make a new
    /// change, the old "future" you undid past is gone).
    redo_stack: VecDeque<SceneHistoryDelta>,
    voxel_undo: VecDeque<VoxelEditJournal>,
    voxel_redo: VecDeque<VoxelEditJournal>,
}

impl Default for SceneDomain {
    fn default() -> Self {
        Self {
            scene: Arc::new(RwLock::new(engine_backend::scene::new_scene())),
            world_settings: WorldSettingsData::default(),
            rebuild_epoch: 0,
            snapshot: None,
            play_entities: None,
            editor_mode: EditorMode::Edit,
            class_updates: 0,
            current_scene: None,
            has_unsaved_changes: false,
            revision: 0,
            pending_selection_sync: false,
            undo_stack: VecDeque::with_capacity(MAX_UNDO_HISTORY),
            redo_stack: VecDeque::with_capacity(MAX_UNDO_HISTORY),
            voxel_undo: VecDeque::new(),
            voxel_redo: VecDeque::new(),
        }
    }
}

impl SceneDomain {
    /// Make the lightweight scene copy used by read-only tool UI queries.
    ///
    /// Tool-mode toolbar/status generation needs the editor fields and a
    /// shared scene handle, but never needs undo snapshots. Cloning those
    /// here made toolbar refresh cost grow with voxel history size.
    pub(crate) fn clone_for_tool_query(&self) -> Self {
        Self {
            scene: Arc::clone(&self.scene),
            world_settings: self.world_settings.clone(),
            rebuild_epoch: self.rebuild_epoch,
            snapshot: None,
            play_entities: None,
            editor_mode: self.editor_mode,
            class_updates: self.class_updates,
            current_scene: self.current_scene.clone(),
            has_unsaved_changes: self.has_unsaved_changes,
            revision: self.revision,
            pending_selection_sync: self.pending_selection_sync,
            undo_stack: VecDeque::new(),
            redo_stack: VecDeque::new(),
            voxel_undo: VecDeque::new(),
            voxel_redo: VecDeque::new(),
        }
    }

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
    // Restores write through the World like any edit, so the renderer and
    // every change watch follow them; nothing needs a resync afterward.

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
        self.undo_stack.push_back(SceneHistoryDelta {
            before: pre_state,
            after: post_state,
        });
        if self.undo_stack.len() > MAX_UNDO_HISTORY {
            self.undo_stack.pop_front();
        }
        self.redo_stack.clear();
    }

    /// Commit a voxel append-range without capturing the terrain component.
    pub fn commit_voxel_journal(&mut self, journal: VoxelEditJournal) {
        self.voxel_undo.push_back(journal);
        if self.voxel_undo.len() > MAX_UNDO_HISTORY {
            self.voxel_undo.pop_front();
        }
        self.voxel_redo.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty() || !self.voxel_undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty() || !self.voxel_redo.is_empty()
    }

    /// Undo the last mutating command. Returns `true` if something was
    /// undone (the caller should then bump the revision/mark unsaved, which
    /// this method deliberately leaves to the caller since it has no `cx` to
    /// notify with here).
    pub fn undo(&mut self) -> bool {
        if let Some(journal) = self.voxel_undo.pop_back() {
            if !self.apply_voxel_journal(&journal, false) {
                self.voxel_undo.push_back(journal);
                return false;
            }
            self.voxel_redo.push_back(journal);
            return true;
        }
        let Some(delta) = self.undo_stack.pop_back() else {
            return false;
        };
        let ids = delta.ids();
        let current = scene_edit::history::capture_history_subset(&self.world(), &ids);
        if !self.restore_delta(&delta.before, &ids) {
            self.undo_stack.push_back(delta);
            return false;
        }
        // Redo returns to the state just left. Keeping `before` keeps the
        // ids it names in scope, so redoing a removal removes the object.
        self.redo_stack.push_back(SceneHistoryDelta {
            before: delta.before,
            after: current,
        });
        if self.redo_stack.len() > MAX_UNDO_HISTORY {
            self.redo_stack.pop_front();
        }
        true
    }

    /// Redo the last undone command. See [`Self::undo`]'s doc for the
    /// caller's responsibilities on success.
    pub fn redo(&mut self) -> bool {
        if let Some(journal) = self.voxel_redo.pop_back() {
            if !self.apply_voxel_journal(&journal, true) {
                self.voxel_redo.push_back(journal);
                return false;
            }
            self.voxel_undo.push_back(journal);
            return true;
        }
        let Some(delta) = self.redo_stack.pop_back() else {
            return false;
        };
        let ids = delta.ids();
        let current = scene_edit::history::capture_history_subset(&self.world(), &ids);
        if !self.restore_delta(&delta.after, &ids) {
            self.redo_stack.push_back(delta);
            return false;
        }
        // Undo returns to the state just left; `after` keeps its ids in
        // scope, so undoing a redone add removes the object.
        self.undo_stack.push_back(SceneHistoryDelta {
            before: current,
            after: delta.after,
        });
        if self.undo_stack.len() > MAX_UNDO_HISTORY {
            self.undo_stack.pop_front();
        }
        true
    }

    fn apply_voxel_journal(&mut self, journal: &VoxelEditJournal, redo: bool) -> bool {
        let mut world = self.world_mut();
        for entry in &journal.entries {
            let Some(entity) =
                engine_backend::scene::attachments::instance_by_id(&world, entry.instance)
            else {
                return false;
            };
            let Some(mut terrain) = world.get_mut::<helio_component::VoxelTerrainComponent>(entity)
            else {
                return false;
            };
            if redo {
                if terrain.edits.len() != entry.before_len {
                    return false;
                }
                terrain.edits.extend(entry.edits.iter().cloned());
                terrain.source_revision = entry.after_revision;
            } else {
                // Edits folded into the journal's base (a save) are no
                // longer undoable.
                if terrain.edits.len() < entry.before_len + entry.edits.len()
                    || terrain.edits.base_len() > entry.before_len
                {
                    return false;
                }
                while terrain.edits.len() > entry.before_len {
                    terrain.edits.pop();
                }
                terrain.source_revision = entry.before_revision;
            }
        }
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
    ///
    /// The snapshot is the world as it was before the FIRST Play: pressing
    /// Play again while a game runs (a hot reload) keeps it, so Stop still
    /// restores the pre-Play world, not a mid-play one.
    pub fn enter_play_mode(&mut self) {
        if self.snapshot.is_none() {
            self.snapshot = Some(self.capture_history_snapshot());
            let live = self
                .world()
                .query::<()>()
                .map(|(entity, ())| entity)
                .collect();
            self.play_entities = Some(live);
        }
        self.editor_mode = EditorMode::Play;
    }

    /// Whether a pre-Play snapshot is waiting to be restored.
    pub fn has_play_snapshot(&self) -> bool {
        self.snapshot.is_some()
    }

    /// Exit play mode — restore scene state from snapshot.
    ///
    /// The restore rebuilds the scene from the pre-Play snapshot: every
    /// object with a StableId is removed first, which includes everything
    /// scripts spawned during Play (`world::spawn` objects carry runtime
    /// StableIds, `<Class>_rt<n>`), and the snapshot's objects come back
    /// with their components. Entities spawned during Play without a
    /// StableId (e.g. by native actors) are despawned first. Call it only
    /// once the game stopped (see `end_pie`), so no game code runs against
    /// the restored world.
    pub fn exit_play_mode(&mut self) {
        if let Some(before) = self.play_entities.take() {
            let mut world = self.world_mut();
            let spawned: Vec<Entity> = world
                .query::<()>()
                .map(|(entity, ())| entity)
                .filter(|entity| !before.contains(entity))
                .collect();
            for entity in spawned {
                if world.is_alive(entity) {
                    world.despawn(entity);
                }
            }
        }
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
