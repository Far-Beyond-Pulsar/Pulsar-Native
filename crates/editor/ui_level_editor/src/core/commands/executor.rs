use super::{CommandResult, SceneCommand};
use crate::scene_edit::history::capture_history_subset;
use crate::state::LevelEditorState;
use engine_backend::scene::SceneWorldExt;

fn command_scope(cmd: &SceneCommand) -> Vec<String> {
    match cmd {
        SceneCommand::AddObject { data, .. } => (!data.id.is_empty())
            .then(|| data.id.clone())
            .into_iter()
            .collect(),
        SceneCommand::RemoveObject { id }
        | SceneCommand::ReparentObject { id, .. }
        | SceneCommand::SetTransform { id, .. }
        | SceneCommand::SetName { id, .. }
        | SceneCommand::SetVisibility { id, .. }
        | SceneCommand::SetComponentProperty { id, .. }
        | SceneCommand::RevertComponentProperty { id, .. }
        | SceneCommand::SetClassVariable { id, .. }
        | SceneCommand::AddComponent { id, .. }
        | SceneCommand::RemoveComponent { id, .. }
        | SceneCommand::SetComponentEnabled { id, .. }
        | SceneCommand::DuplicateComponent { id, .. }
        | SceneCommand::ReorderComponent { id, .. }
        | SceneCommand::SetComponentParent { id, .. }
        | SceneCommand::SetComponentData { id, .. }
        | SceneCommand::RevertClassSlot { id, .. }
        | SceneCommand::ResetClassOverrides { id } => vec![id.clone()],
        SceneCommand::UpdateObject { data } => vec![data.id.clone()],
        SceneCommand::SetMovability { ids, .. } => ids.clone(),
        SceneCommand::DuplicateObject { source_id, .. } => vec![source_id.clone()],
        SceneCommand::SelectObject { .. } | SceneCommand::InstantiateClass { .. } => Vec::new(),
    }
}

/// Run a structural component edit on `id` and report whether it changed
/// anything. The scene-edit component functions mostly return `()`, so the
/// object's component list (live values included) is compared before and after.
fn edit_components(
    state: &mut LevelEditorState,
    id: &str,
    no_op_reason: &'static str,
    edit: impl FnOnce(&mut pulsar_scenedb::World),
) -> CommandResult {
    let fingerprint = |state: &LevelEditorState| {
        let world = state.scene.world();
        world.entity_for(id)?;
        serde_json::to_value(crate::scene_edit::components::get_components(&world, id)).ok()
    };
    let Some(before) = fingerprint(state) else {
        return CommandResult::noop("Object not found");
    };
    edit(&mut state.scene.world_mut());
    if fingerprint(state).is_some_and(|after| after != before) {
        state.scene.bump_revision(true);
        CommandResult::ok(vec![id.to_string()])
    } else {
        CommandResult::noop(no_op_reason)
    }
}

