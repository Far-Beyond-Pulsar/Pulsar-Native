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

extern crate self as pulsar_reflection;

pub mod registry;

// New runtime type reflection system
pub mod runtime_types;
pub mod runtime_registry;
pub mod type_traits;
pub mod json_codec;
pub mod dynamic_types;
pub mod type_renderer;

// Primitive type implementations
pub mod prims;

use serde_json::Value;
use std::any::Any;
use std::collections::HashMap;
use std::fmt;

// Re-export for convenience
pub use registry::{ComponentMethodRegistration, EngineClassRegistration, EngineClassRegistry, REGISTRY};

// Re-export inventory for derive macro
pub use inventory;

// Re-export runtime type system
pub use runtime_types::{FieldInfo, RuntimeTypeInfo, TypeStructure, WrapperType};
pub use runtime_registry::{RuntimeTypeRegistration, RuntimeTypeRegistry, RUNTIME_TYPE_REGISTRY};
pub use type_traits::{Reflectable, ReflectError, ReflectResult, TypeDeserializer, TypeSerializer};
pub use json_codec::{JsonDeserializer, JsonSerializer};

// Re-export dynamic type system
pub use dynamic_types::{
    DynamicFieldInfo, DynamicTypeBuilder, DynamicTypeInfo, DynamicTypeRegistry,
    DynamicValue, TypeTag, DYNAMIC_TYPE_REGISTRY,
};

// Re-export type renderer system
pub use type_renderer::{
    RenderResult, TypeRenderer, TypeRendererRegistration, TypeRendererRegistry,
    register_type_renderer, TYPE_RENDERER_REGISTRY,
};

// Re-export derive macro
pub use pulsar_reflection_derive::{pulsar_type, Reflectable};

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

/// Classification for blueprint-callable method behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodType {
    Pure,
    Fn,
    ControlFlow,
}

/// Metadata for a single method parameter.
#[derive(Debug, Clone)]
pub struct MethodParameter {
    pub name: &'static str,
    pub type_info: &'static RuntimeTypeInfo,
}

/// Metadata for a method return type.
#[derive(Debug, Clone)]
pub struct MethodReturnType {
    pub type_info: &'static RuntimeTypeInfo,
}

pub type MethodArgs = Vec<Box<dyn Any>>;
pub type MethodReturnValue = Option<Box<dyn Any>>;
pub type MethodCaller = Box<dyn Fn(&mut dyn EngineClass, MethodArgs) -> MethodReturnValue + Send + Sync>;

/// Metadata for a blueprint-callable method.
pub struct MethodMetadata {
    pub name: &'static str,
    pub display_name: String,
    pub category: Option<&'static str>,
    pub params: Vec<MethodParameter>,
    pub return_type: Option<MethodReturnType>,
    pub method_type: MethodType,
    pub caller: MethodCaller,
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

// Primitive type registrations are now in the prims module

#[cfg(test)]
mod tests {
    use super::*;

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

    // NOTE: Tests using #[derive(Reflectable)] cannot be placed inside this crate
    // due to the absolute path resolution issue. The derive macro works correctly
    // when used from external crates. See the integration tests for usage examples.
}
