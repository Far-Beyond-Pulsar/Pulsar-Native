//! Scripts on the engine event hub (#924).
//!
//! The [`ScriptDriver`](super::ScriptDriver) owns one [`ScriptEvents`] when
//! the tick loop gives it the session's [`EventHub`]:
//!
//! - **Declared events.** A class module's `events` are registered on the
//!   hub (dynamic descriptors, category `Custom/<class>`) before the class
//!   links; handlers link against the hub's catalog ([`ScriptEventBridge`]
//!   is the runtime's [`EventHost`]).
//! - **Subscriptions follow instances.** When the driver starts an instance
//!   it subscribes each of its class's subscriptions on the hub: `Self`
//!   scope on the entity channel of the instance's entity (skipped for an
//!   unbound global script), `Global` on the global channel, `Class` on the
//!   class channel of the instance's class GUID. When the instance stops
//!   (despawn, class change, shutdown) its subscriptions are dropped and
//!   its queued handler calls discarded. Reloading a class resubscribes
//!   its live instances.
//! - **Handlers never run inside the bus.** A hub flush happens while the
//!   world may be borrowed (and from the tick loop, not the script phase),
//!   so a subscription only queues `(instance, handler, event)`. The driver
//!   runs the queue in its next script phase.
//!
//! # Ordering
//!
//! One frame of the tick loop (see `crate::tick::TickLoop::tick_once` and
//! `pulsar_events::hub`):
//!
//! 1. flush *after input*, 2. ECS systems and actors (physics), flush
//!    *after physics*;
//! 3. the script phase: reconcile (subscribe new instances, drop stopped
//!    ones), `begin_play` of new instances (then `BeginPlay` is published
//!    for each bound one), `LevelLoaded` on the first frame, **queued
//!    handler calls**, `tick`, due timers (`TimerFired`), world commands
//!    (`EntitySpawned` / `EntityDestroyed`);
//! 4. flush *after scripts*, 5. flush *end of frame*.
//!
//! Handler calls run in the order the hub delivered them: flushes in frame
//! order, events in publish order within a flush, and for one event its
//! subscribers in subscription order, which is instance start order.
//! Consequences:
//!
//! - An event published during the script phase (a script's `event::send`,
//!   `BeginPlay`, `LevelLoaded`) reaches script handlers in the **next**
//!   frame's script phase, before that frame's `tick`.
//! - Input and physics events of a frame reach handlers in the same
//!   frame's script phase.
//! - An instance only receives events delivered after it subscribed: an
//!   instance started this frame misses events flushed earlier this frame.
//!   `LevelLoaded` is published after the level's instances started, so
//!   each of them handles it exactly once; objects spawned later never see
//!   it.
//! - A handler that waits (`Wait`) finishes on a later tick, like any
//!   latent event.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use pulsar_events::gamma::{
    Channel, DynEvent, DynValue, EventDescriptor, FieldType, SubscribeOptions, SyncSubscription,
};
use pulsar_events::{EventCategory, EventHub, class_channel, entity_channel};
use pulsar_scenedb::Entity;
use pulsar_script_runtime::{EventHost, RuntimeError, ScriptRuntime};
use pulsar_script_vm::{
    EventCatalog, EventDecl, EventField, EventSignature, EventSink, EventTarget, FuncId,
    SubscriptionScope, Type, Value,
};

// ---- type mapping -------------------------------------------------------------

/// The script type of an event field (`u64` fields are entities).
pub fn script_type_of(field: FieldType) -> Option<Type> {
    Some(match field {
        FieldType::Bool => Type::Bool,
        FieldType::I64 => Type::Int,
        FieldType::F64 => Type::Float,
        FieldType::Str => Type::Str,
        FieldType::U64 => Type::Entity,
        FieldType::Bytes => return None,
    })
}

/// The event field type of a script type, if it can be one.
pub fn field_type_of(ty: &Type) -> Option<FieldType> {
    Some(match ty {
        Type::Bool => FieldType::Bool,
        Type::Int => FieldType::I64,
        Type::Float => FieldType::F64,
        Type::Str => FieldType::Str,
        Type::Entity => FieldType::U64,
        _ => return None,
    })
}

/// A script's view of a descriptor. `None` when a field has no script
/// type (raw bytes): scripts cannot handle that event.
pub fn signature_of(descriptor: &EventDescriptor) -> Option<EventSignature> {
    let fields = descriptor
        .fields
        .iter()
        .map(|(name, ty)| Some(EventField::new(name.clone(), script_type_of(*ty)?)))
        .collect::<Option<Vec<_>>>()?;
    Some(EventSignature { id: descriptor.id, name: descriptor.name.clone(), fields })
}

