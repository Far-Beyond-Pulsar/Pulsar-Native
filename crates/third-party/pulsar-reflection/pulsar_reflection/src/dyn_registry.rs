//! Name-keyed dynamic method dispatch for types that are *not* `EngineClass`
//! components — e.g. engine subsystem singletons (physics, audio, a
//! renderer) that don't want `EngineClass`'s spawn/property-panel
//! obligations (`get_properties`, `create_default`, `clone_boxed`).
//!
//! Deliberately additive and separate from [`crate::MethodCaller`] /
//! [`crate::ComponentMethodRegistration`] / [`crate::REGISTRY`]: those are
//! real, in-use call sites (component `begin_play` dispatch, notably —
//! see `pbgc::project`'s codegen template) keyed on `&mut dyn EngineClass`.
//! Generalizing that receiver in place to `&mut dyn Any` would be a
//! breaking change to live behavior for zero benefit; this module gets the
//! same link-time `inventory` registration pattern for a receiver that
//! doesn't (and shouldn't) implement `EngineClass`, without touching any
//! existing type or call site.
//!
//! Mirrors [`crate::registry::ComponentMethodRegistration`] /
//! [`crate::registry::EngineClassRegistry`] shape by design — same
//! `inventory::submit!` link-time collection, same name-keyed lookup — so
//! a caller who already knows that pattern needs nothing new to use this
//! one.

use std::any::Any;
use std::collections::HashMap;

use once_cell::sync::Lazy;

use crate::{MethodParameter, MethodReturnType, MethodType};

/// Boxed method args, reusing the same shape as [`crate::MethodArgs`]
/// (kept as its own type here rather than a re-export so this module has
/// no dependency on `EngineClass` at all, direct or otherwise).
pub type DynMethodArgs = Vec<Box<dyn Any>>;
pub type DynMethodReturnValue = Option<Box<dyn Any>>;

/// Like [`crate::MethodCaller`], except the receiver is `&mut dyn Any`
/// instead of `&mut dyn EngineClass` — callers downcast internally exactly
/// as today's `EngineClass` closures do (via `Any::downcast_mut`, not
/// `EngineClass::as_any_mut`).
pub type DynMethodCaller =
    Box<dyn Fn(&mut dyn Any, DynMethodArgs) -> DynMethodReturnValue + Send + Sync>;

/// Metadata for a single dynamically-dispatchable method on a non-`EngineClass`
/// receiver. Field-for-field parallel to [`crate::MethodMetadata`] other than
/// the caller's receiver type.
pub struct DynMethodMetadata {
    pub name: &'static str,
    pub display_name: String,
    pub category: Option<&'static str>,
    pub params: Vec<MethodParameter>,
    pub return_type: Option<MethodReturnType>,
    pub method_type: MethodType,
    pub caller: DynMethodCaller,
}

/// Registration entry for a named receiver's dynamically-dispatchable
/// methods, submitted via `inventory::submit!` (by hand today; a
/// `#[dyn_methods]`-style derive can generate this later the same way
/// `#[component_methods]` generates [`crate::ComponentMethodRegistration`]).
pub struct DynMethodRegistration {
    /// Registry key — e.g. a subsystem's name. Not tied to any Rust type
    /// name; multiple registrations may target the same receiver type
    /// under different names if that's ever useful.
    pub receiver_name: &'static str,
    pub methods: fn() -> Vec<DynMethodMetadata>,
}

inventory::collect!(DynMethodRegistration);

/// Global, name-keyed lookup over every [`DynMethodRegistration`] collected
/// at link time. Mirrors [`crate::registry::EngineClassRegistry::get_methods`]
/// /`get_method`'s shape.
pub struct DynMethodRegistry {
    receivers: HashMap<&'static str, Vec<&'static str>>,
}

impl DynMethodRegistry {
    fn new() -> Self {
        let mut receivers: HashMap<&'static str, Vec<&'static str>> = HashMap::new();
        for registration in inventory::iter::<DynMethodRegistration> {
            let names = receivers.entry(registration.receiver_name).or_default();
            for method in (registration.methods)() {
                names.push(method.name);
            }
        }
        Self { receivers }
    }

