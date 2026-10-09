use crate::tool_modes::ToolModeId;
use gpui::*;

// Action for tool mode dropdown
#[derive(Action, Clone, PartialEq)]
#[action(namespace = level_editor_toolbar, no_json)]
pub struct SetToolMode(pub ToolModeId);

// Actions for toolbar dropdowns
#[derive(Action, Clone, PartialEq)]
#[action(namespace = level_editor_toolbar, no_json)]
pub struct SetTimeScale(pub f32);

#[derive(Action, Clone, PartialEq)]
#[action(namespace = level_editor_toolbar, no_json)]
pub struct SetTransformSnap(pub u8, pub f32);

/// Save the current scene as the engine's built-in default level.
///
/// Only available in source builds (binary lives in `target/{debug,release}/`).
/// Writes to `<workspace_root>/assets/default.level` so the next compile bakes
/// the scene as the level the engine opens when it cannot find a project.
#[derive(Action, Clone, PartialEq, Default)]
#[action(namespace = level_editor_toolbar, no_json)]
pub struct SaveAsDefaultLevel;
