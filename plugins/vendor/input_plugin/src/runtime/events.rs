//! The `input` event namespace: how actions reach scripts and other plugins.
//!
//! Everything is a Gamma event in the `input` namespace, published on the
//! session's [`EventHub`] and delivered at the tick loop's usual flushes, so
//! a script handler runs in the same frame the key was pressed.
//!
//! # Per action
//!
//! Each action has five events named after it and a phase:
//! `input::Jump.started`, `.ongoing`, `.triggered`, `.completed`,
//! `.canceled` (see [`Phase`]). They are registered when a mapping that
//! binds the action becomes active. Every one carries:
//!
//! | Field | Type | |
//! |---|---|---|
//! | `entity` | u64 | the object the mapping is attached to; 0 for a global mapping |
//! | `value` | f64 | the value's size: 1 or 0 for a button, the number for a 1D axis, the length for 2D and 3D |
//! | `x`, `y`, `z` | f64 | the value's components |
//! | `elapsed` | f64 | seconds since the action left idle |
//! | `triggered_for` | f64 | seconds it has been firing |
//!
//! They go to the owner's entity channel when the mapping is attached to an
//! object (so a script on that object hears its own player's input), and to
//! the global channel otherwise.
//!
//! # For everything at once
//!
//! `input::ActionStarted`, `ActionOngoing`, `ActionTriggered`,
//! `ActionCompleted` and `ActionCanceled` carry the same fields plus
//! `action` (the name), `action_id` and `mapping` (ids), for a plugin that
//! wants to watch all input (a rebinding screen, a recorder). They are always
//! global. The per-action names always contain a dot, so they can never be
//! mistaken for these.
//!
//! # Commands
//!
//! Scripts change which mappings are active by publishing:
//!
//! * `input::AddMapping { mapping, priority, entity }`
//! * `input::RemoveMapping { mapping, entity }`
//! * `input::SetMappingEnabled { mapping, entity, enabled }`
//!
//! `mapping` is the mapping's project-relative path or its id; `entity` is 0
//! for a global mapping. They are queued and applied at the start of the
//! next input update.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use pulsar_events::gamma::{
    Channel, DynEvent, DynValue, EventDescriptor, FieldType, SubscribeOptions, SyncSubscription,
};
use pulsar_events::{EventCategory, EventHub};

use super::system::{ActionEvent, Phase};

/// The Gamma namespace of every event in this module.
pub const NAMESPACE: &str = "input";

/// The per-action field names, in order.
pub const ACTION_FIELDS: [&str; 7] = ["entity", "value", "x", "y", "z", "elapsed", "triggered_for"];

/// The names of the always-registered events that watch all actions.
pub fn generic_name(phase: Phase) -> &'static str {
    match phase {
        Phase::Started => "ActionStarted",
        Phase::Ongoing => "ActionOngoing",
        Phase::Triggered => "ActionTriggered",
        Phase::Completed => "ActionCompleted",
        Phase::Canceled => "ActionCanceled",
    }
}

/// The local name of an action's event: `Jump.triggered`.
pub fn action_event_name(action: &str, phase: Phase) -> String {
    format!("{action}.{}", phase.word())
}

/// The name a script uses: `input::Jump.triggered`.
pub fn qualified(action: &str, phase: Phase) -> String {
    format!("{NAMESPACE}::{}", action_event_name(action, phase))
}

fn generic_descriptor(phase: Phase) -> EventDescriptor {
    let fields = [
        ("action", FieldType::Str),
        ("action_id", FieldType::Str),
        ("mapping", FieldType::Str),
        ("entity", FieldType::U64),
        ("value", FieldType::F64),
        ("x", FieldType::F64),
        ("y", FieldType::F64),
        ("z", FieldType::F64),
        ("elapsed", FieldType::F64),
        ("triggered_for", FieldType::F64),
    ];
    EventDescriptor::dynamic_in(NAMESPACE, generic_name(phase), fields).expect("`input` is not a reserved namespace")
}

/// The descriptor of an action's event for `phase`.
pub fn action_descriptor(action: &str, phase: Phase) -> Result<EventDescriptor, String> {
    EventDescriptor::dynamic_in(
        NAMESPACE,
        action_event_name(action, phase),
        ACTION_FIELDS.map(|f| (f, if f == "entity" { FieldType::U64 } else { FieldType::F64 })),
    )
    .map_err(|e| e.to_string())
}

/// What a script asked for.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Add { mapping: String, priority: i64, entity: u64 },
    Remove { mapping: String, entity: u64 },
    SetEnabled { mapping: String, entity: u64, enabled: bool },
}

