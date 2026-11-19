//! `pulsar.gameplay`: the gameplay roles as scene defaults, and a default
//! player character the input plugin drives.
//!
//! - Enabled, the plugin registers the four roles; the player character has a
//!   native default.
//! - The default character is a scene object with a character (placing its
//!   controller and input), and a camera.
//! - Input events on the pawn's entity channel move it: `Move` sets the
//!   character's `move_input`, `Jump` starts a jump.

use std::sync::Mutex;

use engine_backend::scene::game_hooks::role_default;
use gameplay_plugin::default_character::{bridge, spawn, CAMERA_CLASS, CHARACTER_CLASS};
use installed_plugins::input_plugin::runtime::events::action_descriptor;
use installed_plugins::input_plugin::runtime::system::Phase;
use pulsar_events::gamma::{Channel, DynEvent, DynValue};
use pulsar_events::{EventCategory, EventHub, FlushPoint};
use pulsar_scenedb::{Entity, World};
use pulsar_traits::builtin::{
    GAME_MODE_ROLE, GAME_STATE_ROLE, PLAYER_CHARACTER_ROLE, PLAYER_CONTROLLER_ROLE,
};
use pulsar_world_registry::pulsar_scene_model::attachments;

/// The tests change the process's activated plugins: one at a time.
static ACTIVATION: Mutex<()> = Mutex::new(());

fn everything_on() {
    pulsar_plugins::activate(&pulsar_plugins::resolve(
        pulsar_plugins::installed(),
        &pulsar_plugins::ProjectPlugins::default(),
    ));
}

fn classes(world: &World, owner: Entity) -> Vec<String> {
    attachments::instances(world, owner)
        .iter()
        .filter_map(|instance| attachments::meta(world, *instance).map(|m| m.class_name.clone()))
        .collect()
}

#[test]
fn the_plugin_registers_the_four_roles_and_a_default_player_character() {
    let _guard = ACTIVATION.lock().unwrap_or_else(|p| p.into_inner());
    everything_on();
    let ids: Vec<_> = pulsar_traits::roles::roles().iter().map(|r| r.id).collect();
    for role in [
        GAME_MODE_ROLE,
        GAME_STATE_ROLE,
        PLAYER_CONTROLLER_ROLE,
        PLAYER_CHARACTER_ROLE,
    ] {
        assert!(ids.contains(&role), "{role} is registered: {ids:?}");
    }
    assert!(role_default(PLAYER_CHARACTER_ROLE).is_some());
    assert!(
        role_default(GAME_MODE_ROLE).is_none(),
        "only the character has a native default"
    );
}

#[test]
fn the_default_character_has_a_body_an_input_binding_and_a_view() {
    let _guard = ACTIVATION.lock().unwrap_or_else(|p| p.into_inner());
    everything_on();
    let mut world = World::new();
    let pawn = spawn(&mut world, std::path::Path::new(".")).unwrap();
    let classes = classes(&world, pawn);
    for expected in [
        CHARACTER_CLASS,
        CAMERA_CLASS,
        "CharacterControllerComponent",
        "PlayerInputComponent",
    ] {
        assert!(
            classes.iter().any(|c| c == expected),
            "{expected}: {classes:?}"
        );
    }
}

fn action_event(hub: &EventHub, name: &str, phase: Phase, pawn: Entity, x: f64, y: f64) {
    let id = hub
        .descriptor_by_name(&format!("input::{name}.{}", phase.word()))
        .unwrap()
        .id;
    hub.publish_dyn(
        Channel::Entity(pawn.bits()),
        DynEvent::new(
            id,
            vec![
                DynValue::U64(pawn.bits()),
                DynValue::F64((x * x + y * y).sqrt()),
                DynValue::F64(x),
                DynValue::F64(y),
                DynValue::F64(0.0),
                DynValue::F64(0.0),
                DynValue::F64(0.0),
            ],
        ),
    )
    .unwrap();
}

fn read(world: &World, pawn: Entity, property: &str) -> serde_json::Value {
    pulsar_world_registry::get_component_property(world, pawn, CHARACTER_CLASS, 0, property)
        .unwrap()
}

#[test]
fn the_players_move_and_jump_actions_drive_the_default_character() {
    let _guard = ACTIVATION.lock().unwrap_or_else(|p| p.into_inner());
    everything_on();
    let mut world = World::new();
    let pawn = spawn(&mut world, std::path::Path::new(".")).unwrap();

    // The project's actions are declared when the session begins.
    let hub = EventHub::new();
    for (name, phase) in [
        ("Move", Phase::Triggered),
        ("Move", Phase::Completed),
        ("Jump", Phase::Started),
    ] {
        hub.register(
            action_descriptor(name, phase).unwrap(),
            EventCategory::Input,
        )
        .unwrap();
    }
    let resolve = |_: Entity, _: &str| None;

    // The first frame subscribes the pawn to what the mapping has.
    bridge(&mut world, &hub, &resolve);
    assert_eq!(
        read(&world, pawn, "move_input"),
        serde_json::json!([0.0, 0.0])
    );

    action_event(&hub, "Move", Phase::Triggered, pawn, 0.0, 1.0);
    hub.flush(FlushPoint::Manual);
    bridge(&mut world, &hub, &resolve);
    assert_eq!(
        read(&world, pawn, "move_input"),
        serde_json::json!([0.0, 1.0]),
        "forward"
    );

    action_event(&hub, "Jump", Phase::Started, pawn, 1.0, 0.0);
    hub.flush(FlushPoint::Manual);
    bridge(&mut world, &hub, &resolve);
    assert_eq!(read(&world, pawn, "jump"), serde_json::json!(true));

    action_event(&hub, "Move", Phase::Completed, pawn, 0.0, 0.0);
    hub.flush(FlushPoint::Manual);
    bridge(&mut world, &hub, &resolve);
    assert_eq!(
        read(&world, pawn, "move_input"),
        serde_json::json!([0.0, 0.0]),
        "released"
    );

    // Another object's input is not this pawn's.
    let other = world.spawn();
    action_event(&hub, "Move", Phase::Triggered, other, 1.0, 0.0);
    hub.flush(FlushPoint::Manual);
    bridge(&mut world, &hub, &resolve);
    assert_eq!(
        read(&world, pawn, "move_input"),
        serde_json::json!([0.0, 0.0])
    );
}
