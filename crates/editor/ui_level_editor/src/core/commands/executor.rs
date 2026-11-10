//! The single write path for scene commands: [`execute_command`].
//!
//! This file only dispatches and does the undo bookkeeping. What each command
//! does (label, undo scope, apply) lives in its family module, next to its
//! siblings: [`super::object`], [`super::component`], [`super::class`],
//! [`super::selection`]; plugin commands carry their own (see
//! `extend::commands::ExtensionCommand`).

use super::{class, component, object, selection, CommandResult, SceneCommand};
use crate::scene_edit::history::{capture_history_subset, UndoLabel};
use crate::state::LevelEditorState;
use engine_backend::scene::SceneWorldExt;

/// What a command's undo entry is called in the Edit menu; `None` for
/// commands that leave no undo entry (selection).
fn command_label(cmd: &SceneCommand) -> Option<UndoLabel> {
    match cmd {
        SceneCommand::Extension(command) => Some(UndoLabel::Text(command.label())),
        SceneCommand::SelectObject { .. } => None,
        _ => Some(UndoLabel::Key(
            object::label(cmd)
                .or_else(|| component::label(cmd))
                .or_else(|| class::label(cmd))
                .unwrap_or("LevelEditor.Undo.Edit"),
        )),
    }
}

/// The ids of the objects whose state is captured before the command runs
/// (the undo "before" snapshot).
fn command_scope(world: &pulsar_scenedb::World, cmd: &SceneCommand) -> Vec<String> {
    if let SceneCommand::Extension(command) = cmd {
        return command.scope(world);
    }
    object::scope(cmd)
        .or_else(|| component::scope(cmd))
        .or_else(|| class::scope(cmd))
        .or_else(|| selection::scope(cmd))
        .unwrap_or_default()
}

/// Run the command through the family that owns it.
fn apply_command(state: &mut LevelEditorState, cmd: SceneCommand) -> CommandResult {
    if let SceneCommand::Extension(command) = cmd {
        return command.apply(state);
    }
    let cmd = match object::apply(state, cmd) {
        Ok(result) => return result,
        Err(cmd) => cmd,
    };
    let cmd = match component::apply(state, cmd) {
        Ok(result) => return result,
        Err(cmd) => cmd,
    };
    let cmd = match class::apply(state, cmd) {
        Ok(result) => return result,
        Err(cmd) => cmd,
    };
    match selection::apply(state, cmd) {
        Ok(result) => result,
        Err(_) => {
            tracing::error!("A scene command has no executor; add it to a family module");
            CommandResult::noop("Unhandled scene command")
        }
    }
}

/// Apply `cmd` to `state`.
///
/// Mutations write the shared `SceneDb` world the Helio renderer reads every
/// frame.  `scene_revision`
/// is bumped on every mutation, causing the polling task in
/// `LevelEditorPanel` to notify the GPUI hierarchy and properties panels.
///
/// GPUI-thread callers (panel action handlers) should additionally call
/// `cx.notify()` after this returns.
///
/// Also the sole write path onto the undo stack (Pulsar-Native#554): for
/// every variant except `SelectObject` (selection isn't undo-worthy), the
/// scene's pre-command state is captured before running `cmd` and committed
/// to `state.scene`'s undo history only if the command actually changed
/// something (`CommandResult::changed`) -- a no-op command shouldn't leave a
/// stale checkpoint an undo would just restore right back to.
pub fn execute_command(state: &mut LevelEditorState, cmd: SceneCommand) -> CommandResult {
    let is_undoable = !matches!(cmd, SceneCommand::SelectObject { .. });
    let scope = command_scope(&state.scene.world(), &cmd);
    let label = command_label(&cmd);
    let pre_state = is_undoable.then(|| capture_history_subset(&state.scene.world(), &scope));

    let result = apply_command(state, cmd);

    // An edit may have added, retargeted, disabled or removed a sublevel display
    // (or copied one): bring the contents it shows up to date now, so the
    // viewport and hierarchy never show a stale level. Generated content is not
    // part of the undo snapshot below; restoring the display regenerates it.
    if result.changed {
        let mut world = state.scene.world_mut();
        if engine_backend::scene::sublevel_display::needs_level_instance_sync(&world) {
            engine_backend::scene::sublevel_display::sync_level_instances(&mut world);
            drop(world);
            state.scene.bump_revision(false);
        }
    }

    if result.changed {
        if let Some(pre) = pre_state {
            let mut post_scope = scope;
            for id in &result.affected_ids {
                if !post_scope.iter().any(|existing| existing == id) {
                    post_scope.push(id.clone());
                }
            }
            let post = capture_history_subset(&state.scene.world(), &post_scope);
            state.scene.commit_labeled_checkpoint(pre, post, label);
        }
    }
    result
}
