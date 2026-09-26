//! How scripts meet engine events, without the VM knowing the event bus.
//!
//! The engine (the event hub in `pulsar_events`, wired up by
//! `pulsar_game::scripting`) implements two small traits:
//!
//! - [`EventCatalog`]: the registered events and their fields, so the
//!   [linker](crate::Program::link_with_events) can check a module's
//!   handlers against them;
//! - [`EventSink`]: what the `event::*` natives call to publish. Every
//!   publish is deferred by the engine and delivered at its next flush;
//!   natives never run handlers.
//!
//! A native reaches the sink through [`Host::events`](crate::Host::events).
//!
//! # The event natives
//!
//! | native | signature | target |
//! |---|---|---|
//! | `event::emit` | `(name: string, fields...)` | the global channel |
//! | `event::send` | `(target: entity, name: string, fields...)` | `target`'s entity channel |
//! | `event::emit_to_class` | `(class: string, name: string, fields...)` | the class channel of `class` (GUID or name) |
//! | `game::apply_damage` | `(target: entity, amount: float)` | `Damage` on `target`'s channel, instigated by `self` |
//! | `timer::set` | `(seconds: float, looping: bool) -> int` | `TimerFired` on `self`'s channel (global for unbound scripts) |
//! | `timer::clear` | `(timer: int)` | |
//!
//! `fields...` are the event's fields in order, any number of `bool`,
//! `int`, `float`, `string` or `entity` arguments: the three `event::*`
//! natives are *polymorphic* ([`crate::native::PolyNative`]); each import
//! names its own signature and the linker instantiates it. The arguments
//! are checked against the event's descriptor when the call runs; a
//! mismatch (unknown event, wrong count or type) fails the script call
//! with an error, it never panics.

use pulsar_scenedb::Entity;

use crate::module::{EventDecl, EventField};
use crate::types::Type;
use crate::value::Value;

/// An event as scripts see it: its stable id, name and typed fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventSignature {
    pub id: u64,
    pub name: String,
    pub fields: Vec<EventField>,
}

impl EventSignature {
    pub fn field_types(&self) -> Vec<Type> {
        self.fields.iter().map(|f| f.ty.clone()).collect()
    }
}

/// The events a module can subscribe to, besides those it declares.
pub trait EventCatalog {
    fn event_by_name(&self, name: &str) -> Option<EventSignature>;
    fn event_by_id(&self, id: u64) -> Option<EventSignature>;
}

/// Where a script-published event goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventTarget {
    Global,
    Entity(Entity),
    /// A class, by GUID or name.
    Class(String),
}

/// What the event natives publish through. Implemented by the engine.
pub trait EventSink: Send + Sync {
    /// Queue event `name` with `fields` for `target`. `Err` explains a
    /// refused call (unknown event, fields not matching its descriptor).
    fn emit(&self, target: EventTarget, name: &str, fields: &[Value]) -> Result<(), String>;

    /// Start a timer that publishes `TimerFired` to `owner` (global when
    /// `None`) after `seconds` of game time, repeatedly when `looping`.
    /// Returns its id.
    fn set_timer(&self, owner: Option<Entity>, seconds: f64, looping: bool) -> Result<i64, String> {
        let _ = (owner, seconds, looping);
        Err("timers are not available in this host".into())
    }

    /// Cancel a timer; `false` if it was not running.
    fn clear_timer(&self, timer: i64) -> bool {
        let _ = timer;
        false
    }
}

/// Whether `ty` can be an event field (and so a handler parameter).
pub fn is_event_field_type(ty: &Type) -> bool {
    matches!(ty, Type::Bool | Type::Int | Type::Float | Type::Str | Type::Entity)
}

/// Check `handler_params` against an event's field types: they must be a
/// prefix of them.
pub fn check_handler(handler_params: &[Type], fields: &[Type]) -> Result<(), String> {
    if handler_params.len() > fields.len() {
        return Err(format!(
            "the handler takes {} parameters, the event has {} fields",
            handler_params.len(),
            fields.len()
        ));
    }
    for (i, (param, field)) in handler_params.iter().zip(fields).enumerate() {
        if param != field {
            return Err(format!("parameter {i} is {param}, the event's field {i} is {field}"));
        }
    }
    Ok(())
}

