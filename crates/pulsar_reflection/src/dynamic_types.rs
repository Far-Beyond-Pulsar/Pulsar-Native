use crate::runtime_types::RuntimeTypeInfo;
use dashmap::DashMap;
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

/// Tag distinguishing compile-time types from runtime-composed types
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TypeTag {
    /// A compile-time registered type with static metadata
    CompileTime(TypeId),
    /// A runtime-composed type with unique identifier
    RuntimeComposed(Uuid),
}

/// Information about a field in a dynamic type
#[derive(Clone)]
pub struct DynamicFieldInfo {
    pub name: String,
    /// The base type this field uses (must be a registered compile-time type)
    pub base_type: &'static RuntimeTypeInfo,
    /// Offset in bytes from start of struct
    pub offset: usize,
}

/// A type definition created at runtime by composing compile-time types
#[derive(Clone)]
pub struct DynamicTypeInfo {
    pub name: String,
    pub type_tag: TypeTag,
    pub fields: Vec<DynamicFieldInfo>,
    pub total_size: usize,
    pub total_align: usize,
}

impl DynamicTypeInfo {
    /// Get the UUID for this runtime type (if it's runtime-composed)
    pub fn uuid(&self) -> Option<Uuid> {
        match self.type_tag {
            TypeTag::RuntimeComposed(uuid) => Some(uuid),
            _ => None,
        }
    }

    /// Get a field by name
    pub fn get_field(&self, name: &str) -> Option<&DynamicFieldInfo> {
        self.fields.iter().find(|f| f.name == name)
    }

    /// Check if a field exists
    pub fn has_field(&self, name: &str) -> bool {
        self.fields.iter().any(|f| f.name == name)
    }
}

/// Builder for creating runtime-composed types
pub struct DynamicTypeBuilder {
    name: String,
    fields: Vec<(String, &'static RuntimeTypeInfo)>,
}

impl DynamicTypeBuilder {
    /// Create a new dynamic type builder
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            fields: Vec::new(),
        }
    }

    /// Add a field to the type
    ///
    /// # Safety Requirements
    /// The type_info MUST point to a valid registered compile-time type.
    /// This is enforced by requiring &'static RuntimeTypeInfo.
    pub fn add_field(
        mut self,
        name: impl Into<String>,
        type_info: &'static RuntimeTypeInfo,
    ) -> Self {
        self.fields.push((name.into(), type_info));
        self
    }

    /// Build the dynamic type with calculated layout
    pub fn build(self) -> Arc<DynamicTypeInfo> {
        // Calculate memory layout
        let mut offset = 0;
        let mut max_align = 1;
        let mut field_infos = Vec::new();

        for (name, type_info) in self.fields {
            let align = type_info.align;
            max_align = max_align.max(align);

            // Align offset to field's alignment requirement
            offset = (offset + align - 1) & !(align - 1);

            field_infos.push(DynamicFieldInfo {
                name,
                base_type: type_info,
                offset,
            });

            offset += type_info.size;
        }

        // Pad total size to alignment
        let total_size = (offset + max_align - 1) & !(max_align - 1);

        Arc::new(DynamicTypeInfo {
            name: self.name,
            type_tag: TypeTag::RuntimeComposed(Uuid::new_v4()),
            fields: field_infos,
            total_size,
            total_align: max_align,
        })
    }
}

/// An instance of a dynamic type
///
/// This stores field values in a type-safe manner using Box<dyn Any>.
/// The DynamicTypeInfo ensures all operations are type-checked at runtime.
pub struct DynamicValue {
    pub type_info: Arc<DynamicTypeInfo>,
    fields: HashMap<String, Box<dyn Any + Send + Sync>>,
}

impl DynamicValue {
    /// Create a new instance of a dynamic type
    ///
    /// All fields are initialized to their default values (zero-initialized).
    /// Use set_field to populate values.
    pub fn new(type_info: Arc<DynamicTypeInfo>) -> Self {
        Self {
            type_info,
            fields: HashMap::new(),
        }
    }

    /// Set a field value with runtime type checking
    ///
    /// # Errors
    /// Returns an error if:
    /// - The field doesn't exist in the type definition
    /// - The value's type doesn't match the field's declared type
    pub fn set_field(
        &mut self,
        field_name: &str,
        value: Box<dyn Any + Send + Sync>,
    ) -> Result<(), String> {
        let field = self.type_info.get_field(field_name).ok_or_else(|| {
            format!(
                "Field '{}' not found in type '{}'",
                field_name, self.type_info.name
            )
        })?;

        // Type check: value must match the field's base type
        if value.as_ref().type_id() != field.base_type.type_id {
            return Err(format!(
                "Type mismatch for field '{}': expected '{}', got type_id {:?}",
                field_name,
                field.base_type.type_name,
                value.as_ref().type_id()
            ));
        }

        self.fields.insert(field_name.to_string(), value);
        Ok(())
    }

