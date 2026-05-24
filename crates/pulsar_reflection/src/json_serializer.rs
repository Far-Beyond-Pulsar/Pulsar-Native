//! JSON serialization implementation for runtime types
//!
//! Provides JSON-based TypeSerializer and TypeDeserializer implementations.
//! Compatible with existing prefab asset format.

use crate::runtime_types::{FieldInfo, RuntimeTypeInfo, TypeStructure};
use crate::type_traits::{ReflectError, ReflectResult, TypeDeserializer, TypeSerializer};
use serde_json::Value;
use std::any::Any;
use std::collections::HashMap;

/// JSON serializer implementation
pub struct JsonSerializer {
    value: Value,
}

impl JsonSerializer {
    /// Create a new JSON serializer
    pub fn new() -> Self {
        Self { value: Value::Null }
    }

    /// Consume the serializer and return the JSON value
    pub fn into_json(self) -> Value {
        self.value
    }

    /// Get a reference to the current JSON value
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
    fn serialize_f32(&mut self, value: f32) -> ReflectResult<()> {
        self.value = serde_json::json!(value);
        Ok(())
    }

    fn serialize_i32(&mut self, value: i32) -> ReflectResult<()> {
        self.value = serde_json::json!(value);
        Ok(())
    }

    fn serialize_u64(&mut self, value: u64) -> ReflectResult<()> {
        self.value = serde_json::json!(value);
        Ok(())
    }

    fn serialize_bool(&mut self, value: bool) -> ReflectResult<()> {
        self.value = serde_json::json!(value);
        Ok(())
    }

    fn serialize_string(&mut self, value: &str) -> ReflectResult<()> {
        self.value = serde_json::json!(value);
        Ok(())
    }

    fn serialize_vec3(&mut self, value: [f32; 3]) -> ReflectResult<()> {
        self.value = serde_json::json!([value[0], value[1], value[2]]);
        Ok(())
    }

    fn serialize_color(&mut self, value: [f32; 4]) -> ReflectResult<()> {
        self.value = serde_json::json!([value[0], value[1], value[2], value[3]]);
        Ok(())
    }

    fn serialize_array(
        &mut self,
        values: &[&dyn Any],
        _element_type: &RuntimeTypeInfo,
    ) -> ReflectResult<()> {
        let array: Vec<Value> = values
            .iter()
            .map(|v| {
                // Try to downcast to known types
                if let Some(f) = v.downcast_ref::<f32>() {
                    serde_json::json!(*f)
                } else if let Some(i) = v.downcast_ref::<i32>() {
                    serde_json::json!(*i)
                } else if let Some(b) = v.downcast_ref::<bool>() {
                    serde_json::json!(*b)
                } else if let Some(s) = v.downcast_ref::<String>() {
                    serde_json::json!(s)
                } else {
                    // Fallback to debug representation
                    serde_json::json!(format!("{:?}", v))
                }
            })
            .collect();

        self.value = Value::Array(array);
        Ok(())
    }

    fn serialize_struct(&mut self, fields: &[(&str, &dyn Any)]) -> ReflectResult<()> {
        let mut map = serde_json::Map::new();

        for (name, value) in fields {
            // Try to downcast to known types
            let json_value = if let Some(f) = value.downcast_ref::<f32>() {
                serde_json::json!(*f)
            } else if let Some(i) = value.downcast_ref::<i32>() {
                serde_json::json!(*i)
            } else if let Some(b) = value.downcast_ref::<bool>() {
                serde_json::json!(*b)
            } else if let Some(s) = value.downcast_ref::<String>() {
                serde_json::json!(s)
            } else if let Some(v) = value.downcast_ref::<[f32; 3]>() {
                serde_json::json!([v[0], v[1], v[2]])
            } else if let Some(v) = value.downcast_ref::<[f32; 4]>() {
                serde_json::json!([v[0], v[1], v[2], v[3]])
            } else {
                // Fallback to debug representation
                serde_json::json!(format!("{:?}", value))
            };

            map.insert(name.to_string(), json_value);
        }

        self.value = Value::Object(map);
        Ok(())
    }

    fn serialize_enum(&mut self, _variant_name: &str, variant_index: usize) -> ReflectResult<()> {
        // Serialize enum as variant index (compatible with old PropertyValue::EnumVariant)
        self.value = serde_json::json!(variant_index as u64);
        Ok(())
    }
}

