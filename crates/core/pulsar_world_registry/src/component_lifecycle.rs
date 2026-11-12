//! Per-game lifecycle reconciliation for live components.
//!
//! One registration feeds it: the capability registry's per-class
//! `ActorTable` ([`pulsar_component_lifecycle`]), which this module drives
//! instance by instance in `Step::Components`. A table comes from a class's
//! `ClassSpec`, from an engine crate (`EngineActor`) or from
//! `#[register_component_runtime]`, whose expansion is a table with native
//! handler events.
//!
//! This state stores identities, event subscriptions, and transient payload
//! inboxes only. The authoritative component values remain in SceneDB and
//! are borrowed by each generated typed query for callbacks.

use crate::QueuedComponentEvent;
use pulsar_component_lifecycle::{
    ActorCx, ActorRegistration, ConstructInfo, ConstructReason, SessionKind, SessionScope,
};
use pulsar_events::EventHub;
use pulsar_scenedb::{
    ChangeCursor, ChangeRead, ComponentChange, ComponentChangeKind, ComponentId, Entity, World,
};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
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

/// What the construct pump did since the session began.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConstructStats {
    /// `construct` calls delivered.
    pub delivered: u64,
    /// Full scans: the first pump, a requested rescan (a restore) and an
    /// overflowed change journal each count once, however many classes and
    /// instances they covered.
    pub rescans: u64,
}

#[derive(Clone, Copy, Debug)]
struct PendingConstruct {
    entity: Entity,
    reason: ConstructReason,
}

/// The construct pump's bookkeeping (D4).
#[derive(Default)]
struct ConstructState {
    /// This session's own cursor per class with a `construct`, over the
    /// writes of its component: separate from the removal cursors, so the
    /// two readers never take entries from each other.
    cursors: HashMap<ComponentId, ChangeCursor>,
    /// Instances to construct at the next pump, deduplicated: a hundred
    /// writes to one component are one entry. Keyed by entity bits (a
    /// deterministic order).
    pending: HashMap<ComponentId, BTreeMap<u64, PendingConstruct>>,
    /// A restore or an undo replaced the world's content: scan everything
    /// once at the next pump instead of reading N changes.
    rescan: bool,
    /// The pump is not delivering (a play session owns the world).
    suspended: bool,
    /// Properties a writer named for an instance, delivered with its
    /// construct.
    named: HashMap<Entity, BTreeSet<String>>,
    stats: ConstructStats,
}

/// A session: one running (or edited) world and everything the lifecycle
/// keeps for it.
///
/// It is the evolution of `ComponentRuntimeState` (still its name, as a type
/// alias): the same identity bookkeeping plus a [`SessionScope`] (kind, the
/// level's settings, resources a plugin registers by type) and the construct
/// pump's state. It is owned by whoever drives the world: the `TickLoop` for
/// play, the editor's pump for [`SessionKind::Editor`].
///
/// The maps contain only stable SceneDB identities, never cloned component
/// data. They must live beside the `TickLoop` that drives these components;
/// keeping them global would let independent PIE/game worlds share runtime
/// activation state accidentally.
pub struct Session {
    scope: SessionScope,
    construct: ConstructState,
    /// Ends of world-form classes read from the removal journal through a
    /// `&World` (`process_component_removals`), to run at the next point
    /// that has a mutable world.
    deferred_ends: Vec<(Lifecycle, Entity, Entity)>,
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
}

/// The session's old name, kept so every existing user compiles.
pub type ComponentRuntimeState = Session;

