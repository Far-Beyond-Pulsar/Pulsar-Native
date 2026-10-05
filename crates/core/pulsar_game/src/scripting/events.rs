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
use pulsar_events::{class_channel, entity_channel, EventCategory, EventHub};
use pulsar_scenedb::{ComponentRef, Entity, World};
use pulsar_script_runtime::{EventHost, RuntimeError, ScriptRuntime};
use pulsar_script_vm::{
    EventCatalog, EventDecl, EventField, EventSignature, EventSink, EventTarget, FuncId,
    SubscriptionScope, Type, TypeRegistry, Value,
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
        // Gamma exposes opaque bytes only. The script's stable value type is
        // retained in EventSignature and decoded after delivery on this host.
        Type::Object(_) => FieldType::Bytes,
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
    Some(EventSignature {
        id: descriptor.id,
        name: descriptor.name.clone(),
        fields,
    })
}

/// The descriptor a script declaration registers.
pub fn descriptor_of(decl: &EventDecl) -> Result<EventDescriptor, String> {
    let fields = decl
        .fields
        .iter()
        .map(|f| {
            field_type_of(&f.ty)
                .map(|t| (f.name.clone(), t))
                .ok_or_else(|| {
                    format!(
                        "field `{}` is {}, which cannot be an event field",
                        f.name, f.ty
                    )
                })
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
        Value::Object(object) => DynValue::Bytes(
            pulsar_script_vm::TypeRegistry::global()
                .encode_event_value(object)
                .map_err(|error| format!("argument {index}: {error}"))?,
        ),
        other => {
            return Err(format!(
                "argument {index}: a {} cannot be an event field",
                other.kind()
            ))
        }
    })
}

fn to_value(value: &DynValue, expected: Option<&Type>) -> Result<Value, String> {
    Ok(match value {
        DynValue::Bool(b) => Value::Bool(*b),
        DynValue::I64(i) => Value::Int(*i),
        DynValue::F64(f) => Value::Float(*f),
        DynValue::Str(s) => Value::Str(s.as_str().into()),
        DynValue::U64(bits) => Value::Entity(Entity::from_bits(*bits)),
        DynValue::Bytes(bytes) => {
            let Some(Type::Object(name)) = expected else {
                return Err(
                    "received opaque event bytes without a registered object field type".into(),
                );
            };
            Value::Object(
                pulsar_script_vm::TypeRegistry::global()
                    .decode_event_value(name, bytes)
                    .map_err(|error| format!("event payload `{name}`: {error}"))?,
            )
        }
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
    /// Gamma deliberately sees object fields only as `Bytes`; keep their
    /// local script types beside the registered descriptor.
    signatures: RwLock<HashMap<u64, EventSignature>>,
    timers: Mutex<Timers>,
}

impl ScriptEventBridge {
    pub fn new(hub: EventHub) -> Self {
        Self {
            hub,
            classes: RwLock::new(HashMap::new()),
            signatures: RwLock::new(HashMap::new()),
            timers: Mutex::new(Timers::default()),
        }
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
        self.classes
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(class)
            .cloned()
    }

    fn timers(&self) -> std::sync::MutexGuard<'_, Timers> {
        self.timers.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn typed_signature(&self, id: u64) -> Option<EventSignature> {
        self.signatures
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(&id)
            .cloned()
    }

    /// Register a dynamic Gamma descriptor while retaining its local script
    /// payload types. Object payloads appear as `FieldType::Bytes` to Gamma;
    /// this signature supplies the stable value type used to decode bytes
    /// after local delivery.
    pub fn register_event_decl(&self, class: &str, decl: &EventDecl) -> Result<u64, String> {
        let descriptor = descriptor_of(decl)?;
        let id = self
            .hub
            .register(descriptor, EventCategory::Custom(class.to_owned()))
            .map_err(|error| error.to_string())?;
        let mut signature = EventSignature::from(decl);
        signature.id = id;
        let mut signatures = self.signatures.write().unwrap_or_else(|p| p.into_inner());
        if let Some(previous) = signatures.get(&id) {
            if previous.name != signature.name || previous.fields != signature.fields {
                return Err(format!(
                    "event `{}` has conflicting local payload types: {:?} vs {:?}",
                    decl.name, previous.fields, signature.fields
                ));
            }
        } else {
            signatures.insert(id, signature);
        }
        Ok(id)
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
            timers
                .timers
                .sort_by(|a, b| a.due.total_cmp(&b.due).then(a.id.cmp(&b.id)));
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
            self.hub
                .publish(channel, pulsar_events::builtin::TimerFired { timer: *id });
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
                let guid = self
                    .class_guid(class)
                    .ok_or_else(|| format!("no class `{class}` in this project"))?;
                class_channel(&guid)
            }
        };
        let values = fields
            .iter()
            .enumerate()
            .map(|(i, v)| to_dyn_value(i, v))
            .collect::<Result<Vec<_>, _>>()?;
        self.hub
            .publish_named(channel, name, values)
            .map_err(|e| e.to_string())
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
        timers.timers.push(Timer {
            id,
            owner,
            due,
            interval: looping.then_some(seconds),
        });
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
        let descriptor = self.hub.descriptor_by_name(name)?;
        self.typed_signature(descriptor.id)
            .or_else(|| signature_of(&descriptor))
    }

    fn event_by_id(&self, id: u64) -> Option<EventSignature> {
        let descriptor = self.hub.descriptor(id)?;
        self.typed_signature(id)
            .or_else(|| signature_of(&descriptor))
    }
}