/// JSON deserializer implementation
pub struct JsonDeserializer {
    value: Value,
}

impl JsonDeserializer {
    /// Create a new JSON deserializer from a JSON value
    pub fn new(value: Value) -> Self {
        Self { value }
    }

    /// Create a new JSON deserializer from a JSON string
    pub fn from_str(json: &str) -> ReflectResult<Self> {
        let value = serde_json::from_str(json)
            .map_err(|e| ReflectError::DeserializationFailed(e.to_string()))?;
        Ok(Self { value })
    }
}

impl TypeDeserializer for JsonDeserializer {
    fn deserialize_f32(&mut self) -> ReflectResult<f32> {
        self.value.as_f64().map(|v| v as f32).ok_or_else(|| {
            ReflectError::TypeMismatch {
                expected: "f32",
                found: format!("{:?}", self.value),
            }
        })
    }

    fn deserialize_i32(&mut self) -> ReflectResult<i32> {
        self.value.as_i64().map(|v| v as i32).ok_or_else(|| {
            ReflectError::TypeMismatch {
                expected: "i32",
                found: format!("{:?}", self.value),
            }
        })
    }

    fn deserialize_u64(&mut self) -> ReflectResult<u64> {
        self.value.as_u64().ok_or_else(|| ReflectError::TypeMismatch {
            expected: "u64",
            found: format!("{:?}", self.value),
        })
    }

    fn deserialize_bool(&mut self) -> ReflectResult<bool> {
        self.value.as_bool().ok_or_else(|| ReflectError::TypeMismatch {
            expected: "bool",
            found: format!("{:?}", self.value),
        })
    }