impl Default for Session {
    /// A session with no host: [`SessionKind::Headless`].
    fn default() -> Self {
        Self::new(SessionKind::Headless)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct RegistrationKey {
    component_type: ComponentId,
    type_name: &'static str,
}

impl Session {
    pub fn new(kind: SessionKind) -> Self {
        profiling::profile_scope!("Session::begin");
        Self {
            scope: SessionScope::new(kind),
            construct: ConstructState::default(),
            deferred_ends: Vec::new(),
            scene_identity: None,
            active: HashMap::new(),
            inbox: Arc::default(),
            subscriptions: HashMap::new(),
            removal_cursors: HashMap::new(),
            scratch: Vec::new(),
            order: None,
        }
    }

    /// What kind of session this is.
    pub fn kind(&self) -> SessionKind {
        self.scope.kind()
    }

    pub fn set_kind(&mut self, kind: SessionKind) {
        self.scope.set_kind(kind);
    }

    /// The data a callback reaches through `cx.session`: settings and
    /// resources.
    pub fn scope(&self) -> &SessionScope {
        &self.scope
    }

    pub fn scope_mut(&mut self) -> &mut SessionScope {
        &mut self.scope
    }

    /// The level's settings (`world_settings`), filled by the host.
    pub fn set_settings(&mut self, settings: serde_json::Value) {
        self.scope.set_settings(settings);
    }

    /// What the construct pump did so far.
    pub fn construct_stats(&self) -> ConstructStats {
        self.construct.stats
    }

    /// The world's content was replaced wholesale (a snapshot restore, an
    /// undo of many objects, a level load): the next pump scans every live
    /// instance once and constructs each once, instead of reading N changes.
    pub fn request_rescan(&mut self) {
        self.construct.rescan = true;
    }

    /// Stop or resume delivering `construct` (the world-ownership rule: the
    /// editor's pump is suspended while a play session owns the world).
    /// Resuming requests a rescan, since the world changed under it.
    pub fn set_construct_suspended(&mut self, suspended: bool) {
        if self.construct.suspended && !suspended {
            self.construct.rescan = true;
        }
        self.construct.suspended = suspended;
    }

    /// Whether a rescan is waiting for the next pump (a restore, or a
    /// resume after a play session).
    pub fn rescan_pending(&self) -> bool {
        self.construct.rescan
    }

    pub fn construct_suspended(&self) -> bool {
        self.construct.suspended
    }

    /// A writer that knows which property it changed (an inspector edit)
    /// says so; the instance's next `construct` carries the names in
    /// `cx.construct.changed`. Writing without this is fine: the names are
    /// then empty (unknown).
    pub fn note_changed(&mut self, instance: Entity, property: impl Into<String>) {
        self.construct
            .named
            .entry(instance)
            .or_default()
            .insert(property.into());
    }

    /// Subscribe instance `key` to its table's native handler events
    /// (`event_names`), addressed to `channel`, its owner object's event
    /// channel. Gamma callbacks only copy wire-safe dynamic fields into this
    /// host-owned inbox; they never access World or component memory.
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

    /// End all active component lifecycles **without a world** and forget
    /// the current scene: the entity-only ends run
    /// ([`pulsar_component_lifecycle::Actor::end_play`]); a world-form class's
    /// `end_play` cannot, and is skipped. This is the path for a replaced
    /// scene (the old world is no longer there to hand out) and the fallback
    /// of `TickLoop`'s drop; a session ends through [`Self::end_session`].
    pub fn end_all(&mut self, events: &EventHub) {
        profiling::profile_scope!("Session::end_all");
        self.end_instances(None, events);
        self.reset();
    }

    /// End the session while its world is alive: `end_play` for every live
    /// instance of every class, in the frame's order (class type name, then
    /// entity), the world-form ones with `&mut World`; then drop the
    /// session's resources and bookkeeping. The caller ends what comes after
    /// (scripts, game hooks) next, and returns control to its host only then.
    /// Nothing is live afterwards; the session may be reused.
    pub fn end_session(&mut self, world: &mut World, events: &EventHub) {
        profiling::profile_scope!("Session::end");
        self.run_deferred_ends(world, events);
        self.end_instances(Some(world), events);
        self.reset();
    }

    /// Run the world-form ends a `&World` path queued.
    fn run_deferred_ends(&mut self, world: &mut World, events: &EventHub) {
        profiling::profile_scope!("Lifecycle::deferred_ends");
        for (lifecycle, instance, owner) in std::mem::take(&mut self.deferred_ends) {
            lifecycle.end_play(Some(&mut *world), instance, owner, events, &mut self.scope);
        }
    }

    fn end_instances(&mut self, mut world: Option<&mut World>, events: &EventHub) {
        for lifecycle in lifecycles() {
            let key = lifecycle.key();
            if let Some(entities) = self.active.remove(&key) {
                let mut entities: Vec<_> = entities.into_iter().collect();
                entities.sort_by_key(|(entity, _)| entity.bits());
                for &(instance, owner) in &entities {
                    lifecycle.end_play(
                        world.as_deref_mut(),
                        instance,
                        owner,
                        events,
                        &mut self.scope,
                    );
                }
                for (entity, _) in entities {
                    self.unsubscribe_instance(ComponentInstanceKey {
                        component_type: key.component_type,
                        entity,
                    });
                }
            }
        }
    }

    /// Forget the scene and everything of its life.
    fn reset(&mut self) {
        self.active.clear();
        self.subscriptions.clear();
        self.deferred_ends.clear();
        // Cursors of the replaced world; the next tick opens fresh ones.
        self.removal_cursors.clear();
        self.construct.cursors.clear();
        self.construct.pending.clear();
        self.construct.named.clear();
        *self
            .inbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = EventInbox::default();
        // Resources belong to the world they served; their `Drop` is the
        // plugin's shutdown (a service thread joins there).
        {
            profiling::profile_scope!("Session::drop_resources");
            self.scope.clear_resources();
        }
        self.scene_identity = None;
    }
}

/// One class's lifecycle: its registered table.
#[derive(Clone, Copy)]
struct Lifecycle(&'static ActorRegistration);

impl Lifecycle {
    fn key(self) -> RegistrationKey {
        RegistrationKey {
            component_type: (self.0.table.component_type)(),
            type_name: self.0.table.type_name,
        }
    }

    /// The teardown, if the class has one. A world-form `end_play` needs
    /// `world`; without one it does not run and this returns `false` (the
    /// caller may queue it for a point that has one).
    fn end_play(
        self,
        world: Option<&mut World>,
        instance: Entity,
        owner: Entity,
        events: &EventHub,
        scope: &mut SessionScope,
    ) -> bool {
        let table = &self.0.table;
        let _scope = profiling::interned_scope("Lifecycle::end_play::", self.0.class_name);
        if let Some(end_play) = table.end_play_world {
            let Some(world) = world else {
                return false;
            };
            end_play(
                world,
                &mut ActorCx::new(owner, Some(instance), events, scope),
            );
        } else if let Some(end_play) = table.end_play {
            end_play(&mut ActorCx::new(owner, None, events, scope));
        }
        true
    }
}

/// Every class's lifecycle in the frame's deterministic order (type name).
fn lifecycles() -> Vec<Lifecycle> {
    let mut all: Vec<Lifecycle> = crate::runtime::actors()
        .iter()
        .copied()
        .map(Lifecycle)
        .collect();
    all.sort_by_key(|lifecycle| {
        let key = lifecycle.key();
        (key.type_name, key.component_type)
    });
    all
}

/// Run the frame's gameplay step: the registered classes' systems
/// (`crate::systems`, in their labelled order) around each registered
/// component's live typed query, reconciling lifecycle transitions against
/// the preceding tick.
///
/// Each class's table is driven over the live enabled instances in entity
/// order: `begin_play` only for rows absent from the previous frame's set,
/// then the queued handler events, then `tick`. Removed/disabled/despawned rows are ended after
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
    profiling::profile_scope!("Lifecycle::tick_live_components");
    if state
        .scene_identity
        .is_some_and(|identity| identity != scene_identity)
    {
        state.end_all(events);
    }
    state.scene_identity = Some(scene_identity);

    // An editing session never plays: it only constructs (the editor pump).
    if !state.kind().is_playing() {
        pump_construct(world, events, state);
        return;
    }

    process_component_removals_mut(world, events, state);

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
                profiling::profile_scope!("Lifecycle::Step::pulsar.components");
                // The fixed pump point: every `construct` of the frame's
                // writes, before any `begin_play` or `tick`.
                pump_construct(world, events, state);
                tick_component_lifecycles(world, events, delta_seconds, &registrations, state)
            }
            crate::systems::Step::System(registration) => {
                let _scope =
                    profiling::interned_scope("Lifecycle::System::", registration.system.label);
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
    profiling::profile_scope!("Lifecycle::tick_component_lifecycles");
    for &lifecycle in registrations {
        let key = lifecycle.key();
        let registration = lifecycle.0;
        let previous = state.active.get(&key).cloned().unwrap_or_default();
        let mut current = HashMap::new();
        tick_actor_class(
            world,
            events,
            delta_seconds,
            registration,
            &previous,
            &mut current,
            state,
        );
        // What `begin_play` and `tick` wrote to their own components is not a
        // change for `construct` (D4): read it past.
        if registration.table.construct.is_some() {
            let own: HashSet<Entity> = current.keys().copied().collect();
            absorb_construct_changes(world, state, registration, Some(&own));
        }

        let mut ended: Vec<(Entity, Entity)> = previous
            .iter()
            .filter(|(entity, _)| !current.contains_key(*entity))
            .map(|(entity, owner)| (*entity, *owner))
            .collect();
        ended.sort_by_key(|(entity, _)| entity.bits());
        for &(instance, owner) in &ended {
            lifecycle.end_play(Some(&mut *world), instance, owner, events, &mut state.scope);
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
/// `begin_play` for those absent from `previous`, then the queued native
/// handler events, then `tick`.
fn tick_actor_class(
    world: &mut World,
    events: &EventHub,
    delta_seconds: f32,
    registration: &ActorRegistration,
    previous: &HashMap<Entity, Entity>,
    current: &mut HashMap<Entity, Entity>,
    state: &mut Session,
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
        let key = ComponentInstanceKey {
            component_type: (table.component_type)(),
            entity: instance,
        };
        // Subscribed before `begin_play`, so an event it provokes is seen.
        state.subscribe_instance(key, owner, events, table.handler_events);
        let queued = if table.event.is_some() {
            state.take_events(key)
        } else {
            Vec::new()
        };
        let mut cx = ActorCx::new(owner, Some(instance), events, &mut state.scope);
        if !previous.contains_key(&instance) {
            if let Some(begin_play) = table.begin_play {
                let _scope =
                    profiling::interned_scope("Lifecycle::begin_play::", registration.class_name);
                begin_play(world, &mut cx, instance);
            }
        }
        if let Some(event) = table.event {
            for queued_event in queued {
                let _scope =
                    profiling::interned_scope("Lifecycle::event::", registration.class_name);
                event(
                    world,
                    &mut cx,
                    instance,
                    &queued_event.name,
                    &queued_event.fields,
                );
            }
        }
        if let Some(tick) = table.tick {
            let _scope = profiling::interned_scope("Lifecycle::tick::", registration.class_name);
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
///
/// With only a `&World` this cannot run a world-form class's
/// [`pulsar_component_lifecycle::WorldActor::end_play`]: that end is queued
/// and runs at the next call with a mutable world
/// ([`process_component_removals_mut`], the next tick, or the session end).
/// A `TickLoop` calls the mutable form.
pub fn process_component_removals(
    world: &World,
    events: &EventHub,
    state: &mut ComponentRuntimeState,
) -> usize {
    profiling::profile_scope!("Lifecycle::process_component_removals");
    let removals = collect_removals(world, state);
    end_removed(None, events, state, removals)
}

/// [`process_component_removals`] with a mutable world: the queued world-form
/// ends run first, and a removal's world-form `end_play` runs now.
pub fn process_component_removals_mut(
    world: &mut World,
    events: &EventHub,
    state: &mut ComponentRuntimeState,
) -> usize {
    profiling::profile_scope!("Lifecycle::process_component_removals");
    state.run_deferred_ends(world, events);
    let removals = collect_removals(world, state);
    end_removed(Some(world), events, state, removals)
}

fn collect_removals(world: &World, state: &mut Session) -> Vec<(Entity, ComponentId)> {
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
    removals
}

fn end_removed(
    mut world: Option<&mut World>,
    events: &EventHub,
    state: &mut Session,
    removals: Vec<(Entity, ComponentId)>,
) -> usize {
    if removals.is_empty() {
        return 0;
    }
    profiling::profile_scope!("Lifecycle::end_removed");

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
            if !lifecycle.end_play(
                world.as_deref_mut(),
                entity,
                owner,
                events,
                &mut state.scope,
            ) {
                state.deferred_ends.push((lifecycle, entity, owner));
            }
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
/// This has **no world**: only the entity-only ends run (see
/// [`Session::end_all`]). A session ending with its world alive calls
/// [`end_session`].
pub fn end_live_components(state: &mut ComponentRuntimeState, events: &EventHub) {
    state.end_all(events);
}

/// End the session with its world alive: `end_play` for every live instance
/// (world-form ones with `&mut World`), then its resources and bookkeeping
/// are dropped. See [`Session::end_session`].
pub fn end_session(world: &mut World, events: &EventHub, state: &mut ComponentRuntimeState) {
    state.end_session(world, events);
}

// ── The construct pump (D4) ─────────────────────────────────────────────────

/// The classes with a `construct`, in the frame's order.
fn constructible() -> Vec<&'static ActorRegistration> {
    lifecycles()
        .into_iter()
        .map(|lifecycle| lifecycle.0)
        .filter(|registration| registration.table.construct.is_some())
        .collect()
}

/// Every enabled instance of `registration`'s class is to construct.
fn full_scan(world: &mut World, state: &mut Session, registration: &ActorRegistration) {
    let table = &registration.table;
    let mut instances = Vec::new();
    (table.instances)(world, &mut instances);
    let pending = state
        .construct
        .pending
        .entry((table.component_type)())
        .or_default();
    pending.clear();
    for entity in instances {
        pending.insert(
            entity.bits(),
            PendingConstruct {
                entity,
                reason: ConstructReason::Rescan,
            },
        );
    }
}

/// Read the class's construct cursor into the pending set, skipping what
/// `own` names (the framework's own callbacks' writes). Returns whether the
/// journal overflowed, which needs a full scan.
fn absorb_construct_changes(
    world: &World,
    state: &mut Session,
    registration: &ActorRegistration,
    own: Option<&HashSet<Entity>>,
) -> bool {
    let component_type = (registration.table.component_type)();
    let construct = &mut state.construct;
    let Some(cursor) = construct.cursors.get_mut(&component_type) else {
        return false;
    };
    state.scratch.clear();
    if world.read_changes(cursor, &mut state.scratch) == ChangeRead::Overflowed {
        return true;
    }
    let pending = construct.pending.entry(component_type).or_default();
    for change in &state.scratch {
        match change.kind {
            ComponentChangeKind::Removed => {
                pending.remove(&change.entity.bits());
            }
            kind => {
                if own.is_some_and(|own| own.contains(&change.entity)) {
                    continue;
                }
                let reason = if kind == ComponentChangeKind::Inserted {
                    ConstructReason::Added
                } else {
                    ConstructReason::Changed
                };
                pending
                    .entry(change.entity.bits())
                    .and_modify(|entry| entry.reason = entry.reason.max(reason))
                    .or_insert(PendingConstruct {
                        entity: change.entity,
                        reason,
                    });
            }
        }
    }
    false
}

/// Deliver `construct` for what was written since the last pump: the one
/// fixed pump point. Per instance, once: a hundred writes to one component
/// since the last pump are one call. A write a `construct` makes to its own
/// component is read past, not delivered again. The first pump of a session,
/// a requested rescan ([`Session::request_rescan`]) and an overflowed change
/// journal each scan every live instance once; that is one rescan
/// ([`ConstructStats::rescans`]), whatever the instance count.
///
/// Returns the `construct` calls made. Nothing is delivered while the pump is
/// suspended ([`Session::set_construct_suspended`]).
///
/// The editor drives it each frame for a [`SessionKind::Editor`] session
/// (`engine_backend::scene::construct_pump`); a play session's
/// [`tick_live_components`] calls it at the start of the `pulsar.components`
/// step.
pub fn pump_construct(world: &mut World, events: &EventHub, state: &mut Session) -> usize {
    if state.construct.suspended {
        return 0;
    }
    profiling::profile_scope!("Lifecycle::pump_construct");
    let registrations = constructible();
    if registrations.is_empty() {
        return 0;
    }

    let mut scanned = false;
    if std::mem::take(&mut state.construct.rescan) {
        // The content changed wholesale: forget what was pending and read
        // fresh from the head of every journal.
        state.construct.cursors.clear();
        state.construct.pending.clear();
        state.construct.named.clear();
    }
    for &registration in &registrations {
        let component_type = (registration.table.component_type)();
        if !state.construct.cursors.contains_key(&component_type) {
            // Open before scanning: a write between the two is read next
            // pump (a harmless second construct), never lost.
            let cursor = world.open_change_cursor_id(component_type);
            state.construct.cursors.insert(component_type, cursor);
            full_scan(world, state, registration);
            scanned = true;
        } else if absorb_construct_changes(world, state, registration, None) {
            full_scan(world, state, registration);
            scanned = true;
        }
    }
    if scanned {
        state.construct.stats.rescans += 1;
    }

    let mut delivered = 0;
    for registration in registrations {
        let table = &registration.table;
        let Some(construct) = table.construct else {
            continue;
        };
        let component_type = (table.component_type)();
        let batch = match state.construct.pending.get_mut(&component_type) {
            Some(pending) if !pending.is_empty() => std::mem::take(pending),
            _ => continue,
        };
        let mut enabled = Vec::new();
        (table.instances)(world, &mut enabled);
        let enabled: HashSet<Entity> = enabled.into_iter().collect();
        for pending in batch.into_values() {
            let instance = pending.entity;
            if !enabled.contains(&instance) {
                continue;
            }
            let Some(link) = world
                .get::<crate::pulsar_scene_model::ComponentOwner>(instance)
                .copied()
            else {
                continue;
            };
            let changed = state
                .construct
                .named
                .remove(&instance)
                .map(|names| names.into_iter().collect())
                .unwrap_or_default();
            let mut cx = ActorCx::new(link.entity(), Some(instance), events, &mut state.scope);
            cx.construct = Some(ConstructInfo {
                reason: pending.reason,
                changed,
            });
            {
                let _scope = profiling::interned_scope(
                    "Lifecycle::construct::",
                    registration.class_name,
                );
                construct(world, &mut cx, instance);
            }
            delivered += 1;
            state.construct.stats.delivered += 1;
            // Its own writes are not changes (D4): read them past. Others'
            // writes it made stay pending for the next pump.
            let own = HashSet::from([instance]);
            if absorb_construct_changes(world, state, registration, Some(&own)) {
                full_scan(world, state, registration);
                state.construct.stats.rescans += 1;
            }
        }
    }
    delivered
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
                assert!(count(name) >= 1, "no `{name}` scope in {:?}", names(&recorded));
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

    /// P2: `construct` (D4), the session scope and the session end.
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
}
