//! The component class API's proof (Pulsar-Native#1608 phase 5, #1534):
//! `pulsar.character`, a facade over classes of three other plugins.
//!
//! - Placing a character places its skeletal mesh, animator, character
//!   controller and (with the input plugin) player input, each its own
//!   instance nested under it; the loader accepts each requirement because
//!   the character plugin requires the providers' `class` contracts.
//! - Playing: its movement system, ordered after input and before physics,
//!   drives the controller, and the object walks.
//! - Without the input plugin (an optional contract) a character places
//!   without input, with no problem.
//! - Without the character plugin, a level holding a character loads it as
//!   an unresolved component with its data, and saves it back unchanged;
//!   the members stay their plugins' live components.
//! - A requirement of a class whose plugin is not required is refused.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use character_plugin::{CharacterComponent, CHARACTER_CLASS_NAME, MEMBERS};
use engine_backend::scene::{SceneWorldExt, SpawnObject, Transform};
use pulsar_scenedb::{Entity, World};
use pulsar_world_registry::pulsar_scene_model::attachments::{
    self, NewInstance, UnresolvedComponent,
};
use pulsar_world_registry::{ComponentPayload, ComponentRuntimeState, Presentation, RequiredClass};

use installed_plugins as _;

/// The tests change the process's activated plugins: one at a time.
static ACTIVATION: Mutex<()> = Mutex::new(());

fn enabled_but(off: &[&str]) {
    pulsar_plugins::activate(&pulsar_plugins::resolve(
        pulsar_plugins::installed(),
        &pulsar_plugins::ProjectPlugins::default().with(off, false),
    ));
}

fn character(world: &mut World) -> (Entity, Entity) {
    let owner = world
        .spawn_object(SpawnObject::new("Hero").with_id("hero"))
        .unwrap();
    let instance = pulsar_world_registry::place_component(
        world,
        owner,
        NewInstance::new(CHARACTER_CLASS_NAME),
        ComponentPayload::Default,
    )
    .unwrap();
    (owner, instance)
}

fn classes(world: &World, owner: Entity) -> Vec<String> {
    attachments::instances(world, owner)
        .iter()
        .filter_map(|instance| attachments::meta(world, *instance).map(|m| m.class_name.clone()))
        .collect()
}

#[test]
fn placing_a_character_places_what_it_is_built_from() {
    let _guard = ACTIVATION.lock().unwrap_or_else(|p| p.into_inner());
    pulsar_plugins::deactivate();
    let registration = pulsar_world_registry::world_component(CHARACTER_CLASS_NAME).unwrap();
    assert_eq!(registration.presentation, Presentation::Facade);
    assert_eq!(registration.requires, MEMBERS);
    assert!(
        pulsar_world_registry::class_problems().is_empty(),
        "{:?}",
        pulsar_world_registry::class_problems()
    );

    let mut world = World::new();
    let (owner, instance) = character(&mut world);
    let character_id = attachments::meta(&world, instance).unwrap().id;
    assert_eq!(
        classes(&world, owner),
        [
            CHARACTER_CLASS_NAME,
            "SkeletalMeshComponent",
            "AnimationComponent",
            "CharacterControllerComponent",
            "PlayerInputComponent",
        ]
    );
    for member in &attachments::instances(&world, owner)[1..] {
        assert_eq!(
            attachments::meta(&world, *member).unwrap().parent,
            Some(character_id),
            "a member is nested under its character"
        );
    }
    // Placing another adds no second set of members.
    pulsar_world_registry::place_component(
        &mut world,
        owner,
        NewInstance::new(CHARACTER_CLASS_NAME),
        ComponentPayload::Default,
    )
    .unwrap();
    assert_eq!(classes(&world, owner).len(), 6);
}

