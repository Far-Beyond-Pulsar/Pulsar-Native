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
pub mod dynamic_types;
pub mod json_codec;
pub mod runtime_registry;
pub mod runtime_types;
pub mod type_renderer;
pub mod type_traits;

// Primitive type implementations
pub mod prims;

use serde_json::Value;
use std::any::Any;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

// Re-export for convenience
pub use registry::{
    ComponentMethodRegistration, EngineClassRegistration, EngineClassRegistry, REGISTRY,
};

// Re-export inventory for derive macro
pub use inventory;

// Re-export runtime type system
pub use json_codec::{JsonDeserializer, JsonSerializer};
pub use runtime_registry::{RUNTIME_TYPE_REGISTRY, RuntimeTypeRegistration, RuntimeTypeRegistry};
pub use runtime_types::{FieldInfo, RuntimeTypeInfo, TypeStructure, WrapperType};
pub use type_traits::{ReflectError, ReflectResult, Reflectable, TypeDeserializer, TypeSerializer};

// Re-export dynamic type system
pub use dynamic_types::{
    DYNAMIC_TYPE_REGISTRY, DynamicFieldInfo, DynamicTypeBuilder, DynamicTypeInfo,
    DynamicTypeRegistry, DynamicValue, TypeTag,
};

// Re-export type renderer system
pub use type_renderer::{
    RenderResult, TYPE_RENDERER_REGISTRY, TypeRenderer, TypeRendererRegistration,
    TypeRendererRegistry, register_type_renderer,
};

// Re-export derive macro
pub use pulsar_reflection_derive::{Reflectable, pulsar_type};

/// Property editor arguments passed to registered render functions.
///
/// This shape intentionally mirrors the UI-layer inspector args used by
/// `ui_common` so type-erased editor fn pointers can be invoked safely across
/// crate boundaries.
#[derive(Clone)]
pub struct PropertyEditorArgs<'a> {
    pub id_prefix: &'a str,
    pub class_name: &'a str,
    pub display_name: &'a str,
    pub prop_name: &'a str,
    pub type_info: &'static RuntimeTypeInfo,
    pub current_json: &'a Value,
    pub numeric_input: Option<gpui::Entity<ui::input::InputState>>,
    pub color_picker: Option<gpui::Entity<ui::color_picker::ColorPickerState>>,
    pub mesh_picker: Option<gpui::Entity<ui_common::MeshAssetPicker>>,
    pub on_bool_toggle: Arc<dyn Fn(bool, &mut gpui::Window, &mut gpui::App) + Send + Sync>,
    pub on_enum_select: Arc<dyn Fn(usize, &mut gpui::Window, &mut gpui::App) + Send + Sync>,
}

pub use ui_common::MeshAssetPickerState;

// ── UI property-editor hint ───────────────────────────────────────────────────

/// Type-erased hint that a concrete type has a registered GPUI property editor.
///
/// Submitted via `inventory::submit!` — either directly or by the
/// `editor = fn` argument on [`pulsar_type`].
///
/// The `fn_ptr` field stores a function pointer erased to `fn()` so that this
/// type remains free of GPUI dependencies.  The framework layer (`ui_common`)
/// is responsible for transmuting it back to the correct concrete signature:
/// `fn(&PropertyEditorArgs<'_>, &gpui::App) -> gpui::AnyElement`.
///
/// Storing as `fn()` (rather than `usize`) allows the `inventory::submit!`
/// static-initialiser to compile without triggering E0658 (fn ptr → int cast
/// is forbidden in const context since Rust 1.83).
///
/// # Safety contract
///
/// Only submit function pointers whose actual Rust type matches the above
/// signature.  The transmute in `ui_common` is safe by construction as long as
/// this invariant is upheld.
pub struct UiPropertyEditorHint {
    /// [`TypeId`](std::any::TypeId) of the type this editor handles.
    pub type_id: std::any::TypeId,
    /// Erased function pointer — actual type is `PropertyEditorRenderFn`.
    /// Cast to `fn()` so it can appear in `const` / `static` initialisers.
    pub fn_ptr: fn(),
}

inventory::collect!(UiPropertyEditorHint);

