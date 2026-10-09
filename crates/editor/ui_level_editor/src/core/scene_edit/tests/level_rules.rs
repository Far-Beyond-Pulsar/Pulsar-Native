//! What a level may hold only one of, through the editor's producers: one
//! sky (#1057) and one directional light.

use engine_backend::scene::level_rules;
use helio_component::components::{AtmosphereComponent, LightComponent, LightType};

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

fn light(light_type: LightType, enabled: bool) -> LightComponent {
    let mut light = LightComponent::default();
    light.general.light_type = light_type;
    light.general.enabled = enabled;
    light
}

/// An object holding `light`, on an instance enabled or not.
fn add_light(
    state: &mut LevelEditorState,
    name: &str,
    light: LightComponent,
    instance_enabled: bool,
) -> crate::commands::CommandResult {
    let mut component = TypedComponent::new(light);
    component.enabled = instance_enabled;
    execute_command(
        state,
        SceneCommand::AddObjectWithComponents {
            data: object(name),
            parent_id: None,
            components: vec![component],
        },
    )
}

fn light_at(state: &LevelEditorState, id: &str) -> LightComponent {
    let world = state.scene.world();
    let instance = components::instance_at(&world, id, 0).unwrap();
    world.get::<LightComponent>(instance).unwrap().clone()
}

fn set_light(
    state: &mut LevelEditorState,
    id: &str,
    prop_name: &str,
    value: Box<dyn std::any::Any + Send>,
) -> crate::commands::CommandResult {
    execute_command(
        state,
        SceneCommand::SetComponentProperty {
            id: id.to_string(),
            class_name: "LightComponent".into(),
            component_index: 0,
            prop_name: prop_name.into(),
            value,
        },
    )
}

fn sun_count(state: &LevelEditorState) -> usize {
    level_rules::directional_lights(&state.scene.world()).len()
}

#[test]
fn a_second_directional_light_is_refused_with_a_message() {
    let mut state = LevelEditorState::new();
    let sun = add_light(&mut state, "Sun", light(LightType::Directional, true), true);
    assert!(sun.changed, "{}", sun.no_op_reason);
    let sun = sun.affected_ids[0].clone();

    // Adding another, on a new object or on an existing one.
    let second = add_light(
        &mut state,
        "Sun 2",
        light(LightType::Directional, true),
        true,
    );
    assert!(!second.changed);
    assert_eq!(second.no_op_reason, level_rules::ONE_DIRECTIONAL_LIGHT);
    let other = add_object(&mut state, "Other");
    let result = execute_command(
        &mut state,
        SceneCommand::AddComponent {
            id: other.clone(),
            class_name: "LightComponent".into(),
            value: Some(Box::new(light(LightType::Directional, true))),
        },
    );
    assert_eq!(result.no_op_reason, level_rules::ONE_DIRECTIONAL_LIGHT);
    // The class default is a point light: allowed.
    let point = execute_command(
        &mut state,
        SceneCommand::AddComponent {
            id: other.clone(),
            class_name: "LightComponent".into(),
            value: None,
        },
    );
    assert!(point.changed, "{}", point.no_op_reason);

    // Turning the point light directional is refused and leaves it a point light.
    let result = set_light(
        &mut state,
        &other,
        "light_type",
        Box::new(LightType::Directional),
    );
    assert!(!result.changed);
    assert_eq!(result.no_op_reason, level_rules::ONE_DIRECTIONAL_LIGHT);
    assert_eq!(
        light_at(&state, &other).general.light_type,
        LightType::Point
    );
    // Its other edits still go through.
    assert!(set_light(&mut state, &other, "intensity", Box::new(5.0f32)).changed);

    // Duplicating the sun.
    let result = execute_command(
        &mut state,
        SceneCommand::DuplicateComponent {
            id: sun.clone(),
            component_index: 0,
        },
    );
    assert_eq!(result.no_op_reason, level_rules::ONE_DIRECTIONAL_LIGHT);
    let copy = objects::duplicate_object(&mut state.scene.world_mut(), &sun).unwrap();
    assert_eq!(components::component_count(&state.scene.world(), &copy), 0);
    assert_eq!(sun_count(&state), 1);

    // With the sun disabled, the point light may become the sun.
    assert!(
        execute_command(
            &mut state,
            SceneCommand::SetComponentEnabled {
                id: sun.clone(),
                component_index: 0,
                enabled: false,
            },
        )
        .changed
    );
    assert!(
        set_light(
            &mut state,
            &other,
            "light_type",
            Box::new(LightType::Directional)
        )
        .changed
    );
    // ... and the old sun cannot be switched back on.
    let result = execute_command(
        &mut state,
        SceneCommand::SetComponentEnabled {
            id: sun.clone(),
            component_index: 0,
            enabled: true,
        },
    );
    assert_eq!(result.no_op_reason, level_rules::ONE_DIRECTIONAL_LIGHT);
    assert!(!components::set_component_enabled(
        &mut state.scene.world_mut(),
        &sun,
        0,
        true
    ));
    assert_eq!(sun_count(&state), 1);
}

