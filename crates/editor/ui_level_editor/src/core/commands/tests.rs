#[cfg(test)]
mod undo_redo_tests {
    use super::super::*;
    use crate::scene_edit::{ObjectType, SceneObjectData, Transform};
    use crate::state::LevelEditorState;

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
            SceneCommand::AddObject {
                data: object("Cube"),
                parent_id: None,
            },
        );
        assert!(result.changed);
        assert!(state.scene.can_undo());
        assert_eq!(
            crate::scene_edit::objects::get_all_objects(&state.scene.world()).len(),
            1
        );

        assert!(state.scene.undo());
        assert!(crate::scene_edit::objects::get_all_objects(&state.scene.world()).is_empty());
        assert!(!state.scene.can_undo());
        assert!(state.scene.can_redo());
    }

    #[test]
    fn redo_reapplies_an_undone_command() {
        let mut state = LevelEditorState::new();
        execute_command(
            &mut state,
            SceneCommand::AddObject {
                data: object("Cube"),
                parent_id: None,
            },
        );
        state.scene.undo();
        assert!(crate::scene_edit::objects::get_all_objects(&state.scene.world()).is_empty());

        assert!(state.scene.redo());

        assert_eq!(
            crate::scene_edit::objects::get_all_objects(&state.scene.world()).len(),
            1
        );
        assert!(state.scene.can_undo());
        assert!(!state.scene.can_redo());
    }

    fn object_count(state: &LevelEditorState) -> usize {
        crate::scene_edit::objects::get_all_objects(&state.scene.world()).len()
    }

    /// Each history entry keeps the ids of both sides: redoing a removal
    /// removes the object again, and undoing a redone add removes it again.
    #[test]
    fn redo_and_undo_keep_working_after_a_round_trip() {
        let mut state = LevelEditorState::new();
        let id = execute_command(
            &mut state,
            SceneCommand::AddObject {
                data: object("Cube"),
                parent_id: None,
            },
        )
        .affected_ids[0]
            .clone();
        execute_command(&mut state, SceneCommand::RemoveObject { id });
        assert_eq!(object_count(&state), 0);

        assert!(state.scene.undo());
        assert_eq!(object_count(&state), 1, "undo restores the removed object");
        assert!(state.scene.redo());
        assert_eq!(object_count(&state), 0, "redo removes it again");
        assert!(state.scene.undo());
        assert_eq!(object_count(&state), 1);

        assert!(state.scene.undo());
        assert_eq!(object_count(&state), 0, "undo the add");
        assert!(state.scene.redo());
        assert_eq!(object_count(&state), 1, "redo the add");
        assert!(state.scene.undo());
        assert_eq!(object_count(&state), 0, "undo a redone add");
    }

    #[test]
    fn a_new_mutating_command_clears_the_redo_stack() {
        let mut state = LevelEditorState::new();
        execute_command(
            &mut state,
            SceneCommand::AddObject {
                data: object("A"),
                parent_id: None,
            },
        );
        state.scene.undo();
        assert!(state.scene.can_redo());

        execute_command(
            &mut state,
            SceneCommand::AddObject {
                data: object("B"),
                parent_id: None,
            },
        );

        assert!(!state.scene.can_redo());
    }

    #[test]
    fn a_noop_command_does_not_push_an_undo_checkpoint() {
        let mut state = LevelEditorState::new();
        let result = execute_command(
            &mut state,
            SceneCommand::RemoveObject {
                id: "nope".to_string(),
            },
        );
        assert!(!result.changed);
        assert!(!state.scene.can_undo());
    }

    #[test]
    fn selecting_an_object_does_not_push_an_undo_checkpoint() {
        let mut state = LevelEditorState::new();
        let add = execute_command(
            &mut state,
            SceneCommand::AddObject {
                data: object("Cube"),
                parent_id: None,
            },
        );
        let id = add.affected_ids[0].clone();
        assert!(state.scene.can_undo()); // the AddObject checkpoint

        execute_command(&mut state, SceneCommand::SelectObject { id: Some(id) });

        // Still exactly the one checkpoint from AddObject -- undoing once
        // now must remove the object, not merely revert the selection.
        assert!(state.scene.undo());
        assert!(crate::scene_edit::objects::get_all_objects(&state.scene.world()).is_empty());
        assert!(!state.scene.can_undo());
    }

    #[test]
    fn undo_and_redo_on_an_empty_history_are_no_ops() {
        let mut state = LevelEditorState::new();
        assert!(!state.scene.undo());
        assert!(!state.scene.redo());
    }

    // ── SetComponentProperty: end-to-end through the command layer ─────────
    //
    // Pulsar-Native#561: proves the whole live-edit call graph a real
    // properties-panel color/intensity edit takes -- widget produces a typed
    // `Box<dyn Any + Send>`, `SceneCommand::SetComponentProperty` carries it
    // unchanged, `execute_command` applies it -- actually reaches the live
    // `World` value and is undo-tracked, with no `serde_json::Value`
    // anywhere on this call path (unlike the tests in `scene_database.rs`,
    // which exercise `SceneDatabase` methods directly, this goes through the
    // actual `SceneCommand` enum + `execute_command` UI code calls).
    #[test]
    fn set_component_property_reaches_the_live_world_value_with_no_json() {
        let mut state = LevelEditorState::new();
        let id = execute_command(
            &mut state,
            SceneCommand::AddObject {
                data: SceneObjectData {
                    id: String::new(),
                    name: "Light".to_string(),
                    object_type: crate::scene_edit::ObjectType::Light(
                        crate::scene_edit::LightType::Point,
                    ),
                    transform: crate::scene_edit::Transform::default(),
                    visible: true,
                    locked: false,
                    parent: None,
                    children: vec![],
                    scene_path: String::new(),
                    props: Default::default(),
                    component_instances: None,
                },
                parent_id: None,
            },
        )
        .affected_ids[0]
            .clone();

        let default_light_json =
            serde_json::to_value(helio_component::LightComponent::default()).unwrap();
        {
            let mut world = state.scene.world_mut();
            crate::scene_edit::components::add_component(
                &mut world,
                &id,
                "LightComponent".to_string(),
                default_light_json,
            );
        }

        // The widget layer's actual contract: a boxed, already-typed value --
        // never JSON. `intensity` is a leaf of the `#[sub_props]`-nested
        // `IntensityLightProps`, so this also exercises the nested getter/
        // setter closure chain, not just a top-level field.
        let result = execute_command(
            &mut state,
            SceneCommand::SetComponentProperty {
                id: id.clone(),
                class_name: "LightComponent".to_string(),
                component_index: 0,
                prop_name: "intensity".to_string(),
                value: Box::new(4242.0_f32),
            },
        );
        assert!(
            result.changed,
            "typed live write must succeed, not fall through to the JSON path"
        );

        let live = {
            let world = state.scene.world();
            crate::scene_edit::components::read_live_component_property(
                &world,
                &id,
                "LightComponent",
                0,
                "intensity",
            )
        }
        .expect("intensity must be live-readable after the edit");
        assert_eq!(live.downcast_ref::<f32>(), Some(&4242.0));

        // Undo-tracked like every other command: restoring the pre-edit
        // snapshot must revert the live World value too, not just
        // `metadata_db`'s mirror.
        assert!(state.scene.undo());
        let reverted = {
            let world = state.scene.world();
            crate::scene_edit::components::read_live_component_property(
                &world,
                &id,
                "LightComponent",
                0,
                "intensity",
            )
        }
        .expect("intensity must still be live-readable after undo");
        assert_eq!(reverted.downcast_ref::<f32>(), Some(&1000.0)); // IntensityLightProps::default()
    }

    // Bool twin of the f32 test above: proves a widget toggling a `bool`
    // property (`BoolEditor`'s Switch → `SetComponentProperty` with a
    // `Box::new(bool)`) flips the live `World` value and is undo-tracked.
    #[test]
    fn set_component_property_flips_a_live_world_bool() {
        let mut state = LevelEditorState::new();
        let id = execute_command(
            &mut state,
            SceneCommand::AddObject {
                data: SceneObjectData {
                    id: String::new(),
                    name: "Light".to_string(),
                    object_type: crate::scene_edit::ObjectType::Light(
                        crate::scene_edit::LightType::Point,
                    ),
                    transform: crate::scene_edit::Transform::default(),
                    visible: true,
                    locked: false,
                    parent: None,
                    children: vec![],
                    scene_path: String::new(),
                    props: Default::default(),
                    component_instances: None,
                },
                parent_id: None,
            },
        )
        .affected_ids[0]
            .clone();

        let default_light_json =
            serde_json::to_value(helio_component::LightComponent::default()).unwrap();
        {
            let mut world = state.scene.world_mut();
            crate::scene_edit::components::add_component(
                &mut world,
                &id,
                "LightComponent".to_string(),
                default_light_json,
            );
        }

        // `enabled` defaults to `true`; a switch toggle writes `false`.
        let result = execute_command(
            &mut state,
            SceneCommand::SetComponentProperty {
                id: id.clone(),
                class_name: "LightComponent".to_string(),
                component_index: 0,
                prop_name: "enabled".to_string(),
                value: Box::new(false),
            },
        );
        assert!(
            result.changed,
            "typed live bool write must succeed, not fall through to the JSON path"
        );

        let live = {
            let world = state.scene.world();
            crate::scene_edit::components::read_live_component_property(
                &world,
                &id,
                "LightComponent",
                0,
                "enabled",
            )
        }
        .expect("enabled must be live-readable after the toggle");
        assert_eq!(live.downcast_ref::<bool>(), Some(&false));

        // Undo must revert the live bool back to `true`.
        assert!(state.scene.undo());
        let reverted = {
            let world = state.scene.world();
            crate::scene_edit::components::read_live_component_property(
                &world,
                &id,
                "LightComponent",
                0,
                "enabled",
            )
        }
        .expect("enabled must still be live-readable after undo");
        assert_eq!(reverted.downcast_ref::<bool>(), Some(&true)); // GeneralLightProps::default()
    }

    // A component property command writes the live value through the World:
    // the revision moves, and the selected object's subscription (what the
    // properties panel follows) delivers the new value.
    #[test]
    fn a_component_property_command_reaches_the_object_feed() {
        use engine_backend::scene::SceneWorldExt;
        use pulsar_world_registry::{ObjectFeed, ObjectUpdate};

        let mut state = LevelEditorState::new();
        let id = execute_command(
            &mut state,
            SceneCommand::AddObject {
                data: SceneObjectData {
                    id: String::new(),
                    name: "Light".to_string(),
                    object_type: crate::scene_edit::ObjectType::Light(
                        crate::scene_edit::LightType::Point,
                    ),
                    transform: crate::scene_edit::Transform::default(),
                    visible: true,
                    locked: false,
                    parent: None,
                    children: vec![],
                    scene_path: String::new(),
                    props: Default::default(),
                    component_instances: None,
                },
                parent_id: None,
            },
        )
        .affected_ids[0]
            .clone();

        let default_light_json =
            serde_json::to_value(helio_component::LightComponent::default()).unwrap();
        let feed = {
            let mut world = state.scene.world_mut();
            crate::scene_edit::components::add_component(
                &mut world,
                &id,
                "LightComponent".to_string(),
                default_light_json,
            );
            let entity = world.entity_for(&id).unwrap();
            ObjectFeed::subscribe(&mut world, entity, || {}).unwrap()
        };

        let revision_before = state.scene.world().revision();
        execute_command(
            &mut state,
            SceneCommand::SetComponentProperty {
                id: id.clone(),
                class_name: "LightComponent".to_string(),
                component_index: 0,
                prop_name: "enabled".to_string(),
                value: Box::new(false),
            },
        );
        assert!(
            state.scene.world().revision() > revision_before,
            "live component writes must move the World revision"
        );

        let delivered = feed.take().into_iter().find_map(|update| match update {
            ObjectUpdate::Changed(delta) => delta
                .value?
                .downcast::<helio_component::LightComponent>()
                .ok(),
            ObjectUpdate::Despawned => None,
        });
        assert_eq!(
            delivered.map(|light| light.general.enabled),
            Some(false),
            "the object's subscription delivers the edited value"
        );
    }

    // Pulsar-Native#837: "Mark selection Static" sets every mesh/light on
    // every selected object in one undo step, skips objects with neither,
    // and is a no-op when everything already matches.
    #[test]
    fn set_movability_marks_the_selection_in_one_undo_step() {
        use helio_component::components::ObjectMovability;
        let mut state = LevelEditorState::new();
        let add = |state: &mut LevelEditorState, name: &str| {
            execute_command(
                state,
                SceneCommand::AddObject {
                    data: object(name),
                    parent_id: None,
                },
            )
            .affected_ids[0]
                .clone()
        };
        let lamp = add(&mut state, "Lamp");
        let empty = add(&mut state, "Empty");
        {
            let mut light = helio_component::LightComponent::default();
            light.general.movability = ObjectMovability::Movable;
            let mut world = state.scene.world_mut();
            crate::scene_edit::components::add_component(
                &mut world,
                &lamp,
                "LightComponent".to_string(),
                serde_json::to_value(light).unwrap(),
            );
        }
        let read = |state: &LevelEditorState| {
            let world = state.scene.world();
            crate::scene_edit::components::read_live_component_property(
                &world,
                &lamp,
                "LightComponent",
                0,
                "movability",
            )
            .and_then(|v| v.downcast_ref::<ObjectMovability>().copied())
        };
        assert_eq!(read(&state), Some(ObjectMovability::Movable));

        let mark_static = || SceneCommand::SetMovability {
            ids: vec![lamp.clone(), empty.clone()],
            movability: ObjectMovability::Static,
        };
        let result = execute_command(&mut state, mark_static());
        assert!(result.changed);
        assert_eq!(result.affected_ids, vec![lamp.clone()]);
        assert_eq!(read(&state), Some(ObjectMovability::Static));

        assert!(
            !execute_command(&mut state, mark_static()).changed,
            "already Static"
        );

        assert!(state.scene.undo());
        assert_eq!(read(&state), Some(ObjectMovability::Movable));
    }

    // Pulsar-Native#1035, Phase 3: undo restores typed snapshots in place.
    // Surviving instances keep their entity and id (nothing is respawned or
    // decoded), a removed one comes back with its id, and an unresolved
    // payload is kept as it was.
    #[test]
    fn undo_restores_component_instances_in_place() {
        use engine_backend::scene::{attachments, SceneWorldExt};
        let mut state = LevelEditorState::new();
        let id = execute_command(
            &mut state,
            SceneCommand::AddObject {
                data: object("Lamp"),
                parent_id: None,
            },
        )
        .affected_ids[0]
            .clone();
        let (light, unresolved) = {
            let mut world = state.scene.world_mut();
            let owner = world.entity_for(&id).unwrap();
            let light = pulsar_world_registry::attach_value(
                &mut world,
                owner,
                helio_component::LightComponent::default(),
            )
            .unwrap();
            let unresolved = pulsar_world_registry::attach_unresolved(
                &mut world,
                owner,
                attachments::NewInstance::new("NotInThisBuild"),
                serde_json::json!({ "kept": [1, 2, 3] }),
                "not registered".to_string(),
            )
            .unwrap();
            (light, unresolved)
        };
        let ids = |state: &LevelEditorState| {
            let world = state.scene.world();
            let owner = world.entity_for(&id).unwrap();
            attachments::instances(&world, owner)
                .into_iter()
                .map(|instance| (instance, attachments::meta(&world, instance).unwrap().id))
                .collect::<Vec<_>>()
        };
        let before = ids(&state);
        let intensity = |state: &LevelEditorState| {
            let world = state.scene.world();
            world
                .get::<helio_component::LightComponent>(light)
                .map(|light| light.intensity.intensity)
        };

        let result = execute_command(
            &mut state,
            SceneCommand::SetComponentProperty {
                id: id.clone(),
                class_name: "LightComponent".to_string(),
                component_index: 0,
                prop_name: "intensity".to_string(),
                value: Box::new(7.0_f32),
            },
        );
        assert!(result.changed);
        assert_eq!(intensity(&state), Some(7.0));
        assert!(state.scene.undo());
        assert_eq!(ids(&state), before, "same entities, ids and order");
        assert_eq!(intensity(&state), Some(1000.0), "value restored in place");
        assert!(state.scene.redo());
        assert_eq!(ids(&state), before);
        assert_eq!(intensity(&state), Some(7.0));

        let result = execute_command(
            &mut state,
            SceneCommand::RemoveComponent {
                id: id.clone(),
                component_index: 1,
            },
        );
        assert!(result.changed);
        assert_eq!(ids(&state).len(), 1);
        assert!(state.scene.undo());
        let after = ids(&state);
        assert_eq!(after[0], before[0], "the light kept its entity");
        assert_eq!(after[1].1, before[1].1, "the removed instance kept its id");
        let world = state.scene.world();
        let payload = world
            .get::<attachments::UnresolvedComponent>(after[1].0)
            .expect("still unresolved");
        assert_eq!(payload.data, serde_json::json!({ "kept": [1, 2, 3] }));
        assert_ne!(after[1].0, unresolved, "re-attached as a new entity");
    }

    // Pulsar-Native#1035, Phase 3: component commands carry typed values
    // and report a change only when one happened.
    #[test]
    fn typed_component_commands_are_undoable_and_report_no_ops() {
        use engine_backend::scene::{attachments, SceneWorldExt};
        let mut state = LevelEditorState::new();
        let mut light = helio_component::LightComponent::default();
        light.intensity.intensity = 25.0;
        let added = execute_command(
            &mut state,
            SceneCommand::AddObjectWithComponents {
                data: object("Lamp"),
                parent_id: None,
                components: vec![
                    super::super::TypedComponent::new(light.clone()),
                    super::super::TypedComponent {
                        enabled: false,
                        ..super::super::TypedComponent::new(light)
                    },
                ],
            },
        );
        let id = added.affected_ids[0].clone();
        let instances = |state: &LevelEditorState| {
            let world = state.scene.world();
            let owner = world.entity_for(&id).unwrap();
            attachments::instances(&world, owner)
                .into_iter()
                .map(|instance| {
                    (
                        attachments::is_enabled(&world, instance),
                        world
                            .get::<helio_component::LightComponent>(instance)
                            .map(|light| light.intensity.intensity),
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            instances(&state),
            vec![(true, Some(25.0)), (false, Some(25.0))],
            "attached typed, in order, enabled as asked"
        );

        // Class default when no value is given.
        assert!(
            execute_command(
                &mut state,
                SceneCommand::AddComponent {
                    id: id.clone(),
                    class_name: "LightComponent".into(),
                    value: None,
                },
            )
            .changed
        );
        assert_eq!(instances(&state)[2], (true, Some(1000.0)));

        // A whole-value replacement keeps the instance and is undoable.
        let mut brighter = helio_component::LightComponent::default();
        brighter.intensity.intensity = 50.0;
        assert!(
            execute_command(
                &mut state,
                SceneCommand::SetComponentData {
                    id: id.clone(),
                    component_index: 0,
                    data: super::super::ComponentData::Value(Box::new(brighter)),
                },
            )
            .changed
        );
        assert_eq!(instances(&state)[0], (true, Some(50.0)));
        assert!(state.scene.undo());
        assert_eq!(instances(&state)[0], (true, Some(25.0)));

        // A value of another class is refused; so is an unresolved payload
        // for a live instance.
        for data in [
            super::super::ComponentData::Value(Box::new(7_u32)),
            super::super::ComponentData::Unresolved(serde_json::json!({})),
        ] {
            assert!(
                !execute_command(
                    &mut state,
                    SceneCommand::SetComponentData {
                        id: id.clone(),
                        component_index: 0,
                        data,
                    },
                )
                .changed
            );
        }

        // No-ops push nothing.
        for no_op in [
            SceneCommand::SetComponentEnabled {
                id: id.clone(),
                component_index: 1,
                enabled: false,
            },
            SceneCommand::ReorderComponent {
                id: id.clone(),
                from_index: 1,
                to_index: 1,
            },
            SceneCommand::SetComponentParent {
                id: id.clone(),
                component_index: 1,
                parent_index: None,
            },
            SceneCommand::RemoveComponent {
                id: id.clone(),
                component_index: 9,
            },
        ] {
            assert!(!execute_command(&mut state, no_op).changed);
        }
        assert!(
            execute_command(
                &mut state,
                SceneCommand::SetComponentParent {
                    id: id.clone(),
                    component_index: 1,
                    parent_index: Some(0),
                },
            )
            .changed
        );

        // Undo the parent link and the added default, then the object with
        // both of its components in one step.
        assert!(state.scene.undo());
        assert!(state.scene.undo());
        assert_eq!(instances(&state).len(), 2);
        assert!(state.scene.undo());
        assert!(crate::scene_edit::objects::get_all_objects(&state.scene.world()).is_empty());
        assert!(!state.scene.can_undo());
    }
}