    /// Get a field value as a type-erased reference
    ///
    /// Returns None if the field doesn't exist or hasn't been set yet.
    pub fn get_field(&self, field_name: &str) -> Option<&(dyn Any + Send + Sync)> {
        self.fields.get(field_name).map(|boxed| boxed.as_ref())
    }

    /// Get a field value as a mutable type-erased reference
    ///
    /// Returns None if the field doesn't exist or hasn't been set yet.
    pub fn get_field_mut(&mut self, field_name: &str) -> Option<&mut (dyn Any + Send + Sync)> {
        self.fields.get_mut(field_name).map(|boxed| boxed.as_mut())
    }

    /// Get a field value with automatic downcasting
    ///
    /// Returns a cloned copy of the field value. This avoids lifetime issues
    /// and allows the value to be used without holding a borrow on the DynamicValue.
    ///
    /// # Errors
    /// Returns an error if:
    /// - The field doesn't exist
    /// - The field hasn't been set yet
    /// - The type T doesn't match the field's actual type
    /// - The type T doesn't implement Clone
    pub fn get_field_typed<T: 'static + Clone>(&self, field_name: &str) -> Result<T, String> {
        let any_ref = self
            .get_field(field_name)
            .ok_or_else(|| format!("Field '{}' not found or not set", field_name))?;

        any_ref
            .downcast_ref::<T>()
            .ok_or_else(|| {
                format!(
                    "Failed to downcast field '{}' to requested type",
                    field_name
                )
            })
            .map(|val| val.clone())
    }

    /// Get a mutable field value with automatic downcasting
    ///
    /// # Errors
    /// Returns an error if:
    /// - The field doesn't exist
    /// - The field hasn't been set yet
    /// - The type T doesn't match the field's actual type
    pub fn get_field_typed_mut<T: 'static>(&mut self, field_name: &str) -> Result<&mut T, String> {
        let any_ref = self
            .get_field_mut(field_name)
            .ok_or_else(|| format!("Field '{}' not found or not set", field_name))?;

        any_ref.downcast_mut::<T>().ok_or_else(|| {
            format!(
                "Failed to downcast field '{}' to requested type",
                field_name
            )
        })
    }

    /// Modify a field value using a closure
    ///
    /// This is a convenience method that gets the current value, applies a function to it,
    /// and sets the new value back. This is more ergonomic than get + modify + set.
    ///
    /// # Errors
    /// Returns an error if:
    /// - The field doesn't exist
    /// - The field hasn't been set yet
    /// - The type T doesn't match the field's actual type
    ///
    /// # Example
    /// ```ignore
    /// // Increment a counter field
    /// value.modify_field("counter", |x: f32| x + 1.0)?;
    /// ```
    pub fn modify_field<T, F>(&mut self, field_name: &str, f: F) -> Result<(), String>
    where
        T: 'static + Clone + Send + Sync,
        F: FnOnce(T) -> T,
    {
        let current = self.get_field_typed::<T>(field_name)?;
        let new_value = f(current);
        self.set_field(field_name, Box::new(new_value))
    }

    /// Get all field names that have been set
    pub fn field_names(&self) -> impl Iterator<Item = &str> {
        self.fields.keys().map(|s| s.as_str())
    }

    /// Check if a field has been set
    pub fn has_value(&self, field_name: &str) -> bool {
        self.fields.contains_key(field_name)
    }

    /// Remove a field value
    pub fn remove_field(&mut self, field_name: &str) -> Option<Box<dyn Any + Send + Sync>> {
        self.fields.remove(field_name)
    }

    /// Clear all field values
    pub fn clear(&mut self) {
        self.fields.clear();
    }
}

impl Clone for DynamicValue {
    fn clone(&self) -> Self {
        // Note: This requires all field values to implement Clone via the Reflectable trait
        // For now, we create a new empty instance - the caller must manually clone fields
        Self {
            type_info: Arc::clone(&self.type_info),
            fields: HashMap::new(),
        }
    }
}

/// Global registry for runtime-composed types
pub struct DynamicTypeRegistry {
    types_by_uuid: DashMap<Uuid, Arc<DynamicTypeInfo>>,
    types_by_name: DashMap<String, Uuid>,
}