impl EventHost for ScriptEventBridge {
    fn declare(&self, class: &str, decl: &EventDecl) -> Result<(), String> {
        self.register_event_decl(class, decl).map(drop)
    }
}

// ---- per-instance subscriptions and the call queue -----------------------------

/// A handler call queued by a hub delivery.
struct PendingCall {
    instance: Arc<str>,
    handler: FuncId,
    params: usize,
    event: DynEvent,
    /// Present only for a `Component(variable)` subscription. The handler is
    /// dropped if that variable no longer names this exact live reference.
    component_source: Option<ComponentSubscriptionSource>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ComponentSubscriptionSource {
    variable: u32,
    target: ComponentRef,
}

type CallQueue = Arc<Mutex<Vec<PendingCall>>>;

struct InstanceSubscriptions {
    entity: Option<Entity>,
    subscriptions: Vec<InstalledSubscription>,
}

struct InstalledSubscription {
    event: u64,
    scope: SubscriptionScope,
    handler: FuncId,
    params: usize,
    /// The last live target resolved for a component scope. `None` means
    /// absent, stale, type-mismatched, or not yet checked (hub attachment can
    /// happen before the driver has a World borrow).
    component_target: Option<ComponentRef>,
    handle: Option<SyncSubscription>,
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
        self.instances
            .values()
            .map(|instance| instance.subscriptions.iter().filter(|s| s.handle.is_some()).count())
            .sum()
    }

    /// Subscriptions of one instance.
    pub fn subscriptions_of(&self, instance: &str) -> usize {
        self.instances.get(instance).map_or(0, |i| {
            i.subscriptions.iter().filter(|s| s.handle.is_some()).count()
        })
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
        world: Option<&World>,
    ) -> Vec<String> {
        self.unsubscribe(id);
        self.subscribe_handlers(runtime, id, class, class_guid, entity, world)
            .0
    }

    /// Subscribe instance `id` again after its class was reloaded (#925):
    /// like [`subscribe`](Self::subscribe), but its timers keep running and
    /// handler calls already queued for it go to the new code's handler of
    /// the same event (they are dropped if the class no longer handles it).
    pub fn resubscribe(
        &mut self,
        runtime: &ScriptRuntime,
        id: &str,
        class: &str,
        class_guid: &str,
        entity: Option<Entity>,
        world: Option<&World>,
    ) -> Vec<String> {
        // Only the bus handles; timers and queued calls stay.
        self.instances.remove(id);
        let (failures, handlers) =
            self.subscribe_handlers(runtime, id, class, class_guid, entity, world);
        self.lock_calls().retain_mut(|call| {
            if &*call.instance != id {
                return true;
            }
            // A class reload can change component variable ordering or the
            // assigned target. Queued component-scoped deliveries belong to
            // the old subscription generation and cannot be safely rebound.
            if call.component_source.is_some() {
                return false;
            }
            match handlers
                .iter()
                .find(|(event, _, _)| *event == call.event.id)
            {
                Some(&(_, handler, params)) => {
                    call.handler = handler;
                    call.params = params;
                    true
                }
                None => false,
            }
        });
        failures
    }

