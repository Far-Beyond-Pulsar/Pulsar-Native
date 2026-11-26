//! The component class API's proof:
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

/// The character's capsule falls onto a static collider and stands on it, and
/// never goes below it: no clamp on Y anywhere, the physics plugin's
/// character path rests on the box.
#[test]
fn a_character_capsule_rests_on_a_static_collider() {
    let _guard = ACTIVATION.lock().unwrap_or_else(|p| p.into_inner());
    pulsar_plugins::deactivate();
    let hub = pulsar_events::EventHub::new();
    let mut world = World::new();

    // A 100 x 1 x 100 m static box whose top face is y = 0.
    let ground = world
        .spawn_object(SpawnObject::new("Ground").with_id("ground"))
        .unwrap();
    world.get_mut::<Transform>(ground).unwrap().position = [0.0, -0.5, 0.0];
    pulsar_world_registry::place_component(
        &mut world,
        ground,
        NewInstance::new("ColliderComponent"),
        ComponentPayload::Default,
    )
    .unwrap();
    pulsar_world_registry::set_component_property(
        &mut world,
        ground,
        "ColliderComponent",
        0,
        "half_extents",
        serde_json::json!([50.0, 0.5, 50.0]),
    )
    .unwrap();

    let (owner, _instance) = character(&mut world);
    world.get_mut::<Transform>(owner).unwrap().position = [0.0, 3.0, 0.0];

    let mut state = ComponentRuntimeState::default();
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut lowest = f64::MAX;
    let mut last = 3.0;
    let mut still = 0;
    while Instant::now() < deadline && still < 120 {
        pulsar_world_registry::tick_live_components(&mut world, &hub, 1.0 / 60.0, 1, &mut state);
        hub.flush(pulsar_events::FlushPoint::AfterSystems);
        let y = f64::from(world.get::<Transform>(owner).unwrap().position[1]);
        lowest = lowest.min(y);
        still = if (y - last).abs() < 1e-4 {
            still + 1
        } else {
            0
        };
        last = y;
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(
        last < 2.9,
        "the character fell from y = 3 (ended at {last})"
    );
    assert!(
        lowest >= -0.01,
        "the capsule's origin never went below the floor's top face (lowest {lowest})"
    );
    assert!(last >= 0.0, "it stands on the floor at y = {last}");
}

/// Run the movement system `ticks` times.
fn tick(
    world: &mut World,
    hub: &pulsar_events::EventHub,
    state: &mut ComponentRuntimeState,
    ticks: u32,
) {
    for _ in 0..ticks {
        pulsar_world_registry::tick_live_components(world, hub, 1.0 / 60.0, 1, state);
        hub.flush(pulsar_events::FlushPoint::AfterSystems);
    }
}

fn desired(world: &World, owner: Entity) -> Vec<f64> {
    pulsar_world_registry::get_component_property(
        world,
        owner,
        "CharacterControllerComponent",
        0,
        "desired_velocity",
    )
    .unwrap()
    .as_array()
    .unwrap()
    .iter()
    .map(|v| v.as_f64().unwrap())
    .collect()
}

fn yaw(world: &World, owner: Entity) -> f32 {
    world.get::<Transform>(owner).unwrap().rotation[1]
}

fn near(got: &[f64], want: [f64; 3]) {
    for i in 0..3 {
        assert!((got[i] - want[i]).abs() < 1e-4, "{got:?} vs {want:?}");
    }
}

#[test]
fn movement_is_relative_to_the_control_yaw_and_not_the_pitch() {
    let _guard = ACTIVATION.lock().unwrap_or_else(|p| p.into_inner());
    pulsar_plugins::deactivate();
    let hub = pulsar_events::EventHub::new();
    let mut world = World::new();
    let (owner, instance) = character(&mut world);
    let mut state = ComponentRuntimeState::default();
    let set = |world: &mut World, yaw: f32, pitch: f32, input: [f32; 2]| {
        let mut c = world.get_mut::<CharacterComponent>(instance).unwrap();
        c.control_yaw = yaw;
        c.control_pitch = pitch;
        c.move_input = input;
    };

    // Forward is where the control yaw looks, at every pitch.
    for pitch in [-80.0, 0.0, 45.0, 80.0] {
        set(&mut world, 0.0, pitch, [0.0, 1.0]);
        tick(&mut world, &hub, &mut state, 1);
        near(&desired(&world, owner), [0.0, 0.0, -4.0]);
        set(&mut world, 90.0, pitch, [0.0, 1.0]);
        tick(&mut world, &hub, &mut state, 1);
        near(&desired(&world, owner), [-4.0, 0.0, 0.0]);
        set(&mut world, 180.0, pitch, [0.0, 1.0]);
        tick(&mut world, &hub, &mut state, 1);
        near(&desired(&world, owner), [0.0, 0.0, 4.0]);
    }
    // Strafing is beside it.
    set(&mut world, 90.0, 30.0, [1.0, 0.0]);
    tick(&mut world, &hub, &mut state, 1);
    near(&desired(&world, owner), [0.0, 0.0, -4.0]);
    set(&mut world, 0.0, 0.0, [-1.0, 0.0]);
    tick(&mut world, &hub, &mut state, 1);
    near(&desired(&world, owner), [-4.0, 0.0, 0.0]);
    // A world direction asked of the character is not turned by the view, and
    // is used once.
    set(&mut world, 90.0, 0.0, [0.0, 0.0]);
    world
        .get_mut::<CharacterComponent>(instance)
        .unwrap()
        .world_input = [0.0, 0.0, 1.0];
    tick(&mut world, &hub, &mut state, 1);
    near(&desired(&world, owner), [0.0, 0.0, 4.0]);
    tick(&mut world, &hub, &mut state, 1);
    near(&desired(&world, owner), [0.0, 0.0, 0.0]);
}

#[test]
fn the_body_turns_toward_its_movement_at_the_rotation_rate() {
    let _guard = ACTIVATION.lock().unwrap_or_else(|p| p.into_inner());
    pulsar_plugins::deactivate();
    let hub = pulsar_events::EventHub::new();
    let mut world = World::new();
    let (owner, instance) = character(&mut world);
    let mut state = ComponentRuntimeState::default();
    {
        let mut c = world.get_mut::<CharacterComponent>(instance).unwrap();
        c.orient_rotation_to_movement = true;
        c.rotation_rate = 360.0; // 6 degrees a tick at 60 Hz
        c.move_input = [1.0, 0.0]; // walking right of a camera that stays at yaw 0
    }
    tick(&mut world, &hub, &mut state, 1);
    let first = yaw(&world, owner);
    assert!(
        (first + 6.0).abs() < 1e-3,
        "one tick turns one step toward -90 (right): {first}"
    );
    tick(&mut world, &hub, &mut state, 4);
    assert!(
        (yaw(&world, owner) + 30.0).abs() < 1e-3,
        "smooth, not a snap"
    );
    tick(&mut world, &hub, &mut state, 30);
    assert!(
        (yaw(&world, owner) + 90.0).abs() < 1e-3,
        "faces the walk: {}",
        yaw(&world, owner)
    );
    // The camera's control yaw did not move with the body.
    assert_eq!(
        world
            .get::<CharacterComponent>(instance)
            .unwrap()
            .control_yaw,
        0.0
    );
    // Standing still keeps the facing.
    world
        .get_mut::<CharacterComponent>(instance)
        .unwrap()
        .move_input = [0.0, 0.0];
    tick(&mut world, &hub, &mut state, 10);
    assert!((yaw(&world, owner) + 90.0).abs() < 1e-3);
    // A rate of zero never turns.
    {
        let mut c = world.get_mut::<CharacterComponent>(instance).unwrap();
        c.rotation_rate = 0.0;
        c.move_input = [0.0, 1.0];
    }
    tick(&mut world, &hub, &mut state, 5);
    assert!((yaw(&world, owner) + 90.0).abs() < 1e-3);
}

#[test]
fn use_controller_rotation_yaw_makes_the_body_follow_the_look_direction() {
    let _guard = ACTIVATION.lock().unwrap_or_else(|p| p.into_inner());
    pulsar_plugins::deactivate();
    let hub = pulsar_events::EventHub::new();
    let mut world = World::new();
    let (owner, instance) = character(&mut world);
    let mut state = ComponentRuntimeState::default();
    {
        let mut c = world.get_mut::<CharacterComponent>(instance).unwrap();
        c.orient_rotation_to_movement = false;
        c.control_yaw = 40.0;
        c.control_pitch = 25.0;
    }
    tick(&mut world, &hub, &mut state, 1);
    assert_eq!(
        yaw(&world, owner),
        0.0,
        "with the option off the body keeps its own rotation"
    );
    world
        .get_mut::<CharacterComponent>(instance)
        .unwrap()
        .use_controller_rotation_yaw = true;
    tick(&mut world, &hub, &mut state, 1);
    assert_eq!(
        yaw(&world, owner),
        40.0,
        "the body faces where the camera looks"
    );
    assert_eq!(
        world.get::<Transform>(owner).unwrap().rotation[0],
        0.0,
        "pitch stays off"
    );
    // Walking forward now goes where it faces and where the camera looks.
    world
        .get_mut::<CharacterComponent>(instance)
        .unwrap()
        .move_input = [0.0, 1.0];
    tick(&mut world, &hub, &mut state, 1);
    let want = character_plugin::movement_direction(40.0, [0.0, 1.0]).map(|v| f64::from(v) * 4.0);
    near(&desired(&world, owner), want);
    // The pitch axis follows when asked too.
    world
        .get_mut::<CharacterComponent>(instance)
        .unwrap()
        .use_controller_rotation_pitch = true;
    tick(&mut world, &hub, &mut state, 1);
    assert_eq!(world.get::<Transform>(owner).unwrap().rotation[0], 25.0);
}

/// Walk for `seconds` of wall time on a floor, ticking at 60 Hz, turning the view by `turn`
/// degrees every tick; the length of the path walked along the ground (a turning walk goes round
/// in a circle, so the distance from the start would say nothing).
fn walked_while_turning(turn: f32, seconds: f32, flood: u32) -> f32 {
    // `seconds` of simulated time at 1/60 s a tick, paced at about that rate.
    let hub = pulsar_events::EventHub::new();
    let mut world = World::new();
    let ground = world
        .spawn_object(SpawnObject::new("Ground").with_id("ground"))
        .unwrap();
    world.get_mut::<Transform>(ground).unwrap().position = [0.0, -0.5, 0.0];
    pulsar_world_registry::place_component(
        &mut world,
        ground,
        NewInstance::new("ColliderComponent"),
        ComponentPayload::Default,
    )
    .unwrap();
    pulsar_world_registry::set_component_property(
        &mut world,
        ground,
        "ColliderComponent",
        0,
        "half_extents",
        serde_json::json!([50.0, 0.5, 50.0]),
    )
    .unwrap();
    let (owner, instance) = character(&mut world);
    world.get_mut::<Transform>(owner).unwrap().position = [0.0, 1.5, 0.0];
    {
        let mut c = world.get_mut::<CharacterComponent>(instance).unwrap();
        c.move_input = [0.0, 1.0];
        c.orient_rotation_to_movement = true;
        c.rotation_rate = 720.0;
    }
    let mut state = ComponentRuntimeState::default();
    // Fall onto the floor and settle first (walking is held back until then).
    world
        .get_mut::<CharacterComponent>(instance)
        .unwrap()
        .move_input = [0.0, 0.0];
    let settle = Instant::now();
    while settle.elapsed().as_secs_f32() < 2.0 {
        pulsar_world_registry::tick_live_components(&mut world, &hub, 1.0 / 60.0, 1, &mut state);
        hub.flush(pulsar_events::FlushPoint::AfterSystems);
        std::thread::sleep(Duration::from_micros(16_667));
    }
    eprintln!(
        "settled at y = {:.3}",
        world.get::<Transform>(owner).unwrap().position[1]
    );
    world
        .get_mut::<CharacterComponent>(instance)
        .unwrap()
        .move_input = [0.0, 1.0];
    let start = world.get::<Transform>(owner).unwrap().position;
    let begun = Instant::now();
    let mut next = begun;
    let mut path = 0.0_f64;
    let mut last = start;
    let ticks = (seconds * 60.0) as u32;
    for _ in 0..ticks {
        // Input arriving faster than the game ticks: raw mouse and key events by the hundred.
        for i in 0..flood {
            hub.publish(
                pulsar_events::gamma::Channel::Global,
                pulsar_events::builtin::MouseMoved {
                    x: 0.0,
                    y: 0.0,
                    dx: 1.0,
                    dy: -1.0,
                },
            );
            if i % 10 == 0 {
                hub.publish(
                    pulsar_events::gamma::Channel::Global,
                    pulsar_events::builtin::KeyDown { key: 26 },
                );
            }
        }
        {
            let mut c = world.get_mut::<CharacterComponent>(instance).unwrap();
            c.control_yaw = character_plugin::wrap_degrees(c.control_yaw + turn);
        }
        pulsar_world_registry::tick_live_components(&mut world, &hub, 1.0 / 60.0, 1, &mut state);
        hub.flush(pulsar_events::FlushPoint::AfterSystems);
        next += Duration::from_micros(16_667);
        let at = world.get::<Transform>(owner).unwrap().position;
        path += ((at[0] - last[0]).powi(2) + (at[2] - last[2]).powi(2)).sqrt();
        last = at;
        if let Some(wait) = next.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
    }
    path as f32
}

/// Turning the view does not stall the simulation: a character that walks while its view (and so
/// its body) turns every tick covers about the distance of one that walks straight. A rotation
/// written by gameplay used to count as "moved by someone else", the physics results for the
/// object were dropped as stale and the capsule hardly advanced.
#[test]
fn walking_while_the_view_turns_is_not_stalled() {
    let _guard = ACTIVATION.lock().unwrap_or_else(|p| p.into_inner());
    pulsar_plugins::deactivate();
    let straight = walked_while_turning(0.0, 3.0, 0);
    let turning = walked_while_turning(2.0, 3.0, 0);
    let flooded = walked_while_turning(2.0, 3.0, 300);
    // 4 m/s for 3 s is 12 m.
    eprintln!(
        "walked straight {straight:.2} m, turning the view {turning:.2} m, turning under 300 mouse events a tick {flooded:.2} m, in 3 s"
    );
    assert!(
        (flooded - straight).abs() < straight * 0.2,
        "a flood of input events stalls the simulation: {flooded} m against {straight} m"
    );
    assert!(straight > 8.0, "the control walks ({straight} m)");
    assert!(
        (turning - straight).abs() < straight * 0.2,
        "turning stalls walking: {turning} m against {straight} m"
    );
}
