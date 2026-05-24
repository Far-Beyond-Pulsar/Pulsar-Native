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
use serde_json::Value;
use std::any::Any;
use std::collections::HashMap;
use std::fmt;

// Re-export for convenience
pub use registry::{ComponentMethodRegistration, EngineClassRegistration, EngineClassRegistry, REGISTRY};

// Re-export inventory for derive macro
pub use inventory;

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

    /// Get reflection metadata for all blueprint-callable methods
    ///
    /// Returns a vector of MethodMetadata describing each method exposed to blueprints.
    /// This includes both auto-generated property accessors and manually marked methods.
    fn get_methods() -> Vec<MethodMetadata>
    where
        Self: Sized,
    {
        Vec::new() // Default: no methods
    }

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

/// Metadata for a blueprint-callable method on a component
///
/// Contains all information needed to generate blueprint nodes for calling methods,
/// including parameter/return types, execution type, and a caller closure for runtime invocation.
pub struct MethodMetadata {
    /// Method name (e.g., "apply_impulse")
    pub name: &'static str,

    /// Display name for UI (e.g., "Apply Impulse")
    pub display_name: String,

    /// Optional category for grouping (e.g., "Physics", "Rendering")
    pub category: Option<&'static str>,

    /// Parameters for the method
    pub params: Vec<MethodParameter>,

    /// Return type (None for void methods)
    pub return_type: Option<MethodReturnType>,

    /// Method execution type (affects blueprint node pins)
    pub method_type: MethodType,

    /// Caller closure to invoke the method via reflection
    pub caller: Box<dyn Fn(&mut dyn EngineClass, Vec<PropertyValue>) -> Option<PropertyValue> + Send + Sync>,
}

impl fmt::Debug for MethodMetadata {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MethodMetadata")
            .field("name", &self.name)
            .field("display_name", &self.display_name)
            .field("category", &self.category)
            .field("params", &self.params)
            .field("return_type", &self.return_type)
            .field("method_type", &self.method_type)
            .finish()
    }
}

/// Parameter metadata for a method
#[derive(Clone, Debug)]
pub struct MethodParameter {
    /// Parameter name
    pub name: &'static str,

    /// Parameter type information
    pub param_type: PropertyType,
}

/// Return type metadata for a method
#[derive(Clone, Debug)]
pub struct MethodReturnType {
    /// Return type information
    pub return_type: PropertyType,
}

/// Method execution type (determines blueprint node behavior)
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MethodType {
    /// Pure function - no side effects, no execution pins
    Pure,

    /// Function with side effects - requires execution flow pins
    Fn,

    /// Control flow node - can branch execution (future feature)
    ControlFlow,
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
