//! Commands that edit the components attached to an object.
//!
//! Entry points are [`label`], [`scope`] and [`apply`]. The helpers
//! `edit_components`, `checked_value` and `revert_if_refused` are shared by
//! the commands here: the first wraps "object exists, edit, bump revision",
//! the other two enforce level rules (e.g. only one sun light) after an edit.

use super::{CommandResult, ComponentData, SceneCommand};
use crate::state::LevelEditorState;
use engine_backend::scene::level_rules;
use engine_backend::scene::SceneWorldExt;
use std::any::Any;

/// The Edit-menu translation key for the component commands, `None` when
/// `cmd` is not one of them.
pub(super) fn label(cmd: &SceneCommand) -> Option<&'static str> {
    Some(match cmd {
        SceneCommand::AddComponent { .. } | SceneCommand::DuplicateComponent { .. } => {
            "LevelEditor.Undo.AddComponent"
        }
        SceneCommand::RemoveComponent { .. } => "LevelEditor.Undo.RemoveComponent",
        SceneCommand::SetComponentProperty { .. }
        | SceneCommand::SetComponentEnabled { .. }
        | SceneCommand::ReorderComponent { .. }
        | SceneCommand::SetComponentParent { .. }
        | SceneCommand::SetComponentData { .. }
        | SceneCommand::SetMovability { .. } => "LevelEditor.Undo.Edit",
        _ => return None,
    })
}

/// The objects a component command changes (captured before it runs), `None`
/// when `cmd` is not one of them.
pub(super) fn scope(cmd: &SceneCommand) -> Option<Vec<String>> {
    Some(match cmd {
        SceneCommand::SetComponentProperty { id, .. }
        | SceneCommand::AddComponent { id, .. }
        | SceneCommand::RemoveComponent { id, .. }
        | SceneCommand::SetComponentEnabled { id, .. }
        | SceneCommand::DuplicateComponent { id, .. }
        | SceneCommand::ReorderComponent { id, .. }
        | SceneCommand::SetComponentParent { id, .. }
        | SceneCommand::SetComponentData { id, .. } => vec![id.clone()],
        SceneCommand::SetMovability { ids, .. } => ids.clone(),
        _ => return None,
    })
}

/// Run a component command; hands `cmd` back untouched when it is not one.
pub(super) fn apply(
    state: &mut LevelEditorState,
    cmd: SceneCommand,
) -> Result<CommandResult, SceneCommand> {
    Ok(match cmd {
        SceneCommand::SetComponentProperty {
            id,
            class_name,
            component_index,
            prop_name,
            value,
        } => set_component_property(state, &id, &class_name, component_index, &prop_name, value),
        SceneCommand::AddComponent {
            id,
            class_name,
            value,
        } => add_component(state, &id, class_name, value),
        SceneCommand::RemoveComponent {
            id,
            component_index,
        } => remove_component(state, &id, component_index),
        SceneCommand::SetComponentEnabled {
            id,
            component_index,
            enabled,
        } => set_component_enabled(state, &id, component_index, enabled),
        SceneCommand::DuplicateComponent {
            id,
            component_index,
        } => duplicate_component(state, &id, component_index),
        SceneCommand::ReorderComponent {
            id,
            from_index,
            to_index,
        } => reorder_component(state, &id, from_index, to_index),
        SceneCommand::SetComponentParent {
            id,
            component_index,
            parent_index,
        } => set_component_parent(state, &id, component_index, parent_index),
        SceneCommand::SetComponentData {
            id,
            component_index,
            data,
        } => set_component_data(state, &id, component_index, data),
        SceneCommand::SetMovability { ids, movability } => set_movability(state, &ids, movability),
        other => return Err(other),
    })
}

/// Run a component edit on `id`; `edit` reports whether it changed
/// anything (each scene-edit component function says so itself, so nothing
/// is compared or encoded here).
fn edit_components(
    state: &mut LevelEditorState,
    id: &str,
    no_op_reason: &'static str,
    edit: impl FnOnce(&mut pulsar_scenedb::World) -> bool,
) -> CommandResult {
    if state.scene.world().entity_for(id).is_none() {
        return CommandResult::noop("Object not found");
    }
    if edit(&mut state.scene.world_mut()) {
        state.scene.bump_revision(true);
        CommandResult::ok(vec![id.to_string()])
    } else {
        CommandResult::noop(no_op_reason)
    }
}

/// The value of `id`'s instance at `component_index` when edits of its class
/// are checked against a level rule (a light), for [`revert_if_refused`].
fn checked_value(
    world: &pulsar_scenedb::World,
    id: &str,
    component_index: usize,
) -> Option<(pulsar_scenedb::Entity, Box<dyn Any + Send + Sync>)> {
    let instance = crate::scene_edit::components::instance_at(world, id, component_index)?;
    let class = engine_backend::scene::attachments::meta(world, instance)?
        .class_name
        .clone();
    if !level_rules::edits_are_checked(&class) {
        return None;
    }
    let value = pulsar_world_registry::clone_world_component_value(&class, world, instance)?;
    Some((instance, value))
}

