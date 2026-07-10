/// Modular Level Editor
///
/// A professional, studio-quality level editor with multi-panel layout
/// inspired by industry-standard tools like Unity, Unreal, and Godot.
///
/// Features:
/// - Scene Browser: Browse and manage scene files
/// - Hierarchy: Tree view of all scene objects
/// - Properties: Inspector for selected object properties
/// - Viewport: 3D rendering with camera controls
/// - Asset Browser: Browse and preview project assets
/// - Toolbar: Transform tools and quick actions
/// - Scene Database: Unified write path — updates both SceneDb and Helio
pub mod core;
pub mod state;
mod ui;
pub mod workspace;

// Module aliases so existing `crate::level_editor::X::Y` paths still compile
pub use core::commands;
pub use core::scene_database;
pub use core::world_settings_data;
pub use workspace::panels as workspace_panels;

// Public API
pub use core::commands::{execute_command, CommandResult, SceneCommand};
pub use core::scene_database::{SceneDatabase, SceneObjectData};
pub use core::world_settings_data::*;
pub use state::request_thumbnail_capture;
pub use state::LevelEditorState;
pub use state::{CameraMode, EditorMode, TransformTool};
pub use workspace::panels::*;

// Re-export LevelEditorPanel from ui
pub use ui::LevelEditorPanel;
