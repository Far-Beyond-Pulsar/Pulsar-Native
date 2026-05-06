//! Core reflection system for Pulsar Engine
//!
//! Provides runtime type information for engine classes (components, actors, etc.)
//! similar to Unreal's UPROPERTY system. Enables automatic UI generation,
//! serialization, and runtime introspection.
//!
//! # Example
//!
//! ```ignore
//! use pulsar_reflection::*;
//! use engine_class_derive::EngineClass;
//!
//! #[derive(EngineClass, Default)]
//! pub struct PhysicsComponent {
//!     #[property(min = 0.0, max = 1000.0)]
//!     pub mass: f32,
//!
//!     #[property]
//!     pub friction: f32,
//! }
//! ```

pub mod registry;

use serde::{Deserialize, Serialize};
use std::any::Any;
use std::fmt;

// Re-export for convenience
pub use registry::{EngineClassRegistration, EngineClassRegistry, REGISTRY};

// Re-export inventory for derive macro
pub use inventory;

/// Core trait for all engine classes (components, actors, etc.)
///
/// This trait is automatically implemented by the `#[derive(EngineClass)]` macro.
/// It provides runtime reflection capabilities for automatic UI generation,
/// serialization, and property inspection.
pub trait EngineClass: Any + Send + Sync {
    /// Get the class name for display and serialization
    fn class_name() -> &'static str
    where
        Self: Sized;

    /// Get reflection metadata for all properties
    ///
    /// Returns a vector of PropertyMetadata describing each field marked with #[property]
    fn get_properties(&self) -> Vec<PropertyMetadata>;

    /// Create default instance (used by object creation menu)
    fn create_default() -> Box<dyn EngineClass>
    where
        Self: Sized;

    /// Downcast to concrete type
    fn as_any(&self) -> &dyn Any;

    /// Downcast to mutable concrete type
    fn as_any_mut(&mut self) -> &mut dyn Any;

    /// Clone into a boxed trait object
    fn clone_boxed(&self) -> Box<dyn EngineClass>;
}

/// Metadata for a single property field
///
/// Contains all information needed to display and edit a property in the UI,
/// including getters/setters, type information, and constraints.
pub struct PropertyMetadata {
    /// Field name (e.g., "mass")
    pub name: &'static str,

    /// Display name for UI (e.g., "Mass")
    pub display_name: String,

    /// Optional category for grouping (e.g., "Physics", "Rendering")
    pub category: Option<&'static str>,

    /// Type information for UI generation
    pub property_type: PropertyType,

    /// Getter closure to read current value
    pub getter: Box<dyn Fn(&dyn EngineClass) -> PropertyValue + Send + Sync>,

    /// Setter closure to write new value
    pub setter: Box<dyn Fn(&mut dyn EngineClass, PropertyValue) + Send + Sync>,
}

impl fmt::Debug for PropertyMetadata {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PropertyMetadata")
            .field("name", &self.name)
            .field("display_name", &self.display_name)
            .field("category", &self.category)
            .field("property_type", &self.property_type)
            .finish()
    }
}

/// Property type information for UI generation
///
/// Each variant contains metadata specific to that type (constraints, options, etc.)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum PropertyType {
    /// 32-bit floating point with optional constraints
    F32 {
        min: Option<f32>,
        max: Option<f32>,
        step: Option<f32>,
    },

    /// 32-bit integer with optional constraints
    I32 { min: Option<i32>, max: Option<i32> },

    /// Boolean (checkbox)
    Bool,

    /// String with optional max length
    String { max_length: Option<usize> },

    /// 3D vector [x, y, z]
    Vec3,

    /// RGBA color [r, g, b, a]
    Color,

    /// Enum with a list of possible variants
    Enum { variants: Vec<&'static str> },

    /// Dynamic array of elements
    Vec {
        element_type: Box<PropertyType>,
    },

    /// Nested component
    Component { class_name: &'static str },
}

/// Runtime property value (for generic getter/setter)
///
/// This enum wraps all possible property value types for dynamic access.
/// Used by getters/setters that operate on trait objects.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum PropertyValue {
    F32(f32),
    I32(i32),
    Bool(bool),
    String(String),
    Vec3([f32; 3]),
    Color([f32; 4]),

    /// Index into enum variants (e.g., 0 = first variant)
    EnumVariant(usize),

    /// Vec<T> contents
    Vec(Vec<PropertyValue>),

    /// Nested component (boxed trait object not serializable, so we store class name + data)
    Component {
        class_name: String,
        // NOTE: Actual component data would be serialized separately
        // This is just a placeholder for the reflection system
    },
}

impl PropertyValue {
    /// Try to extract f32 value
    pub fn as_f32(&self) -> Option<f32> {
        match self {
            PropertyValue::F32(v) => Some(*v),
            _ => None,
        }
    }

    /// Try to extract i32 value
    pub fn as_i32(&self) -> Option<i32> {
        match self {
            PropertyValue::I32(v) => Some(*v),
            _ => None,
        }
    }

    /// Try to extract bool value
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            PropertyValue::Bool(v) => Some(*v),
            _ => None,
        }
    }

    /// Try to extract string value
    pub fn as_string(&self) -> Option<&str> {
        match self {
            PropertyValue::String(v) => Some(v),
            _ => None,
        }
    }

    /// Try to extract Vec3 value
    pub fn as_vec3(&self) -> Option<[f32; 3]> {
        match self {
            PropertyValue::Vec3(v) => Some(*v),
            _ => None,
        }
    }

    /// Try to extract Color value
    pub fn as_color(&self) -> Option<[f32; 4]> {
        match self {
            PropertyValue::Color(v) => Some(*v),
            _ => None,
        }
    }

    /// Try to extract enum variant index
    pub fn as_enum_variant(&self) -> Option<usize> {
        match self {
            PropertyValue::EnumVariant(v) => Some(*v),
            _ => None,
        }
    }

    /// Try to extract vec of values
    pub fn as_vec(&self) -> Option<&[PropertyValue]> {
        match self {
            PropertyValue::Vec(v) => Some(v),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_property_value_as_methods() {
        let f32_val = PropertyValue::F32(42.0);
        assert_eq!(f32_val.as_f32(), Some(42.0));
        assert_eq!(f32_val.as_i32(), None);

        let vec3_val = PropertyValue::Vec3([1.0, 2.0, 3.0]);
        assert_eq!(vec3_val.as_vec3(), Some([1.0, 2.0, 3.0]));
    }
}
