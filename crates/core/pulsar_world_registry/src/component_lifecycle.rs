//! Per-game lifecycle reconciliation for macro-registered live components.
//!
//! This state stores identities, event subscriptions, and transient payload
//! inboxes only. The authoritative component values remain in SceneDB and
//! are borrowed by each generated typed query for callbacks.

use crate::{ComponentTickRegistration, QueuedComponentEvent};
use pulsar_events::EventHub;
use pulsar_scenedb::{ComponentId, Entity, World};
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
    active: HashMap<RegistrationKey, HashSet<Entity>>,
    inbox: Arc<Mutex<EventInbox>>,
    subscriptions: HashMap<ComponentInstanceKey, Vec<pulsar_events::gamma::SyncSubscription>>,
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
    pub fn subscribe_instance(
        &mut self,
        key: ComponentInstanceKey,
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
                tracing::warn!(event = name, "native component handler event is not registered");
                continue;
            };
            let inbox = Arc::clone(&self.inbox);
            let callback_key = key;
            let callback_name = name.to_owned();
            subscriptions.push(hub.bus().subscribe_dyn(
                descriptor.id,
                pulsar_events::gamma::SubscribeOptions::channel(
                    pulsar_events::gamma::Channel::Entity(key.entity.bits()),
                ),
                move |event| {
                    let mut inbox = inbox.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    if inbox.active.contains(&callback_key) {
                        inbox.queued.entry(callback_key).or_default().push(QueuedComponentEvent {
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
        let mut inbox = self.inbox.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
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
        let mut registrations: Vec<_> = inventory::iter::<ComponentTickRegistration>
            .into_iter()
            .collect();
        registrations.sort_by_key(|registration| registration.type_name);

        for registration in registrations {
            let key = registration_key(registration);
            if let Some(entities) = self.active.remove(&key) {
                let mut entities: Vec<_> = entities.into_iter().collect();
                entities.sort_by_key(|entity| entity.bits());
                if let Some(end_play) = registration.end_play {
                    for &entity in &entities {
                        end_play(entity, events);
                    }
                }
                for entity in entities {
                    self.unsubscribe_instance(ComponentInstanceKey {
                        component_type: key.component_type,
                        entity,
                    });
                }
            }
        }
        self.active.clear();
        self.subscriptions.clear();
        *self.inbox.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = EventInbox::default();
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
    if state.scene_identity.is_some_and(|identity| identity != scene_identity) {
        state.end_all(events);
    }
    state.scene_identity = Some(scene_identity);

    process_component_removals(world, events, state);

    let mut registrations: Vec<_> = inventory::iter::<ComponentTickRegistration>
        .into_iter()
        .collect();
    registrations.sort_by_key(|registration| registration.type_name);

    for registration in registrations {
        let key = registration_key(registration);
        let previous = state.active.get(&key).cloned().unwrap_or_default();
        let mut current = HashSet::new();
        (registration.tick)(world, events, delta_seconds, &previous, &mut current, state);

        let mut ended: Vec<_> = previous.difference(&current).copied().collect();
        ended.sort_by_key(|entity| entity.bits());
        if let Some(end_play) = registration.end_play {
            for entity in ended {
                end_play(entity, events);
            }
        }
        for entity in previous.difference(&current) {
            state.unsubscribe_instance(ComponentInstanceKey {
                component_type: key.component_type,
                entity: *entity,
            });
        }
        state.active.insert(key, current);
    }
}

/// Consume SceneDB's independent post-removal journal and end any active
/// native component instances it names. Call this after mutation phases as
/// well as before component ticks: actors and scripts can remove components
/// after the native component phase has already run.
pub fn process_component_removals(
    world: &World,
    events: &EventHub,
    state: &mut ComponentRuntimeState,
) -> usize {
    let Some(tracker) = world.change_tracker() else { return 0 };
    let removals = tracker.drain_component_removals();
    if removals.is_empty() {
        return 0;
    }

    let mut registrations: Vec<_> = inventory::iter::<ComponentTickRegistration>
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
        let was_active = state
            .active
            .get_mut(&key)
            .is_some_and(|entities| entities.remove(&entity));
        if was_active {
            if let Some(end_play) = registration.end_play {
                end_play(entity, events);
            }
            state.unsubscribe_instance(ComponentInstanceKey { component_type, entity });
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
