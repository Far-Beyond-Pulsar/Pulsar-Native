//! Per-frame lifecycle: begin, tick and end of live components, and processing their removals.

use super::construct::{absorb_construct_changes, pump_construct};
use super::session::{ComponentInstanceKey, ComponentRuntimeState, RegistrationKey, Session};
use pulsar_component_lifecycle::{ActorCx, ActorRegistration, SessionScope};
use pulsar_events::EventHub;
use pulsar_scenedb::{ChangeRead, ComponentChangeKind, ComponentId, Entity, World};
use std::collections::{HashMap, HashSet};

/// One class's lifecycle: its registered table.
#[derive(Clone, Copy)]
pub(super) struct Lifecycle(pub(super) &'static ActorRegistration);

impl Lifecycle {
    pub(super) fn key(self) -> RegistrationKey {
        RegistrationKey {
            component_type: (self.0.table.component_type)(),
            type_name: self.0.table.type_name,
        }
    }

    /// The teardown, if the class has one. A world-form `end_play` needs
    /// `world`; without one it does not run and this returns `false` (the
    /// caller may queue it for a point that has one).
    pub(super) fn end_play(
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
pub(super) fn lifecycles() -> Vec<Lifecycle> {
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
