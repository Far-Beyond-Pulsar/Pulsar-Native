//! Lifecycle tests: end on removal, begin/tick/end order, profiler scopes, construct and the session scope.

use super::*;
use pulsar_component_lifecycle::{
    ActorCx, ActorRegistration, ConstructInfo, ConstructReason, SessionKind,
};
use pulsar_events::EventHub;
use pulsar_scenedb::{Entity, World};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

struct Ticked;

static ENDED: AtomicUsize = AtomicUsize::new(0);

impl pulsar_component_lifecycle::Actor for Ticked {
    fn end_play(_: &mut ActorCx<'_>) {
        ENDED.fetch_add(1, Ordering::SeqCst);
    }
}

/// The removal reaches this session even when another reader drained
/// the world's change history first (the editor renderer closing its
/// change window on a world shared with Play-in-Editor).
#[test]
fn a_removal_drained_by_another_reader_still_ends_the_instance() {
    crate::runtime::register_actor_for_test(Box::leak(Box::new(ActorRegistration {
        origin: "test",
        class_name: "Ticked",
        table: pulsar_component_lifecycle::ActorTable::of::<Ticked>(),
    })));
    let mut world = World::new();
    world.attach_change_tracker(pulsar_scenedb::SharedChangeTracker::new());
    let events = EventHub::new();
    let mut state = ComponentRuntimeState::default();
    let owner = world.spawn();
    let e = world.spawn();
    world.insert(e, Ticked);
    world.insert(e, pulsar_scene_model::ComponentOwner::new(owner, true));
    tick_live_components(&mut world, &events, 0.0, 1, &mut state);
    assert!(state.has_active_components());

    let before = ENDED.load(Ordering::SeqCst);
    world.remove::<Ticked>(e);
    let tracker = world.change_tracker().unwrap();
    drop(tracker.drain_component_removals());
    assert_eq!(process_component_removals(&world, &events, &mut state), 1);
    assert_eq!(ENDED.load(Ordering::SeqCst) - before, 1);
}

mod actor {
    use super::*;
    use pulsar_component_lifecycle::{Actor, ActorRegistration, ActorTable};
    use pulsar_scene_model::ComponentOwner;

    static LOG: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

    struct Played;

    impl Actor for Played {
        fn begin_play(&mut self, cx: &mut ActorCx<'_>) {
            assert!(cx.instance.is_some());
            LOG.lock().unwrap().push("begin_play");
        }
        fn tick(&mut self, _: &mut ActorCx<'_>, _: f32) {
            LOG.lock().unwrap().push("tick");
        }
        fn end_play(cx: &mut ActorCx<'_>) {
            assert!(cx.instance.is_none());
            LOG.lock().unwrap().push("end_play");
        }
    }