/// Erase a two-argument render function to the opaque `fn()` stored in
/// [`UiPropertyEditorHint::fn_ptr`].
///
/// All Rust function pointer types are pointer-sized, so transmuting between
/// them preserves size.  This is a `const unsafe fn` so it can be called
/// inside `inventory::submit!` static initialisers.
///
/// # Safety
/// `f` must be a function whose actual signature is:
/// `fn(&ui_common::PropertyEditorArgs<'_>, &gpui::App) -> gpui::AnyElement`.
pub const unsafe fn erase_property_editor_fn_ptr<A, B, C>(f: fn(A, B) -> C) -> fn() {
    // SAFETY: fn(A, B) -> C and fn() are both pointer-sized on every supported
    // platform; transmuting between any two fn-pointer types is defined behaviour.
    unsafe { std::mem::transmute(f) }
}

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

/// Hash a SceneDb string ID to a compact `u64` tag for storage in helio actors.
///
/// Components call this to compute the [`ObjectDescriptor::user_tag`] /
/// [`SceneActor::light_with_tag`] value before inserting an actor.  The picker
/// returns the same tag in [`PickHit::user_tag`], allowing the engine to
/// identify the owning SceneDb object without any reverse-lookup maps.
///
/// The hash uses Rust's default hasher (SipHash) which is stable within a
/// single process run.  It is not persisted to disk.
pub fn scene_id_to_tag(id: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut h);
    h.finish()
}

/// Context provided to every component's [`ComponentRuntimeBehavior::sync_component`].
///
/// Exposes **generic services only** — no knowledge of what any specific
/// component does.  Each component is fully self-contained: it parses its own
/// data, constructs any GPU payloads, and writes directly to the renderer.
///
/// # Implementing this trait
///
/// * **Editor (`HelioRuntimeContext`)** — clears the helio scene each sync pass
///   and lets components re-insert fresh.  Rebuilds reverse-lookup maps from the
///   inserted actor IDs reported via [`track_actor`].
/// * **Game (`SceneObjectContext`)** — one-shot scene load; components insert
///   once and the loader records the resulting IDs.
pub trait ComponentRuntimeContext {
    /// Raw mutable access to the Helio renderer.
    ///
    /// Components use this to insert lights, meshes, and objects directly.
    /// The context owns no knowledge of what they insert or how.
    #[cfg(feature = "prims-helio")]
    fn renderer_mut(&mut self) -> &mut helio::Renderer;

    /// Project root for resolving relative asset paths.
    fn project_root(&self) -> &std::path::Path;

    /// Load a mesh file, with optional context-level caching.
    ///
    /// The path may be absolute or relative to [`project_root`].
    /// Returns `None` when the file cannot be loaded; the component should
    /// call [`report_error`] and return early in that case.
    ///
    /// Default implementation returns `None`; override in contexts that
    /// support mesh loading.
    #[cfg(feature = "prims-helio")]
    fn load_mesh_file(&mut self, _path: &std::path::Path) -> Option<helio::MeshUpload> {
        None
    }

    /// Mark an actor key as live in the current sync pass **without** inserting
    /// a render actor.
    ///
    /// Used by components (e.g. `ScriptComponent`) that register with
    /// non-renderer systems and still need stale-cleanup semantics.
    fn mark_live(&mut self, _actor_key: &str) {}

    /// Called by `StaticMeshComponent` after obtaining a `MeshUpload` from
    /// [`load_mesh_file`].  The context decides whether to update an existing
    /// object's transform (fast path) or upload geometry and insert a new
    /// object instance (slow path, runs once per unique asset).
    ///
    /// Default is a no-op so game-side contexts that insert once on load are
    /// unaffected.
    /// Sync a mesh scene object into the renderer.
    ///
    /// The context owns the full insert-vs-update decision:
    /// - If an object with `tag` already exists using the same `mesh_asset` →
    ///   update transform only (no disk I/O, no GPU upload).
    /// - If the mesh geometry has not been uploaded yet →
    ///   load from disk once, upload to GPU, cache both.
    ///
    /// The component passes the *resolved absolute asset path* so the context
    /// can use it as a cache key without re-doing path resolution.
    /// Default is a no-op; contexts that support incremental mesh sync override.
    #[cfg(all(feature = "prims-helio", feature = "prims-glam"))]
    fn sync_mesh_object(
        &mut self,
        _tag: u64,
        _mesh_asset: &str,
        _transform: glam::Mat4,
        _bounds: [f32; 4],
    ) {
    }

    /// Report a non-fatal component error.
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
pub type MethodCaller =
    Box<dyn Fn(&mut dyn EngineClass, MethodArgs) -> MethodReturnValue + Send + Sync>;

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
