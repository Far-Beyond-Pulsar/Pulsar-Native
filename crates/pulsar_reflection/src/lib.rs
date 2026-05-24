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

// New runtime type reflection system
pub mod runtime_types;
pub mod runtime_registry;
pub mod type_traits;
pub mod json_serializer;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::any::Any;
use std::collections::HashMap;
use std::fmt;

// Re-export for convenience
pub use registry::{EngineClassRegistration, EngineClassRegistry, REGISTRY};

// Re-export inventory for derive macro
pub use inventory;

// Re-export runtime type system
pub use runtime_types::{FieldInfo, RuntimeTypeInfo, TypeStructure, WrapperType};
pub use runtime_registry::{RuntimeTypeRegistration, RuntimeTypeRegistry, RUNTIME_TYPE_REGISTRY};
pub use type_traits::{Reflectable, ReflectError, ReflectResult, TypeDeserializer, TypeSerializer};
pub use json_serializer::{JsonDeserializer, JsonSerializer};

// Re-export derive macro
pub use pulsar_reflection_derive::Reflectable;

/// Trait for component-owned projection of reflection data into scene snapshot props.
///
/// This keeps per-component prop mapping logic modular and out of central systems.
pub trait ScenePropsProjector {
    /// Reflection class name this projector handles.
    const CLASS_NAME: &'static str;

    /// Apply component-derived props to the scene-level props map.
    ///
    /// `component_data == None` should clear or reset any props managed by this
    /// projector so stale values do not linger.
    fn apply_scene_props(props: &mut HashMap<String, Value>, component_data: Option<&Value>);
}

/// Inventory registration entry for scene props projectors.
pub struct ScenePropsApplierRegistration {
    pub class_name: &'static str,
    pub apply: fn(&mut HashMap<String, Value>, Option<&Value>),
}

inventory::collect!(ScenePropsApplierRegistration);

/// Apply registered scene-props logic for one component class.
///
/// Returns true if a registered applier handled this class.
pub fn apply_scene_props_for_class(
    class_name: &str,
    props: &mut HashMap<String, Value>,
    component_data: Option<&Value>,
) -> bool {
    for registration in inventory::iter::<ScenePropsApplierRegistration> {
        if registration.class_name == class_name {
            (registration.apply)(props, component_data);
            return true;
        }
    }
    false
}

/// Return all registered scene-props applier class names.
pub fn registered_scene_props_classes() -> Vec<&'static str> {
    inventory::iter::<ScenePropsApplierRegistration>
        .into_iter()
        .map(|r| r.class_name)
        .collect()
}

/// Scene object state available to runtime component behaviors.
pub struct RuntimeComponentOwner<'a> {
    pub scene_object_id: &'a str,
    pub position: [f32; 3],
    pub rotation: [f32; 3],
    pub scale: [f32; 3],
    pub props: &'a HashMap<String, Value>,
}

#[derive(Clone, Copy, Debug)]
pub enum RuntimeLightType {
    Directional,
    Point,
    Spot,
    Area,
}

#[derive(Clone, Debug)]
pub struct RuntimeLightDesc {
    pub actor_key: String,
    pub light_type: RuntimeLightType,
    pub color: [f32; 4],
    pub intensity: f32,
    pub range: f32,
    pub inner_cone_angle_deg: f32,
    pub outer_cone_angle_deg: f32,
}

#[derive(Clone, Debug)]
pub struct RuntimeMeshDesc {
    pub actor_key: String,
    pub mesh_asset: String,
}

/// Runtime context implemented by renderer-side systems.
pub trait ComponentRuntimeContext {
    fn upsert_light(&mut self, desc: RuntimeLightDesc);
    fn upsert_mesh(&mut self, desc: RuntimeMeshDesc);
    fn report_error(&mut self, message: String);
}

/// Trait for component-owned runtime behavior projection.
pub trait ComponentRuntimeBehavior {
    /// Reflection class name this runtime behavior handles.
    const CLASS_NAME: &'static str;

    /// Sync one component instance into runtime systems.
    fn sync_component(
        owner: &RuntimeComponentOwner,
        component_index: usize,
        component_data: &Value,
        context: &mut dyn ComponentRuntimeContext,
    );
}

/// Inventory registration entry for runtime behavior handlers.
pub struct RuntimeBehaviorRegistration {
    pub class_name: &'static str,
    pub sync: fn(&RuntimeComponentOwner, usize, &Value, &mut dyn ComponentRuntimeContext),
}

inventory::collect!(RuntimeBehaviorRegistration);

/// Apply registered runtime behavior for one component class.
///
/// Returns true if a registered behavior handled this class.
pub fn apply_runtime_behavior_for_class(
    class_name: &str,
    owner: &RuntimeComponentOwner,
    component_index: usize,
    component_data: &Value,
    context: &mut dyn ComponentRuntimeContext,
) -> bool {
    for registration in inventory::iter::<RuntimeBehaviorRegistration> {
        if registration.class_name == class_name {
            (registration.sync)(owner, component_index, component_data, context);
            return true;
        }
    }
    false
}

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
///
/// Now uses runtime type reflection instead of enum-based PropertyType!
pub struct PropertyMetadata {
    /// Field name (e.g., "mass")
    pub name: &'static str,

