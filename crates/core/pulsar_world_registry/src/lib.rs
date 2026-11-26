//! Bridges reflection-typed components into `pulsar_scenedb::World`, so SceneDB, Helio
//! and the reflection/properties-panel path share one typed store.
//!
//! ## What this solves
//!
//! Before this crate, a component's *live* runtime shape was always
//! `serde_json::Value`, re-deserialized into its typed struct on every
//! rendered frame by a JSON dispatch.
//!
//! `#[register_world_component]` (`engine_class_derive`) lets a component opt
//! into this crate's registry, which provides:
//! - **Erased values** ([`values`]): a default factory, a JSON boundary
//!   decoder, a clone, and insertion of the resulting owned value through
//!   SceneDB's type-erased `World::insert_dyn`, which runs exactly the write
//!   hooks a typed insert runs. Hydrating a component is decode + erased
//!   insert; nothing class-specific happens at insertion.
//! - **`remove`**: drop the typed value when the component is deleted,
//!   disabled, or its owning object is despawned.
//! - **[`GpuMirrored`]**: the GPU companion layout auto-derived by
//!   `engine_class_derive` for any `#[property]` field marked `#[gpu]`
//!   (packed/fixed-size scalars). It is not a component: the authored type's
//!   own SceneDB GPU dispatch derives it and writes its row on every insert,
//!   write, removal and mirror replay. Var-len (`Vec<T>`) and heavy storage shapes
//!   are SceneDB's own `#[derive(SceneStore)]` concern exclusively -- this
//!   crate deliberately has no opinion on them (see the engine-class GPU
//!   de-duplication audit). See [`GpuMirrored`]'s doc for the full design
//!   and packing rules.
//!
//! ## Why this crate exists instead of extending `pulsar_reflection` directly
//!
//! `pulsar_reflection` is a separate repository, also used by non-SceneDB
//! contexts (the Blueprint and shader editors reuse its property-reflection
//! machinery), so it deliberately has no dependency on `pulsar_scenedb` --
//! and never will, regardless of how much friction pinning `pulsar_scenedb`
//! itself does or doesn't have day to day (that pin has moved several times
//! without incident since this crate was first written; the constraint here
//! was always about the dependency direction, not about pin friction).
//! Every actual consumer of this crate (`helio_component`, the plugins'
//! component crates, `engine_backend`, `ui_level_editor`) already depends on both
//! `pulsar_scenedb` and `pulsar_reflection` directly, so a small
//! Pulsar-Native-internal crate sitting alongside them -- itself depending
//! on both -- is a clean fit with no cross-repo coordination needed at all.
//!
//! ## The runtime-behavior dispatch is gone
//!
//! The JSON runtime-behavior dispatch
//! (`ComponentRuntimeBehavior::sync_component`) and this crate's typed
//! `dispatch` path are gone: no registration or stub remains,
//! and `pulsar_reflection` no longer defines the API. Components reach their
//! consumers through their data (SceneDB GPU rows and graph-owned
//! derivations); `#[register_world_component]` names the class after the
//! type.

#![warn(clippy::undocumented_unsafe_blocks)]

// Re-exported so `#[register_world_component]` (`engine_class_derive`) can
// emit `pulsar_world_registry::inventory::submit! { .. }` in the calling
// crate without that crate needing its own direct `inventory` dependency --
// the same pattern `pulsar_reflection` uses for its own registrations.
pub use inventory;
/// Re-exported so `#[register_component_runtime]`'s expansion can name the
/// lifecycle table types without its own dependency.
pub use pulsar_component_lifecycle;
/// Re-exported so generated component code can name the instance types
/// (`ComponentOwner`) without its own dependency.
pub use pulsar_scene_model;
/// VM declarations/codecs referenced by generated component event metadata.
pub use pulsar_script_vm;
pub use unfinished::{
    unfinished_component, unfinished_components, UnfinishedComponentRegistration,
};

