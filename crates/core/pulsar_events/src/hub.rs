//! The engine event hub: one Gamma bus per world / game session.
//!
//! # Delivery model
//!
//! Everything published through the hub is **deferred**: it waits in the
//! bus's queue until the owner flushes the hub. The tick loop
//! (`pulsar_game::tick::TickLoop`) flushes at four fixed points every
//! frame, in this order:
//!
//! 1. [`FlushPoint::AfterInput`]: input received since the last frame
//!    (key and mouse events) and anything queued between frames;
//! 2. [`FlushPoint::AfterPhysics`]: after the ECS schedule and native actor
//!    phases, where physics runs (hits and overlaps);
//! 3. [`FlushPoint::AfterScripts`]: what scripts sent during the script
//!    phase (`event::emit` / `event::send` / `event::emit_to_class`,
//!    `BeginPlay`, `EntitySpawned`, ...);
//! 4. [`FlushPoint::EndOfFrame`]: stragglers queued after the script
//!    phase (render / window callbacks).
//!
//! Handlers run at flush time, in priority then subscription order, and
//! events queued by handlers during a flush are delivered in the same flush
//! (Gamma's rounds). Script handlers do not run the VM inside the flush:
//! they only queue a call that the script driver runs in its next script
//! phase (see `pulsar_game::scripting::events`).
//!
//! # Registry
//!
//! [`EventHub::new`] registers every [built-in event](crate::builtin).
//! Script modules register the events they declare with
//! [`register`](EventHub::register); plugins can register through Gamma's
//! FFI on the bus [`export_raw`](EventHub::export_raw) hands them.
//!
//! # Performance (Pulsar-Native#939)
//!
//! Gamma's deferred queue is one mutex. The hub's publish methods and
//! [`flush`](EventHub::flush) run inside profiler scopes
//! (`EventHub::enqueue`, `EventHub::flush`), so lock waits under load show
//! up in the engine profiler.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use gamma::ffi::RawBus;
use gamma::{
    Channel, DynEvent, DynEventError, DynValue, Event, EventDescriptor, FlushReport,
    RegistryError, SyncEventBus,
};

use crate::tap::{Pending, Tap, TapRecord, summarize};

/// Where in the frame a flush happens. See the module doc.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FlushPoint {
    AfterInput,
    AfterPhysics,
    AfterScripts,
    EndOfFrame,
    /// A flush outside the tick loop (tools, tests, shutdown).
    Manual,
}

impl fmt::Display for FlushPoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::AfterInput => "after input",
            Self::AfterPhysics => "after physics",
            Self::AfterScripts => "after scripts",
            Self::EndOfFrame => "end of frame",
            Self::Manual => "manual",
        })
    }
}

/// Palette grouping of an event.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EventCategory {
    Lifecycle,
    World,
    Input,
    Physics,
    Gameplay,
    /// Declared by a script class or a plugin; the string names the owner
    /// (a class name, a plugin id).
    Custom(String),
}

impl fmt::Display for EventCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lifecycle => f.write_str("Lifecycle"),
            Self::World => f.write_str("World"),
            Self::Input => f.write_str("Input"),
            Self::Physics => f.write_str("Physics"),
            Self::Gameplay => f.write_str("Gameplay"),
            Self::Custom(owner) if owner.is_empty() => f.write_str("Custom"),
            Self::Custom(owner) => write!(f, "Custom/{owner}"),
        }
    }
}

/// A registered event and its category.
#[derive(Clone, Debug, PartialEq)]
pub struct EventInfo {
    pub descriptor: Arc<EventDescriptor>,
    pub category: EventCategory,
    pub builtin: bool,
}

/// Why a named publish was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PublishError {
    /// No event with this name is registered.
    UnknownEvent(String),
    /// The fields do not match the descriptor.
    Fields { event: String, error: DynEventError },
}

impl fmt::Display for PublishError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownEvent(name) => write!(f, "no event named `{name}` is registered"),
            Self::Fields { event, error } => write!(f, "event `{event}`: {error}"),
        }
    }
}

impl std::error::Error for PublishError {}

struct Inner {
    bus: SyncEventBus,
    info: RwLock<HashMap<u64, (EventCategory, bool)>>,
    tap: Tap,
    frame: AtomicU64,
}

/// The event hub of one world / game session. Cheap to clone (shared).
#[derive(Clone)]
pub struct EventHub {
    inner: Arc<Inner>,
}

impl Default for EventHub {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for EventHub {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventHub")
            .field("queued", &self.queued_len())
            .field("frame", &self.frame())
            .finish()
    }
}

