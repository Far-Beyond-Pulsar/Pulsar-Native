//! Linking a verified [`Module`] against a [`NativeRegistry`] into a
//! runnable [`Program`].

use std::sync::Arc;

use crate::error::LinkError;
use crate::events::{check_handler, EventCatalog, EventSignature};
use crate::module::{Constant, EventRef, Module, SubscriptionScope};
use crate::native::{NativeFn, NativeRegistry};
use crate::types::{Type, TypeRegistry};
use crate::value::Value;
use crate::verify::verify;

/// A module bound to the natives it imports, ready to run. Holds its
/// natives (and so any library they come from) alive.
pub struct Program {
    module: Arc<Module>,
    pub(crate) natives: Vec<Arc<NativeFn>>,
    pub(crate) constants: Vec<Value>,
    /// Initial register values per function (defaults for every register).
    pub(crate) registers: Vec<Vec<Value>>,
    variables: Vec<Value>,
    subscriptions: Vec<LinkedSubscription>,
    generation: u64,
}

/// A module subscription after linking: the handler checked against the
/// event's fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkedSubscription {
    pub event: EventRef,
    /// The event's name, when known (always for events by name).
    pub event_name: Option<String>,
    /// The event's id, when the catalog knows it (0 is never an id).
    pub event_id: Option<u64>,
    pub handler: FuncId,
    pub scope: SubscriptionScope,
    /// Handler parameters: the first `params` fields of the event.
    pub params: usize,
}

/// Per-instance state of a program: its variables. Read and written
/// through [`Program::var`] / [`Program::set_var`], which keep every value
/// of its declared type.
/// Tied to its module, so it survives relinking (e.g. after a native
/// library reload) but cannot be used with another module's program.
#[derive(Clone, Debug)]
pub struct Instance {
    pub(crate) module: Arc<Module>,
    pub(crate) vars: Vec<Value>,
}

/// Index of a function in a program.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FuncId(pub u32);

impl Program {
    /// Verify `module`, check that every type it names exists, and bind its
    /// imports to `registry`'s natives (names and full signatures must
    /// match).
    ///
    /// Event handlers are checked against events the module declares only;
    /// use [`link_with_events`](Self::link_with_events) to check the rest.
    pub fn link(module: Arc<Module>, registry: &NativeRegistry) -> Result<Self, LinkError> {
        Self::link_with_events(module, registry, None)
    }

    /// [`link`](Self::link), also checking every subscription's handler
    /// against `events` (the engine's event catalog): an event neither the
    /// module declares nor the catalog knows is an error.
    pub fn link_with_events(
        module: Arc<Module>,
        registry: &NativeRegistry,
        events: Option<&dyn EventCatalog>,
    ) -> Result<Self, LinkError> {
        verify(&module)?;
        let types = TypeRegistry::global();
        let default = |ty: &Type| {
            types.default_value(ty).ok_or_else(|| LinkError::UnknownType { name: ty.to_string() })
        };

        let mut natives = Vec::with_capacity(module.imports.len());
        for import in &module.imports {
            for param in &import.sig.params {
                default(&param.ty)?;
            }
            default(&import.sig.ret)?;
            let native = match registry.get(&import.name) {
                Some(native) => Arc::clone(native),
                None => match registry.poly(&import.name) {
                    Some(poly) => Arc::new(
                        poly.instantiate(&import.sig)
                            .map_err(|message| LinkError::PolyNative { name: import.name.clone(), message })?,
                    ),
                    None => return Err(LinkError::MissingNative { name: import.name.clone() }),
                },
            };
            if native.sig != import.sig {
                return Err(LinkError::SignatureMismatch {
                    name: import.name.clone(),
                    expected: Box::new(import.sig.clone()),
                    found: Box::new(native.sig.clone()),
                });
            }
            natives.push(native);
        }

        let registers = module
            .functions
            .iter()
            .map(|f| {
                default(&f.ret)?;
                f.registers.iter().map(default).collect::<Result<Vec<_>, _>>()
            })
            .collect::<Result<Vec<_>, _>>()?;
        let variables = module
            .variables
            .iter()
            .map(|v| match &v.default {
                Some(constant) => Ok(constant_value(constant)),
                None => default(&v.ty),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let constants = module.constants.iter().map(constant_value).collect();
        let subscriptions = link_subscriptions(&module, events)?;

        Ok(Self { module, natives, constants, registers, variables, subscriptions, generation: registry.generation() })
    }

    /// The module's event subscriptions, checked.
    pub fn subscriptions(&self) -> &[LinkedSubscription] {
        &self.subscriptions
    }

    pub fn module(&self) -> &Arc<Module> {
        &self.module
    }

    /// The registry generation this was linked against. If the registry's
    /// generation has moved on (a library was reloaded), relink.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// A fresh instance with every variable at its default.
    pub fn instantiate(&self) -> Instance {
        Instance { module: Arc::clone(&self.module), vars: self.variables.clone() }
    }

    /// An exported function by name.
    pub fn entry(&self, name: &str) -> Option<FuncId> {
        self.module.function(name).filter(|(_, f)| f.exported).map(|(i, _)| FuncId(i))
    }

    /// Index of variable `name`.
    pub fn variable(&self, name: &str) -> Option<usize> {
        self.module.variables.iter().position(|v| v.name == name)
    }

    /// Variable `index` of `instance`.
    pub fn var<'i>(&self, instance: &'i Instance, index: usize) -> Option<&'i Value> {
        instance.vars.get(index)
    }

