//! What a level may hold only one of, through the editor's producers: one
//! sky (#1057).

use engine_backend::scene::level_rules;
use helio_component::components::AtmosphereComponent;

use crate::commands::{execute_command, SceneCommand, TypedComponent};
use crate::scene_edit::{components, objects, sky, ObjectType, SceneObjectData, Transform};
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

fn add_object(state: &mut LevelEditorState, name: &str) -> String {
    execute_command(
        state,
        SceneCommand::AddObject {
            data: object(name),
            parent_id: None,
        },
    )
    .affected_ids[0]
        .clone()
}

fn atmosphere_count(state: &LevelEditorState) -> usize {
    level_rules::atmospheres(&state.scene.world()).len()
}

#[test]
fn create_sky_adds_the_levels_one_sky_and_world_settings_finds_it() {
    let mut state = LevelEditorState::new();
    assert_eq!(sky::level_sky(&state.scene.world()), None);

    let created = sky::create_sky(&mut state);
    assert!(created.changed, "{}", created.no_op_reason);
    let found = sky::level_sky(&state.scene.world()).expect("the level has a sky");
    assert_eq!(found.object_id, created.affected_ids[0]);
    assert_eq!(found.object_name, sky::SKY_OBJECT_NAME);
    assert_eq!(found.component_index, 0);
    assert!(found.enabled);

    // The section edits it in place, through the properties panel's command.
    let edit = execute_command(
        &mut state,
        SceneCommand::SetComponentProperty {
            id: found.object_id.clone(),
            class_name: "AtmosphereComponent".into(),
            component_index: 0,
            prop_name: "planet_radius_km".into(),
            value: Box::new(1737.0f32),
        },
    );
    assert!(edit.changed, "{}", edit.no_op_reason);
    let instance = components::instance_at(&state.scene.world(), &found.object_id, 0).unwrap();
    assert_eq!(
        state
            .scene
            .world()
            .get::<AtmosphereComponent>(instance)
            .unwrap()
            .planet_radius_km,
        1737.0
    );

    // Undo removes the edit, then the sky.
    state.scene.undo();
    state.scene.undo();
    assert_eq!(sky::level_sky(&state.scene.world()), None);
}

#[test]
fn a_second_atmosphere_is_refused_with_a_message() {
    let mut state = LevelEditorState::new();
    assert!(sky::create_sky(&mut state).changed);
    let other = add_object(&mut state, "Other");

    // The Create Sky button, again.
    let again = sky::create_sky(&mut state);
    assert!(!again.changed);
    assert_eq!(again.no_op_reason, level_rules::ONE_ATMOSPHERE);

    // Add Component on another object (class default and a typed value).
    for value in [
        None,
        Some(Box::new(AtmosphereComponent::default()) as Box<dyn std::any::Any + Send + Sync>),
    ] {
        let result = execute_command(
            &mut state,
            SceneCommand::AddComponent {
                id: other.clone(),
                class_name: "AtmosphereComponent".into(),
                value,
            },
        );
        assert!(!result.changed);
        assert_eq!(result.no_op_reason, level_rules::ONE_ATMOSPHERE);
    }

    // A new object carrying one.
    let result = execute_command(
        &mut state,
        SceneCommand::AddObjectWithComponents {
            data: object("Second sky"),
            parent_id: None,
            components: vec![TypedComponent::new(AtmosphereComponent::default())],
        },
    );
    assert!(!result.changed);
    assert_eq!(result.no_op_reason, level_rules::ONE_ATMOSPHERE);

    // Duplicating the sky component, or its object.
    let sky_object = sky::level_sky(&state.scene.world()).unwrap().object_id;
    let result = execute_command(
        &mut state,
        SceneCommand::DuplicateComponent {
            id: sky_object.clone(),
            component_index: 0,
        },
    );
    assert!(!result.changed);
    assert_eq!(result.no_op_reason, level_rules::ONE_ATMOSPHERE);
    let copy = objects::duplicate_object(&mut state.scene.world_mut(), &sky_object)
        .expect("the object is copied");
    assert_eq!(components::component_count(&state.scene.world(), &copy), 0);

    // A record (paste, tools) attaches and is detached again.
    let record = crate::scene_edit::ComponentInstance {
        class_name: "AtmosphereComponent".into(),
        enabled: true,
        data: serde_json::to_value(AtmosphereComponent::default()).unwrap(),
    };
    assert_eq!(
        components::add_component_instance(&mut state.scene.world_mut(), &other, record),
        None
    );

    assert_eq!(atmosphere_count(&state), 1);
    assert_eq!(components::component_count(&state.scene.world(), &other), 0);
}

#[test]
fn a_disabled_sky_still_counts_and_removing_it_frees_the_slot() {
    let mut state = LevelEditorState::new();
    assert!(sky::create_sky(&mut state).changed);
    let sky_object = sky::level_sky(&state.scene.world()).unwrap().object_id;
    assert!(
        execute_command(
            &mut state,
            SceneCommand::SetComponentEnabled {
                id: sky_object.clone(),
                component_index: 0,
                enabled: false,
            },
        )
        .changed
    );
    assert!(!sky::level_sky(&state.scene.world()).unwrap().enabled);
    assert!(!sky::create_sky(&mut state).changed);

    assert!(
        execute_command(
            &mut state,
            SceneCommand::RemoveComponent {
                id: sky_object,
                component_index: 0,
            },
        )
        .changed
    );
    assert_eq!(sky::level_sky(&state.scene.world()), None);
    assert!(sky::create_sky(&mut state).changed);
}

#[test]
fn a_level_file_with_two_skies_loads_with_the_first_one_enabled() {
    let mut state = LevelEditorState::new();
    assert!(sky::create_sky(&mut state).changed);
    let second = add_object(&mut state, "Second sky");
    // Only a file can hold two: write the second past the rules.
    {
        let mut world = state.scene.world_mut();
        let owner = engine_backend::scene::SceneWorldExt::entity_for(&*world, &second).unwrap();
        pulsar_world_registry::attach_value(&mut world, owner, AtmosphereComponent::default())
            .unwrap();
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("two_skies.level");
    crate::scene_edit::level_io::save_to_file(&state.scene.world(), &path).unwrap();

    let mut loaded = engine_backend::scene::new_scene();
    crate::scene_edit::level_io::load_from_file(&mut loaded.world, &path).unwrap();
    let skies = level_rules::atmospheres(&loaded.world);
    assert_eq!(skies.len(), 2, "both are kept");
    let enabled: Vec<bool> = skies
        .iter()
        .map(|instance| engine_backend::scene::attachments::is_enabled(&loaded.world, *instance))
        .collect();
    assert_eq!(enabled, [true, false]);
    let found = sky::level_sky(&loaded.world).unwrap();
    assert_eq!(found.object_name, sky::SKY_OBJECT_NAME);
}