impl EventHub {
    /// A hub with every built-in event registered.
    pub fn new() -> Self {
        let hub = Self {
            inner: Arc::new(Inner {
                bus: SyncEventBus::new(),
                info: RwLock::new(HashMap::new()),
                tap: Tap::new(),
                frame: AtomicU64::new(0),
            }),
        };
        for (descriptor, category) in crate::builtin::builtin_events() {
            let id = hub
                .inner
                .bus
                .register_descriptor(descriptor)
                .expect("built-in event descriptors do not conflict");
            hub.write_info().insert(id, (category, true));
        }
        hub
    }

    /// The underlying Gamma bus, for subscribing (typed or dynamic) and for
    /// immediate publishes. Prefer the hub's own publish methods for
    /// deferred events: they feed the debug tap.
    pub fn bus(&self) -> &SyncEventBus {
        &self.inner.bus
    }

    /// `true` if `self` and `other` are the same hub.
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    // ---- registry -----------------------------------------------------------

    /// Register an event (a script- or plugin-declared one). Registering
    /// the same descriptor again is a no-op; a different descriptor with
    /// the same name or id is an error.
    pub fn register(&self, descriptor: EventDescriptor, category: EventCategory) -> Result<u64, RegistryError> {
        let id = self.inner.bus.register_descriptor(descriptor)?;
        self.write_info().entry(id).or_insert((category, false));
        Ok(id)
    }

    pub fn descriptor(&self, id: u64) -> Option<Arc<EventDescriptor>> {
        self.inner.bus.descriptor(id)
    }

    pub fn descriptor_by_name(&self, name: &str) -> Option<Arc<EventDescriptor>> {
        self.inner.bus.descriptor_by_name(name)
    }

    /// Every registered event, sorted by category then name. Events
    /// registered directly on the bus (by a plugin) are `Custom("")`.
    pub fn events(&self) -> Vec<EventInfo> {
        let info = self.inner.info.read().unwrap_or_else(|p| p.into_inner());
        let mut events: Vec<EventInfo> = self
            .inner
            .bus
            .descriptors()
            .into_iter()
            .map(|descriptor| {
                let (category, builtin) = info
                    .get(&descriptor.id)
                    .cloned()
                    .unwrap_or((EventCategory::Custom(String::new()), false));
                EventInfo { descriptor, category, builtin }
            })
            .collect();
        events.sort_by(|a, b| (&a.category, &a.descriptor.name).cmp(&(&b.category, &b.descriptor.name)));
        events
    }

