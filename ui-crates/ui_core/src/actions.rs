//! Actions for the Pulsar application

use std::path::PathBuf;
use gpui::Action;
use schemars::JsonSchema;
use serde::Deserialize;

/// Action to toggle the file manager drawer
#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = pulsar_app)]
pub struct ToggleFileManager;

/// Action to toggle the problems drawer
#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = pulsar_app)]
pub struct ToggleProblems;

/// Action to toggle the type debugger
#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = pulsar_app)]
pub struct ToggleTypeDebugger;

/// Action to toggle multiplayer collaboration
#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = pulsar_app)]
pub struct ToggleMultiplayer;

/// Action to toggle the flamegraph profiler
#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = pulsar_app)]
pub struct ToggleFlamegraph;

/// Action to toggle the command palette
#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = pulsar_app)]
pub struct ToggleCommandPalette;

/// Action to open a file at a specific path
#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = pulsar_app)]
pub struct OpenFile {
    pub path: PathBuf,
}
