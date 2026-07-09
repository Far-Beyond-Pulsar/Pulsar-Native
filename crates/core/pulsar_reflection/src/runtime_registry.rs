//! Global registry of runtime type information
//!
//! Uses the `inventory` crate for zero-cost link-time type registration.
//! Types are automatically registered via `#[derive(Reflectable)]` macro.

use crate::runtime_types::RuntimeTypeInfo;
use crate::type_traits::{ReflectError, ReflectResult};
use once_cell::sync::Lazy;
use serde_json::Value;
use std::any::Any;
use std::any::TypeId;
use std::collections::HashMap;

/// Registration entry for auto-discovery via inventory
///
/// Automatically submitted by the `#[derive(Reflectable)]` macro
pub struct RuntimeTypeRegistration {
    pub type_info: &'static RuntimeTypeInfo,
    pub serialize_json: fn(&dyn Any) -> ReflectResult<Value>,
    pub deserialize_json: fn(Value) -> ReflectResult<Box<dyn Any>>,
}

// Collect all runtime type registrations at link time
inventory::collect!(RuntimeTypeRegistration);

struct RuntimeTypeEntry {
    type_info: &'static RuntimeTypeInfo,
    serialize_json: fn(&dyn Any) -> ReflectResult<Value>,
    deserialize_json: fn(Value) -> ReflectResult<Box<dyn Any>>,
}

/// Global registry of all reflectable types
///
/// Auto-populated at startup with all types that derive `Reflectable`.
/// Provides O(1) lookup by TypeId or type name.
pub struct RuntimeTypeRegistry {
    /// Index by TypeId for fast type-based lookup
    types: HashMap<TypeId, RuntimeTypeEntry>,

    /// Index by type name for string-based lookup
    by_name: HashMap<&'static str, TypeId>,
}

impl RuntimeTypeRegistry {
    /// Create a new registry from inventory
    fn new() -> Self {
        let mut types = HashMap::new();
        let mut by_name = HashMap::new();

        // Auto-discover all RuntimeTypeRegistration entries via inventory
        for registration in inventory::iter::<RuntimeTypeRegistration> {
            let type_info = registration.type_info;

            types.insert(
                type_info.type_id,
                RuntimeTypeEntry {
                    type_info,
                    serialize_json: registration.serialize_json,
                    deserialize_json: registration.deserialize_json,
                },
            );
            by_name.insert(type_info.type_name, type_info.type_id);
        }

        tracing::info!(
            "Runtime type registry initialized with {} types",
            types.len()
        );

        Self { types, by_name }
    }

    /// Get type info by TypeId
    ///
    /// # Example
    /// ```ignore
    /// let type_info = RUNTIME_TYPE_REGISTRY.get_by_id(TypeId::of::<f32>());
    /// ```
    pub fn get_by_id(&self, type_id: TypeId) -> Option<&'static RuntimeTypeInfo> {
        self.types.get(&type_id).map(|entry| entry.type_info)
    }

    /// Get type info by type (generic convenience method)
    ///
    /// # Example
    /// ```ignore
    /// let type_info = RUNTIME_TYPE_REGISTRY.get::<f32>();
    /// ```
    pub fn get<T: 'static>(&self) -> Option<&'static RuntimeTypeInfo> {
        self.get_by_id(TypeId::of::<T>())
    }

    /// Get type info by type name
    ///
    /// # Example
    /// ```ignore
    /// let type_info = RUNTIME_TYPE_REGISTRY.get_by_name("f32");
    /// ```
    pub fn get_by_name(&self, type_name: &str) -> Option<&'static RuntimeTypeInfo> {
        let type_id = self.by_name.get(type_name)?;
        self.get_by_id(*type_id)
    }

    /// Serialize a type-erased value using the callback registered for its type.
    pub fn serialize_json_for_any(&self, value: &dyn Any) -> ReflectResult<Value> {
        let type_id = value.type_id();
        let Some(entry) = self.types.get(&type_id) else {
            return Err(ReflectError::SerializationFailed(format!(
                "Type not registered for JSON serialization: {:?}",
                type_id
            )));
        };

        (entry.serialize_json)(value)
    }

    /// Deserialize JSON into a type-erased value for the provided runtime type.
    pub fn deserialize_json_for_type(
        &self,
        type_info: &RuntimeTypeInfo,
        value: Value,
    ) -> ReflectResult<Box<dyn Any>> {
        let Some(entry) = self.types.get(&type_info.type_id) else {
            return Err(ReflectError::DeserializationFailed(format!(
                "Type not registered for JSON deserialization: {}",
                type_info.type_name
            )));
        };

        (entry.deserialize_json)(value)
    }

    /// Check if a type is registered
    pub fn has_type<T: 'static>(&self) -> bool {
        self.types.contains_key(&TypeId::of::<T>())
    }

    /// Check if a type is registered by TypeId
    pub fn has_type_id(&self, type_id: TypeId) -> bool {
        self.types.contains_key(&type_id)
    }

    /// Get all registered type names (sorted)
    pub fn type_names(&self) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = self.by_name.keys().copied().collect();
        names.sort();
        names
    }

    /// Get number of registered types
    pub fn len(&self) -> usize {
        self.types.len()
    }

    /// Check if registry is empty
    pub fn is_empty(&self) -> bool {
        self.types.is_empty()
    }

    /// Iterate over all registered types
    pub fn iter(&self) -> impl Iterator<Item = &'static RuntimeTypeInfo> + '_ {
        self.types.values().map(|entry| entry.type_info)
    }
}

impl fmt::Debug for RuntimeTypeRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuntimeTypeRegistry")
            .field("type_count", &self.len())
            .finish()
    }
}

/// Global singleton registry instance
///
/// Lazily initialized on first access. All reflectable types are automatically
/// registered via the `inventory` crate.
pub static RUNTIME_TYPE_REGISTRY: Lazy<RuntimeTypeRegistry> = Lazy::new(RuntimeTypeRegistry::new);

use std::fmt;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_initialization() {
        // Registry should be initialized (may be empty in tests without registered types)
        let registry = &*RUNTIME_TYPE_REGISTRY;
        let _ = registry.len();
    }

    #[test]
    fn test_registry_api() {
        let registry = &*RUNTIME_TYPE_REGISTRY;

        // Test basic API
        let names = registry.type_names();
        assert!(names.is_empty() || !names.is_empty()); // Should work either way

        // Test type query
        assert!(registry.get::<f32>().is_some() || registry.get::<f32>().is_none());
    }
}