/// After an edit of a checked instance ([`checked_value`]): when the edited
/// instance breaks a level rule, put its previous value back and return
/// the rule's message.
fn revert_if_refused(
    state: &mut LevelEditorState,
    previous: Option<(pulsar_scenedb::Entity, Box<dyn Any + Send + Sync>)>,
) -> Result<(), &'static str> {
    let Some((instance, previous)) = previous else {
        return Ok(());
    };
    let refused = level_rules::check_instance(&state.scene.world(), instance);
    if let Err(reason) = refused {
        let restored = pulsar_world_registry::set_instance_value(
            &mut state.scene.world_mut(),
            instance,
            pulsar_world_registry::InstanceValue::Value(previous),
        );
        if let Err(error) = restored {
            tracing::error!("Could not restore a refused edit: {error}");
        }
        return Err(reason);
    }
    Ok(())
}

fn set_component_property(
    state: &mut LevelEditorState,
    id: &String,
    class_name: &String,
    component_index: usize,
    prop_name: &String,
    value: Box<dyn Any + Send>,
) -> CommandResult {
    // Typed path first -- `update_live_component_property` writes
    // `value` straight onto the live `World` component via its
    // reflected setter closure, no JSON anywhere (Pulsar-Native#561).
    //
    // `component_index` targets the exact instance being edited
    // (Pulsar-Native#519): every instance is its own entity with
    // its own typed value (Pulsar-Native#1035). Only an
    // unresolved payload falls back to the payload write below.
    //
    // A class a level rule covers is checked after the write
    // (the setter knows which field the name means) and put back
    // when refused.
    let previous = checked_value(&state.scene.world(), id, component_index);
    let update_result = crate::scene_edit::components::update_live_component_property(
        &mut state.scene.world_mut(),
        id,
        class_name,
        component_index,
        prop_name,
        value,
    );
    match update_result {
        Ok(()) => {
            if let Err(reason) = revert_if_refused(state, previous) {
                return CommandResult::noop(reason);
            }
            crate::scene_edit::components::after_property_edit(
                &mut state.scene.world_mut(),
                id,
                class_name,
                component_index,
                prop_name,
            );
            state.scene.bump_revision(true);
            CommandResult::ok(vec![id.clone()])
        }
        Err(value) => {
            match pulsar_reflection::RUNTIME_TYPE_REGISTRY.serialize_json_for_any(value.as_ref()) {
                Ok(value_json) => {
                    // An unresolved payload: its JSON is the
                    // only copy of its values.
                    if crate::scene_edit::components::set_unresolved_property(
                        &mut state.scene.world_mut(),
                        id,
                        class_name,
                        component_index,
                        prop_name,
                        value_json,
                    ) {
                        state.scene.bump_revision(true);
                        CommandResult::ok(vec![id.clone()])
                    } else {
                        CommandResult::noop("No such property or no change")
                    }
                }
                Err(error) => {
                    // Not `World`-registered AND not in
                    // `RUNTIME_TYPE_REGISTRY` either -- nothing this
                    // command can do with the value. Surfaced loudly
                    // rather than silently dropping the edit: this
                    // should only happen for a genuinely new/
                    // misconfigured property type, not real usage.
                    tracing::error!(
                        "[SetComponentProperty] '{class_name}.{prop_name}' on '{id}' has \
                                 no live World value and its type isn't registered for JSON \
                                 fallback either -- edit dropped: {error}"
                    );
                    CommandResult::noop("Property type not registered for World or JSON fallback")
                }
            }
        }
    }
}

fn add_component(
    state: &mut LevelEditorState,
    id: &String,
    class_name: String,
    value: Option<Box<dyn Any + Send + Sync>>,
) -> CommandResult {
    let refused = level_rules::check_new_component(
        &state.scene.world(),
        &class_name,
        value.as_deref().map(|value| value as &dyn Any),
        true,
    );
    if let Err(reason) = refused {
        return CommandResult::noop(reason);
    }
    edit_components(state, id, "Component could not be added", |world| {
        crate::scene_edit::components::add_component_value(world, id, &class_name, value).is_some()
    })
}

fn remove_component(
    state: &mut LevelEditorState,
    id: &String,
    component_index: usize,
) -> CommandResult {
    edit_components(state, id, "No component at that index", |world| {
        crate::scene_edit::components::remove_component(world, id, component_index)
    })
}