    /// Subscribe the handlers; returns the failures and, per subscribed
    /// event id, its handler and parameter count.
    fn subscribe_handlers(
        &mut self,
        runtime: &ScriptRuntime,
        id: &str,
        class: &str,
        class_guid: &str,
        entity: Option<Entity>,
        world: Option<&World>,
    ) -> (Vec<String>, Vec<(u64, FuncId, usize)>) {
        let mut failures = Vec::new();
        let mut handlers = Vec::new();
        let Some(subscriptions) = runtime.subscriptions(class) else {
            return (failures, handlers);
        };
        let hub = self.bridge.hub();
        let instance: Arc<str> = Arc::from(id);
        let mut installed = Vec::with_capacity(subscriptions.len());
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
            let (channel, component_target) = match sub.scope {
                SubscriptionScope::Global => (Some(Channel::Global), None),
                SubscriptionScope::Class => (Some(class_channel(class_guid)), None),
                SubscriptionScope::Self_ => match entity {
                    Some(entity) => (Some(entity_channel(entity.bits())), None),
                    None => {
                        tracing::debug!(instance = %id, event = %descriptor.name, "unbound instance: `Self` subscription skipped");
                        continue;
                    }
                },
                SubscriptionScope::Component(variable) => {
                    let target = match world {
                        Some(world) => match resolve_component_target(runtime, id, variable, world) {
                            Ok(target) => target,
                            Err(error) => {
                                failures.push(format!("script instance '{id}': {error}"));
                                None
                            }
                        },
                        None => None,
                    };
                    (target.map(|target| entity_channel(target.entity.bits())), target)
                }
            };
            let (handler, params) = (sub.handler, sub.params);
            handlers.push((descriptor.id, handler, params));
            let handle = channel.map(|channel| {
                make_subscription(
                    hub,
                    descriptor.id,
                    channel,
                    Arc::clone(&instance),
                    handler,
                    params,
                    match sub.scope {
                        SubscriptionScope::Component(variable) => component_target.map(|target| {
                            ComponentSubscriptionSource { variable, target }
                        }),
                        _ => None,
                    },
                    Arc::clone(&self.calls),
                )
            });
            installed.push(InstalledSubscription {
                event: descriptor.id,
                scope: sub.scope,
                handler,
                params,
                component_target,
                handle,
            });
        }
        self.instances.insert(id.to_owned(), InstanceSubscriptions { entity, subscriptions: installed });
        (failures, handlers)
    }

    /// Re-resolve every component-scoped source variable against the current
    /// script instance and live SceneDB. Only changed component subscriptions
    /// are replaced; global/self/class subscriptions keep their existing
    /// ordering and handles.
    pub fn reconcile_component_subscriptions(
        &mut self,
        runtime: &ScriptRuntime,
        world: &World,
    ) -> Vec<String> {
        let mut failures = Vec::new();
        let ids: Vec<_> = self.instances.keys().cloned().collect();
        for id in ids {
            let variables: Vec<_> = self.instances[&id]
                .subscriptions
                .iter()
                .filter_map(|subscription| match subscription.scope {
                    SubscriptionScope::Component(variable) => Some(variable),
                    _ => None,
                })
                .collect();
            if variables.is_empty() {
                continue;
            }
            let targets: HashMap<_, _> = variables
                .into_iter()
                .map(|variable| {
                    (variable, resolve_component_target(runtime, &id, variable, world))
                })
                .collect();
            let mut changed_variables = HashMap::new();
            {
                let Some(instance) = self.instances.get_mut(&id) else { continue };
                for subscription in &mut instance.subscriptions {
                    let SubscriptionScope::Component(variable) = subscription.scope else { continue };
                    let desired = targets
                        .get(&variable)
                        .and_then(|target| target.as_ref().ok().copied().flatten());
                    if desired == subscription.component_target {
                        continue;
                    }

                    subscription.handle = None;
                    subscription.component_target = desired;
                    changed_variables.insert(variable, desired);
                    if let Some(target) = desired {
                        subscription.handle = Some(make_subscription(
                            self.bridge.hub(),
                            subscription.event,
                            entity_channel(target.entity.bits()),
                            Arc::from(id.as_str()),
                            subscription.handler,
                            subscription.params,
                            Some(ComponentSubscriptionSource { variable, target }),
                            Arc::clone(&self.calls),
                        ));
                    } else if let Some(Err(error)) = targets.get(&variable) {
                        failures.push(format!("script instance '{id}': {error}"));
                    }
                }
            }
            if !changed_variables.is_empty() {
                self.lock_calls().retain(|call| {
                    if &*call.instance != id {
                        return true;
                    }
                    let Some(source) = call.component_source else { return true };
                    changed_variables
                        .get(&source.variable)
                        .map_or(true, |target| *target == Some(source.target))
                });
            }
        }
        failures
    }

    /// Drop instance `id`'s subscriptions, queued calls and timers.
    pub fn unsubscribe(&mut self, id: &str) {
        if let Some(gone) = self.instances.remove(id) {
            if let Some(entity) = gone.entity {
                self.bridge.clear_timers_of(entity);
            }
            drop(gone.subscriptions);
            self.lock_calls().retain(|call| &*call.instance != id);
        }
    }

    /// Drop everything (session end): subscriptions, queued calls and
    /// timers; then drain the hub's queue (no script handler is left to
    /// receive it).
    pub fn clear(&mut self) {
        self.instances.clear();
        self.lock_calls().clear();
        self.bridge.clear_all_timers();
        self.bridge.hub().drain_queued();
    }

    /// Publish `LevelLoaded` once per session. `true` if it did now.
    pub fn announce_level(&mut self, level: &str) -> bool {
        if self.level_announced {
            return false;
        }
        self.level_announced = true;
        self.hub().publish(
            Channel::Global,
            pulsar_events::builtin::LevelLoaded {
                level: level.to_owned(),
            },
        );
        true
    }

    /// Run every queued handler call, in delivery order. Calls for
    /// instances that stopped since are skipped.
    pub fn run_calls(
        &mut self,
        runtime: &mut ScriptRuntime,
        world: &mut pulsar_scenedb::World,
    ) -> Vec<RuntimeError> {
        let calls = std::mem::take(&mut *self.lock_calls());
        let mut errors = Vec::new();
        for call in calls {
            if !self.instances.contains_key(&*call.instance) {
                continue;
            }
            if let Some(source) = call.component_source {
                if resolve_component_target(runtime, &call.instance, source.variable, world)
                    .ok()
                    .flatten()
                    != Some(source.target)
                {
                    continue;
                }
            }
            let signature = self.bridge.typed_signature(call.event.id).or_else(|| {
                self.bridge
                    .hub()
                    .descriptor(call.event.id)
                    .and_then(|descriptor| signature_of(&descriptor))
            });
            let event_name = signature
                .as_ref()
                .map(|signature| signature.name.as_str())
                .unwrap_or("<unknown>");
            let args = call
                .event
                .fields
                .iter()
                .take(call.params)
                .enumerate()
                .map(|(index, value)| {
                    to_value(
                        value,
                        signature
                            .as_ref()
                            .and_then(|sig| sig.fields.get(index))
                            .map(|field| &field.ty),
                    )
                })
                .collect::<Result<Vec<_>, _>>();
            let args = match args {
                Ok(args) => args,
                Err(error) => {
                    tracing::warn!(instance = %call.instance, event = %event_name, "event payload could not be decoded for script handler: {error}");
                    continue;
                }
            };
            if let Err(error) = runtime.call_function(&call.instance, call.handler, &args, world) {
                tracing::warn!("{error}");
                errors.push(error);
            }
        }
        errors
    }
}