/// The `movability` stored in a mesh's (top-level) or light's
/// (`general.movability`, `#[sub_props]`-nested) component data.
pub(crate) fn authored_movability(
    data: &serde_json::Value,
) -> Option<helio_component::components::ObjectMovability> {
    data.get("movability")
        .or_else(|| data.pointer("/general/movability"))
        .and_then(|value| serde_json::from_value(value.clone()).ok())
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
    let arms_new_render_rows = matches!(
        &cmd,
        SceneCommand::AddObject { .. }
            | SceneCommand::DuplicateObject { .. }
            | SceneCommand::InstantiateClass { .. }
    );
    let scope = command_scope(&cmd);
    let pre_state = is_undoable.then(|| capture_history_subset(&state.scene.world(), &scope));

    // Reborrowed (not the outer `state` itself) so the `move` closure below
    // can take ownership of this reborrow without moving the actual `state`
    // parameter -- which is used again after the closure returns, to commit
    // the checkpoint captured above.
    let state_ref = &mut *state;
    let result = (move || -> CommandResult {
        let state = state_ref;
        match cmd {
            SceneCommand::AddObject { data, parent_id } => {
                let id = crate::scene_edit::objects::add_object(
                    &mut state.scene.world_mut(),
                    data,
                    parent_id,
                );
                if id.is_empty() {
                    return CommandResult::noop("Object could not be added");
                }
                state.scene.bump_revision(true);
                CommandResult::ok(vec![id])
            }

            SceneCommand::InstantiateClass {
                ref class_dir,
                ref transform,
                ref parent_id,
            } => {
                let placed = crate::scene_edit::classes::instantiate_class_dir(
                    &mut state.scene.world_mut(),
                    class_dir,
                    transform,
                    parent_id.as_deref(),
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

            SceneCommand::RevertComponentProperty {
                ref id,
                ref class_name,
                component_index,
                ref prop_name,
            } => {
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
                    let Some(json) = json else {
                        return CommandResult::noop("Class default could not be written");
                    };
                    crate::scene_edit::components::update_component_property(
                        &mut state.scene.world_mut(),
                        id,
                        class_name,
                        component_index,
                        prop_name,
                        json,
                    );
                }
                state.scene.bump_revision(true);
                CommandResult::ok(vec![id.clone()])
            }

            SceneCommand::SetClassVariable {
                ref id,
                ref name,
                ref value,
            } => {
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
                    None => crate::scene_edit::classes::revert_variable(
                        &mut state.scene.world_mut(),
                        id,
                        name,
                    ),
                };
                if changed {
                    state.scene.bump_revision(true);
                    CommandResult::ok(vec![id.clone()])
                } else {
                    CommandResult::noop("Not a class instance, or nothing to revert")
                }
            }

            SceneCommand::RemoveObject { ref id } => {
                let removed =
                    crate::scene_edit::objects::remove_object(&mut state.scene.world_mut(), id);
                if removed {
                    if crate::scene_edit::objects::get_selected_object_id(&state.scene.world())
                        .as_deref()
                        == Some(id)
                    {
                        crate::scene_edit::objects::select_object(
                            &mut state.scene.world_mut(),
                            None,
                        );
                    }
                    state.scene.bump_revision(true);
                    CommandResult::ok(vec![id.clone()])
                } else {
                    CommandResult::noop("Object not found")
                }
            }

            SceneCommand::UpdateObject { data } => {
                let id = data.id.clone();
                if crate::scene_edit::objects::update_object(&mut state.scene.world_mut(), data) {
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
                let moved = crate::scene_edit::objects::reparent_object(
                    &mut state.scene.world_mut(),
                    id,
                    new_parent_id.clone(),
                );
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
                let src_pos =
                    crate::scene_edit::objects::get_object(&state.scene.world(), source_id)
                        .map(|o| o.transform.position);
                let mut created = Vec::new();
                for i in 0..count {
                    // Results are bound before each `if let`: a scene guard
                    // taken in its scrutinee would live through the block,
                    // which locks the scene again.
                    let duplicated = crate::scene_edit::objects::duplicate_object(
                        &mut state.scene.world_mut(),
                        source_id,
                    );
                    if let Some(new_id) = duplicated {
                        if let (Some(off), Some(src)) = (position_offset, src_pos) {
                            let n = (i + 1) as f32;
                            let copy = crate::scene_edit::objects::get_object(
                                &state.scene.world(),
                                &new_id,
                            );
                            if let Some(mut copy) = copy {
                                copy.transform.position = [
                                    src[0] + off[0] * n,
                                    src[1] + off[1] * n,
                                    src[2] + off[2] * n,
                                ];
                                crate::scene_edit::objects::update_object(
                                    &mut state.scene.world_mut(),
                                    copy,
                                );
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
                crate::scene_edit::objects::select_object(
                    &mut state.scene.world_mut(),
                    id.as_deref(),
                );
                state.scene.bump_revision(false);
                CommandResult::ok(id.into_iter().collect())
            }

            SceneCommand::SetTransform {
                ref id,
                position,
                rotation,
                scale,
            } => {
                // `SceneDatabase::set_transform`, NOT `get_object`+`update_object`
                // (Pulsar-Native#561): the old whole-object round trip triggered
                // `sync_registered_component_props_to_scene_db` -- a full
                // re-serialize/re-hydrate of every component on the object --
                // on every keystroke of a position/rotation/scale field, for a
                // change that has nothing to do with component data at all.
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

            SceneCommand::SetName { ref id, name } => {
                if crate::scene_edit::objects::set_name(&mut state.scene.world_mut(), id, name) {
                    state.scene.bump_revision(true);
                    CommandResult::ok(vec![id.clone()])
                } else {
                    CommandResult::noop("Object not found")
                }
            }

            SceneCommand::SetVisibility {
                ref id,
                visible,
                locked,
            } => {
                let mut changed = false;
                if let Some(v) = visible {
                    changed |= crate::scene_edit::objects::set_visible(
                        &mut state.scene.world_mut(),
                        id,
                        v,
                    );
                }
                if let Some(l) = locked {
                    changed |=
                        crate::scene_edit::objects::set_locked(&mut state.scene.world_mut(), id, l);
                }
                if changed {
                    state.scene.bump_revision(true);
                    CommandResult::ok(vec![id.clone()])
                } else {
                    CommandResult::noop("Object not found or no visibility fields changed")
                }
            }

            SceneCommand::SetComponentProperty {
                ref id,
                ref class_name,
                component_index,
                ref prop_name,
                value,
            } => {
                // Typed path first -- `update_live_component_property` writes
                // `value` straight onto the live `World` component via its
                // reflected setter closure, no JSON anywhere (Pulsar-Native#561).
                //
                // `component_index` targets the exact instance being edited
                // (Pulsar-Native#519): every instance is its own entity with
                // its own typed value (Pulsar-Native#1035). Only an instance
                // whose class this build does not register (an unresolved
                // payload) falls back to the indexed JSON write below.
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
                        match pulsar_reflection::RUNTIME_TYPE_REGISTRY
                            .serialize_json_for_any(value.as_ref())
                        {
                            Ok(value_json) => {
                                crate::scene_edit::components::update_component_property(
                                    &mut state.scene.world_mut(),
                                    id,
                                    class_name,
                                    component_index,
                                    prop_name,
                                    value_json,
                                );
                                state.scene.bump_revision(true);
                                CommandResult::ok(vec![id.clone()])
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
                                CommandResult::noop(
                                    "Property type not registered for World or JSON fallback",
                                )
                            }
                        }
                    }
                }
            }

            SceneCommand::AddComponent {
                ref id,
                class_name,
                data,
            } => edit_components(state, id, "Component could not be added", |world| {
                crate::scene_edit::components::add_component(world, id, class_name, data);
            }),

            SceneCommand::RemoveComponent {
                ref id,
                component_index,
            } => edit_components(state, id, "No component at that index", |world| {
                crate::scene_edit::components::remove_component(world, id, component_index)
            }),

            SceneCommand::SetComponentEnabled {
                ref id,
                component_index,
                enabled,
            } => edit_components(
                state,
                id,
                "No component at that index, or already in that state",
                |world| {
                    crate::scene_edit::components::set_component_enabled(
                        world,
                        id,
                        component_index,
                        enabled,
                    );
                },
            ),

            SceneCommand::DuplicateComponent {
                ref id,
                component_index,
            } => edit_components(state, id, "No component at that index", |world| {
                crate::scene_edit::components::duplicate_component(world, id, component_index);
            }),

            SceneCommand::ReorderComponent {
                ref id,
                from_index,
                to_index,
            } => edit_components(state, id, "Index out of range or unchanged", |world| {
                crate::scene_edit::components::reorder_component(world, id, from_index, to_index)
            }),

            SceneCommand::SetComponentParent {
                ref id,
                component_index,
                parent_index,
            } => edit_components(state, id, "Parent rejected or unchanged", |world| {
                crate::scene_edit::components::set_component_parent(
                    world,
                    id,
                    component_index,
                    parent_index,
                )
            }),

            SceneCommand::SetComponentData {
                ref id,
                component_index,
                data,
            } => edit_components(
                state,
                id,
                "No component at that index, or no change",
                |world| {
                    if component_index < crate::scene_edit::components::component_count(world, id) {
                        crate::scene_edit::components::update_component(
                            world,
                            id,
                            component_index,
                            data,
                        )
                    }
                },
            ),

            SceneCommand::RevertClassSlot {
                ref id,
                ref slot_id,
                ref path,
            } => {
                let registry = crate::scene_edit::classes::project_registry();
                if crate::scene_edit::classes::revert_slot(
                    &mut state.scene.world_mut(),
                    id,
                    slot_id,
                    path.as_deref(),
                    &registry,
                ) {
                    state.scene.bump_revision(true);
                    CommandResult::ok(vec![id.clone()])
                } else {
                    CommandResult::noop("Not a class instance, or nothing to revert")
                }
            }

            SceneCommand::ResetClassOverrides { ref id } => {
                use crate::scene_edit::classes;
                let registry = classes::project_registry();
                let Some(view) = classes::class_instance_view(&state.scene.world(), id, &registry)
                else {
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
                        changed |=
                            classes::revert_slot(&mut world, id, &slot.slot_id, None, &registry);
                    }
                }
                if changed {
                    state.scene.bump_revision(true);
                    CommandResult::ok(vec![id.clone()])
                } else {
                    CommandResult::noop("Nothing overridden")
                }
            }

            SceneCommand::SetMovability { ids, movability } => {
                use crate::scene_edit::components;
                let mut affected = Vec::new();
                {
                    let mut world = state.scene.world_mut();
                    for id in &ids {
                        let targets: Vec<(usize, String)> = components::get_components(&world, id)
                            .into_iter()
                            .enumerate()
                            .filter(|(_, c)| {
                                matches!(
                                    c.class_name.as_str(),
                                    "StaticMeshComponent" | "LightComponent"
                                ) && authored_movability(&c.data) != Some(movability)
                            })
                            .map(|(index, c)| (index, c.class_name))
                            .collect();
                        for (index, class_name) in targets {
                            // Typed setter; `movability` is a flat property
                            // name on both classes (lights via `#[sub_props]`).
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
                    CommandResult::noop("No mesh or light to change")
                } else {
                    state.scene.bump_revision(true);
                    CommandResult::ok(affected)
                }
            }
        }
    })();

    if result.changed {
        if arms_new_render_rows {
            let mut world = state.scene.world_mut();
            // Include descendants: a duplicated class instance brings
            // generated child objects along with its root.
            let mut entities = Vec::new();
            for id in &result.affected_ids {
                if let Some(entity) = world.entity_for(id) {
                    let mut stack = vec![entity];
                    while let Some(e) = stack.pop() {
                        if !entities.contains(&e) {
                            entities.push(e);
                            stack.extend(world.children_of(Some(e)));
                        }
                    }
                }
            }
            for entity in entities {
                engine_backend::scene::arm_render_row_subscriptions_for_entity(&mut world, entity);
            }
        }
        if let Some(pre) = pre_state {
            let mut post_scope = scope;
            for id in &result.affected_ids {
                if !post_scope.iter().any(|existing| existing == id) {
                    post_scope.push(id.clone());
                }
            }
            let post = capture_history_subset(&state.scene.world(), &post_scope);
            state.scene.commit_undo_checkpoint(pre, post);
        }
    }
    result
}