impl From<&EventDecl> for EventSignature {
    /// A declared event's signature. Its `id` is 0: the engine assigns ids
    /// when it registers the event.
    fn from(decl: &EventDecl) -> Self {
        Self { id: 0, name: decl.name.clone(), fields: decl.fields.clone() }
    }
}

// ---------------------------------------------------------------------------
// The natives
// ---------------------------------------------------------------------------

use crate::error::ScriptError;
use crate::native::{Host, NativeFn, NativeRegistry, PolyNative};

const NO_HUB: &str = "no event hub is running (events are unavailable in this host)";

fn sink<'h>(host: &'h Host<'_>) -> Result<&'h dyn EventSink, ScriptError> {
    host.events.ok_or_else(|| ScriptError::native(NO_HUB))
}

fn str_arg(args: &[Value], index: usize) -> Result<String, ScriptError> {
    args.get(index)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ScriptError::native(format!("argument {index}: expected a string")))
}

fn emit(host: &Host<'_>, target: EventTarget, name: &str, fields: &[Value]) -> Result<Value, ScriptError> {
    sink(host)?.emit(target, name, fields).map_err(ScriptError::native)?;
    Ok(Value::Unit)
}

pub(crate) fn register(registry: &mut NativeRegistry) {
    let polys = [
        PolyNative::new("event::emit", vec![("event", Type::Str)], Type::Unit, |host, args| {
            let name = str_arg(args, 0)?;
            emit(host, EventTarget::Global, &name, &args[1..])
        })
        .doc("Broadcast an event on the global channel with its fields in order. Delivered at the next event flush."),
        PolyNative::new(
            "event::send",
            vec![("target", Type::Entity), ("event", Type::Str)],
            Type::Unit,
            |host, args| {
                let Some(Value::Entity(target)) = args.first().cloned() else {
                    return Err(ScriptError::native("argument 0: expected an entity"));
                };
                if target == Entity::DANGLING {
                    return Err(ScriptError::native("event::send: the target entity is none"));
                }
                let name = str_arg(args, 1)?;
                emit(host, EventTarget::Entity(target), &name, &args[2..])
            },
        )
        .doc("Send an event to one entity (its entity channel) with its fields in order. Delivered at the next event flush."),
        PolyNative::new(
            "event::emit_to_class",
            vec![("class", Type::Str), ("event", Type::Str)],
            Type::Unit,
            |host, args| {
                let class = str_arg(args, 0)?;
                let name = str_arg(args, 1)?;
                emit(host, EventTarget::Class(class), &name, &args[2..])
            },
        )
        .doc("Send an event to every instance of a class (GUID or name) listening on its class channel. Delivered at the next event flush."),
    ];
    for native in polys {
        if let Err(err) = registry.register_poly(native.attr("category", "Events")) {
            tracing::error!("script stdlib: {err}");
        }
    }

    let natives = [
        NativeFn::builder("game::apply_damage")
            .doc("Send `Damage` to `target` (its entity channel), instigated by this object.")
            .attr("category", "Events")
            .params(["target", "amount"])
            .build(|host: &mut Host<'_>, target: Entity, amount: f64| -> Result<(), String> {
                let instigator = host.bound_entity().unwrap_or(Entity::DANGLING);
                let fields = [Value::Entity(target), Value::Float(amount), Value::Entity(instigator)];
                host.events.ok_or(NO_HUB)?.emit(EventTarget::Entity(target), "Damage", &fields)
            }),
        NativeFn::builder("timer::set")
            .doc("Start a timer: `TimerFired(timer)` arrives on this object's channel (global for unbound scripts) after `seconds` of game time, and again every `seconds` when looping. Returns the timer id.")
            .attr("category", "Events")
            .params(["seconds", "looping"])
            .build(|host: &mut Host<'_>, seconds: f64, looping: bool| -> Result<i64, String> {
                let owner = host.bound_entity();
                host.events.ok_or(NO_HUB)?.set_timer(owner, seconds, looping)
            }),
        NativeFn::builder("timer::clear")
            .doc("Stop a timer started with timer::set. Returns false if it was not running.")
            .attr("category", "Events")
            .params(["timer"])
            .build(|host: &mut Host<'_>, timer: i64| -> Result<bool, String> {
                Ok(host.events.ok_or(NO_HUB)?.clear_timer(timer))
            }),
    ];
    for native in natives {
        if let Err(err) = registry.register(native) {
            tracing::error!("script stdlib: {err}");
        }
    }
}