    fn deserialize_string(&mut self) -> ReflectResult<String> {
        self.value
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| ReflectError::TypeMismatch {
                expected: "String",
                found: format!("{:?}", self.value),
            })
    }

    fn deserialize_vec3(&mut self) -> ReflectResult<[f32; 3]> {
        let arr = self.value.as_array().ok_or_else(|| ReflectError::TypeMismatch {
            expected: "[f32; 3]",
            found: format!("{:?}", self.value),
        })?;

        if arr.len() != 3 {
            return Err(ReflectError::TypeMismatch {
                expected: "[f32; 3]",
                found: format!("array of length {}", arr.len()),
            });
        }

        Ok([
            arr[0].as_f64().unwrap_or(0.0) as f32,
            arr[1].as_f64().unwrap_or(0.0) as f32,
            arr[2].as_f64().unwrap_or(0.0) as f32,
        ])
    }

    fn deserialize_color(&mut self) -> ReflectResult<[f32; 4]> {
        let arr = self.value.as_array().ok_or_else(|| ReflectError::TypeMismatch {
            expected: "[f32; 4]",
            found: format!("{:?}", self.value),
        })?;

        if arr.len() != 4 {
            return Err(ReflectError::TypeMismatch {
                expected: "[f32; 4]",
                found: format!("array of length {}", arr.len()),
            });
        }

        Ok([
            arr[0].as_f64().unwrap_or(0.0) as f32,
            arr[1].as_f64().unwrap_or(0.0) as f32,
            arr[2].as_f64().unwrap_or(0.0) as f32,
            arr[3].as_f64().unwrap_or(0.0) as f32,
        ])
    }

    fn deserialize_array(
        &mut self,
        element_type: &RuntimeTypeInfo,
    ) -> ReflectResult<Vec<Box<dyn Any>>> {
        let arr = self.value.as_array().ok_or_else(|| ReflectError::TypeMismatch {
            expected: "array",
            found: format!("{:?}", self.value),
        })?;

        let mut result = Vec::new();

        for item in arr {
            let mut deserializer = JsonDeserializer::new(item.clone());

            let value: Box<dyn Any> = match &element_type.structure {
                TypeStructure::Primitive if element_type.type_id == std::any::TypeId::of::<f32>() => {
                    Box::new(deserializer.deserialize_f32()?)
                }
                TypeStructure::Primitive if element_type.type_id == std::any::TypeId::of::<i32>() => {
                    Box::new(deserializer.deserialize_i32()?)
                }
                TypeStructure::Primitive if element_type.type_id == std::any::TypeId::of::<bool>() => {
                    Box::new(deserializer.deserialize_bool()?)
                }
                TypeStructure::String => Box::new(deserializer.deserialize_string()?),
                _ => {
                    return Err(ReflectError::DeserializationFailed(format!(
                        "Unsupported array element type: {}",
                        element_type.type_name
                    )))
                }
            };

            result.push(value);
        }

        Ok(result)
    }

    fn deserialize_struct(
        &mut self,
        fields: &[FieldInfo],
    ) -> ReflectResult<HashMap<&'static str, Box<dyn Any>>> {
        let obj = self.value.as_object().ok_or_else(|| ReflectError::TypeMismatch {
            expected: "object",
            found: format!("{:?}", self.value),
        })?;

        let mut result = HashMap::new();

        for field in fields {
            let field_value = obj.get(field.name).ok_or_else(|| ReflectError::MissingField {
                struct_name: "unknown",
                field_name: field.name,
            })?;

            let mut deserializer = JsonDeserializer::new(field_value.clone());

            let value: Box<dyn Any> = match &field.type_info.structure {
                TypeStructure::Primitive if field.type_info.type_id == std::any::TypeId::of::<f32>() => {
                    Box::new(deserializer.deserialize_f32()?)
                }
                TypeStructure::Primitive if field.type_info.type_id == std::any::TypeId::of::<i32>() => {
                    Box::new(deserializer.deserialize_i32()?)
                }
                TypeStructure::Primitive if field.type_info.type_id == std::any::TypeId::of::<bool>() => {
                    Box::new(deserializer.deserialize_bool()?)
                }
                TypeStructure::String => Box::new(deserializer.deserialize_string()?),
                _ => {
                    return Err(ReflectError::DeserializationFailed(format!(
                        "Unsupported field type: {}",
                        field.type_info.type_name
                    )))
                }
            };

            result.insert(field.name, value);
        }

        Ok(result)
    }

    fn deserialize_enum(&mut self, variants: &[&'static str]) -> ReflectResult<usize> {
        // Try to deserialize as variant index (u64)
        if let Some(index) = self.value.as_u64() {
            let index = index as usize;
            if index < variants.len() {
                return Ok(index);
            }
        }

        // Try to deserialize as variant name (string)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_serializer_primitives() {
        let mut serializer = JsonSerializer::new();

        serializer.serialize_f32(42.5).unwrap();
        assert_eq!(serializer.as_json(), &serde_json::json!(42.5));

        serializer.serialize_i32(-123).unwrap();
        assert_eq!(serializer.as_json(), &serde_json::json!(-123));

        serializer.serialize_bool(true).unwrap();
        assert_eq!(serializer.as_json(), &serde_json::json!(true));

        serializer.serialize_string("hello").unwrap();
        assert_eq!(serializer.as_json(), &serde_json::json!("hello"));
    }

    #[test]
    fn test_json_serializer_vec3() {
        let mut serializer = JsonSerializer::new();
        serializer.serialize_vec3([1.0, 2.0, 3.0]).unwrap();
        assert_eq!(serializer.as_json(), &serde_json::json!([1.0, 2.0, 3.0]));
    }

    #[test]
    fn test_json_deserializer_primitives() {
        let mut deserializer = JsonDeserializer::new(serde_json::json!(42.5));
        assert_eq!(deserializer.deserialize_f32().unwrap(), 42.5);

        let mut deserializer = JsonDeserializer::new(serde_json::json!(-123));
        assert_eq!(deserializer.deserialize_i32().unwrap(), -123);

        let mut deserializer = JsonDeserializer::new(serde_json::json!(true));
        assert!(deserializer.deserialize_bool().unwrap());

        let mut deserializer = JsonDeserializer::new(serde_json::json!("hello"));
        assert_eq!(deserializer.deserialize_string().unwrap(), "hello");
    }

    #[test]
    fn test_json_deserializer_vec3() {
        let mut deserializer = JsonDeserializer::new(serde_json::json!([1.0, 2.0, 3.0]));
        let vec3 = deserializer.deserialize_vec3().unwrap();
        assert_eq!(vec3, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn test_json_round_trip() {
        let mut serializer = JsonSerializer::new();
        serializer.serialize_f32(99.9).unwrap();
        let json = serializer.into_json();

        let mut deserializer = JsonDeserializer::new(json);
        let value = deserializer.deserialize_f32().unwrap();
        assert_eq!(value, 99.9);
    }
}