    /// Set variable `index` of `instance`, if `value` has its type.
    pub fn set_var(&self, instance: &mut Instance, index: usize, value: Value) -> Result<(), String> {
        let var = self.module.variables.get(index).ok_or_else(|| format!("no variable {index}"))?;
        let fits = match (&value, &var.ty) {
            (Value::Object(obj), Type::Object(name)) => obj.type_name() == name,
            (Value::Component(c), Type::Component(name)) => TypeRegistry::global()
                .component(name)
                .is_some_and(|b| b.component_id() == c.component),
            (value, ty) => value.fits(ty),
        };
        if !fits {
            return Err(format!("`{}` is {}, got {}", var.name, var.ty, value.kind()));
        }
        instance.vars[index] = value;
        Ok(())
    }
}

fn link_subscriptions(module: &Module, events: Option<&dyn EventCatalog>) -> Result<Vec<LinkedSubscription>, LinkError> {
    let mut linked = Vec::with_capacity(module.subscriptions.len());
    for subscription in &module.subscriptions {
        // The verifier checked the handler index.
        let handler = &module.functions[subscription.handler as usize];
        let declared = match &subscription.event {
            EventRef::Name(name) => module.events.iter().find(|e| &e.name == name).map(EventSignature::from),
            EventRef::Id(_) => None,
        };
        let known = events.and_then(|catalog| match &subscription.event {
            EventRef::Name(name) => catalog.event_by_name(name),
            EventRef::Id(id) => catalog.event_by_id(*id),
        });
        let signature = match (known, declared) {
            (Some(known), _) => Some(known),
            (None, Some(declared)) => Some(declared),
            (None, None) if events.is_some() => {
                return Err(LinkError::UnknownEvent { event: subscription.event.to_string() })
            }
            (None, None) => None,
        };
        if let Some(signature) = &signature {
            check_handler(&handler.params, &signature.field_types()).map_err(|message| LinkError::HandlerMismatch {
                event: signature.name.clone(),
                handler: handler.name.clone(),
                message,
            })?;
        }
        linked.push(LinkedSubscription {
            event: subscription.event.clone(),
            event_name: signature.as_ref().map(|s| s.name.clone()).or_else(|| match &subscription.event {
                EventRef::Name(name) => Some(name.clone()),
                EventRef::Id(_) => None,
            }),
            event_id: signature.as_ref().map(|s| s.id).filter(|id| *id != 0).or(match subscription.event {
                EventRef::Id(id) => Some(id),
                EventRef::Name(_) => None,
            }),
            handler: FuncId(subscription.handler),
            scope: subscription.scope,
            params: handler.params.len(),
        });
    }
    Ok(linked)
}

fn constant_value(constant: &Constant) -> Value {
    match constant {
        Constant::Bool(b) => Value::Bool(*b),
        Constant::Int(i) => Value::Int(*i),
        Constant::Float(f) => Value::Float(*f),
        Constant::Str(s) => Value::Str(s.as_str().into()),
    }
}
