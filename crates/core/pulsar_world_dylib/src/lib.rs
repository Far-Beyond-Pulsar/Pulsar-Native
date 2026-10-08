//! The world crates as one shared library (Pulsar-Native#1035, #1081).
//!
//! A plugin is a separately loaded library. Linked statically, it carries
//! its own copy of `pulsar_scenedb`, `pulsar_reflection` and
//! `pulsar_world_registry`: its component ids, `inventory` registries and
//! reflection tables are not the editor's, so a component class it defines
//! can never be a live component of the editor's `World`.
//!
//! An executable or plugin that depends on this crate (and links it, e.g.
//! `use pulsar_world_dylib as _;`) instead reaches all of them through this
//! one `dylib`. Code keeps naming the crates themselves -- derive macros
//! included -- and rustc resolves each to the copy inside this library. A
//! plugin loaded into such a host registers its classes into the host's
//! registries when it is loaded.
//!
//! Requirement: the host and its plugins are built by the same compiler,
//! with the same features for these crates, so the library they name is
//! the same one. The standard library is then linked dynamically, which
//! leaves a binary's `#[global_allocator]` serving only the generic code
//! instantiated in that binary. The editor does not link this crate yet;
//! `plugin_manager/tests/world_component_plugins.rs` covers both effects.

pub use inventory;
pub use pulsar_reflection;
pub use pulsar_scene_model;
pub use pulsar_scenedb;
pub use pulsar_world_registry;
