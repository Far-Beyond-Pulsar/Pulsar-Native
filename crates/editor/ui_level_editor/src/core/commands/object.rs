//! Commands that create, remove, move and describe whole scene objects.
//!
//! Entry points are [`label`], [`scope`] and [`apply`]; the executor tries
//! each family module in turn and a family returns `None` / `Err(cmd)` for
//! variants it does not own. Adding an object-level command means adding one
//! arm to each of the three, next to its function below.

use super::{CommandResult, SceneCommand, TypedComponent};
use crate::scene_edit::SceneObjectData;
use crate::state::LevelEditorState;
use engine_backend::scene::level_rules;
use engine_backend::scene::SceneWorldExt;

/// Why a command with a non-finite transform did nothing
/// (`scene_edit::objects::validate_transform` refuses it; this names it for
/// the caller, since a no-op reason is static text).
const INVALID_TRANSFORM: &str =
    "Transform is not finite: position, rotation and scale must be finite numbers";

/// The Edit-menu translation key for the object commands, `None` when `cmd`
/// is not one of them.
pub(super) fn label(cmd: &SceneCommand) -> Option<&'static str> {
    Some(match cmd {
        SceneCommand::AddObject { .. } | SceneCommand::AddObjectWithComponents { .. } => {
            "LevelEditor.Undo.Add"
        }
        SceneCommand::RemoveObject { .. } => "LevelEditor.Undo.Delete",
        SceneCommand::DuplicateObject { .. } => "LevelEditor.Undo.Duplicate",
        SceneCommand::ReparentObject { .. } => "LevelEditor.Undo.Reparent",
        SceneCommand::SetTransform { .. } => "LevelEditor.Undo.Move",
        SceneCommand::SetName { .. } => "LevelEditor.Undo.Rename",
        SceneCommand::SetVisibility { .. } => "LevelEditor.Undo.Visibility",
        SceneCommand::UpdateObject { .. } => "LevelEditor.Undo.Edit",
        _ => return None,
    })
}

/// The objects an object command changes (captured before it runs), `None`
/// when `cmd` is not one of them.
pub(super) fn scope(cmd: &SceneCommand) -> Option<Vec<String>> {
    Some(match cmd {
        SceneCommand::AddObject { data, .. }
        | SceneCommand::AddObjectWithComponents { data, .. } => {
            // A new object has no id yet; the executor adds it from the result.
            (!data.id.is_empty())
                .then(|| data.id.clone())
                .into_iter()
                .collect()
        }
        SceneCommand::RemoveObject { id }
        | SceneCommand::ReparentObject { id, .. }
        | SceneCommand::SetTransform { id, .. }
        | SceneCommand::SetName { id, .. }
        | SceneCommand::SetVisibility { id, .. } => vec![id.clone()],
        SceneCommand::UpdateObject { data } => vec![data.id.clone()],
        SceneCommand::DuplicateObject { source_id, .. } => vec![source_id.clone()],
        _ => return None,
    })
}

/// Run an object command; hands `cmd` back untouched when it is not one.
pub(super) fn apply(
    state: &mut LevelEditorState,
    cmd: SceneCommand,
) -> Result<CommandResult, SceneCommand> {
    Ok(match cmd {
        SceneCommand::AddObject { data, parent_id } => add_object(state, data, parent_id),
        SceneCommand::AddObjectWithComponents {
            data,
            parent_id,
            components,
        } => add_object_with_components(state, data, parent_id, components),
        SceneCommand::RemoveObject { id } => remove_object(state, &id),
        SceneCommand::UpdateObject { data } => update_object(state, data),
        SceneCommand::ReparentObject { id, new_parent_id } => {
            reparent_object(state, &id, new_parent_id)
        }
        SceneCommand::DuplicateObject {
            source_id,
            count,
            position_offset,
        } => duplicate_object(state, &source_id, count, position_offset),
        SceneCommand::SetTransform {
            id,
            position,
            rotation,
            scale,
        } => set_transform(state, &id, position, rotation, scale),
        SceneCommand::SetName { id, name } => set_name(state, &id, name),
        SceneCommand::SetVisibility {
            id,
            visible,
            locked,
        } => set_visibility(state, &id, visible, locked),
        other => return Err(other),
    })
}

fn add_object(
    state: &mut LevelEditorState,
    data: SceneObjectData,
    parent_id: Option<String>,
) -> CommandResult {
    if crate::scene_edit::objects::validate_transform(&data.transform).is_err() {
        return CommandResult::noop(INVALID_TRANSFORM);
    }
    let id = crate::scene_edit::objects::add_object(&mut state.scene.world_mut(), data, parent_id);
    if id.is_empty() {
        return CommandResult::noop("Object could not be added");
    }
    state.scene.bump_revision(true);
    CommandResult::ok(vec![id])
}

/// Adds the object and its components in one step; level rules are checked
/// before anything is written so a refused component leaves no object behind.
fn add_object_with_components(
    state: &mut LevelEditorState,
    data: SceneObjectData,
    parent_id: Option<String>,
    components: Vec<TypedComponent>,
) -> CommandResult {
    if crate::scene_edit::objects::validate_transform(&data.transform).is_err() {
        return CommandResult::noop(INVALID_TRANSFORM);
    }
    let planned: Vec<_> = components
        .iter()
        .map(|component| level_rules::NewComponent {
            class_name: &component.class_name,
            value: Some(component.value.as_ref() as &dyn std::any::Any),
            enabled: component.enabled,
        })
        .collect();
    if let Err(reason) = level_rules::check_new_components(&state.scene.world(), &planned) {
        return CommandResult::noop(reason);
    }
    drop(planned);
    let mut world = state.scene.world_mut();
    let id = crate::scene_edit::objects::add_object(&mut world, data, parent_id);
    if id.is_empty() {
        return CommandResult::noop("Object could not be added");
    }
    for component in components {
        crate::scene_edit::components::add_component_value_enabled(
            &mut world,
            &id,
            &component.class_name,
            Some(component.value),
            component.enabled,
        );
    }
    drop(world);
    state.scene.bump_revision(true);
    CommandResult::ok(vec![id])
}

