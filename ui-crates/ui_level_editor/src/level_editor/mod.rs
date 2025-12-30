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
/// - Scene Database: In-memory scene management with undo/redo
/// - Gizmos: 3D manipulation tools (translate, rotate, scale)

mod ui;
mod scene_database;
mod gizmos;
mod workspace_panels;

pub use ui::LevelEditorPanel;
pub use scene_database::SceneDatabase;
pub use gizmos::{GizmoState, GizmoType};
pub use workspace_panels::*;
