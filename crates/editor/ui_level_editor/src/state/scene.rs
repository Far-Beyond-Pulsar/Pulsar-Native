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
use engine_backend::scene::{SceneWorldExt as _, SharedScene};
use engine_backend::subsystems::render::VoxelChunkSaves;
use parking_lot::{
    MappedRwLockReadGuard, MappedRwLockWriteGuard, RwLock, RwLockReadGuard, RwLockWriteGuard,
};
use plugin_editor_api::undo::{lock, DocumentId, History, SharedHistory, UndoEntry, UndoService};
use pulsar_scenedb::{Entity, World};
use std::collections::HashSet;
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
    /// The level's undo history (Pulsar-Native#554, #931): one entry per
    /// mutating `SceneCommand` (`execute_command` commits them) or voxel
    /// stroke, in the order they were made, bounded at
    /// [`MAX_UNDO_HISTORY`]. Registered with the host's `UndoService` under
    /// the level's file (see [`Self::set_current_scene`]).
    history: SharedHistory<SceneEdit>,
    /// The host's undo histories, when the editor was given them: the
    /// level's history is registered there under its file.
    pub undo_service: Option<UndoService>,
    /// Where saves ask the viewport's renderer for the voxel terrain's
    /// edited chunks (`ui::save`), once the viewport has one.
    pub voxel_chunk_saves: Option<VoxelChunkSaves>,
}

/// One undoable change to a level.
#[derive(Debug)]
pub enum SceneEdit {
    /// A command's before/after state of the objects it touched.
    Scene(SceneHistoryDelta),
    /// A voxel sculpt stroke: the edits it appended.
    Voxel(VoxelEditJournal),
}

impl UndoEntry for SceneEdit {
    fn label(&self) -> String {
        let key = match self {
            SceneEdit::Scene(delta) => delta.label,
            SceneEdit::Voxel(_) => Some("LevelEditor.Undo.Sculpt"),
        };
        key.map(|key| rust_i18n::t!(key).to_string())
            .unwrap_or_default()
    }
}

