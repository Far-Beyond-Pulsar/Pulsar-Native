//! The selection command: the one scene command that is not undoable.
//!
//! Entry points are [`scope`] and [`apply`]; it has no undo label because the
//! executor never records a checkpoint for it.

use super::{CommandResult, SceneCommand};
use crate::state::LevelEditorState;
use engine_backend::scene::SceneWorldExt;

/// Selection changes touch no object data, so nothing is captured for undo.
pub(super) fn scope(cmd: &SceneCommand) -> Option<Vec<String>> {
    matches!(cmd, SceneCommand::SelectObject { .. }).then(Vec::new)
}

/// Run `SelectObject`; hands `cmd` back untouched when it is another command.
pub(super) fn apply(
    state: &mut LevelEditorState,
    cmd: SceneCommand,
) -> Result<CommandResult, SceneCommand> {
    match cmd {
        SceneCommand::SelectObject { id } => Ok(select_object(state, id)),
        other => Err(other),
    }
}

fn select_object(state: &mut LevelEditorState, id: Option<String>) -> CommandResult {
    crate::scene_edit::objects::select_object(&mut state.scene.world_mut(), id.as_deref());
    state.scene.bump_revision(false);
    CommandResult::ok(id.into_iter().collect())
}
