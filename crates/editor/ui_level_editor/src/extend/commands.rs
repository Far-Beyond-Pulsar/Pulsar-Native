//! A plugin's commands and undo steps.
//!
//! [`ExtensionCommand`] is a scene command a plugin defines (adding a
//! ready-made voxel world): it runs through
//! [`crate::commands::execute_command`] like the shell's own, so the
//! objects it names are captured before and after and it is one undo step.
//! [`ExtensionUndo`] is an undo step a plugin records itself, for edits a
//! before/after copy of objects would make too large (a sculpt stroke
//! appends to a terrain's edit journal; its undo pops what it appended).

use pulsar_scenedb::World;

use crate::commands::CommandResult;
use crate::state::{LevelEditorState, SceneDomain};

/// A plugin's scene command, run by `execute_command`
/// (`SceneCommand::Extension`).
pub trait ExtensionCommand: Send {
    /// The undo entry's text (the plugin translates it).
    fn label(&self) -> String;
    /// The objects it changes, captured before it runs (objects it adds
    /// are captured after, from the result's ids).
    fn scope(&self, world: &World) -> Vec<String>;
    /// Run it.
    fn apply(self: Box<Self>, state: &mut LevelEditorState) -> CommandResult;
    /// For logs.
    fn describe(&self) -> String;
}

/// A plugin's own undo step (`SceneDomain::commit_extension_edit`).
pub trait ExtensionUndo: Send + std::fmt::Debug {
    /// The undo entry's text (the plugin translates it).
    fn label(&self) -> String;
    /// Revert it; `false` when it can no longer be reverted (the entry is
    /// dropped).
    fn undo(&mut self, scene: &mut SceneDomain) -> bool;
    /// Apply it again; `false` when it can no longer be.
    fn redo(&mut self, scene: &mut SceneDomain) -> bool;
}
