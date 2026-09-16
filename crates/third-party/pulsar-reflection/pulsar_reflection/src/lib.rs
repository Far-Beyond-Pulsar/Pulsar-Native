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

// Dynamic (non-`EngineClass`) method dispatch — subsystems, singletons, etc.
pub mod dyn_registry;

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

// Re-export dynamic (non-`EngineClass`) method dispatch
pub use dyn_registry::{
    DynDispatchError, DynMethodArgs, DynMethodCaller, DynMethodMetadata, DynMethodRegistration,
    DynMethodRegistry, DynMethodReturnValue, DYN_METHOD_REGISTRY,
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

// Re-export generic enum dropdown editor (auto-registered for all enums)
#[cfg(feature = "prims-gpui")]
pub use prims::core::enum_dropdown::enum_dropdown_editor;

/// Write-back callback: hands a new, concretely-typed value back to whatever
/// owns the underlying data (scene database, prefab asset, node property, …).
///
/// The `Box<dyn Any + Send>` always holds the property's own Rust type — the
/// same type its editor was registered for — so the receiving side can round
/// it through [`RuntimeTypeRegistry::serialize_json_for_any`] without knowing
/// what that type is.
#[cfg(feature = "prims-gpui")]
pub type PropertyWriteBack =
    Arc<dyn Fn(Box<dyn Any + Send>, &mut gpui::Window, &mut gpui::App) + Send + Sync>;

/// Arguments handed to a property editor's [factory](PropertyEditorFactory)
/// when it is first constructed for a given property.
///
/// Deliberately carries *no* widget state: an editor creates and owns whatever
/// child entities it needs, so the reflection crate — and the framework layer
/// above it — never learn that widgets exist at all.
pub struct PropertyEditorArgs<'a> {
    pub id_prefix: &'a str,
    pub class_name: &'a str,
    pub display_name: &'a str,
    pub prop_name: &'a str,
    pub type_info: &'static RuntimeTypeInfo,
    /// Current value as a type-erased reference.  Downcast to the concrete
    /// type this editor was registered for.
    ///
    /// This is the *only* value channel — there is deliberately no JSON view.
    /// Serialisation is the data source's concern, not the editor's.
    pub current_value: &'a dyn Any,
    #[cfg(feature = "prims-gpui")]
    pub write_back: PropertyWriteBack,
}

// ── Bound editor instances ────────────────────────────────────────────────────

/// A live property-editor instance: the renderable view plus a type-erased
/// hook for pushing a fresh value into it.
///
/// The framework caches one of these per property and calls `set_value` on
/// every render so externally-driven changes (undo, a viewport drag, another
/// panel) still reach the editor.  `set_value` receives the value as
/// `&dyn Any`; only the editor that produced the closure knows the concrete
/// type, which is what keeps widget knowledge out of the framework.
#[cfg(feature = "prims-gpui")]
#[derive(Clone)]
pub struct BoundPropertyEditor {
    /// Type-erased renderable handle to the editor entity.
    pub view: gpui::AnyView,
    /// Push a fresh value into the editor.  A no-op if the value is unchanged.
    pub set_value: Arc<dyn Fn(&dyn Any, &mut gpui::Window, &mut gpui::App) + Send + Sync>,
}

#[cfg(feature = "prims-gpui")]
impl BoundPropertyEditor {
    /// Bind an editor entity together with a typed value-setter.
    ///
    /// `set` is invoked only when the incoming `&dyn Any` actually holds a `T`,
    /// so editors never have to write downcasting boilerplate themselves.
    pub fn new<T, V>(
        entity: gpui::Entity<V>,
        set: impl Fn(&mut V, &T, &mut gpui::Window, &mut gpui::Context<V>) + Send + Sync + 'static,
    ) -> Self
    where
        T: 'static,
        V: gpui::Render,
    {
        let for_set = entity.clone();
        Self {
            view: entity.into(),
            set_value: Arc::new(move |value, window, cx| {
                let Some(typed) = value.downcast_ref::<T>() else {
                    return;
                };
                for_set.update(cx, |editor, cx| set(editor, typed, window, cx));
            }),
        }
    }

    /// Bind an editor entity that has no value to refresh (read-only or
    /// self-contained editors).
    pub fn stateless<V: gpui::Render>(entity: gpui::Entity<V>) -> Self {
        Self {
            view: entity.into(),
            set_value: Arc::new(|_, _, _| {}),
        }
    }
}

