//! JSON serializer and deserializer implementations.
//!
//! This module does not maintain per-type conversion tables. Type-specific
//! JSON logic is resolved through runtime type registrations.

use crate::runtime_registry::RUNTIME_TYPE_REGISTRY;
use crate::runtime_types::{FieldInfo, RuntimeTypeInfo};
use crate::type_traits::{ReflectError, ReflectResult, TypeDeserializer, TypeSerializer};
use serde_json::Value;
use std::any::Any;
use std::collections::HashMap;

/// JSON serializer implementation.
pub struct JsonSerializer {
    value: Value,
}

impl JsonSerializer {
    /// Create a new JSON serializer.
    pub fn new() -> Self {
        Self { value: Value::Null }
    }

    /// Consume the serializer and return the JSON value.
    pub fn into_json(self) -> Value {
        self.value
    }

    /// Get a reference to the current JSON value.
    pub fn as_json(&self) -> &Value {
        &self.value
    }
}

impl Default for JsonSerializer {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeSerializer for JsonSerializer {
    fn serialize_registered(&mut self, value: &dyn Any) -> ReflectResult<()> {
        self.value = RUNTIME_TYPE_REGISTRY.serialize_json_for_any(value)?;
        Ok(())
    }

    fn serialize_array(
        &mut self,
        values: &[&dyn Any],
        _element_type: &RuntimeTypeInfo,
    ) -> ReflectResult<()> {
        let array = values
            .iter()
            .map(|value| RUNTIME_TYPE_REGISTRY.serialize_json_for_any(*value))
            .collect::<ReflectResult<Vec<_>>>()?;

        self.value = Value::Array(array);
        Ok(())
    }

    fn serialize_struct(&mut self, fields: &[(&str, &dyn Any)]) -> ReflectResult<()> {
        let mut map = serde_json::Map::new();

        for (name, value) in fields {
            let json_value = RUNTIME_TYPE_REGISTRY.serialize_json_for_any(*value)?;
            map.insert((*name).to_string(), json_value);
        }

        self.value = Value::Object(map);
        Ok(())
    }

    fn serialize_enum(&mut self, _variant_name: &str, variant_index: usize) -> ReflectResult<()> {
        self.value = serde_json::json!(variant_index as u64);
        Ok(())
    }
}

/// JSON deserializer implementation.
pub struct JsonDeserializer {
    value: Value,
}

impl JsonDeserializer {
    /// Create a new JSON deserializer from a JSON value.
    pub fn new(value: Value) -> Self {
        Self { value }
    }

    /// Create a new JSON deserializer from a JSON string.
    pub fn from_str(json: &str) -> ReflectResult<Self> {
        let value = serde_json::from_str(json)
            .map_err(|e| ReflectError::DeserializationFailed(e.to_string()))?;
        Ok(Self { value })
    }
}

impl TypeDeserializer for JsonDeserializer {
    fn deserialize_registered(
        &mut self,
        type_info: &RuntimeTypeInfo,
    ) -> ReflectResult<Box<dyn Any>> {
        RUNTIME_TYPE_REGISTRY.deserialize_json_for_type(type_info, self.value.clone())
    }

    fn deserialize_array(
        &mut self,
        element_type: &RuntimeTypeInfo,
    ) -> ReflectResult<Vec<Box<dyn Any>>> {
        let arr = self
            .value
            .as_array()
            .ok_or_else(|| ReflectError::TypeMismatch {
                expected: "array",
                found: format!("{:?}", self.value),
            })?;

        arr.iter()
            .map(|item| RUNTIME_TYPE_REGISTRY.deserialize_json_for_type(element_type, item.clone()))
            .collect()
    }

    fn deserialize_struct(
        &mut self,
        fields: &[FieldInfo],
    ) -> ReflectResult<HashMap<&'static str, Box<dyn Any>>> {
        let obj = self
            .value
            .as_object()
            .ok_or_else(|| ReflectError::TypeMismatch {
                expected: "object",
                found: format!("{:?}", self.value),
            })?;

        let mut result = HashMap::new();

        for field in fields {
            let field_value = obj
                .get(field.name)
                .ok_or_else(|| ReflectError::MissingField {
                    struct_name: "unknown",
                    field_name: field.name,
                })?;

            let value = RUNTIME_TYPE_REGISTRY
                .deserialize_json_for_type(field.type_info, field_value.clone())?;
            result.insert(field.name, value);
        }

        Ok(result)
    }

    fn deserialize_enum(&mut self, variants: &[&'static str]) -> ReflectResult<usize> {
        if let Some(index) = self.value.as_u64() {
            let index = index as usize;
            if index < variants.len() {
                return Ok(index);
            }
        }

        if let Some(name) = self.value.as_str() {
            if let Some(index) = variants.iter().position(|&v| v == name) {
                return Ok(index);
            }
        }

        Err(ReflectError::InvalidVariant {
            enum_name: "unknown",
            variant: format!("{:?}", self.value),
        })
    }
}