fn make_subscription(
    hub: &EventHub,
    event: u64,
    channel: Channel,
    instance: Arc<str>,
    handler: FuncId,
    params: usize,
    component_source: Option<ComponentSubscriptionSource>,
    calls: CallQueue,
) -> SyncSubscription {
    hub.bus().subscribe_dyn(
        event,
        SubscribeOptions::channel(channel),
        move |event| {
            calls
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(PendingCall {
                    instance: Arc::clone(&instance),
                    handler,
                    params,
                    event: event.clone(),
                    component_source,
                });
        },
    )
}

/// Resolve a module variable index to its current live component reference.
/// The index is module slot order (as verified by the VM), while the runtime
/// API exposes variables by name; `class_variables` provides that mapping in
/// the same order. A target must match the declared component type and still
/// exist in SceneDB before its owner's entity channel is subscribed.
fn resolve_component_target(
    runtime: &ScriptRuntime,
    instance: &str,
    variable: u32,
    world: &World,
) -> Result<Option<ComponentRef>, String> {
    let class = runtime
        .class_of(instance)
        .ok_or_else(|| format!("script instance '{instance}' is no longer loaded"))?;
    let variables = runtime
        .class_variables(class)
        .ok_or_else(|| format!("script class '{class}' is no longer loaded"))?;
    let (name, ty) = variables
        .get(variable as usize)
        .ok_or_else(|| format!("component source variable {variable} is out of range"))?;
    let Type::Component(component_class) = ty else {
        return Err(format!("variable `{name}` has type {ty}, expected a component reference"));
    };
    let expected = TypeRegistry::global()
        .component(component_class)
        .map(|binding| binding.component_id())
        .ok_or_else(|| format!("component class `{component_class}` is not registered"))?;
    let Some(Value::Component(target)) = runtime.variable(instance, name) else {
        return Ok(None);
    };
    if target.component != expected {
        return Err(format!(
            "component source `{name}` refers to {:?}, expected `{component_class}` ({expected:?})",
            target.component
        ));
    }
    // `ComponentRef` carries only the typed SceneDB component identity; the
    // entity's generation and the exact live row are both checked here. A
    // reference to an owner entity by itself must never create a subscription.
    if !world.has_component(target.entity, expected) {
        return Ok(None);
    }
    Ok(Some(*target))
}
