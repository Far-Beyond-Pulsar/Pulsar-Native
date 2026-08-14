/// Scene command system — single execution path for all scene mutations.
///
/// `SceneCommand` is a self-contained description of one editor operation.
/// `execute_command()` applies it through `SceneDatabase`, which in turn
/// writes to **both** `SceneDb` (the canonical store) and the Helio renderer
/// (immediate viewport update) in one call.
///
/// Both user GPUI action handlers and AI tool implementations call
/// `execute_command()`, giving a single auditable code path that is ready for
/// undo / redo to be layered on top.
use crate::level_editor::scene_database::SceneObjectData;
use crate::level_editor::state::LevelEditorState;

// ── Command types ─────────────────────────────────────────────────────────────

/// A self-contained scene mutation.  All fields use owned data so the command
/// can be constructed on a background thread and executed on the UI thread.
#[derive(Debug, Clone)]
pub enum SceneCommand {
    /// Add a new object.  The `id` field in `data` is ignored — SceneDb assigns it.
    AddObject {
        data: SceneObjectData,
        parent_id: Option<String>,
    },
    /// Remove an object and all descendants.
    RemoveObject { id: String },
    /// Overwrite all mutable fields of an existing object (looked up by `data.id`).
    UpdateObject { data: SceneObjectData },
    /// Move an object to a different parent (or root when `None`).
    ReparentObject {
        id: String,
        new_parent_id: Option<String>,
    },
    /// Clone an object `count` times.
    /// `position_offset` is applied cumulatively: copy i is at src_pos + offset × i.
    DuplicateObject {
        source_id: String,
        count: usize,
        position_offset: Option<[f32; 3]>,
    },
    /// Change the editor selection (`None` clears it).
    SelectObject { id: Option<String> },
    /// Set absolute world-space transform fields; `None` fields are unchanged.
    SetTransform {
        id: String,
        position: Option<[f32; 3]>,
        rotation: Option<[f32; 3]>,
        scale: Option<[f32; 3]>,
    },
}

// ── Outcome ───────────────────────────────────────────────────────────────────

/// Outcome of executing a `SceneCommand`.
#[derive(Debug)]
pub struct CommandResult {
    /// Whether any state was actually modified.
    pub changed: bool,
    /// IDs of objects that were created or meaningfully affected.
    pub affected_ids: Vec<String>,
    /// Human-readable reason when `changed` is false.
    pub no_op_reason: &'static str,
}

impl CommandResult {
    pub fn noop(reason: &'static str) -> Self {
        Self {
            changed: false,
            affected_ids: vec![],
            no_op_reason: reason,
        }
    }
    pub fn ok(ids: Vec<String>) -> Self {
        Self {
            changed: true,
            affected_ids: ids,
            no_op_reason: "",
        }
    }
}

// ── Executor ──────────────────────────────────────────────────────────────────

/// Apply `cmd` to `state`.
///
/// Mutations go through `state.scene.database`, which writes to the shared
/// `WorldSceneStore` the Helio renderer reads every frame.  `scene_revision`
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
/// stale checkpoint an undo would just restore right back to. The match body
/// below is unchanged from before this wiring; it's wrapped in an immediately-
/// invoked closure purely so its several `return CommandResult::noop(...)`
/// early-exits stay scoped to computing `result` instead of returning from
/// this whole function before the checkpoint-commit step below runs.
pub fn execute_command(state: &mut LevelEditorState, cmd: SceneCommand) -> CommandResult {
    let is_undoable = !matches!(cmd, SceneCommand::SelectObject { .. });
    let pre_state = is_undoable.then(|| state.scene.capture_history_snapshot());

    // Reborrowed (not the outer `state` itself) so the `move` closure below
    // can take ownership of this reborrow without moving the actual `state`
    // parameter -- which is used again after the closure returns, to commit
    // the checkpoint captured above.
    let state_ref = &mut *state;
    let result = (move || -> CommandResult {
        let state = state_ref;
        match cmd {
        SceneCommand::AddObject { data, parent_id } => {
            let id = state.scene.database.add_object(data, parent_id);
            state.scene.bump_revision(true);
            CommandResult::ok(vec![id])
        }

        SceneCommand::RemoveObject { ref id } => {
            let removed = state.scene.database.remove_object(id);
            if removed {
                if state.scene.database.get_selected_object_id().as_deref() == Some(id) {
                    state.scene.database.select_object(None);
                }
                state.scene.bump_revision(true);
                CommandResult::ok(vec![id.clone()])
            } else {
                CommandResult::noop("Object not found")
            }
        }

        SceneCommand::UpdateObject { data } => {
            let id = data.id.clone();
            if state.scene.database.update_object(data) {
                state.scene.bump_revision(true);
                CommandResult::ok(vec![id])
            } else {
                CommandResult::noop("Object not found")
            }
        }

        SceneCommand::ReparentObject {
            ref id,
            ref new_parent_id,
        } => {
            let moved = state
                .scene
                .database
                .reparent_object(id, new_parent_id.clone());
            if moved {
                state.scene.bump_revision(true);
                CommandResult::ok(vec![id.clone()])
            } else {
                CommandResult::noop("Object not found or reparent rejected")
            }
        }

        SceneCommand::DuplicateObject {
            ref source_id,
            count,
            position_offset,
        } => {
            let src_pos = state
                .scene
                .database
                .get_object(source_id)
                .map(|o| o.transform.position);
            let mut created = Vec::new();
            for i in 0..count {
                if let Some(new_id) = state.scene.database.duplicate_object(source_id) {
                    if let (Some(off), Some(src)) = (position_offset, src_pos) {
                        let n = (i + 1) as f32;
                        if let Some(mut copy) = state.scene.database.get_object(&new_id) {
                            copy.transform.position = [
                                src[0] + off[0] * n,
                                src[1] + off[1] * n,
                                src[2] + off[2] * n,
                            ];
                            state.scene.database.update_object(copy);
                        }
                    }
                    created.push(new_id);
                } else {
                    break;
                }
            }
            if created.is_empty() {
                CommandResult::noop("Source object not found")
            } else {
                state.scene.bump_revision(true);
                CommandResult::ok(created)
            }
        }

        SceneCommand::SelectObject { id } => {
            state.scene.database.select_object(id.clone());
            state.scene.bump_revision(false);
            CommandResult::ok(id.into_iter().collect())
        }

        SceneCommand::SetTransform {
            ref id,
            position,
            rotation,
            scale,
        } => {
            let Some(mut obj) = state.scene.database.get_object(id) else {
                return CommandResult::noop("Object not found");
            };
            let mut changed = false;
            if let Some(p) = position {
                if obj.transform.position != p {
                    obj.transform.position = p;
                    changed = true;
                }
            }
            if let Some(r) = rotation {
                if obj.transform.rotation != r {
                    obj.transform.rotation = r;
                    changed = true;
                }
            }
            if let Some(s) = scale {
                if obj.transform.scale != s {
                    obj.transform.scale = s;
                    changed = true;
                }
            }

            if !changed {
                return CommandResult::noop("No transform fields changed");
            }
            if state.scene.database.update_object(obj.clone()) {
                state.scene.bump_revision(true);
                CommandResult::ok(vec![id.clone()])
            } else {
                CommandResult::noop("Transform update failed")
            }
        }
        }
    })();

    if result.changed {
        if let Some(pre) = pre_state {
            state.scene.commit_undo_checkpoint(pre);
        }
    }
    result
}

