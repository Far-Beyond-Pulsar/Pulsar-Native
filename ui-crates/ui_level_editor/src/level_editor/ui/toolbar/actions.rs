use super::super::state::{BuildConfig, BuildMode, MultiplayerMode, TargetPlatform};
use gpui::*;

// Actions for toolbar dropdowns
#[derive(Action, Clone, PartialEq)]
#[action(namespace = level_editor_toolbar, no_json)]
pub struct SetTimeScale(pub f32);

#[derive(Action, Clone, PartialEq)]
#[action(namespace = level_editor_toolbar, no_json)]
pub struct SetMultiplayerMode(pub MultiplayerMode);

#[derive(Action, Clone, PartialEq)]
#[action(namespace = level_editor_toolbar, no_json)]
pub struct SetBuildConfig(pub BuildConfig);

#[derive(Action, Clone, PartialEq)]
#[action(namespace = level_editor_toolbar, no_json)]
pub struct SetTargetPlatform(pub TargetPlatform);

/// Trigger a full project build: compile all scene blueprints and emit a runnable
/// Pulsar game crate under `<project_root>/build/`.
#[derive(Action, Clone, PartialEq, Default)]
#[action(namespace = level_editor_toolbar, no_json)]
pub struct BuildCore;

/// Switch the build button's primary action mode.
#[derive(Action, Clone, PartialEq)]
#[action(namespace = level_editor_toolbar, no_json)]
pub struct SetBuildMode(pub BuildMode);

/// Save the current scene as the engine's built-in default level.
///
/// Only available in source builds (binary lives in `target/{debug,release}/`).
/// Writes to `<workspace_root>/assets/default.level` so the next compile bakes
/// the scene as the level the engine opens when it cannot find a project.
#[derive(Action, Clone, PartialEq, Default)]
#[action(namespace = level_editor_toolbar, no_json)]
pub struct SaveAsDefaultLevel;
