#[cfg(test)]
mod undo_redo_tests {
    use super::super::*;
    use crate::level_editor::scene_edit::{ObjectType, SceneObjectData, Transform};
    use crate::level_editor::state::LevelEditorState;

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
        assert_eq!(crate::level_editor::scene_edit::objects::get_all_objects(&state.scene.world()).len(), 1);

        assert!(state.scene.undo());
        assert!(crate::level_editor::scene_edit::objects::get_all_objects(&state.scene.world()).is_empty());
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
        assert!(crate::level_editor::scene_edit::objects::get_all_objects(&state.scene.world()).is_empty());

        assert!(state.scene.redo());

        assert_eq!(crate::level_editor::scene_edit::objects::get_all_objects(&state.scene.world()).len(), 1);
        assert!(state.scene.can_undo());
        assert!(!state.scene.can_redo());
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
        assert!(crate::level_editor::scene_edit::objects::get_all_objects(&state.scene.world()).is_empty());
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
                    object_type: crate::level_editor::scene_edit::ObjectType::Light(
                        crate::level_editor::scene_edit::LightType::Point,
                    ),
                    transform: crate::level_editor::scene_edit::Transform::default(),
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
            crate::level_editor::scene_edit::components::add_component(
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
            crate::level_editor::scene_edit::components::read_live_component_property(
                &world,
                &id,
                "LightComponent",
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
            crate::level_editor::scene_edit::components::read_live_component_property(
                &world,
                &id,
                "LightComponent",
                "intensity",
            )
        }
        .expect("intensity must still be live-readable after undo");
        assert_eq!(reverted.downcast_ref::<f32>(), Some(&1000.0)); // IntensityLightProps::default()
    }
}