mod component_lifecycle;
pub use component_lifecycle::{
    end_live_components, end_session, process_component_removals, process_component_removals_mut,
    pump_construct, tick_live_components, ComponentInstanceKey, ComponentRuntimeState,
    ConstructStats, Session,
};
pub use pulsar_component_lifecycle::{
    ConstructInfo, ConstructReason, SessionKind, SessionScope, ViewPose,
};

mod classes;
mod component_events;
mod gpu_mirror;
mod registration;
#[cfg(test)]
mod tests;

pub mod audit;
pub mod change_watch;
pub mod dispatch;
mod engine_class_mut;
pub mod errors;
pub mod instances;
pub mod marshal;
pub mod object_feed;
pub mod runtime;
pub mod scene_payloads;
pub mod scene_subsystems;
pub mod systems;
pub mod type_shims;
pub mod unfinished;
pub mod values;
// Linked so the math value types and natives are in every host that builds
// the script registry; nothing references them by name.
use pulsar_script_math as _;
mod script_natives;

// The unified reflection dispatcher and its property accessors --
// THE entry points every scripting backend (VM opcodes, generated code,
// graph nodes) uses to touch live World components. No bespoke dispatch
// downstream.
pub use dispatch::{
    get_component_property, get_component_property_boxed, invoke_component_method,
    property_descriptor, set_component_property, set_component_property_boxed,
};
// The one script-facing error taxonomy. Canonical home is this
// crate (next to the dispatcher whose failures these are);
// `pulsar_script_object_model::errors` re-exports it unchanged.
pub use engine_class_mut::EngineClassMut;
pub use errors::ScriptRefError;
// Marshalling: JSON ⇄ Box<dyn Any>.
pub use instances::{
    attach_component, attach_record, attach_record_or_unresolved, attach_records,
    attach_unresolved, attach_value, component_metadata_records, component_records,
    duplicate_instance, duplicate_instances, instance_engine_class, instance_metadata_record,
    instance_record, place_component, replace_records, restore_instances, set_instance_value,
    snapshot_instance, snapshot_instances, AttachError, ComponentPayload, InstanceSnapshot,
    InstanceValue,
};
pub use marshal::{any_to_json, json_to_any};
pub use values::{
    clone_value, clone_world_component_value, decode_json, decode_world_component_value,
    insert_world_component_value, new_world_component_value, set_value_property,
    set_world_component_property, value_engine_class, ComponentValueError,
};
// Metadata audit: overload sweep + the deterministic registry
// snapshot CI golden tests diff against.
pub use audit::{
    find_lifecycle_problems, find_overloaded_methods, lifecycle_leaks, metadata_snapshot_json,
    LifecycleAuditError, MetadataAuditError,
};
pub use change_watch::ComponentWatch;
pub use object_feed::{ObjectDelta, ObjectFeed, ObjectUpdate};
pub use scene_payloads::{scene_payloads, scene_settling, ScenePayloadRegistration, ScenePayloads};
pub use scene_subsystems::{install_scene_subsystems, SceneSubsystemRegistration};
pub use systems::{System, SystemContext, SystemRegistration};

pub use classes::{
    class_problems, component_id_for_class, components_with_method,
    get_world_component_as_engine_class, get_world_component_as_engine_class_mut,
    hydrate_world_component_for_class, registered_world_component_classes,
    remove_world_component_for_class, required_class, world_component, RequiredClass,
};
pub(crate) use classes::{find, find_by_component_id};
pub use component_events::{
    component_event_registrations, component_event_sources, flush_component_events,
    queue_component_event, register_component_events, ComponentContext, ComponentEventOutbox,
    ComponentEventRegistration, ComponentEventSourceRegistration, ComponentEventWriter,
    DynamicEvent, QueuedComponentEvent,
};
pub use gpu_mirror::{GpuMirrored, GpuRepr, NoGpuMirror};
pub use registration::{Presentation, WorldComponentClass, WorldComponentRegistration};