/// Constructs a property editor for one property.
///
/// Called once per `(owner, property)` pair; the resulting
/// [`BoundPropertyEditor`] is cached and reused for the lifetime of that pair.
/// The `&mut App` is enough to call `cx.new(...)`, which hands the editor a
/// correctly-typed `&mut Context<Self>` for creating child entities and
/// registering its own subscriptions.
#[cfg(feature = "prims-gpui")]
pub type PropertyEditorFactory =
    fn(&PropertyEditorArgs<'_>, &mut gpui::Window, &mut gpui::App) -> BoundPropertyEditor;

// ── UI property-editor hint ───────────────────────────────────────────────────

/// Type-erased hint that a concrete type has a registered GPUI property editor.
///
/// Submitted via `inventory::submit!` — either directly or by the
/// `editor = fn` argument on [`pulsar_type`].
///
/// The `fn_ptr` field stores a function pointer erased to `fn()` so that this
/// type remains free of GPUI dependencies.  The framework layer (`ui_common`)
/// is responsible for transmuting it back to [`PropertyEditorFactory`].
///
/// Storing as `fn()` (rather than `usize`) allows the `inventory::submit!`
/// static-initialiser to compile without triggering E0658 (fn ptr → int cast
/// is forbidden in const context since Rust 1.83).
///
/// # Safety contract
///
/// Only submit function pointers whose actual Rust type is
/// [`PropertyEditorFactory`].  The transmute in `ui_common` is safe by
/// construction as long as this invariant is upheld — which
/// [`erase_property_editor_fn_ptr`] enforces at compile time.
pub struct UiPropertyEditorHint {
    /// [`TypeId`](std::any::TypeId) of the type this editor handles.
    pub type_id: std::any::TypeId,
    /// Erased function pointer — actual type is [`PropertyEditorFactory`].
    /// Cast to `fn()` so it can appear in `const` / `static` initialisers.
    pub fn_ptr: fn(),
}

inventory::collect!(UiPropertyEditorHint);

/// Erase a [`PropertyEditorFactory`] to the opaque `fn()` stored in
/// [`UiPropertyEditorHint::fn_ptr`].
///
/// All Rust function pointer types are pointer-sized, so transmuting between
/// them preserves size.  The input is intentionally constrained to the one
/// valid factory signature so registration sites fail fast at compile time
/// when a mismatched function is provided.
///
/// This is a `const fn` so it can be called inside `inventory::submit!`
/// static initialisers emitted by proc macros.
///
/// Gated behind `prims-gpui` (M3-alpha Task 2): the signature is GPUI-typed,
/// so it can only exist when the crate is compiled with GPUI available.
#[cfg(feature = "prims-gpui")]
pub const fn erase_property_editor_fn_ptr(f: PropertyEditorFactory) -> fn() {
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
///
/// `scene_object_id` deliberately stays `&'a str` in *this* change --
/// switching it to a numeric identity (e.g. `pulsar_scenedb::Entity::bits()`)
/// is a separate migration with its own blast radius (every
/// `RuntimeComponentOwner` construction site, plus every `sync_component`
/// body that currently formats/logs/keys off the string), scoped to
/// whichever engine-side phase actually replaces string scene-object ids
/// with real entity identity -- not bundled into the `component_data: &Value
/// -> component: &Self` change this trait signature makes here. Splitting
/// them keeps this change purely about JSON leaving the live path, with no
/// forced changes to callers that haven't made that separate identity
/// migration yet.
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
///
/// `sync_component` takes `component: &Self` -- a real, typed value, not
/// `serde_json::Value` -- so an implementation never hand-parses JSON: it
/// reads its own fields directly. The dispatch boundary this trait is
/// reached through ([`RuntimeBehaviorRegistration`]/
/// [`apply_runtime_behavior_for_class`]) still deals in `&serde_json::Value`
/// today, because most embedding-engine callers (e.g. a scene-file loader)
/// only have JSON on hand at the point they know *which* component class
/// they're looking at -- deserializing into the concrete `Self` is exactly
/// what a per-class dispatch step is for. `#[register_runtime_behavior]`
/// generates that one `serde_json::from_value::<Self>(..)` call for you (see
/// [`RuntimeBehaviorRegistration`]'s doc), so this is still a single,
/// centralized JSON boundary -- not, as before this trait's `component`
/// parameter was typed, one hand-rolled `.as_object().and_then(...)` parser
/// per component. A caller whose data is *already* a live `Self` (e.g. a
/// `World`/ECS-backed engine) can skip JSON entirely by calling
/// `Self::sync_component` directly instead of going through
/// `apply_runtime_behavior_for_class` -- the JSON step is this dispatch
/// path's concern, not the trait's.
pub trait ComponentRuntimeBehavior {
    /// Reflection class name this runtime behavior handles.
    const CLASS_NAME: &'static str;

    /// Sync one component instance into runtime systems.
    fn sync_component(
        owner: &RuntimeComponentOwner,
        component_index: usize,
        component: &Self,
        context: &mut dyn ComponentRuntimeContext,
    );
}

/// Inventory registration entry for runtime behavior handlers.
///
/// `sync`'s third parameter is still `&serde_json::Value` -- this is the
/// type-erased dispatch boundary (`inventory::submit!` requires a concrete,
/// non-generic static value, so a generic `fn sync_component` can't be
/// stored here directly the way it can on [`ComponentRuntimeBehavior`]
/// itself), and JSON is the one payload shape every registration can accept
/// without knowing the other components' concrete types.
/// `#[register_runtime_behavior]` generates the small
/// deserialize-and-forward shim that bridges `&Value` to the real typed
/// `sync_component(component: &Self, ..)`, so component authors never write
/// that conversion by hand.
pub struct RuntimeBehaviorRegistration {
    pub class_name: &'static str,
    pub sync: fn(&RuntimeComponentOwner, usize, &Value, &mut dyn ComponentRuntimeContext),
}

inventory::collect!(RuntimeBehaviorRegistration);

/// Apply registered runtime behavior for one component class.
///
/// The matching registration's `sync` shim deserializes `component_data`
/// into that class's concrete type before calling the real, typed
/// `ComponentRuntimeBehavior::sync_component` -- a parse failure is reported
/// via [`ComponentRuntimeContext::report_error`], not a panic.
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

    /// Serialize this instance's *current* field values to JSON, in the same
    /// shape a component's own `#[derive(Serialize)]` produces (including
    /// `#[sub_props]` nesting -- this is the whole struct, not a per-property
    /// flat map).
    ///
    /// Overridden by `#[engine_class(..., serialize, ...)]`'s generated impl
    /// for classes that actually derive `Serialize`. The default here exists
    /// only so adding this method isn't a breaking change for classes that
    /// don't -- callers (the properties panel, `SceneDatabase`) that need
    /// this to succeed should treat `Err` as "this class doesn't support the
    /// typed JSON round trip," not as a bug.
    fn to_json(&self) -> Result<Value, String> {
        Err(format!(
            "{} does not support to_json (no `serialize` on #[engine_class(...)])",
            std::any::type_name::<Self>()
        ))
    }
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
