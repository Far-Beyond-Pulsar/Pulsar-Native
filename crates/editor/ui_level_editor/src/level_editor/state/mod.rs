//! Level Editor State Management
//!
//! The editor state is organized into focused **domains**, each owned by a dedicated
//! sub-struct on [`LevelEditorState`]. This keeps state access patterns explicit and
//! prevents the "one big struct" anti-pattern where unrelated fields are coupled.
//!
//! # Domains
//!
//! | Domain | Purpose |
//! |--------|---------|
//! | [`SceneDomain`](scene::SceneDomain) | Scene database, play-mode snapshot, file path, revision counter |
//! | [`EditorDomain`](editor::EditorDomain) | Tool selection, camera mode, rendering toggles (grid, wireframe, etc.) |
//! | [`OverlayDomain`](overlays::OverlayDomain) | Overlay visibility, collapse state, positions, performance metric toggles |
//! | [`HierarchyDomain`](hierarchy::HierarchyDomain) | Hierarchy expand/collapse state, drag-and-drop state |
//! | [`BuildDomain`](build::BuildDomain) | Build configuration, platform target, game process |
//! | [`PlayDomain`](play::PlayDomain) | Play-mode parameters: time scale, target FPS, multiplayer mode |
//!
//! All mutation goes through [`execute_command`](super::commands::execute_command) or
//! domain-specific methods. Direct field mutation outside of these paths is discouraged
//! as it bypasses revision tracking and undo/redo.
//!
//! # Thread Safety
//!
//! The entire state tree is wrapped in `Arc<parking_lot::RwLock<LevelEditorState>>`.
//! Readers take `.read()`, writers take `.write()`. The `SceneDb` inside
//! `SceneDomain` provides lock-free concurrent reads for the renderer.

pub mod build;
pub mod editor;
pub mod hierarchy;
pub mod overlays;
pub mod play;
pub mod scene;

pub use build::BuildDomain;
pub use editor::EditorDomain;
pub use hierarchy::HierarchyDomain;
pub use overlays::OverlayDomain;
pub use play::{PieControl, PieStartRequest, PlayDomain};
pub use scene::SceneDomain;

use std::path::PathBuf;
use std::sync::Arc;

use crate::level_editor::scene_database::SceneDb;

// ── LevelEditorState ─────────────────────────────────────────────────────────

/// Root editor state — the single source of truth for the level editor.
///
/// Every panel (toolbar, viewport, hierarchy, properties, world settings)
/// receives an `Arc<parking_lot::RwLock<LevelEditorState>>` and accesses
/// the sub-domain that is relevant to it.
#[derive(Clone)]
pub struct LevelEditorState {
    /// Scene data — objects, hierarchy, selection, play-mode snapshots.
    pub scene: SceneDomain,

    /// Editor preferences — tool, camera mode, rendering toggles.
    pub editor: EditorDomain,

    /// Overlay UI state — which overlays are visible, collapsed, dragged.
    pub overlays: OverlayDomain,

    /// Hierarchy UI state — expanded/collapsed items, drag state.
    pub hierarchy: HierarchyDomain,

    /// Build & deploy state — config, platform, game process.
    pub build: BuildDomain,

    /// Play-mode state — time scale, target FPS, multiplayer.
    pub play: PlayDomain,
}

impl Default for LevelEditorState {
    fn default() -> Self {
        Self {
            scene: SceneDomain::default(),
            editor: EditorDomain::default(),
            overlays: OverlayDomain::default(),
            hierarchy: HierarchyDomain::default(),
            build: BuildDomain::default(),
            play: PlayDomain::default(),
        }
    }
}

impl LevelEditorState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a `LevelEditorState` that shares the given `SceneDb` Arc with the renderer.
    /// The database starts empty; `ensure_default_level_file` populates it from disk.
    pub fn new_with_scene_db(scene_db: Arc<SceneDb>) -> Self {
        Self {
            scene: SceneDomain::with_scene_db(scene_db),
            ..Self::default()
        }
    }
}

/// Requests that the viewport capture its framebuffer to `<project>/.pulsar/thumbnail.png`
/// on the next render. Consumed by `HelioViewport::render`. Call this from any scene-save
/// success path (toolbar Save button, Save/Save As actions, etc).
pub fn request_thumbnail_capture(shared_state: &Arc<parking_lot::RwLock<LevelEditorState>>) {
    if let Some(project_path) = engine_state::get_project_path() {
        let thumbnail_path = PathBuf::from(project_path)
            .join(".pulsar")
            .join("thumbnail.png");
        shared_state.write().build.pending_thumbnail_capture = Some(thumbnail_path);
    }
}

// Re-export enums at the module level so `use crate::level_editor::state::{TransformTool, ...}`
// still works. These were previously in the flat state module.
pub use editor::{
    BuildConfig, BuildMode, CameraMode, MultiplayerMode, TargetPlatform, TransformTool,
};
pub use hierarchy::{HierarchyDragPayload, HierarchyDragState};
pub use overlays::OverlayState;
pub use scene::EditorMode;
