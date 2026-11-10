//! Proc macros for `#[engine_class(...)]`/`#[derive(EngineClass)]` --
//! reflection (the properties panel), `World`/SceneDB storage wiring, and
//! GPU mirroring, all generated off ONE struct definition's own
//! `#[property]`/`#[sub_props]`/`#[gpu]` field attributes.
//!
//! # What this crate generates, by attribute
//!
//! - `#[property]` (any field): reflection metadata (`EngineClass::
//!   get_properties`) -- name, category, min/max, a typed getter/setter --
//!   for the properties panel to render an editor for. See the example
//!   below.
//! - `#[sub_props]` (a field whose type is itself `#[engine_class(...,
//!   no_register)]`): flattens that nested type's own properties into the
//!   containing struct's property list, AND (independently) composes its
//!   GPU mirror into the containing struct's, if either has one -- see the
//!   `#[gpu]` bullet.
//! - `#[gpu]` (a `#[property]` field, or a `Vec<T>` field): opts the field
//!   into an auto-generated, `#[derive(pulsar_scenedb::SceneStore)]`-backed
//!   GPU companion layout -- `pulsar_world_registry::GpuMirrored` for a
//!   fixed-size/packed field (numeric primitives and arrays as-is, `bool`/
//!   a plain enum cast to `u32`), `GpuListMirrored` for a `Vec<T>` one (a
//!   SEPARATE companion, deliberately -- see that trait's own doc). Every
//!   `#[engine_class(...)]`-processed struct gets both impls
//!   unconditionally, `NoGpuMirror` when there's nothing to mirror, so
//!   `#[sub_props]` composition never needs special-casing. See
//!   `gpu_mirror_codegen`'s doc for the full packing rules.
//! - `scene_store` (a struct-level `#[engine_class(...)]` flag, not a field
//!   attribute): a DIFFERENT, older mechanism -- routes the WHOLE struct
//!   through `#[derive(SceneStore)]` directly instead of a companion. The
//!   right choice when the struct's `Vec<T>` `#[gpu]` field payload itself
//!   IS the component (`StaticMeshComponent::vertices`/`indices`), not a
//!   translation of some other editor-facing shape.
//! - `#[register_world_component(...)]`: on an inherent `impl Type {}`,
//!   wires the component into `pulsar_world_registry`'s `World`-storage
//!   bridge -- a default factory, a JSON boundary decoder, a clone, erased
//!   SceneDB registration, `remove`, and a
//!   `property_written` normalization hook. The `#[gpu]` companions above
//!   are never inserted or refreshed by anyone: the authored struct's own
//!   SceneDB GPU dispatch derives and writes them on every write. See that
//!   macro's own doc for the full option list.
//!
//! # Example
//!
//! ```ignore
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

use proc_macro::TokenStream;

mod component_events;
mod component_runtime;
mod derive_engine_class;
mod engine_class_attr;
mod field_attrs;
mod gpu;
mod properties;
mod scene_props;
mod world_component;

#[proc_macro_derive(
    EngineClass,
    attributes(
        property,
        category,
        engine_class_category,
        sub_props,
        engine_class_no_register,
        engine_class_serialize,
        engine_class_deserialize,
        engine_class_scene_store,
        engine_class_gpu_rows,
        gpu
    )
)]
pub fn derive_engine_class(input: TokenStream) -> TokenStream {
    derive_engine_class::expand(input)
}

#[proc_macro_attribute]
pub fn engine_class(attr: TokenStream, item: TokenStream) -> TokenStream {
    engine_class_attr::expand(attr, item)
}

#[proc_macro_attribute]
pub fn register_component_runtime(attr: TokenStream, item: TokenStream) -> TokenStream {
    component_runtime::expand(attr, item)
}

/// Declare named events owned by a component. The generated writer method
/// accepts the declared arguments followed by the declared return payload.
/// Each argument and the payload remain their original Rust types and become
/// matching Blueprint pins; the macro uses stable names and versioned codecs
/// only at the Gamma DLL boundary.
///
/// ```ignore
/// #[component_events(class = "VoxelTerrainComponent")]
/// pub trait TerrainEvents {
///     #[bp_event]
///     fn block_broken() -> BlockData {}
///     fn block_changed(x: i32, y: i32, material: String) -> BlockData {}
/// }
/// // `context.events.block_broken(data)` and
/// // `context.events.block_changed(x, y, material, data)` after importing the generated
/// // `TerrainEventWriterExt` trait.
/// ```
#[proc_macro_attribute]
pub fn component_events(attr: TokenStream, item: TokenStream) -> TokenStream {
    component_events::expand(attr, item)
}

/// Opt an engine component into `pulsar_world_registry`'s `World` bridge
/// (Pulsar-Native#555/#556, Phase B4/B5) and register it: its typed value
/// can be hydrated from JSON once (a boundary decode) and inserted into
/// `pulsar_scenedb::World` through SceneDB's erased insert.
///
/// Applied to an inherent `impl Type {}` block (emitted unchanged); the
/// class is named after the type. Components reach their consumers through
/// their data: there is no per-component runtime behavior to register
/// (Pulsar-Native#1035, Phases 4 and 6).
///
/// For the engine's own components. A plugin's component uses
/// [`macro@world_component`] instead, and its root crate registers it.
#[proc_macro_attribute]
pub fn register_world_component(attr: TokenStream, item: TokenStream) -> TokenStream {
    world_component::expand(attr, item, true)
}

/// Make a plugin's component a `World` class, as
/// [`macro@register_world_component`] does, without registering it: the
/// type gets `PULSAR_WORLD_COMPONENT`, its registration, and the plugin's
/// root crate submits it with `pulsar_world_registry::submit_world_component!`
/// in the one place the plugin registers everything (the registration then
/// names the root as its origin, so the plugin gate skips the class while
/// the plugin is disabled). Same options as `register_world_component`.
#[proc_macro_attribute]
pub fn world_component(attr: TokenStream, item: TokenStream) -> TokenStream {
    world_component::expand(attr, item, false)
}

#[proc_macro_attribute]
pub fn register_scene_props_applier(attr: TokenStream, item: TokenStream) -> TokenStream {
    scene_props::expand(attr, item)
}