#[cfg(test)]
mod undo_redo_tests {
    use super::*;
    use crate::level_editor::scene_database::{ObjectType, SceneObjectData, Transform};

    fn object(name: &str) -> SceneObjectData {
        SceneObjectData {
            id: String::new(),
            name: name.to_string(),
            object_type: ObjectType::Empty,
            transform: Transform::default(),
            visible: true,
            locked: false,
            parent: None,
            children: vec![],
            scene_path: String::new(),
            props: Default::default(),
            component_instances: None,
        }
    }

    #[test]
    fn undo_reverts_an_add_object_command() {
        let mut state = LevelEditorState::new();
        assert!(!state.scene.can_undo());

        let result = execute_command(
            &mut state,
            SceneCommand::AddObject { data: object("Cube"), parent_id: None },
        );
        assert!(result.changed);
        assert!(state.scene.can_undo());
        assert_eq!(state.scene.database.get_all_objects().len(), 1);

        assert!(state.scene.undo());
        assert!(state.scene.database.get_all_objects().is_empty());
        assert!(!state.scene.can_undo());
        assert!(state.scene.can_redo());
    }

    #[test]
    fn redo_reapplies_an_undone_command() {
        let mut state = LevelEditorState::new();
        execute_command(
            &mut state,
            SceneCommand::AddObject { data: object("Cube"), parent_id: None },
        );
        state.scene.undo();
        assert!(state.scene.database.get_all_objects().is_empty());

        assert!(state.scene.redo());

        assert_eq!(state.scene.database.get_all_objects().len(), 1);
        assert!(state.scene.can_undo());
        assert!(!state.scene.can_redo());
    }

    #[test]
    fn a_new_mutating_command_clears_the_redo_stack() {
        let mut state = LevelEditorState::new();
        execute_command(
            &mut state,
            SceneCommand::AddObject { data: object("A"), parent_id: None },
        );
        state.scene.undo();
        assert!(state.scene.can_redo());

        execute_command(
            &mut state,
            SceneCommand::AddObject { data: object("B"), parent_id: None },
        );

        assert!(!state.scene.can_redo());
    }

    #[test]
    fn a_noop_command_does_not_push_an_undo_checkpoint() {
        let mut state = LevelEditorState::new();
        let result =
            execute_command(&mut state, SceneCommand::RemoveObject { id: "nope".to_string() });
        assert!(!result.changed);
        assert!(!state.scene.can_undo());
    }

    #[test]
    fn selecting_an_object_does_not_push_an_undo_checkpoint() {
        let mut state = LevelEditorState::new();
        let add = execute_command(
            &mut state,
            SceneCommand::AddObject { data: object("Cube"), parent_id: None },
        );
        let id = add.affected_ids[0].clone();
        assert!(state.scene.can_undo()); // the AddObject checkpoint

        execute_command(&mut state, SceneCommand::SelectObject { id: Some(id) });

        // Still exactly the one checkpoint from AddObject -- undoing once
        // now must remove the object, not merely revert the selection.
        assert!(state.scene.undo());
        assert!(state.scene.database.get_all_objects().is_empty());
        assert!(!state.scene.can_undo());
    }

    #[test]
    fn undo_and_redo_on_an_empty_history_are_no_ops() {
        let mut state = LevelEditorState::new();
        assert!(!state.scene.undo());
        assert!(!state.scene.redo());
    }
}