#[test]
fn a_disabled_directional_light_is_allowed_but_cannot_be_turned_on_beside_the_sun() {
    let mut state = LevelEditorState::new();
    assert!(add_light(&mut state, "Sun", light(LightType::Directional, true), true).changed);

    // Its light switched off.
    let off = add_light(
        &mut state,
        "Moon",
        light(LightType::Directional, false),
        true,
    );
    assert!(off.changed, "{}", off.no_op_reason);
    let off = off.affected_ids[0].clone();
    let result = set_light(&mut state, &off, "enabled", Box::new(true));
    assert_eq!(result.no_op_reason, level_rules::ONE_DIRECTIONAL_LIGHT);
    assert!(!light_at(&state, &off).general.enabled);

    // Its instance switched off.
    let parked = add_light(
        &mut state,
        "Spare",
        light(LightType::Directional, true),
        false,
    );
    assert!(parked.changed, "{}", parked.no_op_reason);
    let result = execute_command(
        &mut state,
        SceneCommand::SetComponentEnabled {
            id: parked.affected_ids[0].clone(),
            component_index: 0,
            enabled: true,
        },
    );
    assert_eq!(result.no_op_reason, level_rules::ONE_DIRECTIONAL_LIGHT);
    assert_eq!(sun_count(&state), 1);
}

#[test]
fn a_level_file_with_two_directional_lights_loads_with_one_casting() {
    let mut state = LevelEditorState::new();
    let first = add_light(&mut state, "Sun", light(LightType::Directional, true), true)
        .affected_ids[0]
        .clone();
    let second = add_object(&mut state, "Second sun");
    // Only a file can hold two: write the second past the rules.
    {
        let mut world = state.scene.world_mut();
        let owner = engine_backend::scene::SceneWorldExt::entity_for(&*world, &second).unwrap();
        pulsar_world_registry::attach_value(&mut world, owner, light(LightType::Directional, true))
            .unwrap();
    }
    assert_eq!(sun_count(&state), 2);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("two_suns.level");
    crate::scene_edit::level_io::save_to_file(&state.scene.world(), &path).unwrap();

    let mut loaded = engine_backend::scene::new_scene();
    crate::scene_edit::level_io::load_from_file(&mut loaded.world, &path).unwrap();
    let world = &loaded.world;
    let suns = level_rules::directional_lights(world);
    assert_eq!(suns.len(), 1);
    let owner = engine_backend::scene::attachments::owner_of(world, suns[0]).unwrap();
    assert_eq!(
        engine_backend::scene::SceneWorldExt::stable_id_of(world, owner),
        Some(first.as_str())
    );
    // The other is kept, still directional, its instance disabled.
    let other = components::instance_at(world, &second, 0).unwrap();
    assert!(!engine_backend::scene::attachments::is_enabled(
        world, other
    ));
    let kept = world.get::<LightComponent>(other).unwrap();
    assert_eq!(kept.general.light_type, LightType::Directional);
    assert!(kept.general.enabled);
}