    fn take() -> Vec<&'static str> {
        std::mem::take(&mut *LOG.lock().unwrap())
    }

    /// The legacy test's order for a table class: `begin_play` on the
    /// first frame, `tick` each frame, `end_play` on disable, removal and
    /// despawn.
    #[test]
    fn a_table_class_gets_begin_tick_and_end_in_order() {
        crate::runtime::register_actor_for_test(Box::leak(Box::new(ActorRegistration {
            origin: "test",
            class_name: "Played",
            table: ActorTable::of::<Played>(),
        })));
        let mut world = World::new();
        world.attach_change_tracker(pulsar_scenedb::SharedChangeTracker::new());
        let events = EventHub::new();
        let mut state = ComponentRuntimeState::default();
        let owner = world.spawn();
        let instance = world.spawn();
        world.insert(instance, Played);
        world.insert(instance, ComponentOwner::new(owner, true));

        tick_live_components(&mut world, &events, 0.0, 1, &mut state);
        assert_eq!(take(), ["begin_play", "tick"]);
        tick_live_components(&mut world, &events, 0.0, 1, &mut state);
        assert_eq!(take(), ["tick"]);

        // Disabled: ended, not ticked.
        world.get_mut::<ComponentOwner>(instance).unwrap().enabled = 0;
        tick_live_components(&mut world, &events, 0.0, 1, &mut state);
        assert_eq!(take(), ["end_play"]);
        tick_live_components(&mut world, &events, 0.0, 1, &mut state);
        assert!(take().is_empty());

        // Enabled again: a new life.
        world.get_mut::<ComponentOwner>(instance).unwrap().enabled = 1;
        tick_live_components(&mut world, &events, 0.0, 1, &mut state);
        assert_eq!(take(), ["begin_play", "tick"]);

        // Removed: ended from the journal.
        world.remove::<Played>(instance);
        assert_eq!(process_component_removals(&world, &events, &mut state), 1);
        assert_eq!(take(), ["end_play"]);
        tick_live_components(&mut world, &events, 0.0, 1, &mut state);
        assert!(take().is_empty());

        // Despawned: ended.
        world.insert(instance, Played);
        tick_live_components(&mut world, &events, 0.0, 1, &mut state);
        assert_eq!(take(), ["begin_play", "tick"]);
        world.despawn(instance);
        tick_live_components(&mut world, &events, 0.0, 1, &mut state);
        assert_eq!(take(), ["end_play"]);
        assert!(!state.has_active_components());
    }
}

/// Every piece of lifecycle execution is a profiler scope with a stable,
/// per-class name (never per entity), and nothing is recorded while the
/// profiler is off.
mod profiler_scopes {
    use super::*;
    use pulsar_component_lifecycle::{Actor, ActorRegistration, ActorTable};
    use pulsar_scene_model::ComponentOwner;

    struct Scoped;