fn set_component_enabled(
    state: &mut LevelEditorState,
    id: &String,
    component_index: usize,
    enabled: bool,
) -> CommandResult {
    let refused = {
        let world = state.scene.world();
        crate::scene_edit::components::instance_at(&world, id, component_index)
            .filter(|_| enabled)
            .map(|instance| level_rules::check_enable(&world, instance))
    };
    if let Some(Err(reason)) = refused {
        return CommandResult::noop(reason);
    }
    edit_components(
        state,
        id,
        "No component at that index, or already in that state",
        |world| {
            let instance = crate::scene_edit::components::instance_at(world, id, component_index);
            instance.is_some_and(|instance| {
                engine_backend::scene::attachments::is_enabled(world, instance) != enabled
            }) && crate::scene_edit::components::set_component_enabled(
                world,
                id,
                component_index,
                enabled,
            )
        },
    )
}

fn duplicate_component(
    state: &mut LevelEditorState,
    id: &String,
    component_index: usize,
) -> CommandResult {
    let refused = {
        let world = state.scene.world();
        crate::scene_edit::components::instance_at(&world, id, component_index)
            .map(|instance| level_rules::check_copy(&world, instance))
    };
    if let Some(Err(reason)) = refused {
        return CommandResult::noop(reason);
    }
    edit_components(state, id, "No component at that index", |world| {
        crate::scene_edit::components::duplicate_component(world, id, component_index).is_some()
    })
}

fn reorder_component(
    state: &mut LevelEditorState,
    id: &String,
    from_index: usize,
    to_index: usize,
) -> CommandResult {
    edit_components(state, id, "Index out of range or unchanged", |world| {
        crate::scene_edit::components::reorder_component(world, id, from_index, to_index)
    })
}

fn set_component_parent(
    state: &mut LevelEditorState,
    id: &String,
    component_index: usize,
    parent_index: Option<usize>,
) -> CommandResult {
    edit_components(state, id, "Parent rejected or unchanged", |world| {
        crate::scene_edit::components::set_component_parent(
            world,
            id,
            component_index,
            parent_index,
        )
    })
}

fn set_component_data(
    state: &mut LevelEditorState,
    id: &String,
    component_index: usize,
    data: ComponentData,
) -> CommandResult {
    let previous = checked_value(&state.scene.world(), id, component_index);
    let result = edit_components(
        state,
        id,
        "No component at that index, or no change",
        |world| {
            let value = match data {
                ComponentData::Value(value) => pulsar_world_registry::InstanceValue::Value(value),
                ComponentData::Unresolved(data) => {
                    // Only an unresolved instance takes a new payload.
                    let instance =
                        crate::scene_edit::components::instance_at(world, id, component_index);
                    let Some(current) = instance.and_then(|instance| {
                        world
                            .get::<engine_backend::scene::attachments::UnresolvedComponent>(
                                instance,
                            )
                            .cloned()
                    }) else {
                        return false;
                    };
                    if current.data == data {
                        return false;
                    }
                    pulsar_world_registry::InstanceValue::Unresolved(
                        engine_backend::scene::attachments::UnresolvedComponent {
                            data,
                            reason: current.reason,
                        },
                    )
                }
            };
            crate::scene_edit::components::set_component_value(world, id, component_index, value)
        },
    );
    if result.changed {
        if let Err(reason) = revert_if_refused(state, previous) {
            return CommandResult::noop(reason);
        }
    }
    result
}

fn set_movability(
    state: &mut LevelEditorState,
    ids: &[String],
    movability: helio_component::components::ObjectMovability,
) -> CommandResult {
    use crate::scene_edit::components;
    let mut affected = Vec::new();
    {
        let mut world = state.scene.world_mut();
        for id in ids {
            // Every component that authors a movability (a
            // mesh, a light: a `movability` property of that
            // type) and holds another one.
            let targets: Vec<(usize, String)> = components::get_component_class_names(&world, id)
                .into_iter()
                .enumerate()
                .filter(|(index, class_name)| {
                    components::read_live_component_property(
                        &world,
                        id,
                        class_name,
                        *index,
                        "movability",
                    )
                    .and_then(|value| {
                        value
                            .downcast_ref::<helio_component::components::ObjectMovability>()
                            .copied()
                    })
                    .is_some_and(|current| current != movability)
                })
                .collect();
            for (index, class_name) in targets {
                // Typed setter; `movability` is a flat property
                // name (lights' via `#[sub_props]`).
                if components::update_live_component_property(
                    &mut world,
                    id,
                    &class_name,
                    index,
                    "movability",
                    Box::new(movability),
                )
                .is_ok()
                    && !affected.contains(id)
                {
                    affected.push(id.clone());
                }
            }
        }
    }
    if affected.is_empty() {
        CommandResult::noop("Nothing with a movability to change")
    } else {
        state.scene.bump_revision(true);
        CommandResult::ok(affected)
    }
}
