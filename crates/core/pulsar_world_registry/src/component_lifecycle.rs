//! Per-game lifecycle reconciliation for live components.
//!
//! Two registrations feed it, driven together in one `Step::Components`: the
//! legacy macro's [`ComponentTickRegistration`] (a whole-class loop shim) and
//! the capability registry's per-class `ActorTable`
//! ([`pulsar_component_lifecycle`]), which this module drives instance by
//! instance. A class in both is an error ([`lifecycle_conflicts`]); the
//! legacy one runs.
//!
//! This state stores identities, event subscriptions, and transient payload
//! inboxes only. The authoritative component values remain in SceneDB and
//! are borrowed by each generated typed query for callbacks.

use crate::{ComponentTickRegistration, QueuedComponentEvent};
use pulsar_component_lifecycle::{ActorCx, ActorRegistration};
use pulsar_events::EventHub;
use pulsar_scenedb::{
    ChangeCursor, ChangeRead, ComponentChange, ComponentChangeKind, ComponentId, Entity, World,
};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct EventInbox {
    active: HashSet<ComponentInstanceKey>,
    queued: HashMap<ComponentInstanceKey, Vec<QueuedComponentEvent>>,
}

/// Runtime identity of one registered component instance.
///
/// `Entity` includes its generation, so an entity slot recycled after
/// despawn cannot inherit the previous component's lifecycle state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ComponentInstanceKey {
    pub component_type: ComponentId,
    pub entity: Entity,
}

