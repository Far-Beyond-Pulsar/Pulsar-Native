//! The component lifecycle registry's proofs (the component lifecycle plan,
//! phase 1: `.agents/COMPONENT_LIFECYCLE_PLAN.md`), over every installed
//! plugin plus a fixture plugin (`tests/fixtures/lifecycle_plugin`) whose
//! class has an `ActorTable`.
//!
//! - A class with a table gets `begin_play` on its first frame, `tick` each
//!   frame and `end_play` on disable, removal and despawn, as a legacy
//!   `#[register_component_runtime]` class does.
//! - The table is gated with its plugin: a disabled plugin's table is not
//!   handed out, its instances are not driven, and the catalog still lists
//!   it under `component.lifecycle`.
//! - The leak audit: no reflected method or script native of any installed
//!   class is named like a lifecycle event, and no class registers a
//!   lifecycle twice.

use std::sync::Mutex;

use engine_backend::scene::{SceneWorldExt, SpawnObject};
use lifecycle_fixture_plugin::{take_log, LifecycleProbe, CLASS_NAME};
use pulsar_events::EventHub;
use pulsar_scenedb::{Entity, World};
use pulsar_world_registry::pulsar_scene_model::attachments::NewInstance;
use pulsar_world_registry::pulsar_scene_model::ComponentOwner;
use pulsar_world_registry::{tick_live_components, ComponentPayload, ComponentRuntimeState};

use installed_plugins as _;

const FIXTURE: &str = "pulsar.test_lifecycle";
const FIXTURE_ROOT: &str = "lifecycle_fixture_plugin";

/// The tests change the process's activated plugins and share the fixture's
/// log: one at a time.
static ACTIVATION: Mutex<()> = Mutex::new(());

fn probe(world: &mut World) -> (Entity, Entity) {
    let owner = world
        .spawn_object(SpawnObject::new("Probe").with_id("probe"))
        .unwrap();
    let instance = pulsar_world_registry::place_component(
        world,
        owner,
        NewInstance::new(CLASS_NAME),
        ComponentPayload::Default,
    )
    .unwrap();
    (owner, instance)
}

fn tick(world: &mut World, events: &EventHub, state: &mut ComponentRuntimeState) {
    tick_live_components(world, events, 0.016, 1, state);
}

#[test]
fn a_class_with_a_table_gets_begin_tick_and_end_in_order() {
    let _guard = ACTIVATION.lock().unwrap_or_else(|p| p.into_inner());
    pulsar_plugins::deactivate();
    take_log();
    let mut world = World::new();
    world.attach_change_tracker(pulsar_scenedb::SharedChangeTracker::new());
    let events = EventHub::new();
    let mut state = ComponentRuntimeState::default();
    let (owner, instance) = probe(&mut world);

    tick(&mut world, &events, &mut state);
    assert_eq!(take_log(), ["begin_play", "tick"], "the first frame");
    tick(&mut world, &events, &mut state);
    assert_eq!(take_log(), ["tick"]);

    // Disabled: ended, not ticked; enabled again: a new life.
    world.get_mut::<ComponentOwner>(instance).unwrap().enabled = 0;
    tick(&mut world, &events, &mut state);
    assert_eq!(take_log(), ["end_play"]);
    world.get_mut::<ComponentOwner>(instance).unwrap().enabled = 1;
    tick(&mut world, &events, &mut state);
    assert_eq!(take_log(), ["begin_play", "tick"]);

    // Removed (the class's own removal, from the journal).
    world.remove::<LifecycleProbe>(instance);
    pulsar_world_registry::process_component_removals(&world, &events, &mut state);
    assert_eq!(take_log(), ["end_play"]);

    // Placed again, then the owner is despawned.
    let instance_again = pulsar_world_registry::place_component(
        &mut world,
        owner,
        NewInstance::new(CLASS_NAME),
        ComponentPayload::Default,
    )
    .unwrap();
    tick(&mut world, &events, &mut state);
    assert_eq!(take_log(), ["begin_play", "tick"]);
    world.despawn(instance_again);
    tick(&mut world, &events, &mut state);
    assert_eq!(take_log(), ["end_play"]);
    assert!(!state.has_active_components());
}

#[test]
fn a_disabled_plugins_table_is_not_returned_or_driven() {
    let _guard = ACTIVATION.lock().unwrap_or_else(|p| p.into_inner());
    pulsar_plugins::deactivate();
    take_log();
    let mut world = World::new();
    world.attach_change_tracker(pulsar_scenedb::SharedChangeTracker::new());
    let events = EventHub::new();
    let mut state = ComponentRuntimeState::default();
    probe(&mut world);

    let ours = |summaries: Vec<pulsar_plugins::Summary>| -> Vec<pulsar_plugins::Summary> {
        summaries
            .into_iter()
            .filter(|summary| {
                summary.origin == FIXTURE_ROOT && summary.category == "component.lifecycle"
            })
            .collect()
    };
    let enabled = ours(pulsar_plugins::active_contributions());
    assert_eq!(enabled.len(), 1, "{enabled:?}");
    assert_eq!(enabled[0].id, CLASS_NAME);

    pulsar_plugins::activate(&pulsar_plugins::resolve(
        pulsar_plugins::installed(),
        &pulsar_plugins::ProjectPlugins::default().with(&[FIXTURE], false),
    ));
    let handed_out = ours(pulsar_plugins::active_contributions());
    let listed = ours(pulsar_plugins::contributions());
    tick(&mut world, &events, &mut state);
    let driven = take_log();
    pulsar_plugins::deactivate();

    assert!(handed_out.is_empty(), "{handed_out:?}");
    assert_eq!(listed.len(), 1, "the catalog still lists it, to diagnose");
    assert!(driven.is_empty(), "a disabled plugin's class ran: {driven:?}");
}

#[test]
fn no_lifecycle_name_reaches_the_blueprint_surface() {
    let _guard = ACTIVATION.lock().unwrap_or_else(|p| p.into_inner());
    pulsar_plugins::deactivate();
    let problems = pulsar_world_registry::find_lifecycle_problems();
    assert!(problems.is_empty(), "{problems:#?}");
    // The sweep sees every installed class, the fixture's among them.
    assert!(pulsar_world_registry::world_component(CLASS_NAME).is_some());
}
