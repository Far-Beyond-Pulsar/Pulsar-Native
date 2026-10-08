//! Per-game lifecycle reconciliation for macro-registered live components.
//!
//! This state stores identities, event subscriptions, and transient payload
//! inboxes only. The authoritative component values remain in SceneDB and
//! are borrowed by each generated typed query for callbacks.

use crate::{ComponentTickRegistration, QueuedComponentEvent};
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
        let mut registrations: Vec<_> = crate::runtime::ticks().iter().copied()
            .into_iter()
            .collect();
        registrations.sort_by_key(|registration| registration.type_name);

        for registration in registrations {
            let key = registration_key(registration);
            if let Some(entities) = self.active.remove(&key) {
                let mut entities: Vec<_> = entities.into_iter().collect();
                entities.sort_by_key(|(entity, _)| entity.bits());
                if let Some(end_play) = registration.end_play {
                    for &(_, owner) in &entities {
                        end_play(owner, events);
                    }
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

fn registration_key(registration: &ComponentTickRegistration) -> RegistrationKey {
    RegistrationKey {
        component_type: (registration.component_type)(),
        type_name: registration.type_name,
    }
}

/// Run each registered component's live typed query and reconcile lifecycle
/// transitions against the preceding tick.
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

    process_component_removals(world, events, state);

    let mut registrations: Vec<_> = crate::runtime::ticks().iter().copied()
        .into_iter()
        .collect();
    registrations.sort_by_key(|registration| registration.type_name);
    for registration in &registrations {
        let component_type = (registration.component_type)();
        state
            .removal_cursors
            .entry(component_type)
            .or_insert_with(|| world.open_change_cursor_id(component_type));
    }

    for registration in registrations {
        let key = registration_key(registration);
        let previous = state.active.get(&key).cloned().unwrap_or_default();
        let mut current = HashMap::new();
        (registration.tick)(world, events, delta_seconds, &previous, &mut current, state);

        let mut ended: Vec<(Entity, Entity)> = previous
            .iter()
            .filter(|(entity, _)| !current.contains_key(*entity))
            .map(|(entity, owner)| (*entity, *owner))
            .collect();
        ended.sort_by_key(|(entity, _)| entity.bits());
        if let Some(end_play) = registration.end_play {
            for &(_, owner) in &ended {
                end_play(owner, events);
            }
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

    let mut registrations: Vec<_> = crate::runtime::ticks().iter().copied()
        .into_iter()
        .collect();
    registrations.sort_by_key(|registration| registration.type_name);
    let mut ended = 0;
    for (entity, component_type) in removals {
        let Some(registration) = registrations
            .iter()
            .copied()
            .find(|registration| (registration.component_type)() == component_type)
        else {
            continue;
        };
        let key = registration_key(registration);
        let ended_owner = state
            .active
            .get_mut(&key)
            .and_then(|entities| entities.remove(&entity));
        if let Some(owner) = ended_owner {
            if let Some(end_play) = registration.end_play {
                end_play(owner, events);
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
/// This can end subscriptions and other runtime-owned state using the
/// owner identity, but by definition cannot borrow component data that was
/// already removed from SceneDB.
pub fn end_live_components(state: &mut ComponentRuntimeState, events: &EventHub) {
    state.end_all(events);
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
}
