//! ComponentStore — per-actor runtime storage for EngineClass component instances.
//!
//! Each actor spawned in the game holds one `ComponentStore`. Components are created
//! via the reflection registry (`REGISTRY`) using their class name, with optional
//! property overrides supplied as JSON (matching the prefab asset format).
//!
//! # Blueprint integration
//!
//! Blueprint logic functions access the component store through a thread-local
//! "execution context" pointer that the Actor sets before invoking graph code:
//!
//! ```text
//! Actor::begin_play()
//!   └─ __bp_set_comp_ctx(ptr)
//!        └─ logic::begin_play()  ← reads/writes via __bp_with_comp()
//!   └─ __bp_clear_comp_ctx()
//! ```
//!
//! This approach keeps PBGC-generated logic functions as plain free functions (no
//! `&mut self` threading) while still giving them full access to component state.

use pulsar_reflection::{EngineClass, REGISTRY, RUNTIME_TYPE_REGISTRY};
use std::cell::Cell;

// ─── Thread-local execution context ──────────────────────────────────────────

thread_local! {
    /// Raw pointer to the current actor's `ComponentStore`.
    ///
    /// Set by `__bp_set_comp_ctx` immediately before any blueprint logic call and
    /// cleared by `__bp_clear_comp_ctx` as soon as the call returns. The PBGC-
    /// generated glue in the `vars` module wraps this via `__bp_with_comp`.
    static BP_COMP_CTX: Cell<usize> = Cell::new(0);
}

/// Set the component store execution context.
///
/// # Safety
/// `store` must remain valid for the duration of any blueprint logic call made
/// while this context is set. The caller MUST call `__bp_clear_comp_ctx` before
/// the borrow of `store` expires.
#[inline]
pub fn __bp_set_comp_ctx(store: &mut ComponentStore) {
    BP_COMP_CTX.with(|c| c.set(store as *mut ComponentStore as usize));
}

/// Clear the component store execution context.
///
/// Must be called after every blueprint logic invocation to prevent stale pointers.
#[inline]
pub fn __bp_clear_comp_ctx() {
    BP_COMP_CTX.with(|c| c.set(0));
}

/// Run `f` with mutable access to the current execution context's component store.
///
/// Panics if called outside an actor lifecycle call (i.e., context is null).
#[inline]
pub fn __bp_with_comp<R>(f: impl FnOnce(&mut ComponentStore) -> R) -> R {
    BP_COMP_CTX.with(|c| {
        let ptr = c.get() as *mut ComponentStore;
        assert!(
            !ptr.is_null(),
            "Blueprint component access outside Actor lifecycle — \
             did you call a component node from a non-actor blueprint?"
        );
        // SAFETY: pointer is set by __bp_set_comp_ctx which receives a valid &mut
        // with a lifetime that spans this call.
        unsafe { f(&mut *ptr) }
    })
}

// ─── ComponentStore ───────────────────────────────────────────────────────────

/// Holds all component instances attached to a single actor.
///
/// Each entry is `(class_name, Box<dyn EngineClass>)`. Class names are the
/// same strings used in the reflection registry (e.g., `"PhysicsComponent"`).
pub struct ComponentStore {
    entries: Vec<(String, Box<dyn EngineClass>)>,
}

impl Default for ComponentStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ComponentStore {
    /// Create an empty store.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    // ── Construction ─────────────────────────────────────────────────────────

    /// Create and add a component by class name, applying JSON property overrides.
    ///
    /// Property values in `data` are applied via the reflection setter for each
    /// matching property name. Unknown keys are silently ignored.
    ///
    /// Returns `true` on success, `false` if the class is not in the registry.
    pub fn add_from_registry(&mut self, class_name: &str, data: &serde_json::Value) -> bool {
        let Some(mut instance) = REGISTRY.create_instance(class_name) else {
            tracing::warn!(
                "ComponentStore: unknown class '{}' — not in reflection registry",
                class_name
            );
            return false;
        };

        if let Some(obj) = data.as_object() {
            // Collect (type_info, setter, json_value) triples first so we
            // don't hold overlapping borrows on `instance`.
            let apply_list: Vec<_> = {
                let props = instance.get_properties();
                props
                    .into_iter()
                    .filter_map(|prop| {
                        obj.get(prop.name)
                            .cloned()
                            .map(|jv| (prop.type_info, prop.setter, jv))
                    })
                    .collect()
            };

            for (type_info, setter, json_val) in apply_list {
                match RUNTIME_TYPE_REGISTRY.deserialize_json_for_type(type_info, json_val) {
                    Ok(any_val) => (setter)(instance.as_mut(), any_val),
                    Err(e) => {
                        tracing::warn!(
                            "ComponentStore: failed to apply property on '{}': {}",
                            class_name,
                            e
                        );
                    }
                }
            }
        }

        self.entries.push((class_name.to_string(), instance));
        true
    }

    /// Add a pre-constructed component.
    pub fn add_boxed(&mut self, class_name: impl Into<String>, comp: Box<dyn EngineClass>) {
        self.entries.push((class_name.into(), comp));
    }

    // ── Typed access ─────────────────────────────────────────────────────────

