//! Commands that place class instances and edit their overrides.
//!
//! A class instance is an object built from a class definition on disk
//! (#921); its variables and component slots can be overridden per instance
//! and reverted back to the class default. Entry points are [`label`],
//! [`scope`] and [`apply`].

use super::{CommandResult, SceneCommand};
use crate::state::LevelEditorState;
use engine_backend::scene::SceneWorldExt;

/// The Edit-menu translation key for the class commands, `None` when `cmd`
/// is not one of them.
pub(super) fn label(cmd: &SceneCommand) -> Option<&'static str> {
    Some(match cmd {
        SceneCommand::InstantiateClass { .. } => "LevelEditor.Undo.Add",
        SceneCommand::RevertComponentProperty { .. }
        | SceneCommand::SetClassVariable { .. }
        | SceneCommand::RevertClassSlot { .. }
        | SceneCommand::ResetClassOverrides { .. } => "LevelEditor.Undo.Edit",
        _ => return None,
    })
}

/// The objects a class command changes (captured before it runs), `None`
/// when `cmd` is not one of them.
pub(super) fn scope(cmd: &SceneCommand) -> Option<Vec<String>> {
    Some(match cmd {
        SceneCommand::RevertComponentProperty { id, .. }
        | SceneCommand::SetClassVariable { id, .. }
        | SceneCommand::RevertClassSlot { id, .. }
        | SceneCommand::ResetClassOverrides { id } => vec![id.clone()],
        // Placed objects are captured after the command, from its result.
        SceneCommand::InstantiateClass { .. } => Vec::new(),
        _ => return None,
    })
}

/// Run a class command; hands `cmd` back untouched when it is not one.
pub(super) fn apply(
    state: &mut LevelEditorState,
    cmd: SceneCommand,
) -> Result<CommandResult, SceneCommand> {
    Ok(match cmd {
        SceneCommand::InstantiateClass {
            class_dir,
            transform,
            parent_id,
        } => instantiate_class(state, &class_dir, &transform, parent_id.as_deref()),
        SceneCommand::RevertComponentProperty {
            id,
            class_name,
            component_index,
            prop_name,
        } => revert_component_property(state, &id, &class_name, component_index, &prop_name),
        SceneCommand::SetClassVariable { id, name, value } => {
            set_class_variable(state, &id, &name, value.as_ref())
        }
        SceneCommand::RevertClassSlot { id, slot_id, path } => {
            revert_class_slot(state, &id, &slot_id, path.as_deref())
        }
        SceneCommand::ResetClassOverrides { id } => reset_class_overrides(state, &id),
        other => return Err(other),
    })
}

fn instantiate_class(
    state: &mut LevelEditorState,
    class_dir: &std::path::PathBuf,
    transform: &crate::scene_edit::Transform,
    parent_id: Option<&str>,
) -> CommandResult {
    let placed = crate::scene_edit::classes::instantiate_class_dir(
        &mut state.scene.world_mut(),
        class_dir,
        transform,
        parent_id,
    );
    match placed {
        Ok(ids) => {
            state.scene.bump_revision(true);
            CommandResult::ok(ids)
        }
        Err(error) => {
            tracing::error!("Could not place class: {error}");
            CommandResult::noop("Class could not be placed")
        }
    }
}

fn revert_component_property(
    state: &mut LevelEditorState,
    id: &String,
    class_name: &String,
    component_index: usize,
    prop_name: &String,
) -> CommandResult {
    let registry = crate::scene_edit::classes::project_registry();
    let default = {
        let world = state.scene.world();
        crate::scene_edit::classes::slot_defaults(&world, id, &registry)
            .remove(&component_index)
            .filter(|d| &d.class_name == class_name)
            .and_then(|d| d.property(prop_name))
    };
    let Some(default) = default else {
        return CommandResult::noop("No class default for this property");
    };
    let json = pulsar_reflection::RUNTIME_TYPE_REGISTRY
        .serialize_json_for_any(default.as_ref())
        .ok();
    let updated = crate::scene_edit::components::update_live_component_property(
        &mut state.scene.world_mut(),
        id,
        class_name,
        component_index,
        prop_name,
        default,
    );
    if updated.is_err() {
        // An unresolved payload: write the default into it.
        let written = json.is_some_and(|json| {
            crate::scene_edit::components::set_unresolved_property(
                &mut state.scene.world_mut(),
                id,
                class_name,
                component_index,
                prop_name,
                json,
            )
        });
        if !written {
            return CommandResult::noop("Class default could not be written");
        }
    }
    state.scene.bump_revision(true);
    CommandResult::ok(vec![id.clone()])
}

fn set_class_variable(
    state: &mut LevelEditorState,
    id: &String,
    name: &String,
    value: Option<&serde_json::Value>,
) -> CommandResult {
    let changed = match value {
        Some(value) => {
            let registry = crate::scene_edit::classes::project_registry();
            crate::scene_edit::classes::set_variable(
                &mut state.scene.world_mut(),
                id,
                name,
                value.clone(),
                &registry,
            )
        }
        None => crate::scene_edit::classes::revert_variable(&mut state.scene.world_mut(), id, name),
    };
    if changed {
        state.scene.bump_revision(true);
        CommandResult::ok(vec![id.clone()])
    } else {
        CommandResult::noop("Not a class instance, or nothing to revert")
    }
}

fn revert_class_slot(
    state: &mut LevelEditorState,
    id: &String,
    slot_id: &String,
    path: Option<&str>,
) -> CommandResult {
    let registry = crate::scene_edit::classes::project_registry();
    if crate::scene_edit::classes::revert_slot(
        &mut state.scene.world_mut(),
        id,
        slot_id,
        path,
        &registry,
    ) {
        state.scene.bump_revision(true);
        CommandResult::ok(vec![id.clone()])
    } else {
        CommandResult::noop("Not a class instance, or nothing to revert")
    }
}

fn reset_class_overrides(state: &mut LevelEditorState, id: &String) -> CommandResult {
    use crate::scene_edit::classes;
    let registry = classes::project_registry();
    let Some(view) = classes::class_instance_view(&state.scene.world(), id, &registry) else {
        return CommandResult::noop("Not a class instance");
    };
    let mut changed = false;
    {
        let mut world = state.scene.world_mut();
        for var in view.variables.iter().filter(|v| v.overridden) {
            changed |= classes::revert_variable(&mut world, id, &var.name);
        }
        for slot in view
            .slots
            .iter()
            .filter(|s| s.removed || !s.overridden.is_empty())
        {
            changed |= classes::revert_slot(&mut world, id, &slot.slot_id, None, &registry);
        }
    }
    if changed {
        state.scene.bump_revision(true);
        CommandResult::ok(vec![id.clone()])
    } else {
        CommandResult::noop("Nothing overridden")
    }
}