    /// Display name for UI (e.g., "Mass")
    pub display_name: String,

    /// Optional category for grouping (e.g., "Physics", "Rendering")
    pub category: Option<&'static str>,

    /// Runtime type information (replaces PropertyType enum)
    pub type_info: &'static RuntimeTypeInfo,

    /// Getter closure to read current value (returns type-erased Any)
    pub getter: Box<dyn Fn(&dyn EngineClass) -> Box<dyn Any> + Send + Sync>,

    /// Setter closure to write new value (accepts type-erased Any)
    pub setter: Box<dyn Fn(&mut dyn EngineClass, Box<dyn Any>) + Send + Sync>,

    /// DEPRECATED: Legacy PropertyType for backward compatibility
    /// This will be removed in a future version
    #[deprecated(note = "Use type_info instead - PropertyType enum is being phased out")]
    pub legacy_property_type: Option<PropertyType>,
}

impl fmt::Debug for PropertyMetadata {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PropertyMetadata")
            .field("name", &self.name)
            .field("display_name", &self.display_name)
            .field("category", &self.category)
            .field("type_info", &self.type_info)
            .finish()
    }
}

impl PropertyMetadata {
    /// Synthesize a legacy PropertyType for backward compatibility
    ///
    /// DEPRECATED: This method exists only for gradual migration.
    /// New code should use `type_info` directly.
    #[deprecated(note = "Use type_info instead - PropertyType enum is being removed")]
    #[allow(deprecated)]
    pub fn synthesize_legacy_type(&self) -> PropertyType {
        use std::any::TypeId;

        match &self.type_info.structure {
            TypeStructure::Primitive if self.type_info.type_id == TypeId::of::<f32>() => {
                PropertyType::F32 {
                    min: None,
                    max: None,
                    step: None,
                }
            }
            TypeStructure::Primitive if self.type_info.type_id == TypeId::of::<i32>() => {
                PropertyType::I32 {
                    min: None,
                    max: None,
                }
            }
            TypeStructure::Primitive if self.type_info.type_id == TypeId::of::<bool>() => {
                PropertyType::Bool
            }
            TypeStructure::String => PropertyType::String { max_length: None },
            TypeStructure::Primitive if self.type_info.type_id == TypeId::of::<[f32; 3]>() => {
                PropertyType::Vec3
            }
            TypeStructure::Primitive if self.type_info.type_id == TypeId::of::<[f32; 4]>() => {
                PropertyType::Color
            }
            TypeStructure::Enum { variants } => PropertyType::Enum {
                variants: variants.to_vec(),
            },
            TypeStructure::Wrapper {
                wrapper_kind: WrapperType::Vec,
                inner,
            } => {
                // Recursively synthesize for inner type
                let inner_meta = PropertyMetadata {
                    name: "",
                    display_name: String::new(),
                    category: None,
                    type_info: inner,
                    getter: Box::new(|_| Box::new(())),
                    setter: Box::new(|_, _| {}),
                    legacy_property_type: None,
                };
                PropertyType::Vec {
                    element_type: Box::new(inner_meta.synthesize_legacy_type()),
                }
            }
            TypeStructure::Struct { .. } => {
                // Default to Component
                PropertyType::Component {
                    class_name: self.type_info.type_name,
                }
            }
            _ => {
                // Fallback to String for unknown types
                PropertyType::String { max_length: None }
            }
        }
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
    Vec { element_type: Box<PropertyType> },

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

// Primitive type registrations
use std::any::TypeId;

// Register f32
static F32_TYPE_INFO: RuntimeTypeInfo = RuntimeTypeInfo {
    type_id: TypeId::of::<f32>(),
    type_name: "f32",
    size: std::mem::size_of::<f32>(),
    align: std::mem::align_of::<f32>(),
    structure: TypeStructure::Primitive,
};

inventory::submit! {
    RuntimeTypeRegistration {
        type_info: &F32_TYPE_INFO,
    }
}

// Register i32
static I32_TYPE_INFO: RuntimeTypeInfo = RuntimeTypeInfo {
    type_id: TypeId::of::<i32>(),
    type_name: "i32",
    size: std::mem::size_of::<i32>(),
    align: std::mem::align_of::<i32>(),
    structure: TypeStructure::Primitive,
};

inventory::submit! {
    RuntimeTypeRegistration {
        type_info: &I32_TYPE_INFO,
    }
}

// Register u64
static U64_TYPE_INFO: RuntimeTypeInfo = RuntimeTypeInfo {
    type_id: TypeId::of::<u64>(),
    type_name: "u64",
    size: std::mem::size_of::<u64>(),
    align: std::mem::align_of::<u64>(),
    structure: TypeStructure::Primitive,
};

inventory::submit! {
    RuntimeTypeRegistration {
        type_info: &U64_TYPE_INFO,
    }
}

// Register bool
static BOOL_TYPE_INFO: RuntimeTypeInfo = RuntimeTypeInfo {
    type_id: TypeId::of::<bool>(),
    type_name: "bool",
    size: std::mem::size_of::<bool>(),
    align: std::mem::align_of::<bool>(),
    structure: TypeStructure::Primitive,
};

inventory::submit! {
    RuntimeTypeRegistration {
        type_info: &BOOL_TYPE_INFO,
    }
}

// Register String
static STRING_TYPE_INFO: RuntimeTypeInfo = RuntimeTypeInfo {
    type_id: TypeId::of::<String>(),
    type_name: "String",
    size: std::mem::size_of::<String>(),
    align: std::mem::align_of::<String>(),
    structure: TypeStructure::String,
};

inventory::submit! {
    RuntimeTypeRegistration {
        type_info: &STRING_TYPE_INFO,
    }
}

// Register [f32; 3] (Vec3)
static VEC3_TYPE_INFO: RuntimeTypeInfo = RuntimeTypeInfo {
    type_id: TypeId::of::<[f32; 3]>(),
    type_name: "[f32; 3]",
    size: std::mem::size_of::<[f32; 3]>(),
    align: std::mem::align_of::<[f32; 3]>(),
    structure: TypeStructure::Primitive, // Treat as primitive for simplicity
};

inventory::submit! {
    RuntimeTypeRegistration {
        type_info: &VEC3_TYPE_INFO,
    }
}

// Register [f32; 4] (Color)
static COLOR_TYPE_INFO: RuntimeTypeInfo = RuntimeTypeInfo {
    type_id: TypeId::of::<[f32; 4]>(),
    type_name: "[f32; 4]",
    size: std::mem::size_of::<[f32; 4]>(),
    align: std::mem::align_of::<[f32; 4]>(),
    structure: TypeStructure::Primitive, // Treat as primitive for simplicity
};

inventory::submit! {
    RuntimeTypeRegistration {
        type_info: &COLOR_TYPE_INFO,
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

    #[test]
    fn test_runtime_type_registry_primitives() {
        // Test that primitive types are registered
        let registry = &*RUNTIME_TYPE_REGISTRY;

        assert!(registry.get::<f32>().is_some());
        assert!(registry.get::<i32>().is_some());
        assert!(registry.get::<bool>().is_some());
        assert!(registry.get::<String>().is_some());
        assert!(registry.get::<[f32; 3]>().is_some());
        assert!(registry.get::<[f32; 4]>().is_some());

        // Verify f32 metadata
        let f32_info = registry.get::<f32>().unwrap();
        assert_eq!(f32_info.type_name, "f32");
        assert_eq!(f32_info.size, 4);
        assert!(f32_info.is_primitive());
    }

    // Test the Reflectable derive macro
    #[derive(Reflectable, Clone, Debug)]
    struct TestStruct {
        value: f32,
        count: i32,
        active: bool,
    }

    #[test]
    fn test_reflectable_derive_struct() {
        // Test that the derived type is registered
        let registry = &*RUNTIME_TYPE_REGISTRY;
        let type_info = registry.get::<TestStruct>();
        assert!(type_info.is_some());

        let type_info = type_info.unwrap();
        assert_eq!(type_info.type_name, "TestStruct");
        assert!(type_info.is_struct());

        // Check fields
        let fields = type_info.fields().unwrap();
        assert_eq!(fields.len(), 3);
        assert_eq!(fields[0].name, "value");
        assert_eq!(fields[1].name, "count");
        assert_eq!(fields[2].name, "active");

        // Test serialization
        let test_instance = TestStruct {
            value: 42.5,
            count: 10,
            active: true,
        };

        let mut serializer = JsonSerializer::new();
        test_instance.serialize(&mut serializer).unwrap();
        let json = serializer.into_json();

        assert_eq!(json["value"], 42.5);
        assert_eq!(json["count"], 10);
        assert_eq!(json["active"], true);
    }

    #[derive(Reflectable, Clone, Debug, PartialEq)]
    enum TestEnum {
        Option1,
        Option2,
        Option3,
    }

    #[test]
    fn test_reflectable_derive_enum() {
        // Test that the derived enum is registered
        let registry = &*RUNTIME_TYPE_REGISTRY;
        let type_info = registry.get::<TestEnum>();
        assert!(type_info.is_some());

        let type_info = type_info.unwrap();
        assert_eq!(type_info.type_name, "TestEnum");
        assert!(type_info.is_enum());

        // Check variants
        let variants = type_info.enum_variants().unwrap();
        assert_eq!(variants.len(), 3);
        assert_eq!(variants[0], "Option1");
        assert_eq!(variants[1], "Option2");
        assert_eq!(variants[2], "Option3");

        // Test serialization
        let test_value = TestEnum::Option2;
        let mut serializer = JsonSerializer::new();
        test_value.serialize(&mut serializer).unwrap();
        let json = serializer.into_json();

        // Should serialize as variant index (1)
        assert_eq!(json, 1);

        // Test deserialization
        let mut deserializer = JsonDeserializer::new(serde_json::json!(1));
        let deserialized = TestEnum::deserialize(&mut deserializer).unwrap();
        assert_eq!(deserialized, TestEnum::Option2);
    }
}