/// Input the devices produced since the last update, as the hub delivered it.
#[derive(Clone, Debug, PartialEq)]
pub enum Raw {
    Key { code: i64, down: bool },
    MouseButton { button: i64, down: bool },
    MouseMoved { dx: f64, dy: f64 },
    Wheel { x: f64, y: f64 },
}

/// Shared with the handlers the bus calls.
#[derive(Default)]
pub struct Inbox {
    pub raw: Vec<Raw>,
    pub commands: Vec<Command>,
}

/// An [`EventHub`] this module has registered on and listens to.
pub struct HubBinding {
    hub: EventHub,
    inbox: Arc<Mutex<Inbox>>,
    generic: HashMap<Phase, u64>,
    per_action: HashMap<(Arc<str>, Phase), u64>,
    _subscriptions: Vec<SyncSubscription>,
}

impl HubBinding {
    /// Register the fixed events on `hub` and listen to the devices'.
    pub fn attach(hub: &EventHub) -> Self {
        use pulsar_events::builtin::*;
        use pulsar_events::gamma::Event;

        let inbox: Arc<Mutex<Inbox>> = Arc::default();
        let mut generic = HashMap::new();
        for phase in Phase::ALL {
            match hub.register(generic_descriptor(phase), EventCategory::Input) {
                Ok(id) => {
                    generic.insert(phase, id);
                }
                Err(error) => tracing::warn!(%error, "input: couldn't register `{}`", generic_name(phase)),
            }
        }

        let mut subscriptions = Vec::new();
        let mut listen = |id: u64, handler: Box<dyn Fn(&DynEvent) + Send + Sync>| {
            subscriptions.push(hub.bus().subscribe_dyn(id, SubscribeOptions::default(), handler));
        };
        let mut route = |id: u64, to_raw: fn(&DynEvent) -> Raw| {
            let inbox = inbox.clone();
            listen(id, Box::new(move |e| lock(&inbox).raw.push(to_raw(e))));
        };
        route(KeyDown::stable_type_id(), |e| Raw::Key { code: int(e, 0), down: true });
        route(KeyUp::stable_type_id(), |e| Raw::Key { code: int(e, 0), down: false });
        route(MouseButtonDown::stable_type_id(), |e| Raw::MouseButton { button: int(e, 0), down: true });
        route(MouseButtonUp::stable_type_id(), |e| Raw::MouseButton { button: int(e, 0), down: false });
        route(MouseMoved::stable_type_id(), |e| Raw::MouseMoved { dx: float(e, 2), dy: float(e, 3) });
        route(MouseWheel::stable_type_id(), |e| Raw::Wheel { x: float(e, 0), y: float(e, 1) });

        // Commands.
        let commands: [(&str, Vec<(&str, FieldType)>); 3] = [
            ("AddMapping", vec![("mapping", FieldType::Str), ("priority", FieldType::I64), ("entity", FieldType::U64)]),
            ("RemoveMapping", vec![("mapping", FieldType::Str), ("entity", FieldType::U64)]),
            ("SetMappingEnabled", vec![("mapping", FieldType::Str), ("entity", FieldType::U64), ("enabled", FieldType::Bool)]),
        ];
        for (name, fields) in commands {
            let Ok(descriptor) = EventDescriptor::dynamic_in(NAMESPACE, name, fields) else { continue };
            let Ok(id) = hub.register(descriptor, EventCategory::Input) else { continue };
            let inbox = inbox.clone();
            let handler: Box<dyn Fn(&DynEvent) + Send + Sync> = match name {
                "AddMapping" => Box::new(move |e| lock(&inbox).commands.push(Command::Add { mapping: text(e, 0), priority: int(e, 1), entity: unsigned(e, 2) })),
                "RemoveMapping" => Box::new(move |e| lock(&inbox).commands.push(Command::Remove { mapping: text(e, 0), entity: unsigned(e, 1) })),
                _ => Box::new(move |e| {
                    lock(&inbox).commands.push(Command::SetEnabled { mapping: text(e, 0), entity: unsigned(e, 1), enabled: matches!(e.fields.get(2), Some(DynValue::Bool(true))) })
                }),
            };
            listen(id, handler);
        }

        Self { hub: hub.clone(), inbox, generic, per_action: HashMap::new(), _subscriptions: subscriptions }
    }

    pub fn hub(&self) -> &EventHub {
        &self.hub
    }

    /// Take what has arrived since the last call.
    pub fn drain(&self) -> Inbox {
        std::mem::take(&mut *lock(&self.inbox))
    }