/// The descriptor a script declaration registers.
pub fn descriptor_of(decl: &EventDecl) -> Result<EventDescriptor, String> {
    let fields = decl
        .fields
        .iter()
        .map(|f| {
            field_type_of(&f.ty)
                .map(|t| (f.name.clone(), t))
                .ok_or_else(|| format!("field `{}` is {}, which cannot be an event field", f.name, f.ty))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(EventDescriptor::dynamic(decl.name.clone(), fields))
}

fn to_dyn_value(index: usize, value: &Value) -> Result<DynValue, String> {
    Ok(match value {
        Value::Bool(b) => DynValue::Bool(*b),
        Value::Int(i) => DynValue::I64(*i),
        Value::Float(f) => DynValue::F64(*f),
        Value::Str(s) => DynValue::Str(s.to_string()),
        Value::Entity(e) => DynValue::U64(e.bits()),
        other => return Err(format!("argument {index}: a {} cannot be an event field", other.kind())),
    })
}

fn to_value(value: &DynValue) -> Option<Value> {
    Some(match value {
        DynValue::Bool(b) => Value::Bool(*b),
        DynValue::I64(i) => Value::Int(*i),
        DynValue::F64(f) => Value::Float(*f),
        DynValue::Str(s) => Value::Str(s.as_str().into()),
        DynValue::U64(bits) => Value::Entity(Entity::from_bits(*bits)),
        DynValue::Bytes(_) => return None,
    })
}

// ---- timers -------------------------------------------------------------------

struct Timer {
    id: i64,
    owner: Option<Entity>,
    due: f64,
    interval: Option<f64>,
}

#[derive(Default)]
struct Timers {
    next_id: i64,
    now: f64,
    timers: Vec<Timer>,
}

// ---- the runtime's event host ---------------------------------------------------

/// The script runtime's [`EventHost`] over a session's [`EventHub`]: the
/// `event::*` natives publish here (deferred), handlers link against the
/// hub's catalog, declared events register on it. Also runs `timer::*`.
pub struct ScriptEventBridge {
    hub: EventHub,
    /// Class name or GUID → class GUID, for `event::emit_to_class`.
    classes: RwLock<HashMap<String, String>>,
    timers: Mutex<Timers>,
}

impl ScriptEventBridge {
    pub fn new(hub: EventHub) -> Self {
        Self { hub, classes: RwLock::new(HashMap::new()), timers: Mutex::new(Timers::default()) }
    }

    pub fn hub(&self) -> &EventHub {
        &self.hub
    }

    /// Make class `name` (GUID `guid`) addressable by `emit_to_class`.
    pub fn add_class(&self, name: &str, guid: &str) {
        let mut classes = self.classes.write().unwrap_or_else(|p| p.into_inner());
        classes.insert(name.to_owned(), guid.to_owned());
        classes.insert(guid.to_owned(), guid.to_owned());
    }

    fn class_guid(&self, class: &str) -> Option<String> {
        self.classes.read().unwrap_or_else(|p| p.into_inner()).get(class).cloned()
    }

    fn timers(&self) -> std::sync::MutexGuard<'_, Timers> {
        self.timers.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Set the game time new timers count from.
    pub fn set_time(&self, now: f64) {
        self.timers().now = now;
    }

    /// Advance timer time (game seconds) and publish `TimerFired` for every
    /// timer now due, earliest first. Returns how many fired.
    pub fn fire_timers(&self, now: f64) -> usize {
        let fired: Vec<(i64, Option<Entity>)> = {
            let mut timers = self.timers();
            timers.now = now;
            let mut fired = Vec::new();
            timers.timers.sort_by(|a, b| a.due.total_cmp(&b.due).then(a.id.cmp(&b.id)));
            timers.timers.retain_mut(|t| {
                if t.due > now {
                    return true;
                }
                fired.push((t.id, t.owner));
                match t.interval {
                    // At most one firing per frame for a looping timer.
                    Some(interval) => {
                        t.due = (t.due + interval).max(now + f64::EPSILON);
                        true
                    }
                    None => false,
                }
            });
            fired
        };
        for (id, owner) in &fired {
            let channel = owner.map_or(Channel::Global, |e| entity_channel(e.bits()));
            self.hub.publish(channel, pulsar_events::builtin::TimerFired { timer: *id });
        }
        fired.len()
    }

    /// Cancel every timer owned by `entity` (it stopped).
    pub fn clear_timers_of(&self, entity: Entity) {
        self.timers().timers.retain(|t| t.owner != Some(entity));
    }

    /// Cancel every timer (session end).
    pub fn clear_all_timers(&self) {
        self.timers().timers.clear();
    }

    pub fn timer_count(&self) -> usize {
        self.timers().timers.len()
    }
}

impl EventSink for ScriptEventBridge {
    fn emit(&self, target: EventTarget, name: &str, fields: &[Value]) -> Result<(), String> {
        let channel = match &target {
            EventTarget::Global => Channel::Global,
            EventTarget::Entity(entity) => entity_channel(entity.bits()),
            EventTarget::Class(class) => {
                let guid = self.class_guid(class).ok_or_else(|| format!("no class `{class}` in this project"))?;
                class_channel(&guid)
            }
        };
        let values = fields.iter().enumerate().map(|(i, v)| to_dyn_value(i, v)).collect::<Result<Vec<_>, _>>()?;
        self.hub.publish_named(channel, name, values).map_err(|e| e.to_string())
    }

    fn set_timer(&self, owner: Option<Entity>, seconds: f64, looping: bool) -> Result<i64, String> {
        if !seconds.is_finite() || seconds < 0.0 {
            return Err(format!("timer::set: {seconds} is not a valid duration"));
        }
        if looping && seconds <= 0.0 {
            return Err("timer::set: a looping timer needs a positive duration".into());
        }
        let mut timers = self.timers();
        timers.next_id += 1;
        let id = timers.next_id;
        let due = timers.now + seconds;
        timers.timers.push(Timer { id, owner, due, interval: looping.then_some(seconds) });
        Ok(id)
    }

    fn clear_timer(&self, timer: i64) -> bool {
        let mut timers = self.timers();
        let before = timers.timers.len();
        timers.timers.retain(|t| t.id != timer);
        timers.timers.len() != before
    }
}

impl EventCatalog for ScriptEventBridge {
    fn event_by_name(&self, name: &str) -> Option<EventSignature> {
        signature_of(&*self.hub.descriptor_by_name(name)?)
    }

    fn event_by_id(&self, id: u64) -> Option<EventSignature> {
        signature_of(&*self.hub.descriptor(id)?)
    }
}

impl EventHost for ScriptEventBridge {
    fn declare(&self, class: &str, decl: &EventDecl) -> Result<(), String> {
        let descriptor = descriptor_of(decl)?;
        self.hub
            .register(descriptor, EventCategory::Custom(class.to_owned()))
            .map(drop)
            .map_err(|e| e.to_string())
    }
}

// ---- per-instance subscriptions and the call queue -----------------------------

/// A handler call queued by a hub delivery.
struct PendingCall {
    instance: Arc<str>,
    handler: FuncId,
    params: usize,
    event: DynEvent,
}

type CallQueue = Arc<Mutex<Vec<PendingCall>>>;

struct InstanceSubscriptions {
    entity: Option<Entity>,
    handles: Vec<SyncSubscription>,
}

/// The driver's side of the hub: subscriptions per instance and the queue
/// of handler calls. See the module doc.
pub struct ScriptEvents {
    bridge: Arc<ScriptEventBridge>,
    calls: CallQueue,
    instances: HashMap<String, InstanceSubscriptions>,
    level_announced: bool,
}

impl ScriptEvents {
    pub fn new(hub: EventHub) -> Self {
        Self {
            bridge: Arc::new(ScriptEventBridge::new(hub)),
            calls: Arc::default(),
            instances: HashMap::new(),
            level_announced: false,
        }
    }

    pub fn hub(&self) -> &EventHub {
        self.bridge.hub()
    }

    pub fn bridge(&self) -> &Arc<ScriptEventBridge> {
        &self.bridge
    }

    /// The runtime's event host.
    pub fn host(&self) -> Arc<dyn EventHost> {
        Arc::clone(&self.bridge) as Arc<dyn EventHost>
    }

    /// Live hub subscriptions held for script instances.
    pub fn subscription_count(&self) -> usize {
        self.instances.values().map(|i| i.handles.len()).sum()
    }

    /// Subscriptions of one instance.
    pub fn subscriptions_of(&self, instance: &str) -> usize {
        self.instances.get(instance).map_or(0, |i| i.handles.len())
    }

    /// Handler calls waiting for the next script phase.
    pub fn pending_calls(&self) -> usize {
        self.lock_calls().len()
    }

    fn lock_calls(&self) -> std::sync::MutexGuard<'_, Vec<PendingCall>> {
        self.calls.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Subscribe instance `id` (class `class` with GUID `class_guid`, bound
    /// to `entity`) to its class's subscriptions, replacing any it had.
    /// Returns messages for subscriptions that could not be made.
    pub fn subscribe(
        &mut self,
        runtime: &ScriptRuntime,
        id: &str,
        class: &str,
        class_guid: &str,
        entity: Option<Entity>,
    ) -> Vec<String> {
        self.unsubscribe(id);
        let mut failures = Vec::new();
        let Some(subscriptions) = runtime.subscriptions(class) else {
            return failures;
        };
        let hub = self.bridge.hub();
        let instance: Arc<str> = Arc::from(id);
        let mut handles = Vec::with_capacity(subscriptions.len());
        for sub in subscriptions {
            let descriptor = match (sub.event_id, &sub.event_name) {
                (Some(event_id), _) => hub.descriptor(event_id),
                (None, Some(name)) => hub.descriptor_by_name(name),
                (None, None) => None,
            };
            let Some(descriptor) = descriptor else {
                failures.push(format!("script instance '{id}': event `{}` is not registered; its handler is not subscribed", sub.event));
                continue;
            };
            let channel = match sub.scope {
                SubscriptionScope::Global => Channel::Global,
                SubscriptionScope::Class => class_channel(class_guid),
                SubscriptionScope::Self_ => match entity {
                    Some(entity) => entity_channel(entity.bits()),
                    None => {
                        tracing::debug!(instance = %id, event = %descriptor.name, "unbound instance: `Self` subscription skipped");
                        continue;
                    }
                },
            };
            let calls = Arc::clone(&self.calls);
            let instance = Arc::clone(&instance);
            let (handler, params) = (sub.handler, sub.params);
            handles.push(hub.bus().subscribe_dyn(descriptor.id, SubscribeOptions::channel(channel), move |event| {
                calls.lock().unwrap_or_else(|p| p.into_inner()).push(PendingCall {
                    instance: Arc::clone(&instance),
                    handler,
                    params,
                    event: event.clone(),
                });
            }));
        }
        self.instances.insert(id.to_owned(), InstanceSubscriptions { entity, handles });
        failures
    }

    /// Drop instance `id`'s subscriptions, queued calls and timers.
    pub fn unsubscribe(&mut self, id: &str) {
        if let Some(gone) = self.instances.remove(id) {
            if let Some(entity) = gone.entity {
                self.bridge.clear_timers_of(entity);
            }
            drop(gone.handles);
            self.lock_calls().retain(|call| &*call.instance != id);
        }
    }

    /// Drop everything (session end): subscriptions, queued calls, timers,
    /// and events still queued on the hub.
    pub fn clear(&mut self) {
        self.instances.clear();
        self.lock_calls().clear();
        self.bridge.clear_all_timers();
        self.bridge.hub().discard_queued();
    }

    /// Publish `LevelLoaded` once per session. `true` if it did now.
    pub fn announce_level(&mut self, level: &str) -> bool {
        if self.level_announced {
            return false;
        }
        self.level_announced = true;
        self.hub().publish(Channel::Global, pulsar_events::builtin::LevelLoaded { level: level.to_owned() });
        true
    }

    /// Run every queued handler call, in delivery order. Calls for
    /// instances that stopped since are skipped.
    pub fn run_calls(&mut self, runtime: &mut ScriptRuntime, world: &mut pulsar_scenedb::World) -> Vec<RuntimeError> {
        let calls = std::mem::take(&mut *self.lock_calls());
        let mut errors = Vec::new();
        for call in calls {
            if !self.instances.contains_key(&*call.instance) {
                continue;
            }
            let Some(args) = call.event.fields.iter().take(call.params).map(to_value).collect::<Option<Vec<_>>>()
            else {
                tracing::warn!(instance = %call.instance, "event with byte fields cannot reach a script handler");
                continue;
            };
            if let Err(error) = runtime.call_function(&call.instance, call.handler, &args, world) {
                tracing::warn!("{error}");
                errors.push(error);
            }
        }
        errors
    }
}
