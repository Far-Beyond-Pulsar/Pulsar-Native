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
use std::any::{Any, TypeId};
use std::collections::{HashMap, HashSet};
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

/// Arguments passed to every registered property editor render function.
///
/// Widget state is carried as a type-erased map so the reflection crate has
/// zero knowledge of what widget types exist.  Each render function retrieves
/// its own stateful entity by concrete type via [`get_widget`].
pub struct PropertyEditorArgs<'a> {
    pub id_prefix: &'a str,
    pub class_name: &'a str,
    pub display_name: &'a str,
    pub prop_name: &'a str,
    pub type_info: &'static RuntimeTypeInfo,
    pub current_json: &'a Value,
    pub widgets: std::collections::HashMap<std::any::TypeId, Arc<dyn std::any::Any + Send + Sync>>,
    #[cfg(feature = "prims-gpui")]
    pub on_bool_toggle: Arc<dyn Fn(bool, &mut gpui::Window, &mut gpui::App) + Send + Sync>,
    #[cfg(feature = "prims-gpui")]
    pub on_enum_select: Arc<dyn Fn(usize, &mut gpui::Window, &mut gpui::App) + Send + Sync>,
}

impl<'a> PropertyEditorArgs<'a> {
    pub fn get_widget<T: std::any::Any + Clone>(&self) -> Option<T> {
        self.widgets
            .get(&std::any::TypeId::of::<T>())
            .and_then(|arc| arc.downcast_ref::<T>())
            .cloned()
    }
}

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
/// them preserves size.  The input is intentionally constrained to the one
/// valid property-editor signature so registration sites fail fast at compile
/// time when a mismatched function is provided.
///
/// This is a `const fn` so it can be called inside `inventory::submit!`
/// static initialisers emitted by proc macros.
///
/// Gated behind `prims-gpui` (M3-alpha Task 2): the signature is GPUI-typed,
/// so it can only exist when the crate is compiled with GPUI available.
#[cfg(feature = "prims-gpui")]
pub const fn erase_property_editor_fn_ptr(
    f: fn(&PropertyEditorArgs<'_>, &gpui::App) -> gpui::AnyElement,
) -> fn() {
    // SAFETY: all function pointers are pointer-sized, so this cast preserves
    // representation. The function signature is validated by the typed input.
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

/// Type-erased registry of runtime subsystems.
///
/// Subsystems (renderer, physics engine, audio system, etc.) register themselves
/// here by type so that components can look them up without the context trait
/// carrying direct knowledge of every subsystem.
///
/// Supports both **owned** subsystems (stored in `Box<dyn Any>`) and
/// **borrowed** subsystems (stored as a raw pointer).  Use [`register`] for
/// owned values and [`register_ref`] for borrowed references.
pub struct Subsystems {
    owned: HashMap<TypeId, Box<dyn Any>>,
    borrow: HashMap<TypeId, *mut ()>,
}

impl Subsystems {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            owned: HashMap::new(),
            borrow: HashMap::new(),
        }
    }

    /// Register an **owned** subsystem value.
    ///
    /// The subsystem is stored in a `Box` and lives for the lifetime of
    /// this registry — no further lifetime management needed.
    pub fn register<T: 'static>(&mut self, subsystem: T) {
        self.owned.insert(TypeId::of::<T>(), Box::new(subsystem));
    }

    /// Register a **borrowed** subsystem reference.
    ///
    /// The pointed-to value must outlive this registry.
    ///
    /// # Safety
    ///
    /// The caller must ensure the reference outlives this registry.
    pub fn register_ref<T: 'static>(&mut self, subsystem: &mut T) {
        let ptr = subsystem as *mut T as *mut ();
        self.borrow.insert(TypeId::of::<T>(), ptr);
    }

    /// Get a registered subsystem by concrete type.
    ///
    /// Returns `None` if no subsystem of this type has been registered.
    pub fn get_mut<T: 'static>(&mut self) -> Option<&mut T> {
        // Check owned map first, then borrow map.
        if let Some(b) = self.owned.get_mut(&TypeId::of::<T>()) {
            return b.downcast_mut::<T>();
        }
        self.borrow
            .get(&TypeId::of::<T>())
            .map(|&ptr| unsafe { &mut *(ptr as *mut T) })
    }

    /// Unregister a subsystem by type.
    pub fn unregister<T: 'static>(&mut self) {
        self.owned.remove(&TypeId::of::<T>());
        self.borrow.remove(&TypeId::of::<T>());
    }
}

/// A set of string keys used by contexts to track which actor keys are live
/// during a sync pass, enabling stale-cleanup after the pass completes.
///
/// Components insert their keys here; the owning context reads the set after
/// the sync pass and removes stale entries from its subsystems.
pub struct LiveKeySet {
    keys: HashSet<String>,
}

impl LiveKeySet {
    pub fn new() -> Self {
        Self {
            keys: HashSet::new(),
        }
    }

    pub fn insert(&mut self, key: String) {
        self.keys.insert(key);
    }

    pub fn contains(&self, key: &str) -> bool {
        self.keys.contains(key)
    }

    pub fn clear(&mut self) {
        self.keys.clear();
    }

    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.keys.iter().map(|s| s.as_str())
    }

    pub fn inner(&self) -> &HashSet<String> {
        &self.keys
    }

    pub fn retain_keys(&mut self, other: &Self) {
        self.keys.retain(|k| other.keys.contains(k));
    }
}

/// Convenience macro to get a registered subsystem from a context, panicking
/// if it is not found.
///
/// # Example
///
/// ```ignore
/// let renderer = get_subsystem!(context, helio::Renderer);
/// renderer.scene_mut().insert_actor(actor);
/// ```
#[macro_export]
macro_rules! get_subsystem {
    ($ctx:expr, $ty:ty) => {
        $crate::ComponentRuntimeContext::subsystems_mut(&mut *$ctx)
            .get_mut::<$ty>()
            .expect(concat!(
                "Subsystem `",
                stringify!($ty),
                "` is not registered"
            ))
    };
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
    /// Access the type-erased subsystem registry.
    ///
    /// Components use this together with [`get_subsystem!`] to retrieve
    /// concrete subsystems such as the renderer, physics engine, etc.
    fn subsystems_mut(&mut self) -> &mut Subsystems;

    /// Project root for resolving relative asset paths.
    fn project_root(&self) -> &std::path::Path;

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

/// Marker trait for structs that are sub-property groups, not standalone components.
///
/// Implemented automatically by `#[engine_class(no_register, ...)]`. Used by the parent
/// component's derive to enforce that `#[sub_props]` fields are never accidentally
/// registered as top-level components.
pub trait EngineSubProps: EngineClass {}

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

    /// Optional hex color for the property category header (e.g., "#58A6FF").
    pub category_color: Option<&'static str>,

    /// Whether this property's category should start collapsed in UI.
    pub category_default_collapsed: bool,

    /// Order index from declared `#[category(...)]` attributes.
    pub category_order: Option<usize>,

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
            .field("category_color", &self.category_color)
            .field(
                "category_default_collapsed",
                &self.category_default_collapsed,
            )
            .field("category_order", &self.category_order)
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