#[test]
fn without_the_input_plugin_a_character_places_without_input() {
    let _guard = ACTIVATION.lock().unwrap_or_else(|p| p.into_inner());
    enabled_but(&["pulsar.input"]);
    let mut world = World::new();
    let (owner, _) = character(&mut world);
    let placed = classes(&world, owner);
    let problems = pulsar_world_registry::class_problems();
    pulsar_plugins::deactivate();
    assert_eq!(
        placed,
        [
            CHARACTER_CLASS_NAME,
            "SkeletalMeshComponent",
            "AnimationComponent",
            "CharacterControllerComponent",
        ]
    );
    assert!(problems.is_empty(), "an optional member: {problems:?}");
}

#[test]
fn a_required_class_of_a_plugin_not_required_is_refused() {
    let _guard = ACTIVATION.lock().unwrap_or_else(|p| p.into_inner());
    pulsar_plugins::deactivate();
    let character = pulsar_world_registry::world_component(CHARACTER_CLASS_NAME).unwrap();
    // The lights provide no class contract the character requires.
    match pulsar_world_registry::required_class(character, "LightComponent") {
        RequiredClass::Refused(reason) => assert!(reason.contains("LightComponent"), "{reason}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        pulsar_world_registry::required_class(character, "CharacterControllerComponent"),
        RequiredClass::Attach
    );
}

#[test]
fn a_character_walks_when_played() {
    let _guard = ACTIVATION.lock().unwrap_or_else(|p| p.into_inner());
    pulsar_plugins::deactivate();
    let hub = pulsar_events::EventHub::new();
    let mut world = World::new();
    let (owner, instance) = character(&mut world);
    world
        .get_mut::<CharacterComponent>(instance)
        .unwrap()
        .move_input = [1.0, 0.0];
    let start = world.get::<Transform>(owner).unwrap().position;

    let mut state = ComponentRuntimeState::default();
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut moved = 0.0;
    while Instant::now() < deadline {
        pulsar_world_registry::tick_live_components(&mut world, &hub, 1.0 / 60.0, 1, &mut state);
        hub.flush(pulsar_events::FlushPoint::AfterSystems);
        moved = world.get::<Transform>(owner).unwrap().position[0] - start[0];
        if moved > 0.5 {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let desired = pulsar_world_registry::get_component_property(
        &world,
        owner,
        "CharacterControllerComponent",
        0,
        "desired_velocity",
    )
    .unwrap();
    assert_eq!(
        desired,
        serde_json::json!([4.0, 0.0, 0.0]),
        "walk speed along x"
    );
    assert!(moved > 0.5, "the character walked {moved} m along x");
}

#[test]
fn without_the_character_plugin_its_data_is_kept() {
    let _guard = ACTIVATION.lock().unwrap_or_else(|p| p.into_inner());
    pulsar_plugins::deactivate();
    let mut world = World::new();
    let (owner, instance) = character(&mut world);
    world
        .get_mut::<CharacterComponent>(instance)
        .unwrap()
        .walk_speed = 7.5;
    let saved = pulsar_world_registry::component_records(&world, owner);

    enabled_but(&["pulsar.character"]);
    let mut reloaded = World::new();
    let object = reloaded
        .spawn_object(SpawnObject::new("Hero").with_id("hero"))
        .unwrap();
    pulsar_world_registry::attach_records(&mut reloaded, object, &saved).unwrap();
    let unresolved: Vec<serde_json::Value> = attachments::instances(&reloaded, object)
        .iter()
        .filter_map(|instance| reloaded.get::<UnresolvedComponent>(*instance))
        .map(|unresolved| unresolved.data.clone())
        .collect();
    let live_members = attachments::instances(&reloaded, object)
        .iter()
        .filter(|instance| reloaded.get::<UnresolvedComponent>(**instance).is_none())
        .count();
    let resaved = pulsar_world_registry::component_records(&reloaded, object);
    pulsar_plugins::deactivate();

    assert_eq!(unresolved.len(), 1, "only the character is unresolved");
    assert_eq!(unresolved[0]["walk_speed"], serde_json::json!(7.5));
    assert_eq!(live_members, 4, "the members stay live");
    let records = |records: &[pulsar_world_registry::pulsar_scene_model::ComponentInstance]| {
        records
            .iter()
            .map(|r| (r.class_name.clone(), r.data.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(records(&resaved), records(&saved), "saved back unchanged");
}