    /// Register an action's five events (idempotent).
    pub fn declare_action(&mut self, name: &Arc<str>) {
        for phase in Phase::ALL {
            if self.per_action.contains_key(&(name.clone(), phase)) {
                continue;
            }
            match action_descriptor(name, phase).and_then(|d| self.hub.register(d, EventCategory::Input).map_err(|e| e.to_string())) {
                Ok(id) => {
                    self.per_action.insert((name.clone(), phase), id);
                }
                Err(error) => tracing::warn!(%error, action = %name, "input: couldn't register `{}`", action_event_name(name, phase)),
            }
        }
    }

    /// Queue `event` for delivery: the action's own event, then the
    /// everything-at-once one. Events nobody listens for are skipped.
    pub fn publish(&self, event: &ActionEvent) {
        let channel = event.owner.map_or(Channel::Global, Channel::Entity);
        let entity = event.owner.unwrap_or(0);
        let [x, y, z] = event.value.map(f64::from);
        let size = f64::from((event.value[0].powi(2) + event.value[1].powi(2) + event.value[2].powi(2)).sqrt());
        let numbers = [size, x, y, z, f64::from(event.elapsed), f64::from(event.triggered_for)];

        if let Some(&id) = self.per_action.get(&(event.name.clone(), event.phase)) {
            if self.wanted(id, channel) {
                let mut fields = vec![DynValue::U64(entity)];
                fields.extend(numbers.map(DynValue::F64));
                self.send(channel, id, fields, &event.name);
            }
        }
        if let Some(&id) = self.generic.get(&event.phase) {
            if self.wanted(id, Channel::Global) {
                let mut fields = vec![DynValue::Str(event.name.to_string()), DynValue::Str(event.action.to_string()), DynValue::Str(event.mapping.to_string()), DynValue::U64(entity)];
                fields.extend(numbers.map(DynValue::F64));
                self.send(Channel::Global, id, fields, &event.name);
            }
        }
    }

    /// Whether anything would see an event: a subscriber, or the debug tap.
    fn wanted(&self, id: u64, channel: Channel) -> bool {
        self.hub.tap_enabled() || self.hub.subscriber_count(id, channel) > 0
    }

