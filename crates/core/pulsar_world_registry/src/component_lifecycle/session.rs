//! The session: identities, subscriptions, event inboxes and the construct pump's bookkeeping.

use super::tick::{lifecycles, Lifecycle};
use crate::QueuedComponentEvent;
use pulsar_component_lifecycle::{ConstructReason, SessionKind, SessionScope};
use pulsar_events::EventHub;
use pulsar_scenedb::{ChangeCursor, ComponentChange, ComponentId, Entity, World};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub(super) struct EventInbox {
    pub(super) active: HashSet<ComponentInstanceKey>,
    pub(super) queued: HashMap<ComponentInstanceKey, Vec<QueuedComponentEvent>>,
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
pub(super) struct PendingConstruct {
    pub(super) entity: Entity,
    pub(super) reason: ConstructReason,
}

/// The construct pump's bookkeeping (D4).
#[derive(Default)]
pub(super) struct ConstructState {
    /// This session's own cursor per class with a `construct`, over the
    /// writes of its component: separate from the removal cursors, so the
    /// two readers never take entries from each other.
    pub(super) cursors: HashMap<ComponentId, ChangeCursor>,
    /// Instances to construct at the next pump, deduplicated: a hundred
    /// writes to one component are one entry. Keyed by entity bits (a
    /// deterministic order).
    pub(super) pending: HashMap<ComponentId, BTreeMap<u64, PendingConstruct>>,
    /// A restore or an undo replaced the world's content: scan everything
    /// once at the next pump instead of reading N changes.
    pub(super) rescan: bool,
    /// The pump is not delivering (a play session owns the world).
    pub(super) suspended: bool,
    /// Properties a writer named for an instance, delivered with its
    /// construct.
    pub(super) named: HashMap<Entity, BTreeSet<String>>,
    pub(super) stats: ConstructStats,
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
    pub(super) scope: SessionScope,
    pub(super) construct: ConstructState,
    /// Ends of world-form classes read from the removal journal through a
    /// `&World` (`process_component_removals`), to run at the next point
    /// that has a mutable world.
    pub(super) deferred_ends: Vec<(Lifecycle, Entity, Entity)>,
    pub(super) scene_identity: Option<usize>,
    /// Per class: active instance entity -> its owner object.
    pub(super) active: HashMap<RegistrationKey, HashMap<Entity, Entity>>,
    pub(super) inbox: Arc<Mutex<EventInbox>>,
    pub(super) subscriptions:
        HashMap<ComponentInstanceKey, Vec<pulsar_events::gamma::SyncSubscription>>,
    /// This session's own change cursor per registered component type,
    /// opened before any of its instances starts. Removals are read from it,
    /// so whoever else reads (or drains) the world's change history cannot
    /// take one from this session.
    pub(super) removal_cursors: HashMap<ComponentId, ChangeCursor>,
    pub(super) scratch: Vec<ComponentChange>,
    /// The frame's order of systems and component lifecycles, for the
    /// registered systems it was computed from (their list's address and
    /// length: the registry rebuilds the list when the classes change).
    pub(super) order: Option<((usize, usize), Vec<crate::systems::Step>)>,
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
pub(super) struct RegistrationKey {
    pub(super) component_type: ComponentId,
    pub(super) type_name: &'static str,
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
    pub(super) fn run_deferred_ends(&mut self, world: &mut World, events: &EventHub) {
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
