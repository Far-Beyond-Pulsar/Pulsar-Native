//! Actions for the Pulsar application

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

/// Action to toggle the terminal
#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = pulsar_app)]
pub struct ToggleTerminal;

/// Action to toggle multiplayer collaboration
#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = pulsar_app)]
pub struct ToggleMultiplayer;

/// Action to toggle the command palette
#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = pulsar_app)]
pub struct ToggleCommandPalette;