/// Lifecycle bookkeeping for one running game session.
///
/// The maps contain only stable SceneDB identities, never cloned component
/// data. They must live beside the `TickLoop` that drives these components;
/// keeping them global would let independent PIE/game worlds share runtime
/// activation state accidentally.
#[derive(Default)]
pub struct ComponentRuntimeState {
    scene_identity: Option<usize>,
    /// Per class: active instance entity -> its owner object.
    active: HashMap<RegistrationKey, HashMap<Entity, Entity>>,
    inbox: Arc<Mutex<EventInbox>>,
    subscriptions: HashMap<ComponentInstanceKey, Vec<pulsar_events::gamma::SyncSubscription>>,
    /// This session's own change cursor per registered component type,
    /// opened before any of its instances starts. Removals are read from it,
    /// so whoever else reads (or drains) the world's change history cannot
    /// take one from this session.
    removal_cursors: HashMap<ComponentId, ChangeCursor>,
    scratch: Vec<ComponentChange>,
    /// The frame's order of systems and component lifecycles, for the
    /// registered systems it was computed from (their list's address and
    /// length: the registry rebuilds the list when the classes change).
    order: Option<((usize, usize), Vec<crate::systems::Step>)>,
    /// Whether this session has logged [`lifecycle_conflicts`].
    conflicts_reported: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct RegistrationKey {
    component_type: ComponentId,
    type_name: &'static str,
}

impl ComponentRuntimeState {
    /// Subscribe a live component instance to its generated native handler
    /// events. Gamma callbacks only copy wire-safe dynamic fields into this
    /// host-owned inbox; they never access World or component memory.
    /// Subscribe instance `key` to `event_names` addressed to `channel`, its
    /// owner object's event channel.
    pub fn subscribe_instance(
        &mut self,
        key: ComponentInstanceKey,
        channel: Entity,
        hub: &EventHub,
        event_names: &[&'static str],
    ) {
        if self.subscriptions.contains_key(&key) || event_names.is_empty() {
            return;
        }
        self.inbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .active
            .insert(key);
        let mut subscriptions = Vec::new();
        for &name in event_names {
            let Some(descriptor) = hub.descriptor_by_name(name) else {
                tracing::warn!(
                    event = name,
                    "native component handler event is not registered"
                );
                continue;
            };
            let inbox = Arc::clone(&self.inbox);
            let callback_key = key;
            let callback_name = name.to_owned();
            subscriptions.push(hub.bus().subscribe_dyn(
                descriptor.id,
                pulsar_events::gamma::SubscribeOptions::channel(
                    pulsar_events::gamma::Channel::Entity(channel.bits()),
                ),
                move |event| {
                    let mut inbox = inbox
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if inbox.active.contains(&callback_key) {
                        inbox
                            .queued
                            .entry(callback_key)
                            .or_default()
                            .push(QueuedComponentEvent {
                                name: callback_name.clone(),
                                fields: event.fields.clone(),
                            });
                    }
                },
            ));
        }
        self.subscriptions.insert(key, subscriptions);
    }

    /// Remove queued events and Gamma listeners as part of component
    /// disable/removal/despawn teardown.
    pub fn unsubscribe_instance(&mut self, key: ComponentInstanceKey) {
        self.subscriptions.remove(&key);
        let mut inbox = self
            .inbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inbox.active.remove(&key);
        inbox.queued.remove(&key);
    }

    /// Take the inbox for one component while its live row is borrowed.
    pub fn take_events(&mut self, key: ComponentInstanceKey) -> Vec<QueuedComponentEvent> {
        self.inbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .queued
            .remove(&key)
            .unwrap_or_default()
    }

    /// Whether the state currently owns any active component lifecycle.
    pub fn has_active_components(&self) -> bool {
        self.active.values().any(|entities| !entities.is_empty())
    }

    /// End all active component lifecycles and forget the current scene.
    /// This is used for explicit runtime shutdown and `TickLoop` drop.
    pub fn end_all(&mut self, events: &EventHub) {
        for lifecycle in lifecycles() {
            let key = lifecycle.key();
            if let Some(entities) = self.active.remove(&key) {
                let mut entities: Vec<_> = entities.into_iter().collect();
                entities.sort_by_key(|(entity, _)| entity.bits());
                for &(_, owner) in &entities {
                    lifecycle.end_play(owner, events);
                }
                for (entity, _) in entities {
                    self.unsubscribe_instance(ComponentInstanceKey {
                        component_type: key.component_type,
                        entity,
                    });
                }
            }
        }
        self.active.clear();
        self.subscriptions.clear();
        // Cursors of the replaced world; the next tick opens fresh ones.
        self.removal_cursors.clear();
        *self
            .inbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = EventInbox::default();
        self.scene_identity = None;
    }
}

/// One class's lifecycle, from either registration.
#[derive(Clone, Copy)]
enum Lifecycle {
    Legacy(&'static ComponentTickRegistration),
    Actor(&'static ActorRegistration),
}

impl Lifecycle {
    fn key(self) -> RegistrationKey {
        match self {
            Self::Legacy(registration) => registration_key(registration),
            Self::Actor(registration) => RegistrationKey {
                component_type: (registration.table.component_type)(),
                type_name: registration.table.type_name,
            },
        }
    }

    /// The owner-only teardown, if the class has one.
    fn end_play(self, owner: Entity, events: &EventHub) {
        match self {
            Self::Legacy(registration) => {
                if let Some(end_play) = registration.end_play {
                    end_play(owner, events);
                }
            }
            Self::Actor(registration) => {
                if let Some(end_play) = registration.table.end_play {
                    end_play(&mut ActorCx::new(owner, None, events));
                }
            }
        }
    }
}

/// Classes registered both through the legacy macro and with an `ActorTable`
/// (`ClassSpec::actor`): each is an error, one lifecycle per class.
pub fn lifecycle_conflicts() -> Vec<String> {
    conflicts_of(crate::runtime::ticks(), crate::runtime::actors())
}

fn conflicts_of(
    legacy: &[&'static ComponentTickRegistration],
    actors: &[&'static ActorRegistration],
) -> Vec<String> {
    let mut conflicts: Vec<String> = actors
        .iter()
        .filter(|actor| legacy.iter().any(|l| l.class_name == actor.class_name))
        .map(|actor| {
            format!(
                "class `{}` registers a lifecycle twice (a `ComponentTickRegistration` and an \
                 `ActorTable`); the legacy one runs",
                actor.class_name
            )
        })
        .collect();
    conflicts.sort();
    conflicts
}

/// Every class's lifecycle in the frame's deterministic order (type name):
/// the legacy registrations and the tables, minus a table whose class is
/// also legacy ([`lifecycle_conflicts`]).
fn lifecycles() -> Vec<Lifecycle> {
    let legacy = crate::runtime::ticks();
    let mut all: Vec<Lifecycle> = legacy.iter().copied().map(Lifecycle::Legacy).collect();
    all.extend(
        crate::runtime::actors()
            .iter()
            .filter(|actor| !legacy.iter().any(|l| l.class_name == actor.class_name))
            .copied()
            .map(Lifecycle::Actor),
    );
    all.sort_by_key(|lifecycle| {
        let key = lifecycle.key();
        (key.type_name, key.component_type)
    });
    all
}

fn registration_key(registration: &ComponentTickRegistration) -> RegistrationKey {
    RegistrationKey {
        component_type: (registration.component_type)(),
        type_name: registration.type_name,
    }
}

/// Run the frame's gameplay step: the registered classes' systems
/// (`crate::systems`, in their labelled order) around each registered
/// component's live typed query, reconciling lifecycle transitions against
/// the preceding tick.
///
/// Generated `tick` shims receive the previous live entity set and fill a
/// fresh current set while they query the authoritative World. They invoke
/// `begin_play` only for rows absent from the previous set, then invoke the
/// normal component tick. Removed/disabled/despawned rows are ended after
/// the typed query has released its mutable borrow.
///
/// `scene_identity` is the address of the shared `SceneDb` allocation held by
/// the `TickLoop`. Replacing that allocation ends all activation state before
/// a new scene is allowed to start component lifecycles.
pub fn tick_live_components(
    world: &mut World,
    events: &EventHub,
    delta_seconds: f32,
    scene_identity: usize,
    state: &mut ComponentRuntimeState,
) {
    if state
        .scene_identity
        .is_some_and(|identity| identity != scene_identity)
    {
        state.end_all(events);
    }
    state.scene_identity = Some(scene_identity);

    if !state.conflicts_reported {
        state.conflicts_reported = true;
        for problem in lifecycle_conflicts() {
            tracing::error!("{problem}");
        }
    }
    process_component_removals(world, events, state);

    let registrations = lifecycles();
    for lifecycle in &registrations {
        let component_type = lifecycle.key().component_type;
        state
            .removal_cursors
            .entry(component_type)
            .or_insert_with(|| world.open_change_cursor_id(component_type));
    }

    // The registered classes' systems around the component lifecycles, in
    // their labelled order (`crate::systems`).
    let systems = crate::runtime::systems();
    let key = (systems.as_ptr() as usize, systems.len());
    if state
        .order
        .as_ref()
        .is_none_or(|(cached, _)| *cached != key)
    {
        let (steps, problems) = crate::systems::order(systems);
        for problem in problems {
            tracing::error!("{problem}");
        }
        state.order = Some((key, steps));
    }
    let steps = state
        .order
        .as_ref()
        .map(|(_, steps)| steps.clone())
        .unwrap_or_default();
    for step in steps {
        match step {
            crate::systems::Step::Components => {
                tick_component_lifecycles(world, events, delta_seconds, &registrations, state)
            }
            crate::systems::Step::System(registration) => {
                (registration.system.run)(&mut crate::systems::SystemContext {
                    world: &mut *world,
                    events,
                    delta_seconds,
                    runtime: &mut *state,
                });
            }
        }
    }
}

/// Every native component class's `begin_play`/`tick`/`end_play` over its
/// enabled instances: the [`crate::systems::COMPONENTS`] step.
fn tick_component_lifecycles(
    world: &mut World,
    events: &EventHub,
    delta_seconds: f32,
    registrations: &[Lifecycle],
    state: &mut ComponentRuntimeState,
) {
    for &lifecycle in registrations {
        let key = lifecycle.key();
        let previous = state.active.get(&key).cloned().unwrap_or_default();
        let mut current = HashMap::new();
        match lifecycle {
            Lifecycle::Legacy(registration) => {
                (registration.tick)(world, events, delta_seconds, &previous, &mut current, state)
            }
            Lifecycle::Actor(registration) => tick_actor_class(
                world,
                events,
                delta_seconds,
                registration,
                &previous,
                &mut current,
            ),
        }

        let mut ended: Vec<(Entity, Entity)> = previous
            .iter()
            .filter(|(entity, _)| !current.contains_key(*entity))
            .map(|(entity, owner)| (*entity, *owner))
            .collect();
        ended.sort_by_key(|(entity, _)| entity.bits());
        for &(_, owner) in &ended {
            lifecycle.end_play(owner, events);
        }
        for (entity, _) in ended {
            state.unsubscribe_instance(ComponentInstanceKey {
                component_type: key.component_type,
                entity,
            });
        }
        state.active.insert(key, current);
    }
}

/// One `ActorTable` class over its enabled instances, in entity order:
/// `begin_play` for those absent from `previous`, then `tick`.
fn tick_actor_class(
    world: &mut World,
    events: &EventHub,
    delta_seconds: f32,
    registration: &ActorRegistration,
    previous: &HashMap<Entity, Entity>,
    current: &mut HashMap<Entity, Entity>,
) {
    let table = &registration.table;
    let mut instances = Vec::new();
    (table.instances)(world, &mut instances);
    instances.sort_by_key(|instance| instance.bits());
    for instance in instances {
        let Some(link) = world
            .get::<crate::pulsar_scene_model::ComponentOwner>(instance)
            .copied()
        else {
            continue;
        };
        if !link.is_enabled() {
            continue;
        }
        let owner = link.entity();
        current.insert(instance, owner);
        let mut cx = ActorCx::new(owner, Some(instance), events);
        if !previous.contains_key(&instance) {
            if let Some(begin_play) = table.begin_play {
                begin_play(world, &mut cx, instance);
            }
        }
        if let Some(tick) = table.tick {
            tick(world, &mut cx, instance, delta_seconds);
        }
    }
}

/// Read the removals of registered component types since the last call,
/// from this session's own change cursors, and end any active native
/// component instances they name. Call this after mutation phases as well
/// as before component ticks: actors and scripts can remove components
/// after the native component phase has already run. An overflowed or
/// replaced history ends nothing here; the next tick's reconciliation ends
/// whatever is no longer live.
pub fn process_component_removals(
    world: &World,
    events: &EventHub,
    state: &mut ComponentRuntimeState,
) -> usize {
    let mut removals = Vec::new();
    for (&component_type, cursor) in &mut state.removal_cursors {
        state.scratch.clear();
        if world.read_changes(cursor, &mut state.scratch) == ChangeRead::Complete {
            removals.extend(
                state
                    .scratch
                    .iter()
                    .filter(|change| change.kind == ComponentChangeKind::Removed)
                    .map(|change| (change.entity, component_type)),
            );
        }
    }
    if removals.is_empty() {
        return 0;
    }

    let registrations = lifecycles();
    let mut ended = 0;
    for (entity, component_type) in removals {
        let Some(lifecycle) = registrations
            .iter()
            .copied()
            .find(|lifecycle| lifecycle.key().component_type == component_type)
        else {
            continue;
        };
        let key = lifecycle.key();
        let ended_owner = state
            .active
            .get_mut(&key)
            .and_then(|entities| entities.remove(&entity));
        if let Some(owner) = ended_owner {
            lifecycle.end_play(owner, events);
            state.unsubscribe_instance(ComponentInstanceKey {
                component_type,
                entity,
            });
            ended += 1;
        }
    }
    ended
}

/// End the running callbacks for every live registered component while the
/// SceneDB allocation is still available, then clear its activation state.
///
/// This can end subscriptions and other runtime-owned state using the
/// owner identity, but by definition cannot borrow component data that was
/// already removed from SceneDB.
pub fn end_live_components(state: &mut ComponentRuntimeState, events: &EventHub) {
    state.end_all(events);
}

fn all_actor_summaries() -> Vec<pulsar_plugins::Summary> {
    use pulsar_plugins::Contribution as _;
    crate::runtime::all_actors()
        .iter()
        .map(|registration| registration.summary())
        .collect()
}

fn active_actor_summaries() -> Vec<pulsar_plugins::Summary> {
    use pulsar_plugins::Contribution as _;
    crate::runtime::actors()
        .iter()
        .map(|registration| registration.summary())
        .collect()
}

// Tables are gated with their class (across plugin libraries, like the
// classes and systems), so their catalog source reads the registry.
inventory::submit! {
    pulsar_plugins::ContributionSource {
        category: "component.lifecycle",
        all: all_actor_summaries,
        active: active_actor_summaries,
        // Tables come only from a class's spec, through a plugin's build.
        linked: Vec::new,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Clone, Copy)]
    struct Ticked;

    static ENDED: AtomicUsize = AtomicUsize::new(0);

    fn tick(
        world: &mut World,
        _: &EventHub,
        _: f32,
        _: &HashMap<Entity, Entity>,
        current: &mut HashMap<Entity, Entity>,
        _: &mut ComponentRuntimeState,
    ) {
        for (entity, _) in world.query::<&Ticked>() {
            current.insert(entity, entity);
        }
    }

    fn end_play(_: Entity, _: &EventHub) {
        ENDED.fetch_add(1, Ordering::SeqCst);
    }

    inventory::submit! {
        ComponentTickRegistration {
            type_name: "component_lifecycle::tests::Ticked",
            class_name: "Ticked",
            handler_events: &[],
            component_type: pulsar_scenedb::component_id::<Ticked>,
            tick,
            end_play: Some(end_play),
        }
    }

    /// The removal reaches this session even when another reader drained
    /// the world's change history first (the editor renderer closing its
    /// change window on a world shared with Play-in-Editor).
    #[test]
    fn a_removal_drained_by_another_reader_still_ends_the_instance() {
        let mut world = World::new();
        world.attach_change_tracker(pulsar_scenedb::SharedChangeTracker::new());
        let events = EventHub::new();
        let mut state = ComponentRuntimeState::default();
        let e = world.spawn();
        world.insert(e, Ticked);
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

        /// A class registered in both paths is an error, naming the class.
        #[test]
        fn a_class_in_both_paths_is_a_conflict() {
            fn tick(
                _: &mut World,
                _: &EventHub,
                _: f32,
                _: &HashMap<Entity, Entity>,
                _: &mut HashMap<Entity, Entity>,
                _: &mut ComponentRuntimeState,
            ) {
            }
            let legacy: &'static ComponentTickRegistration =
                Box::leak(Box::new(ComponentTickRegistration {
                    type_name: "Dual",
                    class_name: "Dual",
                    handler_events: &[],
                    component_type: pulsar_scenedb::component_id::<Played>,
                    tick,
                    end_play: None,
                }));
            let table: &'static ActorRegistration = Box::leak(Box::new(ActorRegistration {
                origin: "test",
                class_name: "Dual",
                table: ActorTable::of::<Played>(),
            }));
            let other: &'static ActorRegistration = Box::leak(Box::new(ActorRegistration {
                origin: "test",
                class_name: "Only",
                table: ActorTable::of::<Played>(),
            }));
            let conflicts = conflicts_of(&[legacy], &[table, other]);
            assert_eq!(conflicts.len(), 1, "{conflicts:?}");
            assert!(conflicts[0].contains("`Dual`"));
            assert!(conflicts_of(&[], &[table]).is_empty());
        }
    }
}