    /// Get an immutable reference to the first component of type `T`.
    pub fn get<T: EngineClass + 'static>(&self) -> Option<&T> {
        self.entries
            .iter()
            .find_map(|(_, e)| e.as_any().downcast_ref::<T>())
    }

    /// Get a mutable reference to the first component of type `T`.
    pub fn get_mut<T: EngineClass + 'static>(&mut self) -> Option<&mut T> {
        self.entries
            .iter_mut()
            .find_map(|(_, e)| e.as_any_mut().downcast_mut::<T>())
    }

    // ── By-name access ───────────────────────────────────────────────────────

    /// Get an immutable reference to the first component with the given class name.
    pub fn get_by_name(&self, class_name: &str) -> Option<&dyn EngineClass> {
        self.entries
            .iter()
            .find(|(name, _)| name == class_name)
            .map(|(_, e)| e.as_ref())
    }

    /// Get a mutable reference to the first component with the given class name.
    pub fn get_by_name_mut(&mut self, class_name: &str) -> Option<&mut dyn EngineClass> {
        self.entries
            .iter_mut()
            .find(|(name, _)| name == class_name)
            .map(|(_, e)| e.as_mut())
    }

    // ── Reflection property access ────────────────────────────────────────────

    /// Read a property from a component by class name and property name.
    ///
    /// Returns the value serialized as `serde_json::Value`, or `None` if the
    /// class or property is not found.
    pub fn get_property_json(
        &self,
        class_name: &str,
        prop_name: &str,
    ) -> Option<serde_json::Value> {
        let (_, comp) = self.entries.iter().find(|(name, _)| name == class_name)?;

        let props = comp.get_properties();
        let prop = props.into_iter().find(|p| p.name == prop_name)?;
        let any_val: Box<dyn std::any::Any> = (prop.getter)(comp.as_ref());
        RUNTIME_TYPE_REGISTRY
            .serialize_json_for_any(any_val.as_ref())
            .ok()
    }

    /// Write a property on a component by class name and property name.
    ///
    /// Returns `true` on success, `false` if the class, property, or type
    /// deserialization fails.
    pub fn set_property_json(
        &mut self,
        class_name: &str,
        prop_name: &str,
        value: serde_json::Value,
    ) -> bool {
        // Find entry index so we can split the borrow.
        let Some(idx) = self.entries.iter().position(|(name, _)| name == class_name) else {
            return false;
        };

        // Phase 1: extract setter + type_info via shared borrow.
        let (type_info, setter) = {
            let comp_ref = self.entries[idx].1.as_ref();
            let props = comp_ref.get_properties();
            match props.into_iter().find(|p| p.name == prop_name) {
                Some(prop) => (prop.type_info, prop.setter),
                None => return false,
            }
        };

        // Phase 2: deserialize.
        let any_val = match RUNTIME_TYPE_REGISTRY.deserialize_json_for_type(type_info, value) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    "ComponentStore::set_property_json failed for {}.{}: {}",
                    class_name,
                    prop_name,
                    e
                );
                return false;
            }
        };

        // Phase 3: apply via mutable borrow.
        let comp_mut = self.entries[idx].1.as_mut();
        (setter)(comp_mut, any_val);
        true
    }

    // ── Method invocation ────────────────────────────────────────────────────

    /// Call a blueprint-registered method on a component by class name and method name.
    ///
    /// `args` are passed in order as JSON values. Returns the return value as JSON,
    /// or `None` for void methods or if the component/method is not found.
    pub fn call_method_json(
        &mut self,
        class_name: &str,
        method_name: &str,
        args: Vec<serde_json::Value>,
    ) -> Option<serde_json::Value> {
        let methods = REGISTRY.get_methods(class_name)?;
        let method = methods.into_iter().find(|m| m.name == method_name)?;

        let idx = self
            .entries
            .iter()
            .position(|(name, _)| name == class_name)?;

        // Phase 1: deserialize args (no mut borrow of self.entries needed)
        let mut any_args: Vec<Box<dyn std::any::Any>> = Vec::new();
        for (param, json_val) in method.params.iter().zip(args.into_iter()) {
            match RUNTIME_TYPE_REGISTRY.deserialize_json_for_type(param.type_info, json_val) {
                Ok(v) => any_args.push(v),
                Err(e) => {
                    tracing::warn!("ComponentStore::call_method_json arg error: {}", e);
                    return None;
                }
            }
        }

        // Phase 2: invoke
        let comp_mut = self.entries[idx].1.as_mut();
        let result = (method.caller)(comp_mut, any_args);

        result.and_then(|rv| {
            RUNTIME_TYPE_REGISTRY
                .serialize_json_for_any(rv.as_ref())
                .ok()
        })
    }

    // ── Queries ───────────────────────────────────────────────────────────────

    /// Check whether this store contains a component with the given class name.
    pub fn has(&self, class_name: &str) -> bool {
        self.entries.iter().any(|(name, _)| name == class_name)
    }

    /// Return the number of components in this store.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Return `true` if no components are present.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate over `(class_name, &dyn EngineClass)` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &dyn EngineClass)> {
        self.entries.iter().map(|(n, e)| (n.as_str(), e.as_ref()))
    }

    /// Iterate mutably over `(class_name, &mut dyn EngineClass)` pairs.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&str, &mut dyn EngineClass)> {
        self.entries
            .iter_mut()
            .map(|(n, e)| (n.as_str(), e.as_mut()))
    }
}