    impl Actor for Scoped {
        fn begin_play(&mut self, _: &mut ActorCx<'_>) {}
        fn tick(&mut self, _: &mut ActorCx<'_>, _: f32) {}
        fn end_play(_: &mut ActorCx<'_>) {}
    }

    #[test]
    fn a_lifecycle_run_records_its_scopes_and_an_idle_profiler_records_none() {
        crate::runtime::register_actor_for_test(Box::leak(Box::new(ActorRegistration {
            origin: "test",
            class_name: "ScopedProbe",
            table: ActorTable::of::<Scoped>(),
        })));
        let mut world = World::new();
        world.attach_change_tracker(pulsar_scenedb::SharedChangeTracker::new());
        let events = EventHub::new();
        let mut state = ComponentRuntimeState::default();
        for _ in 0..3 {
            let owner = world.spawn();
            let instance = world.spawn();
            world.insert(instance, Scoped);
            world.insert(instance, ComponentOwner::new(owner, true));
        }

        // Off: a tick records nothing of its own.
        profiling::disable_profiling();
        profiling::clear_events();
        tick_live_components(&mut world, &events, 0.016, 1, &mut state);
        let idle: Vec<_> = profiling::collect_events()
            .into_iter()
            .filter(|event| event.name.starts_with("Lifecycle::"))
            .collect();
        assert!(idle.is_empty(), "recorded while off: {idle:?}");

        profiling::enable_profiling();
        tick_live_components(&mut world, &events, 0.016, 1, &mut state);
        crate::flush_component_events(&mut world, &events);
        process_component_removals_mut(&mut world, &events, &mut state);
        end_session(&mut world, &events, &mut state);
        profiling::disable_profiling();

        let recorded = profiling::collect_events();
        let count = |name: &str| recorded.iter().filter(|e| e.name == name).count();
        for name in [
            "Lifecycle::tick_live_components",
            "Lifecycle::Step::pulsar.components",
            "Lifecycle::tick_component_lifecycles",
            "Lifecycle::pump_construct",
            "Lifecycle::process_component_removals",
            "Lifecycle::flush_component_events",
            "Session::end",
            "Session::drop_resources",
        ] {
            assert!(
                count(name) >= 1,
                "no `{name}` scope in {:?}",
                names(&recorded)
            );
        }
        // The first tick ran off: the second tick ticked its three
        // instances, and ending the session ended them (one scope each,
        // named by class, not by entity).
        assert_eq!(count("Lifecycle::tick::ScopedProbe"), 3);
        assert_eq!(count("Lifecycle::end_play::ScopedProbe"), 3);
        // `begin_play` happened during the off tick; nothing is recorded for it.
        assert_eq!(count("Lifecycle::begin_play::ScopedProbe"), 0);
        // Low cardinality: no scope name carries an entity.
        assert!(
            recorded
                .iter()
                .filter(|e| e.name.starts_with("Lifecycle::") && e.name.contains("Scoped"))
                .all(|e| e.name.ends_with("::ScopedProbe")),
            "{:?}",
            names(&recorded)
        );
    }

    fn names(events: &[profiling::ProfileEvent]) -> Vec<&str> {
        let mut names: Vec<&str> = events.iter().map(|e| e.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        names
    }
}

/// `construct`, the session scope and the session end.
mod session {
    use super::*;
    use pulsar_component_lifecycle::{Actor, ActorRegistration, ActorTable, WorldActor};
    use pulsar_scene_model::ComponentOwner;
    use std::sync::atomic::AtomicBool;

    /// One probe class per test (`Probe<N>`): the registry is global to
    /// the test binary, so each test owns a distinct component type.
    struct Probe<const N: usize> {
        value: u32,
    }

    static SELF_WRITE: [AtomicBool; 16] = [const { AtomicBool::new(false) }; 16];
    static LOG: Mutex<Vec<(usize, String)>> = Mutex::new(Vec::new());
    static INFO: Mutex<Vec<(usize, ConstructInfo)>> = Mutex::new(Vec::new());

    fn note(n: usize, what: impl Into<String>) {
        LOG.lock().unwrap().push((n, what.into()));
    }

    fn take(n: usize) -> Vec<String> {
        let mut log = LOG.lock().unwrap();
        let mine = log.iter().filter(|(m, _)| *m == n).map(|(_, w)| w.clone());
        let mine = mine.collect();
        log.retain(|(m, _)| *m != n);
        mine
    }

    fn count(n: usize, what: &str) -> usize {
        LOG.lock()
            .unwrap()
            .iter()
            .filter(|(m, w)| *m == n && w == what)
            .count()
    }

    impl<const N: usize> Actor for Probe<N> {
        fn construct(&mut self, cx: &mut ActorCx<'_>) {
            note(N, "construct");
            INFO.lock()
                .unwrap()
                .push((N, cx.construct.clone().unwrap()));
            if SELF_WRITE[N].load(Ordering::SeqCst) {
                self.value += 1;
            }
        }
        fn begin_play(&mut self, cx: &mut ActorCx<'_>) {
            note(N, format!("begin_play:{:?}", cx.kind()));
        }
        fn tick(&mut self, _: &mut ActorCx<'_>, _: f32) {
            note(N, "tick");
            self.value += 1;
        }
        fn end_play(cx: &mut ActorCx<'_>) {
            note(N, format!("end_play:{:?}", cx.kind()));
        }
    }

    struct Fixture {
        world: World,
        events: EventHub,
        state: Session,
    }

    impl Fixture {
        fn new<const N: usize>(kind: SessionKind) -> Self {
            crate::runtime::register_actor_for_test(Box::leak(Box::new(ActorRegistration {
                origin: "test",
                class_name: Box::leak(format!("Probe{N}").into_boxed_str()),
                table: ActorTable::of::<Probe<N>>(),
            })));
            let mut world = World::new();
            world.attach_change_tracker(pulsar_scenedb::SharedChangeTracker::new());
            Self {
                world,
                events: EventHub::new(),
                state: Session::new(kind),
            }
        }

        fn spawn<const N: usize>(&mut self) -> Entity {
            let owner = self.world.spawn();
            let instance = self.world.spawn();
            self.world.insert(instance, Probe::<N> { value: 0 });
            self.world
                .insert(instance, ComponentOwner::new(owner, true));
            instance
        }

        fn pump(&mut self) -> usize {
            pump_construct(&mut self.world, &self.events, &mut self.state)
        }

        fn frame(&mut self) {
            tick_live_components(&mut self.world, &self.events, 0.016, 1, &mut self.state);
        }
    }

    fn write<const N: usize>(world: &mut World, instance: Entity, value: u32) {
        world.get_mut::<Probe<N>>(instance).unwrap().value = value;
    }

    /// Exit criterion 2: a hundred writes in one frame are one construct.
    #[test]
    fn a_hundred_writes_in_one_frame_are_one_construct() {
        let mut f = Fixture::new::<0>(SessionKind::Editor);
        let instance = f.spawn::<0>();
        assert_eq!(f.pump(), 1, "the first pump constructs what exists");
        assert_eq!(take(0), ["construct"]);
        for value in 0..100 {
            write::<0>(&mut f.world, instance, value);
        }
        assert_eq!(f.pump(), 1);
        assert_eq!(take(0), ["construct"]);
        assert_eq!(f.pump(), 0, "nothing changed since");
        let reason = INFO
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(n, _)| *n == 0)
            .cloned();
        assert!(matches!(
            reason.unwrap().1.reason,
            ConstructReason::Changed | ConstructReason::Rescan
        ));
    }

    /// Exit criterion 3: a write inside `construct` does not re-enter it.
    #[test]
    fn a_write_inside_construct_does_not_re_enter_it() {
        SELF_WRITE[1].store(true, Ordering::SeqCst);
        let mut f = Fixture::new::<1>(SessionKind::Editor);
        let instance = f.spawn::<1>();
        assert_eq!(f.pump(), 1);
        assert_eq!(f.world.get::<Probe<1>>(instance).unwrap().value, 1);
        assert_eq!(f.pump(), 0, "its own write is not a change");
        assert_eq!(f.pump(), 0);
        // Someone else writes: one construct, whose own write is again
        // read past.
        write::<1>(&mut f.world, instance, 10);
        assert_eq!(f.pump(), 1);
        assert_eq!(f.world.get::<Probe<1>>(instance).unwrap().value, 11);
        assert_eq!(f.pump(), 0);
        assert_eq!(take(1).len(), 2);
    }

    /// Exit criterion 4: a restore is one rescan, each instance once, not
    /// N constructs.
    #[test]
    fn a_restore_is_one_rescan_not_n_constructs() {
        let mut f = Fixture::new::<2>(SessionKind::Editor);
        let instances: Vec<Entity> = (0..4).map(|_| f.spawn::<2>()).collect();
        f.pump();
        take(2);
        let before = f.state.construct_stats();

        // The restore touched every instance many times over.
        for round in 0..5 {
            for &instance in &instances {
                write::<2>(&mut f.world, instance, round);
            }
        }
        f.state.request_rescan();
        assert_eq!(f.pump(), 4, "each live instance once");
        assert_eq!(take(2).len(), 4);
        let after = f.state.construct_stats();
        assert_eq!(after.rescans - before.rescans, 1, "one rescan");
        assert_eq!(after.delivered - before.delivered, 4);
        assert!(INFO
            .lock()
            .unwrap()
            .iter()
            .filter(|(n, _)| *n == 2)
            .rev()
            .take(4)
            .all(|(_, info)| info.reason == ConstructReason::Rescan));
        assert_eq!(f.pump(), 0);
    }

    /// Without a rescan request, a restore's mass remove/insert still
    /// constructs each instance once (the pending set is per instance).
    #[test]
    fn mass_changes_are_coalesced_per_instance() {
        let mut f = Fixture::new::<3>(SessionKind::Editor);
        let instances: Vec<Entity> = (0..3).map(|_| f.spawn::<3>()).collect();
        f.pump();
        take(3);
        for round in 0..10 {
            for &instance in &instances {
                write::<3>(&mut f.world, instance, round);
            }
        }
        assert_eq!(f.pump(), 3);
    }

    /// An editing session constructs and never plays; a play session
    /// constructs first, then begins, then ticks.
    #[test]
    fn construct_comes_before_begin_play_and_the_editor_never_plays() {
        let mut editor = Fixture::new::<4>(SessionKind::Editor);
        editor.spawn::<4>();
        editor.frame();
        editor.frame();
        assert_eq!(take(4), ["construct"], "no begin_play, tick or end_play");

        let mut play = Fixture::new::<5>(SessionKind::Standalone);
        play.spawn::<5>();
        play.frame();
        assert_eq!(take(5), ["construct", "begin_play:Standalone", "tick"]);
        play.frame();
        // `tick` wrote to its own component: not a construct.
        assert_eq!(take(5), ["tick"]);
    }

    /// A write by someone else during play is delivered once at the next
    /// pump point, before that frame's tick.
    #[test]
    fn a_write_during_play_constructs_at_the_next_frame() {
        let mut f = Fixture::new::<6>(SessionKind::PlayInEditor);
        let instance = f.spawn::<6>();
        f.frame();
        take(6);
        for value in 0..50 {
            write::<6>(&mut f.world, instance, value);
        }
        f.frame();
        assert_eq!(take(6), ["construct", "tick"]);
    }

    /// Exit criterion 5 (the lifecycle half): the kind a callback sees
    /// is the session's.
    #[test]
    fn a_callback_sees_the_sessions_kind() {
        for (n, kind) in [
            (7, SessionKind::PlayInEditor),
            (8, SessionKind::Standalone),
            (9, SessionKind::Headless),
        ] {
            let mut f = match n {
                7 => Fixture::new::<7>(kind),
                8 => Fixture::new::<8>(kind),
                _ => Fixture::new::<9>(kind),
            };
            match n {
                7 => drop(f.spawn::<7>()),
                8 => drop(f.spawn::<8>()),
                _ => drop(f.spawn::<9>()),
            };
            assert_eq!(f.state.kind(), kind);
            f.frame();
            assert!(
                take(n).contains(&format!("begin_play:{kind:?}")),
                "{kind:?}"
            );
        }
        assert_eq!(Session::default().kind(), SessionKind::Headless);
    }

    /// Exit criterion 6 (the pump half): suspended while a play session
    /// owns the world, resumed with one rescan after.
    #[test]
    fn a_suspended_pump_delivers_nothing_and_resumes_with_a_rescan() {
        let mut f = Fixture::new::<10>(SessionKind::Editor);
        let instance = f.spawn::<10>();
        f.pump();
        take(10);

        f.state.set_construct_suspended(true);
        assert!(f.state.construct_suspended());
        write::<10>(&mut f.world, instance, 5);
        assert_eq!(f.pump(), 0);
        assert!(take(10).is_empty());

        let rescans = f.state.construct_stats().rescans;
        f.state.set_construct_suspended(false);
        assert_eq!(f.pump(), 1);
        assert_eq!(f.state.construct_stats().rescans - rescans, 1);
    }

    /// A writer that names its properties gets them delivered.
    #[test]
    fn named_properties_reach_construct() {
        let mut f = Fixture::new::<11>(SessionKind::Editor);
        let instance = f.spawn::<11>();
        f.pump();
        f.state.note_changed(instance, "value");
        f.state.note_changed(instance, "angle");
        write::<11>(&mut f.world, instance, 3);
        assert_eq!(f.pump(), 1);
        let info = INFO
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(n, _)| *n == 11)
            .cloned()
            .unwrap()
            .1;
        assert_eq!(info.changed, ["angle", "value"]);
    }

    // ── the session end ────────────────────────────────────────────────

    /// A world-form class: it sees the world at its end.
    struct Resident<const N: usize>;
    struct Guard(usize);
    impl Drop for Guard {
        fn drop(&mut self) {
            GUARD_DROPPED[self.0 - 100].fetch_add(1, Ordering::SeqCst);
        }
    }
    static GUARD_DROPPED: [AtomicUsize; 4] = [const { AtomicUsize::new(0) }; 4];

    impl<const N: usize> WorldActor for Resident<N> {
        fn begin_play(_: &mut World, cx: &mut ActorCx<'_>, _: Entity) {
            cx.session().insert_resource(Guard(100 + N));
            note(100 + N, "begin_play");
        }
        fn end_play(world: &mut World, cx: &mut ActorCx<'_>) {
            let instance = cx.instance.expect("the world form is told the instance");
            let present = world.get::<Resident<N>>(instance).is_some();
            // The resource is still there while ends run.
            let resource = cx.session().resource::<Guard>().is_some();
            note(
                100 + N,
                format!("end_play:present={present}:resource={resource}"),
            );
        }
    }

    fn resident_fixture<const N: usize>() -> (Fixture, Entity) {
        crate::runtime::register_actor_for_test(Box::leak(Box::new(ActorRegistration {
            origin: "test",
            class_name: Box::leak(format!("Resident{N}").into_boxed_str()),
            table: ActorTable::of_world::<Resident<N>>(),
        })));
        let mut world = World::new();
        world.attach_change_tracker(pulsar_scenedb::SharedChangeTracker::new());
        let mut f = Fixture {
            world,
            events: EventHub::new(),
            state: Session::new(SessionKind::Standalone),
        };
        let owner = f.world.spawn();
        let instance = f.world.spawn();
        f.world.insert(instance, Resident::<N>);
        f.world.insert(instance, ComponentOwner::new(owner, true));
        (f, instance)
    }

    /// The end guarantee: every live instance ends while the world is
    /// alive and the component is still there, then the resources go.
    #[test]
    fn ending_the_session_ends_every_instance_with_the_world_then_drops_resources() {
        let (mut f, _) = resident_fixture::<0>();
        let mut other = Fixture::new::<12>(SessionKind::Standalone);
        other.spawn::<12>();
        f.frame();
        other.frame();
        assert_eq!(take(100), ["begin_play"]);
        take(12);
        assert!(f.state.scope().resource::<Guard>().is_some());

        let dropped = GUARD_DROPPED[0].load(Ordering::SeqCst);
        end_session(&mut f.world, &f.events, &mut f.state);
        assert_eq!(take(100), ["end_play:present=true:resource=true"]);
        assert_eq!(GUARD_DROPPED[0].load(Ordering::SeqCst), dropped + 1);
        assert!(!f.state.has_active_components());

        // The world-less form ends too, with its own kind of session.
        end_session(&mut other.world, &other.events, &mut other.state);
        assert_eq!(count(12, "end_play:Standalone"), 1);
    }

    /// A removal read through `&World` cannot run a world-form end; it is
    /// queued and runs at the next mutable-world call, and the session
    /// end runs whatever is left.
    #[test]
    fn a_removal_read_without_a_world_runs_its_world_end_later() {
        let (mut f, instance) = resident_fixture::<1>();
        f.frame();
        take(101);
        f.world.remove::<Resident<1>>(instance);
        assert_eq!(
            process_component_removals(&f.world, &f.events, &mut f.state),
            1
        );
        assert!(take(101).is_empty(), "deferred");
        f.frame();
        assert_eq!(take(101), ["end_play:present=false:resource=true"]);
    }

    #[test]
    fn a_deferred_end_still_runs_at_the_session_end() {
        let (mut f, instance) = resident_fixture::<2>();
        f.frame();
        take(102);
        f.world.remove::<Resident<2>>(instance);
        process_component_removals(&f.world, &f.events, &mut f.state);
        end_session(&mut f.world, &f.events, &mut f.state);
        assert_eq!(take(102), ["end_play:present=false:resource=true"]);
    }
}
