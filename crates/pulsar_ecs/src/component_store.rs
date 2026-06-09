use pulsar_reflection::{EngineClass, REGISTRY, RUNTIME_TYPE_REGISTRY};
use std::cell::Cell;

thread_local! {
    static BP_COMP_CTX: Cell<usize> = Cell::new(0);
}

/// Set the thread-local blueprint component context to `store`.
///
/// Called by the blueprint executor before dispatching actor lifecycle
/// hooks so that [`__bp_with_comp`] resolves correctly.
#[inline]
pub fn __bp_set_comp_ctx(store: &mut ComponentStore) {
    BP_COMP_CTX.with(|c| c.set(store as *mut ComponentStore as usize));
}

/// Clear the thread-local blueprint component context.
///
/// Called by the blueprint executor after an actor lifecycle hook returns.
#[inline]
pub fn __bp_clear_comp_ctx() {
    BP_COMP_CTX.with(|c| c.set(0));
}

/// Access the current blueprint component store from thread-local context.
///
/// # Panics
///
/// Panics if called outside a `__bp_set_comp_ctx` / `__bp_clear_comp_ctx`
/// pair (i.e., outside an actor lifecycle hook).
#[inline]
pub fn __bp_with_comp<R>(f: impl FnOnce(&mut ComponentStore) -> R) -> R {
    BP_COMP_CTX.with(|c| {
        let ptr = c.get() as *mut ComponentStore;
        assert!(
            !ptr.is_null(),
            "Blueprint component access outside Actor lifecycle"
        );
        unsafe { f(&mut *ptr) }
    })
}

/// Runtime store for blueprint (visual scripting) components attached to an
/// actor or object.
///
/// Each entry is a `(class_name, Box<dyn EngineClass>)` pair.  The
/// `EngineClass` trait comes from `pulsar_reflection` and provides
/// reflection-based property get/set and method dispatch via JSON.
///
/// This is the bridge between the ECS world and the blueprint runtime:
/// blueprint instances read and write their reflected properties through
/// a `ComponentStore` rather than through direct ECS column access.
///
/// The thread-local accessor functions ([`__bp_set_comp_ctx`],
/// [`__bp_clear_comp_ctx`], [`__bp_with_comp`]) let blueprint VM bytecode
/// operate on the *current* actor's store without plumbing it through every
/// call site.
pub struct ComponentStore {
    entries: Vec<(String, Box<dyn EngineClass>)>,
}

impl Default for ComponentStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ComponentStore {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Create a component from the reflection registry and deserialize its
    /// properties from a JSON map.
    ///
    /// Returns `false` if `class_name` is not registered in the reflection
    /// registry.
    pub fn add_from_registry(&mut self, class_name: &str, data: &serde_json::Value) -> bool {
        let Some(mut instance) = REGISTRY.create_instance(class_name) else {
            tracing::warn!(
                "ComponentStore: unknown class '{}' — not in reflection registry",
                class_name
            );
            return false;
        };

        if let Some(obj) = data.as_object() {
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

    /// Add a pre-constructed engine-class instance.
    pub fn add_boxed(&mut self, class_name: impl Into<String>, comp: Box<dyn EngineClass>) {
        self.entries.push((class_name.into(), comp));
    }

    /// Get a shared reference to the first component of type `T`.
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

    /// Get a shared reference to a component by its registered class name.
    pub fn get_by_name(&self, class_name: &str) -> Option<&dyn EngineClass> {
        self.entries
            .iter()
            .find(|(name, _)| name == class_name)
            .map(|(_, e)| e.as_ref())
    }

    /// Get a mutable reference to a component by its registered class name.
    pub fn get_by_name_mut(&mut self, class_name: &str) -> Option<&mut dyn EngineClass> {
        self.entries
            .iter_mut()
            .find(|(name, _)| name == class_name)
            .map(|(_, e)| e.as_mut())
    }

    /// Serialize a component property to JSON.
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

    /// Deserialize a JSON value into a component property.
    ///
    /// Returns `false` if `class_name` or `prop_name` is not found.
    pub fn set_property_json(
        &mut self,
        class_name: &str,
        prop_name: &str,
        value: serde_json::Value,
    ) -> bool {
        let Some(idx) = self.entries.iter().position(|(name, _)| name == class_name) else {
            return false;
        };

        let (type_info, setter) = {
            let comp_ref = self.entries[idx].1.as_ref();
            let props = comp_ref.get_properties();
            match props.into_iter().find(|p| p.name == prop_name) {
                Some(prop) => (prop.type_info, prop.setter),
                None => return false,
            }
        };

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

        let comp_mut = self.entries[idx].1.as_mut();
        (setter)(comp_mut, any_val);
        true
    }

    /// Call a reflected method on a component with JSON-serialized arguments.
    ///
    /// Returns the JSON-serialized return value, if any.
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

        let comp_mut = self.entries[idx].1.as_mut();
        let result = (method.caller)(comp_mut, any_args);

        result.and_then(|rv| {
            RUNTIME_TYPE_REGISTRY
                .serialize_json_for_any(rv.as_ref())
                .ok()
        })
    }

    /// Returns `true` if a component with `class_name` is stored.
    pub fn has(&self, class_name: &str) -> bool {
        self.entries.iter().any(|(name, _)| name == class_name)
    }

    /// Number of component entries in this store.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` if the store is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate over `(class_name, component)` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &dyn EngineClass)> {
        self.entries.iter().map(|(n, e)| (n.as_str(), e.as_ref()))
    }

    /// Iterate mutably over `(class_name, component)` pairs.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&str, &mut dyn EngineClass)> {
        self.entries
            .iter_mut()
            .map(|(n, e)| (n.as_str(), e.as_mut()))
    }
}