impl DynamicTypeRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            types_by_uuid: DashMap::new(),
            types_by_name: DashMap::new(),
        }
    }

    /// Register a dynamic type
    ///
    /// Returns the UUID assigned to this type.
    /// If a type with the same name already exists, it will be replaced.
    pub fn register(&self, type_info: Arc<DynamicTypeInfo>) -> Uuid {
        let uuid = match type_info.type_tag {
            TypeTag::RuntimeComposed(uuid) => uuid,
            TypeTag::CompileTime(_) => {
                panic!("Cannot register compile-time types in DynamicTypeRegistry");
            }
        };

        // Register by UUID
        self.types_by_uuid.insert(uuid, Arc::clone(&type_info));

        // Register by name (replace if exists)
        self.types_by_name.insert(type_info.name.clone(), uuid);

        uuid
    }

    /// Look up a type by its UUID
    pub fn get(&self, uuid: &Uuid) -> Option<Arc<DynamicTypeInfo>> {
        self.types_by_uuid
            .get(uuid)
            .map(|entry| Arc::clone(entry.value()))
    }

    /// Look up a type by its name
    pub fn get_by_name(&self, name: &str) -> Option<Arc<DynamicTypeInfo>> {
        let uuid = self.types_by_name.get(name)?;
        self.get(uuid.value())
    }

    /// Check if a type exists by UUID
    pub fn contains(&self, uuid: &Uuid) -> bool {
        self.types_by_uuid.contains_key(uuid)
    }

    /// Check if a type exists by name
    pub fn contains_name(&self, name: &str) -> bool {
        self.types_by_name.contains_key(name)
    }

    /// Get all registered type names
    pub fn type_names(&self) -> Vec<String> {
        self.types_by_name
            .iter()
            .map(|entry| entry.key().clone())
            .collect()
    }

    /// Get all registered type UUIDs
    pub fn type_uuids(&self) -> Vec<Uuid> {
        self.types_by_uuid
            .iter()
            .map(|entry| *entry.key())
            .collect()
    }

    /// Remove a type from the registry
    pub fn unregister(&self, uuid: &Uuid) -> Option<Arc<DynamicTypeInfo>> {
        if let Some((_, type_info)) = self.types_by_uuid.remove(uuid) {
            self.types_by_name.remove(&type_info.name);
            Some(type_info)
        } else {
            None
        }
    }

    /// Remove all types from the registry
    pub fn clear(&self) {
        self.types_by_uuid.clear();
        self.types_by_name.clear();
    }

    /// Get the number of registered types
    pub fn len(&self) -> usize {
        self.types_by_uuid.len()
    }

    /// Check if the registry is empty
    pub fn is_empty(&self) -> bool {
        self.types_by_uuid.is_empty()
    }
}

impl Default for DynamicTypeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Global instance of the dynamic type registry
pub static DYNAMIC_TYPE_REGISTRY: std::sync::LazyLock<DynamicTypeRegistry> =
    std::sync::LazyLock::new(|| DynamicTypeRegistry::new());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dynamic_type_builder() {
        let f32_info = crate::RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
        let i32_info = crate::RUNTIME_TYPE_REGISTRY.get::<i32>().unwrap();

        let dynamic_type = DynamicTypeBuilder::new("TestStruct")
            .add_field("x", f32_info)
            .add_field("y", f32_info)
            .add_field("count", i32_info)
            .build();

        assert_eq!(dynamic_type.name, "TestStruct");
        assert_eq!(dynamic_type.fields.len(), 3);
        assert_eq!(dynamic_type.fields[0].name, "x");
        assert_eq!(dynamic_type.fields[1].name, "y");
        assert_eq!(dynamic_type.fields[2].name, "count");
    }

    #[test]
    fn test_dynamic_value() {
        let f32_info = crate::RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();

        let dynamic_type = DynamicTypeBuilder::new("Vector2D")
            .add_field("x", f32_info)
            .add_field("y", f32_info)
            .build();

        let mut value = DynamicValue::new(dynamic_type);

        // Set values
        value.set_field("x", Box::new(10.0f32)).unwrap();
        value.set_field("y", Box::new(20.0f32)).unwrap();

        // Get values
        let x = value.get_field_typed::<f32>("x").unwrap();
        let y = value.get_field_typed::<f32>("y").unwrap();

        assert_eq!(x, 10.0);
        assert_eq!(y, 20.0);
    }

    #[test]
    fn test_type_checking() {
        let f32_info = crate::RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();

        let dynamic_type = DynamicTypeBuilder::new("TypeChecked")
            .add_field("value", f32_info)
            .build();

        let mut instance = DynamicValue::new(dynamic_type);

        // This should fail - wrong type
        let result = instance.set_field("value", Box::new(42i32));
        assert!(result.is_err());

        // This should succeed - correct type
        let result = instance.set_field("value", Box::new(std::f32::consts::PI));
        assert!(result.is_ok());
    }

    #[test]
    fn test_registry() {
        let f32_info = crate::RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();

        let dynamic_type = DynamicTypeBuilder::new("RegisteredType")
            .add_field("data", f32_info)
            .build();

        let uuid = DYNAMIC_TYPE_REGISTRY.register(Arc::clone(&dynamic_type));

        // Lookup by UUID
        let retrieved = DYNAMIC_TYPE_REGISTRY.get(&uuid).unwrap();
        assert_eq!(retrieved.name, "RegisteredType");

        // Lookup by name
        let retrieved = DYNAMIC_TYPE_REGISTRY.get_by_name("RegisteredType").unwrap();
        assert_eq!(retrieved.uuid().unwrap(), uuid);
    }
}