    fn send(&self, channel: Channel, id: u64, fields: Vec<DynValue>, action: &str) {
        if let Err(error) = self.hub.publish_dyn(channel, DynEvent::new(id, fields)) {
            tracing::warn!(%error, action, "input: an event was refused");
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn int(e: &DynEvent, i: usize) -> i64 {
    match e.fields.get(i) {
        Some(DynValue::I64(v)) => *v,
        _ => 0,
    }
}

fn unsigned(e: &DynEvent, i: usize) -> u64 {
    match e.fields.get(i) {
        Some(DynValue::U64(v)) => *v,
        _ => 0,
    }
}

fn float(e: &DynEvent, i: usize) -> f64 {
    match e.fields.get(i) {
        Some(DynValue::F64(v)) => *v,
        _ => 0.0,
    }
}

fn text(e: &DynEvent, i: usize) -> String {
    match e.fields.get(i) {
        Some(DynValue::Str(v)) => v.clone(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::behavior::ValueType;
    use crate::model::AssetId;
    use pulsar_events::FlushPoint;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn event(name: &str, phase: Phase, owner: Option<u64>) -> ActionEvent {
        ActionEvent {
            context: super::super::system::ContextHandle(1),
            mapping: AssetId::new(),
            action: AssetId::new(),
            name: name.into(),
            owner,
            phase,
            value_type: ValueType::Axis2D,
            value: [3.0, 4.0, 0.0],
            elapsed: 0.5,
            triggered_for: 0.25,
        }
    }

    #[test]
    fn names_are_namespaced_and_always_contain_a_dot_for_actions() {
        assert_eq!(qualified("Jump", Phase::Triggered), "input::Jump.triggered");
        assert!(!generic_name(Phase::Started).contains('.'));
    }

    #[test]
    fn raw_input_published_on_the_hub_reaches_the_inbox_at_the_flush() {
        let hub = EventHub::new();
        let binding = HubBinding::attach(&hub);
        hub.publish(Channel::Global, pulsar_events::builtin::KeyDown { key: 41 });
        hub.publish(Channel::Global, pulsar_events::builtin::MouseMoved { x: 1.0, y: 1.0, dx: 2.0, dy: -3.0 });
        hub.publish(Channel::Global, pulsar_events::builtin::MouseWheel { x: 0.0, y: 1.0 });
        hub.publish(Channel::Global, pulsar_events::builtin::MouseButtonUp { button: 2 });
        assert!(binding.drain().raw.is_empty(), "nothing is delivered before the flush");
        hub.flush(FlushPoint::AfterInput);
        let raw = binding.drain().raw;
        assert_eq!(
            raw,
            [
                Raw::Key { code: 41, down: true },
                Raw::MouseMoved { dx: 2.0, dy: -3.0 },
                Raw::Wheel { x: 0.0, y: 1.0 },
                Raw::MouseButton { button: 2, down: false },
            ]
        );
        assert!(binding.drain().raw.is_empty(), "drained");
    }

    #[test]
    fn script_commands_arrive_as_commands() {
        let hub = EventHub::new();
        let binding = HubBinding::attach(&hub);
        hub.publish_named(Channel::Global, "input::AddMapping", vec![DynValue::Str("input/Game.inputmap".into()), DynValue::I64(5), DynValue::U64(9)]).unwrap();
        hub.publish_named(Channel::Global, "input::SetMappingEnabled", vec![DynValue::Str("x".into()), DynValue::U64(0), DynValue::Bool(true)]).unwrap();
        hub.flush(FlushPoint::Manual);
        assert_eq!(
            binding.drain().commands,
            [
                Command::Add { mapping: "input/Game.inputmap".into(), priority: 5, entity: 9 },
                Command::SetEnabled { mapping: "x".into(), entity: 0, enabled: true },
            ]
        );
    }

    #[test]
    fn an_action_event_reaches_a_subscriber_by_its_script_name() {
        let hub = EventHub::new();
        let mut binding = HubBinding::attach(&hub);
        let name: Arc<str> = "Jump".into();
        binding.declare_action(&name);
        let descriptor = hub.descriptor_by_name("input::Jump.triggered").expect("registered under its qualified name");
        let seen = Arc::new(AtomicUsize::new(0));
        let sink = seen.clone();
        let _sub = hub.bus().subscribe_dyn(descriptor.id, SubscribeOptions::default(), move |e| {
            assert_eq!(e.fields.len(), ACTION_FIELDS.len());
            assert_eq!(e.fields[1], DynValue::F64(5.0), "value is the length");
            sink.fetch_add(1, Ordering::SeqCst);
        });
        binding.publish(&event("Jump", Phase::Triggered, None));
        binding.publish(&event("Jump", Phase::Completed, None));
        hub.flush(FlushPoint::Manual);
        assert_eq!(seen.load(Ordering::SeqCst), 1, "only the phase subscribed to");
    }

    #[test]
    fn an_owned_action_goes_to_the_owners_channel_and_the_watcher_hears_all() {
        let hub = EventHub::new();
        let mut binding = HubBinding::attach(&hub);
        let name: Arc<str> = "Fire".into();
        binding.declare_action(&name);
        let id = hub.descriptor_by_name("input::Fire.started").unwrap().id;
        let owner_hits = Arc::new(AtomicUsize::new(0));
        let global_hits = Arc::new(AtomicUsize::new(0));
        let (a, b) = (owner_hits.clone(), global_hits.clone());
        let _own = hub.bus().subscribe_dyn(id, SubscribeOptions::channel(Channel::Entity(77)), move |_| {
            a.fetch_add(1, Ordering::SeqCst);
        });
        let _global = hub.bus().subscribe_dyn(id, SubscribeOptions::default(), move |_| {
            b.fetch_add(1, Ordering::SeqCst);
        });
        let watcher = hub.descriptor_by_name("input::ActionStarted").unwrap().id;
        let watched = Arc::new(Mutex::new(Vec::new()));
        let w = watched.clone();
        let _watch = hub.bus().subscribe_dyn(watcher, SubscribeOptions::default(), move |e| w.lock().unwrap().push(e.fields.clone()));

        binding.publish(&event("Fire", Phase::Started, Some(77)));
        hub.flush(FlushPoint::Manual);
        assert_eq!(owner_hits.load(Ordering::SeqCst), 1);
        assert_eq!(global_hits.load(Ordering::SeqCst), 0, "an owned action isn't also global");
        let watched = watched.lock().unwrap();
        assert_eq!(watched.len(), 1);
        assert_eq!(watched[0][0], DynValue::Str("Fire".into()));
        assert_eq!(watched[0][3], DynValue::U64(77));
    }

    #[test]
    fn events_nobody_listens_for_are_not_queued() {
        let hub = EventHub::new();
        let mut binding = HubBinding::attach(&hub);
        let name: Arc<str> = "Idle".into();
        binding.declare_action(&name);
        binding.publish(&event("Idle", Phase::Ongoing, None));
        assert_eq!(hub.queued_len(), 0);
    }

    #[test]
    fn declaring_twice_is_harmless_and_a_bad_name_is_refused_not_fatal() {
        let hub = EventHub::new();
        let mut binding = HubBinding::attach(&hub);
        let name: Arc<str> = "Jump".into();
        binding.declare_action(&name);
        binding.declare_action(&name);
        assert_eq!(hub.events().iter().filter(|e| e.descriptor.name.starts_with("Jump.")).count(), 5);
        binding.declare_action(&"".into());
    }
}