    /// Get every method registered for `receiver_name`. Re-invokes each
    /// registration's `methods` fn (same non-caching tradeoff as
    /// `EngineClassRegistry::get_methods`) so [`DynMethodCaller`] closures
    /// never need to be `Clone`.
    pub fn get_methods(&self, receiver_name: &str) -> Option<Vec<DynMethodMetadata>> {
        if !self.receivers.contains_key(receiver_name) {
            return None;
        }
        let mut all = Vec::new();
        for registration in inventory::iter::<DynMethodRegistration> {
            if registration.receiver_name == receiver_name {
                all.extend((registration.methods)());
            }
        }
        Some(all)
    }

    /// Get one named method for `receiver_name`.
    pub fn get_method(&self, receiver_name: &str, method_name: &str) -> Option<DynMethodMetadata> {
        self.get_methods(receiver_name)?
            .into_iter()
            .find(|m| m.name == method_name)
    }

    /// Look up and invoke a method by name in one call.
    ///
    /// Returns `Err` distinguishing "no such receiver/method registered"
    /// from "found it" so callers don't have to pattern-match `Option`
    /// against a caller they can't otherwise construct.
    pub fn invoke(
        &self,
        receiver_name: &str,
        method_name: &str,
        target: &mut dyn Any,
        args: DynMethodArgs,
    ) -> Result<DynMethodReturnValue, DynDispatchError> {
        let method = self
            .get_method(receiver_name, method_name)
            .ok_or_else(|| DynDispatchError::NotFound {
                receiver_name: receiver_name.to_string(),
                method_name: method_name.to_string(),
            })?;
        Ok((method.caller)(target, args))
    }

    pub fn receiver_names(&self) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = self.receivers.keys().copied().collect();
        names.sort();
        names
    }
}

#[derive(Debug, Clone)]
pub enum DynDispatchError {
    NotFound {
        receiver_name: String,
        method_name: String,
    },
}

impl std::fmt::Display for DynDispatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound {
                receiver_name,
                method_name,
            } => write!(
                f,
                "no method '{method_name}' registered for receiver '{receiver_name}'"
            ),
        }
    }
}

impl std::error::Error for DynDispatchError {}

/// Global singleton, lazily built from every [`DynMethodRegistration`]
/// collected at link time — the `dyn`-receiver counterpart to
/// [`crate::registry::REGISTRY`].
pub static DYN_METHOD_REGISTRY: Lazy<DynMethodRegistry> = Lazy::new(DynMethodRegistry::new);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_types::RuntimeTypeInfo;

    struct Widget {
        count: u32,
    }

    fn widget_methods() -> Vec<DynMethodMetadata> {
        vec![DynMethodMetadata {
            name: "bump",
            display_name: "Bump".to_string(),
            category: None,
            params: Vec::new(),
            return_type: None,
            method_type: MethodType::Fn,
            caller: Box::new(|target: &mut dyn Any, _args: DynMethodArgs| {
                let widget = target.downcast_mut::<Widget>().expect("downcast");
                widget.count += 1;
                None
            }),
        }]
    }

    inventory::submit! {
        DynMethodRegistration {
            receiver_name: "widget",
            methods: widget_methods,
        }
    }

    #[test]
    fn invoke_by_name_reaches_the_real_instance() {
        let registry = DynMethodRegistry::new();
        let mut widget = Widget { count: 0 };

        assert!(registry.get_method("widget", "bump").is_some());
        assert!(registry.get_method("widget", "missing").is_none());
        assert!(matches!(
            registry.invoke("missing-receiver", "bump", &mut widget, vec![]),
            Err(DynDispatchError::NotFound { .. })
        ));

        registry
            .invoke("widget", "bump", &mut widget, vec![])
            .expect("dispatch succeeds");
        assert_eq!(widget.count, 1);
    }

    // Keeps `RuntimeTypeInfo` imported for parity with `MethodParameter`'s
    // shape (params: Vec<MethodParameter { type_info: &'static
    // RuntimeTypeInfo, .. }>), documenting the intended construction path
    // without requiring a real one for this smoke test.
    #[allow(dead_code)]
    fn _type_shape_reference(_: &'static RuntimeTypeInfo) {}
}
