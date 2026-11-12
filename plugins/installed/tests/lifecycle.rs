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

// ── Phase 2: construct, the editor pump, the session scope ─────────────────

mod session {
    use super::*;
    use engine_backend::scene::{EditorConstructPump, SharedScene};
    use lifecycle_fixture_plugin::{set_construct_writes, take_constructs, take_kinds};
    use pulsar_world_registry::{ConstructReason, Session, SessionKind};
    use std::sync::Arc;

    fn scene() -> SharedScene {
        Arc::new(parking_lot::RwLock::new(engine_backend::scene::new_scene()))
    }

    fn reset() {
        pulsar_plugins::deactivate();
        set_construct_writes(false);
        take_log();
        take_constructs();
        take_kinds();
    }

    fn write(scene: &SharedScene, instance: Entity, value: i64) {
        scene
            .write()
            .world
            .get_mut::<LifecycleProbe>(instance)
            .unwrap()
            .value = value;
    }

    /// Exit criterion 2: a hundred writes in one frame are one `construct`;
    /// criterion 3: a write inside `construct` does not re-enter it.
    #[test]
    fn the_editor_pump_coalesces_and_does_not_re_enter() {
        let _guard = ACTIVATION.lock().unwrap_or_else(|p| p.into_inner());
        reset();
        let scene = scene();
        let (_, instance) = probe(&mut scene.write().world);
        let pump = EditorConstructPump::new();
        assert_eq!(pump.kind(), SessionKind::Editor);

        assert_eq!(pump.pump(&scene), 1, "the first pump constructs what exists");
        assert_eq!(take_constructs()[0].reason, ConstructReason::Rescan);
        assert_eq!(pump.pump(&scene), 0, "idle frames deliver nothing");

        for value in 0..100 {
            write(&scene, instance, value);
        }
        assert_eq!(pump.pump(&scene), 1, "100 writes, one construct");
        assert_eq!(take_constructs().len(), 1);
        assert_eq!(pump.pump(&scene), 0);

        // `construct` writes to its own component: not a change.
        set_construct_writes(true);
        write(&scene, instance, 7);
        assert_eq!(pump.pump(&scene), 1);
        assert_eq!(pump.pump(&scene), 0, "its own write did not re-enter it");
        assert_eq!(pump.pump(&scene), 0);
        set_construct_writes(false);
        assert_eq!(take_constructs().len(), 1);
        // The editor session never plays.
        assert!(take_log().is_empty());
        assert!(take_kinds()
            .iter()
            .all(|(_, kind)| *kind == SessionKind::Editor));
    }

    /// A disabled plugin's class gets no `construct` either.
    #[test]
    fn a_disabled_plugin_constructs_nothing() {
        let _guard = ACTIVATION.lock().unwrap_or_else(|p| p.into_inner());
        reset();
        let scene = scene();
        probe(&mut scene.write().world);
        pulsar_plugins::activate(&pulsar_plugins::resolve(
            pulsar_plugins::installed(),
            &pulsar_plugins::ProjectPlugins::default().with(&[FIXTURE], false),
        ));
        let delivered = EditorConstructPump::new().pump(&scene);
        pulsar_plugins::deactivate();
        assert_eq!(delivered, 0);
    }

    /// Exit criterion 5: `kind()` per host path, through the real registry.
    #[test]
    fn a_callback_sees_the_kind_of_its_session() {
        let _guard = ACTIVATION.lock().unwrap_or_else(|p| p.into_inner());
        for kind in [
            SessionKind::PlayInEditor,
            SessionKind::Standalone,
            SessionKind::Headless,
        ] {
            reset();
            let mut world = World::new();
            world.attach_change_tracker(pulsar_scenedb::SharedChangeTracker::new());
            let events = EventHub::new();
            let mut state = Session::new(kind);
            probe(&mut world);
            tick(&mut world, &events, &mut state);
            pulsar_world_registry::end_session(&mut world, &events, &mut state);
            assert_eq!(
                take_kinds(),
                [("construct", kind), ("begin_play", kind), ("end_play", kind)],
                "{kind:?}"
            );
        }
    }

    /// Exit criteria 1, 4 and 6 through the editor's own state: the play
    /// session owns the world (the editor pump is suspended), its `end_play`s
    /// run before `exit_play_mode`, and after the restore the pump resumes
    /// with one rescan, not one construct per touched component.
    #[test]
    fn the_play_session_owns_the_world_and_ends_before_the_restore() {
        let _guard = ACTIVATION.lock().unwrap_or_else(|p| p.into_inner());
        reset();
        let mut state = ui_level_editor::LevelEditorState::new();
        let instances: Vec<Entity> = (0..3)
            .map(|index| {
                let mut world = state.scene.world_mut();
                let owner = world
                    .spawn_object(SpawnObject::new("Probe").with_id(&format!("probe{index}")))
                    .unwrap();
                pulsar_world_registry::place_component(
                    &mut world,
                    owner,
                    NewInstance::new(CLASS_NAME),
                    ComponentPayload::Default,
                )
                .unwrap()
            })
            .collect();
        let scene = state.scene.shared_scene();
        let pump = state.scene.construct_pump.clone();
        assert_eq!(pump.pump(&scene), 3);
        take_constructs();
        take_kinds();

        // Play: the editor's pump steps aside, the game's session takes over.
        state.scene.enter_play_mode();
        assert!(pump.is_suspended(), "a play session owns the world");
        let events = EventHub::new();
        let mut game = Session::new(SessionKind::PlayInEditor);
        tick_live_components(&mut scene.write().world, &events, 0.016, 1, &mut game);
        let begun = take_log().iter().filter(|e| **e == "begin_play").count();
        assert_eq!(begun, 3);
        let constructed_by_the_game = take_kinds()
            .iter()
            .filter(|(event, kind)| *event == "construct" && *kind == SessionKind::PlayInEditor)
            .count();
        assert_eq!(constructed_by_the_game, 3, "the game's session constructs");
        take_constructs();

        // A slider drag during Play: the editor pump delivers nothing; the
        // game's session constructs it once at its next frame.
        for value in 0..50 {
            write(&scene, instances[0], value);
        }
        assert_eq!(pump.pump(&scene), 0, "suspended");
        tick_live_components(&mut scene.write().world, &events, 0.016, 1, &mut game);
        assert_eq!(take_constructs().len(), 1);

        // Stop, in the one order the guarantee needs: the game ends its
        // session (every live instance gets `end_play`) while the world is
        // alive; only then does the editor restore its snapshot.
        take_log();
        take_kinds();
        pulsar_world_registry::end_session(&mut scene.write().world, &events, &mut game);
        assert_eq!(
            take_log().iter().filter(|e| **e == "end_play").count(),
            3,
            "every begin has its end before exit_play_mode"
        );

        let rescans = pump.stats().rescans;
        state.scene.exit_play_mode();
        assert!(!pump.is_suspended(), "the editor owns the world again");
        take_constructs();
        let delivered = pump.pump(&scene);
        assert_eq!(delivered, 3, "each restored instance constructs once");
        assert_eq!(pump.stats().rescans - rescans, 1, "one rescan, not N");
        assert!(take_constructs()
            .iter()
            .all(|info| info.reason == ConstructReason::Rescan));
        assert_eq!(pump.pump(&scene), 0);
    }
}