    fn write_info(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<u64, (EventCategory, bool)>> {
        self.inner.info.write().unwrap_or_else(|p| p.into_inner())
    }

    // ---- publishing (deferred) ------------------------------------------------

    /// Queue a typed event on `channel` for the next flush.
    pub fn publish<T: Event + Send>(&self, channel: Channel, event: T) {
        if self.inner.tap.enabled() {
            let descriptor = self.descriptor(T::stable_type_id());
            self.note(T::stable_type_id(), channel, descriptor.as_deref(), event.to_dyn().as_ref());
        }
        scope!("EventHub::enqueue");
        self.inner.bus.publish_deferred_to(channel, event);
    }

    /// Queue a dynamic event on `channel` for the next flush. Its fields
    /// must match its registered descriptor.
    pub fn publish_dyn(&self, channel: Channel, event: DynEvent) -> Result<(), DynEventError> {
        if self.inner.tap.enabled() {
            let descriptor = self.descriptor(event.id);
            if let Some(d) = &descriptor {
                if d.check(&event).is_ok() {
                    self.note(event.id, channel, Some(d), Some(&event));
                }
            }
        }
        scope!("EventHub::enqueue");
        self.inner.bus.publish_dyn_deferred(channel, event)
    }

    /// Queue the event registered as `name` with `fields` (in descriptor
    /// order) on `channel`. What script natives use.
    pub fn publish_named(&self, channel: Channel, name: &str, fields: Vec<DynValue>) -> Result<(), PublishError> {
        let descriptor = self
            .descriptor_by_name(name)
            .ok_or_else(|| PublishError::UnknownEvent(name.to_owned()))?;
        let event = DynEvent::new(descriptor.id, fields);
        descriptor
            .check(&event)
            .map_err(|error| PublishError::Fields { event: name.to_owned(), error })?;
        self.publish_dyn(channel, event)
            .map_err(|error| PublishError::Fields { event: name.to_owned(), error })
    }

    fn note(&self, id: u64, channel: Channel, descriptor: Option<&EventDescriptor>, event: Option<&DynEvent>) {
        let name = descriptor.map_or_else(|| format!("#{id:016x}"), |d| d.name.clone());
        self.inner.tap.note(Pending { id, name, channel, summary: summarize(descriptor, event) });
    }

    // ---- flushing -----------------------------------------------------------

    /// Deliver everything queued. [`FlushPoint::EndOfFrame`] also advances
    /// the hub's frame counter.
    pub fn flush(&self, point: FlushPoint) -> FlushReport {
        scope!("EventHub::flush");
        let bus = &self.inner.bus;
        let report = bus.flush();
        if self.inner.tap.enabled() {
            self.inner
                .tap
                .delivered(self.frame(), point, |id, channel| bus.subscriber_count(id, channel));
        }
        if report.hit_round_limit {
            tracing::warn!(%point, remaining = report.remaining, "event flush hit its round limit; the rest waits for the next flush");
        }
        if point == FlushPoint::EndOfFrame {
            self.inner.frame.fetch_add(1, Ordering::Relaxed);
        }
        report
    }

    /// Frames completed (end-of-frame flushes).
    pub fn frame(&self) -> u64 {
        self.inner.frame.load(Ordering::Relaxed)
    }

    /// Events waiting for a flush.
    pub fn queued_len(&self) -> usize {
        self.inner.bus.queued_len()
    }

    /// Subscribers of event `id` on `channel`.
    pub fn subscriber_count(&self, id: u64, channel: Channel) -> usize {
        self.inner.bus.subscriber_count(id, channel)
    }

    /// Global-channel subscriber count of every registered event, by name.
    pub fn global_subscriber_counts(&self) -> Vec<(String, usize)> {
        self.inner
            .bus
            .descriptors()
            .iter()
            .map(|d| (d.name.clone(), self.subscriber_count(d.id, Channel::Global)))
            .collect()
    }

    /// Drop every queued event without delivering it (session teardown).
    /// Handlers subscribed at this point still run for what is delivered;
    /// call it after the session dropped its subscriptions.
    pub fn discard_queued(&self) -> usize {
        let report = self.inner.bus.flush_with_limit(u32::MAX);
        self.inner.tap.clear();
        report.delivered
    }

    // ---- plugins ------------------------------------------------------------

    /// This hub's bus as a Gamma FFI table, for a plugin compiled as a
    /// separate library (it wraps it in `gamma::ffi::ForeignBus`). The table
    /// holds a strong reference, released when the plugin drops its
    /// `ForeignBus`.
    pub fn export_raw(&self) -> RawBus {
        self.inner.bus.export_raw()
    }

    // ---- debug tap ----------------------------------------------------------

    /// Turn the debug tap on (keeping the last `capacity` events) or off.
    pub fn set_tap(&self, enabled: bool, capacity: usize) {
        self.inner.tap.set_enabled(enabled, capacity);
    }

    pub fn tap_enabled(&self) -> bool {
        self.inner.tap.enabled()
    }

    /// The tap's records, oldest first.
    pub fn recent_events(&self) -> Vec<TapRecord> {
        self.inner.tap.recent()
    }

    /// Everything the events debug panel shows.
    pub fn snapshot(&self) -> crate::tap::EventsSnapshot {
        let mut events: Vec<crate::tap::SnapshotEvent> = self
            .events()
            .into_iter()
            .map(|e| crate::tap::SnapshotEvent {
                global_subscribers: self.subscriber_count(e.descriptor.id, Channel::Global),
                name: e.descriptor.name.clone(),
                category: e.category.to_string(),
            })
            .collect();
        events.sort_by(|a, b| a.name.cmp(&b.name));
        crate::tap::EventsSnapshot {
            frame: self.frame(),
            queued: self.queued_len(),
            tap_enabled: self.tap_enabled(),
            recent: self.recent_events().iter().map(Into::into).collect(),
            events,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::{Hit, KeyDown, LevelLoaded};
    use gamma::{FieldType, SubscribeOptions};
    use std::sync::Mutex;

    #[test]
    fn builtins_are_registered_with_categories() {
        let hub = EventHub::new();
        let events = hub.events();
        let hit = events.iter().find(|e| e.descriptor.name == "Hit").unwrap();
        assert_eq!(hit.category, EventCategory::Physics);
        assert!(hit.builtin);
        assert!(hub.descriptor_by_name("LevelLoaded").is_some());
    }

    #[test]
    fn deferred_until_flush_and_entity_channels_do_not_fan_out() {
        let hub = EventHub::new();
        let got = Arc::new(Mutex::new(Vec::new()));
        let g = Arc::clone(&got);
        let _mine = hub.bus().subscribe_with(SubscribeOptions::channel(Channel::Entity(7)), move |e: &Hit| {
            g.lock().unwrap().push(e.other);
        });
        let g = Arc::clone(&got);
        let _global = hub.bus().subscribe(move |_: &Hit| g.lock().unwrap().push(999));

        hub.publish(Channel::Entity(7), Hit { entity: 7, other: 1, impulse: 1.0 });
        hub.publish(Channel::Entity(8), Hit { entity: 8, other: 2, impulse: 1.0 });
        assert!(got.lock().unwrap().is_empty(), "nothing before the flush");
        let report = hub.flush(FlushPoint::AfterPhysics);
        assert_eq!(report.delivered, 2);
        assert_eq!(*got.lock().unwrap(), vec![1], "only entity 7's subscriber");
    }

    #[test]
    fn named_publish_checks_fields_and_reaches_typed_subscribers() {
        let hub = EventHub::new();
        let got = Arc::new(Mutex::new(Vec::new()));
        let g = Arc::clone(&got);
        let _s = hub.bus().subscribe(move |e: &KeyDown| g.lock().unwrap().push(e.key));
        assert!(matches!(
            hub.publish_named(Channel::Global, "Nope", vec![]),
            Err(PublishError::UnknownEvent(_))
        ));
        assert!(matches!(
            hub.publish_named(Channel::Global, "KeyDown", vec![DynValue::F64(1.0)]),
            Err(PublishError::Fields { .. })
        ));
        hub.publish_named(Channel::Global, "KeyDown", vec![DynValue::I64(32)]).unwrap();
        hub.flush(FlushPoint::AfterInput);
        assert_eq!(*got.lock().unwrap(), vec![32]);
    }

    #[test]
    fn custom_events_register_once() {
        let hub = EventHub::new();
        let d = EventDescriptor::dynamic("Door.Opened", [("by", FieldType::U64)]);
        let id = hub.register(d.clone(), EventCategory::Custom("Door".into())).unwrap();
        assert_eq!(hub.register(d, EventCategory::Custom("Door".into())).unwrap(), id);
        let conflicting = EventDescriptor::dynamic("Door.Opened", [("by", FieldType::I64)]);
        assert!(hub.register(conflicting, EventCategory::Custom("Door".into())).is_err());
        let info = hub.events().into_iter().find(|e| e.descriptor.id == id).unwrap();
        assert_eq!(info.category.to_string(), "Custom/Door");
    }

    #[test]
    fn tap_records_flushed_events_with_subscriber_counts() {
        let hub = EventHub::new();
        hub.set_tap(true, 2);
        let _s = hub.bus().subscribe_with(SubscribeOptions::channel(Channel::Entity(3)), |_: &Hit| {});
        hub.publish(Channel::Global, LevelLoaded { level: "a.level".into() });
        hub.publish(Channel::Entity(3), Hit { entity: 3, other: 4, impulse: 0.5 });
        hub.publish(Channel::Global, KeyDown { key: 1 });
        assert!(hub.recent_events().is_empty(), "recorded at flush");
        hub.flush(FlushPoint::AfterScripts);
        let recent = hub.recent_events();
        assert_eq!(recent.len(), 2, "bounded");
        assert_eq!(recent[0].name, "Hit");
        assert_eq!(recent[0].subscribers, 1);
        assert_eq!(recent[0].channel, Channel::Entity(3));
        assert!(recent[0].summary.contains("impulse=0.500"), "{}", recent[0].summary);
        assert_eq!(recent[1].point, FlushPoint::AfterScripts);
        let snapshot = hub.snapshot();
        assert_eq!(snapshot.recent[0].channel, "entity:3");
        let json = serde_json::to_string(&snapshot).unwrap();
        let back: crate::tap::EventsSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(back, snapshot);
        assert!(back.events.iter().any(|e| e.name == "Hit" && e.category == "Physics"));
        hub.set_tap(false, 0);
        assert!(hub.recent_events().is_empty());
    }

    #[test]
    fn discard_drops_queued_events() {
        let hub = EventHub::new();
        hub.publish(Channel::Global, KeyDown { key: 1 });
        assert_eq!(hub.queued_len(), 1);
        hub.discard_queued();
        assert_eq!(hub.queued_len(), 0);
    }
}
