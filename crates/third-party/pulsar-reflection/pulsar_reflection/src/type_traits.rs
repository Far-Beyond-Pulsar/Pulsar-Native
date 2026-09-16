//! Core traits for runtime type reflection
//!
//! Provides trait definitions for reflectable types, serialization, and
//! deserialization. These traits enable type-safe operations on dynamically
//! typed values without enum pattern matching.

use crate::runtime_types::{FieldInfo, RuntimeTypeInfo};
use std::any::Any;
use std::collections::HashMap;
use std::error::Error;
use std::fmt;

/// Result type for reflection operations
pub type ReflectResult<T> = Result<T, ReflectError>;

/// Errors that can occur during reflection operations
#[derive(Debug, Clone)]
pub enum ReflectError {
    /// Type mismatch during deserialization
    TypeMismatch {
        expected: &'static str,
        found: String,
    },

    /// Missing required field in struct deserialization
    MissingField {
        struct_name: &'static str,
        field_name: &'static str,
    },

    /// Invalid enum variant
    InvalidVariant {
        enum_name: &'static str,
        variant: String,
    },

    /// Serialization failed
    SerializationFailed(String),

    /// Deserialization failed
    DeserializationFailed(String),

    /// Custom error
    Custom(String),
}

impl fmt::Display for ReflectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TypeMismatch { expected, found } => {
                write!(f, "Type mismatch: expected {}, found {}", expected, found)
            }
            Self::MissingField {
                struct_name,
                field_name,
            } => {
                write!(
                    f,
                    "Missing required field '{}' in struct '{}'",
                    field_name, struct_name
                )
            }
            Self::InvalidVariant { enum_name, variant } => {
                write!(f, "Invalid variant '{}' for enum '{}'", variant, enum_name)
            }
            Self::SerializationFailed(msg) => write!(f, "Serialization failed: {}", msg),
            Self::DeserializationFailed(msg) => write!(f, "Deserialization failed: {}", msg),
            Self::Custom(msg) => write!(f, "{}", msg),
        }
    }
}

impl Error for ReflectError {}

/// Trait for types that can be reflected at runtime
///
/// Automatically implemented by `#[derive(Reflectable)]`.
/// Provides runtime type information and serialization capabilities.
pub trait Reflectable: Any + Send + Sync {
    /// Get static type information for this type
    fn type_info() -> &'static RuntimeTypeInfo
    where
        Self: Sized;

    /// Serialize this value using the provided serializer
    fn serialize(&self, serializer: &mut dyn TypeSerializer) -> ReflectResult<()>;

    /// Deserialize a value of this type from the provided deserializer
    fn deserialize(deserializer: &mut dyn TypeDeserializer) -> ReflectResult<Self>
    where
        Self: Sized;

    /// Clone this value into a Box<dyn Any>
    fn clone_any(&self) -> Box<dyn Any>;
}

/// Trait for type serialization
///
/// Implementors define how to serialize values to their target format
/// (JSON, binary, etc.). The type-driven design eliminates pattern matching.
pub trait TypeSerializer {
    /// Serialize a registered value through runtime type callbacks.
    fn serialize_registered(&mut self, value: &dyn Any) -> ReflectResult<()>;

    /// Serialize an array of values
    fn serialize_array(
        &mut self,
        values: &[&dyn Any],
        element_type: &RuntimeTypeInfo,
    ) -> ReflectResult<()>;

    /// Serialize a struct with named fields
    fn serialize_struct(&mut self, fields: &[(&str, &dyn Any)]) -> ReflectResult<()>;

    /// Serialize an enum variant
    fn serialize_enum(&mut self, variant_name: &str, variant_index: usize) -> ReflectResult<()>;
}

/// Trait for type deserialization
///
/// Implementors define how to deserialize values from their source format
/// (JSON, binary, etc.). Returns type-erased Box<dyn Any> for flexibility.
pub trait TypeDeserializer {
    /// Deserialize one registered value from runtime type callbacks.
    fn deserialize_registered(
        &mut self,
        type_info: &RuntimeTypeInfo,
    ) -> ReflectResult<Box<dyn Any>>;

    /// Deserialize an array of values
    fn deserialize_array(
        &mut self,
        element_type: &RuntimeTypeInfo,
    ) -> ReflectResult<Vec<Box<dyn Any>>>;

    /// Deserialize a struct with named fields
    fn deserialize_struct(
        &mut self,
        fields: &[FieldInfo],
    ) -> ReflectResult<HashMap<&'static str, Box<dyn Any>>>;

    /// Deserialize an enum variant (returns variant index)
    fn deserialize_enum(&mut self, variants: &[&'static str]) -> ReflectResult<usize>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reflect_error_display() {
        let err = ReflectError::TypeMismatch {
            expected: "f32",
            found: "i32".to_string(),
        };
        assert!(err.to_string().contains("Type mismatch"));

        let err = ReflectError::MissingField {
            struct_name: "TestStruct",
            field_name: "value",
        };
        assert!(err.to_string().contains("Missing required field"));
    }
}
