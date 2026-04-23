use super::super::state::{BuildConfig, MultiplayerMode, TargetPlatform};
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