fn new_history() -> SharedHistory<SceneEdit> {
    Arc::new(std::sync::Mutex::new(History::with_capacity(
        MAX_UNDO_HISTORY,
    )))
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
            history: new_history(),
            undo_service: None,
            voxel_chunk_saves: None,
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
            history: new_history(),
            undo_service: self.undo_service.clone(),
            voxel_chunk_saves: self.voxel_chunk_saves.clone(),
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
    // commits scene edits -- it captures the pre-state before running a
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

    /// Show the level stored at `path` (`None`: a never-saved one), and use
    /// its history. `keep_history` carries the edits made so far over, for
    /// the same level saved under a new name; opening or creating a level
    /// starts from that level's own history instead, so an undo never
    /// applies another level's edits.
    pub fn set_current_scene(&mut self, path: Option<PathBuf>, keep_history: bool) {
        self.current_scene = path;
        if !keep_history {
            self.history = new_history();
        }
        if let (Some(service), Some(path)) = (&self.undo_service, &self.current_scene) {
            service.adopt(&DocumentId::for_path(path), &self.history);
        }
    }

    /// Use the host's undo histories from now on, keeping the edits made
    /// so far.
    pub fn set_undo_service(&mut self, service: UndoService) {
        self.undo_service = Some(service);
        self.set_current_scene(self.current_scene.clone(), true);
    }

    /// The level's history, for menus and the host's undo service.
    pub fn history(&self) -> &SharedHistory<SceneEdit> {
        &self.history
    }

    /// Commit a previously captured pre-state as one undoable edit. Called
    /// by `execute_command` only when the command it preceded actually
    /// changed something.
    pub fn commit_undo_checkpoint(
        &mut self,
        pre_state: SceneHistorySnapshot,
        post_state: SceneHistorySnapshot,
    ) {
        self.commit_labeled_checkpoint(pre_state, post_state, None);
    }

    /// [`Self::commit_undo_checkpoint`], named for the Edit menu by the
    /// locale key `label`.
    pub fn commit_labeled_checkpoint(
        &mut self,
        pre_state: SceneHistorySnapshot,
        post_state: SceneHistorySnapshot,
        label: Option<&'static str>,
    ) {
        let mut history = lock(&self.history);
        history.push(SceneEdit::Scene(SceneHistoryDelta {
            before: pre_state,
            after: post_state,
            label,
        }));
        history.seal();
    }

    /// Record a finished gizmo drag as one undo step (#931). The renderer
    /// moves the object directly while dragging; this captures the object
    /// as it was (by briefly putting the starting transform back, so a
    /// placed class's overrides are captured the same way as any edit) and
    /// as it is now.
    pub fn record_gizmo_drag(
        &mut self,
        entity: Entity,
        before: engine_backend::scene::Transform,
    ) -> bool {
        let (pre, post) = {
            let mut world = self.world_mut();
            let Some(id) = world.stable_id_of(entity).map(str::to_string) else {
                return false;
            };
            let Some(now) = world
                .get::<engine_backend::scene::Transform>(entity)
                .copied()
            else {
                return false;
            };
            if now == before {
                return false;
            }
            let ids = [id];
            let post = scene_edit::history::capture_history_subset(&world, &ids);
            if let Some(mut transform) = world.get_mut::<engine_backend::scene::Transform>(entity) {
                *transform = before;
            }
            let pre = scene_edit::history::capture_history_subset(&world, &ids);
            if let Some(mut transform) = world.get_mut::<engine_backend::scene::Transform>(entity) {
                *transform = now;
            }
            (pre, post)
        };
        self.commit_labeled_checkpoint(pre, post, Some("LevelEditor.Undo.Move"));
        true
    }

    /// Commit a voxel append-range without capturing the terrain component.
    pub fn commit_voxel_journal(&mut self, journal: VoxelEditJournal) {
        let mut history = lock(&self.history);
        history.push(SceneEdit::Voxel(journal));
        history.seal();
    }

    pub fn can_undo(&self) -> bool {
        lock(&self.history).can_undo()
    }

    pub fn can_redo(&self) -> bool {
        lock(&self.history).can_redo()
    }

    /// Undo the last edit, scene command or voxel stroke, in the order they
    /// were made. Returns `true` if something was undone (the caller should
    /// then bump the revision/mark unsaved, which this method deliberately
    /// leaves to the caller since it has no `cx` to notify with here). An
    /// edit that can no longer be reverted is dropped.
    pub fn undo(&mut self) -> bool {
        let history = Arc::clone(&self.history);
        let mut history = lock(&history);
        history
            .undo(|edit| match edit {
                SceneEdit::Voxel(journal) => self.apply_voxel_journal(journal, false),
                SceneEdit::Scene(delta) => {
                    let ids = delta.ids();
                    let current = scene_edit::history::capture_history_subset(&self.world(), &ids);
                    if !self.restore_delta(&delta.before, &ids) {
                        return false;
                    }
                    // Redo returns to the state just left. Keeping `before`
                    // keeps the ids it names in scope, so redoing a removal
                    // removes the object.
                    delta.after = current;
                    true
                }
            })
            .is_some()
    }

    /// Redo the last undone edit. See [`Self::undo`]'s doc for the caller's
    /// responsibilities on success.
    pub fn redo(&mut self) -> bool {
        let history = Arc::clone(&self.history);
        let mut history = lock(&history);
        history
            .redo(|edit| match edit {
                SceneEdit::Voxel(journal) => self.apply_voxel_journal(journal, true),
                SceneEdit::Scene(delta) => {
                    let ids = delta.ids();
                    let current = scene_edit::history::capture_history_subset(&self.world(), &ids);
                    if !self.restore_delta(&delta.after, &ids) {
                        return false;
                    }
                    // Undo returns to the state just left; `after` keeps its
                    // ids in scope, so undoing a redone add removes the object.
                    delta.before = current;
                    true
                }
            })
            .is_some()
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
        // The write guard must be gone before the sync below takes it again.
        let restored =
            scene_edit::history::restore_history_delta(&mut self.world_mut(), snapshot, ids);
        match restored {
            Ok(()) => {
                // Displays that came back (or went) show (or drop) their levels.
                engine_backend::scene::sublevel_display::sync_level_instances(&mut self.world_mut());
                true
            }
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
