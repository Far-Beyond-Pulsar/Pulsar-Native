//! Scene Domain — the actual level data (objects, hierarchy, play-mode snapshots)
//!
//! This is the only domain that directly wraps [`SceneDatabase`] and is therefore
//! the bridge to the concurrency-safe `SceneDb` shared with the renderer.
//!
//! The `revision` counter is bumped on every mutation so that observer tasks
//! (running on the GPUI main thread) can detect changes made by background
//! threads (AI tools, asset import, etc.) and trigger a re-render.

use std::path::PathBuf;
use std::sync::Arc;

use crate::level_editor::scene_database::{ObjectId, SceneDb, SceneObjectData};
use crate::level_editor::SceneDatabase;

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
/// - `database` — the `SceneDatabase` (wraps `Arc<SceneDb>` + `SceneMetadataDb`).
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
    pub snapshot: Option<Arc<parking_lot::RwLock<Vec<SceneObjectData>>>>,
    /// Current editor mode.
    pub editor_mode: EditorMode,
    /// Currently open scene file path.
    pub current_scene: Option<PathBuf>,
    /// Whether the scene has unsaved changes.
    pub has_unsaved_changes: bool,
    /// Monotonic revision counter — bumped on every mutation so pollers
    /// (and the observer system) can detect external changes.
    pub revision: u64,
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
        }
    }
}

impl SceneDomain {
    /// Create using a caller-supplied `SceneDb` Arc that is shared with the renderer.
    pub fn with_scene_db(scene_db: Arc<SceneDb>) -> Self {
        Self {
            database: SceneDatabase::with_shared_db(scene_db),
            ..Self::default()
        }
    }

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

    // ── Play mode ─────────────────────────────────────────────────────────

    /// Enter play mode — snapshot scene and start game thread.
    pub fn enter_play_mode(&mut self) {
        let objects = self.database.get_all_objects();
        self.snapshot = Some(Arc::new(parking_lot::RwLock::new(objects)));
        self.editor_mode = EditorMode::Play;
    }

    /// Exit play mode — restore scene state from snapshot.
    pub fn exit_play_mode(&mut self) {
        if let Some(ref snapshot) = self.snapshot {
            let objects = snapshot.read().clone();
            self.database.clear();
            for obj in objects {
                self.database.add_object(obj, None);
            }
        }
        self.editor_mode = EditorMode::Edit;
        self.snapshot = None;
    }
}