fn remove_object(state: &mut LevelEditorState, id: &String) -> CommandResult {
    let removed = crate::scene_edit::objects::remove_object(&mut state.scene.world_mut(), id);
    if removed {
        if crate::scene_edit::objects::get_selected_object_id(&state.scene.world()).as_deref()
            == Some(id)
        {
            crate::scene_edit::objects::select_object(&mut state.scene.world_mut(), None);
        }
        state.scene.bump_revision(true);
        CommandResult::ok(vec![id.clone()])
    } else {
        CommandResult::noop("Object not found")
    }
}

fn update_object(state: &mut LevelEditorState, data: SceneObjectData) -> CommandResult {
    let id = data.id.clone();
    if crate::scene_edit::objects::validate_transform(&data.transform).is_err() {
        return CommandResult::noop(INVALID_TRANSFORM);
    }
    if crate::scene_edit::objects::update_object(&mut state.scene.world_mut(), data) {
        state.scene.bump_revision(true);
        CommandResult::ok(vec![id])
    } else {
        CommandResult::noop("Object not found")
    }
}

fn reparent_object(
    state: &mut LevelEditorState,
    id: &String,
    new_parent_id: Option<String>,
) -> CommandResult {
    let moved = crate::scene_edit::objects::reparent_object(
        &mut state.scene.world_mut(),
        id,
        new_parent_id,
    );
    if moved {
        state.scene.bump_revision(true);
        CommandResult::ok(vec![id.clone()])
    } else {
        CommandResult::noop("Object not found or reparent rejected")
    }
}

fn duplicate_object(
    state: &mut LevelEditorState,
    source_id: &String,
    count: usize,
    position_offset: Option<[f32; 3]>,
) -> CommandResult {
    let src_pos = crate::scene_edit::objects::get_object(&state.scene.world(), source_id)
        .map(|o| o.transform.position);
    let mut created = Vec::new();
    for i in 0..count {
        // Results are bound before each `if let`: a scene guard
        // taken in its scrutinee would live through the block,
        // which locks the scene again.
        let duplicated =
            crate::scene_edit::objects::duplicate_object(&mut state.scene.world_mut(), source_id);
        if let Some(new_id) = duplicated {
            if let (Some(off), Some(src)) = (position_offset, src_pos) {
                let n = (i + 1) as f32;
                let copy = crate::scene_edit::objects::get_object(&state.scene.world(), &new_id);
                if let Some(mut copy) = copy {
                    copy.transform.position = [
                        src[0] + f64::from(off[0] * n),
                        src[1] + f64::from(off[1] * n),
                        src[2] + f64::from(off[2] * n),
                    ];
                    crate::scene_edit::objects::update_object(&mut state.scene.world_mut(), copy);
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

fn set_transform(
    state: &mut LevelEditorState,
    id: &String,
    position: Option<[f64; 3]>,
    rotation: Option<[f32; 3]>,
    scale: Option<[f32; 3]>,
) -> CommandResult {
    // `scene_edit::objects::set_transform`, NOT `get_object`+`update_object`
    // (Pulsar-Native#561): the old whole-object round trip triggered
    // `sync_registered_component_props_to_scene_db` -- a full
    // re-serialize/re-hydrate of every component on the object --
    // on every keystroke of a position/rotation/scale field, for a
    // change that has nothing to do with component data at all.
    if crate::scene_edit::objects::validate_transform_fields(position, rotation, scale).is_err() {
        return CommandResult::noop(INVALID_TRANSFORM);
    }
    if crate::scene_edit::objects::set_transform(
        &mut state.scene.world_mut(),
        id,
        position,
        rotation,
        scale,
    ) {
        state.scene.bump_revision(true);
        CommandResult::ok(vec![id.clone()])
    } else {
        CommandResult::noop("Object not found or no transform fields changed")
    }
}

fn set_name(state: &mut LevelEditorState, id: &String, name: String) -> CommandResult {
    if crate::scene_edit::objects::set_name(&mut state.scene.world_mut(), id, name) {
        state.scene.bump_revision(true);
        CommandResult::ok(vec![id.clone()])
    } else {
        CommandResult::noop("Object not found")
    }
}

fn set_visibility(
    state: &mut LevelEditorState,
    id: &String,
    visible: Option<bool>,
    locked: Option<bool>,
) -> CommandResult {
    let mut changed = false;
    if let Some(v) = visible {
        changed |= crate::scene_edit::objects::set_visible(&mut state.scene.world_mut(), id, v);
    }
    if let Some(l) = locked {
        changed |= crate::scene_edit::objects::set_locked(&mut state.scene.world_mut(), id, l);
    }
    if changed {
        state.scene.bump_revision(true);
        CommandResult::ok(vec![id.clone()])
    } else {
        CommandResult::noop("Object not found or no visibility fields changed")
    }
}
