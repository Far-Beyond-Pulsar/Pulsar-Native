//! The installed plugins: every plugin under `plugins/vendor` that
//! describes itself (`pulsar-plugin.toml`), as one crate to link.
//!
//! - The editor links it with `editor` (its editor modules too): an editor
//!   serves every project, so it has every installed plugin, and the project
//!   it opens enables some of them (`pulsar_plugins::activate_project`).
//! - A game built for a project links only that project's enabled runtime
//!   modules (`engine_backend::services::core_project_builder`), not this.
//! - Tests that need every plugin (a game with all of them) link it without
//!   `editor`.
//!
//! No host names a plugin: what is here is generated from the descriptors
//! (`tests/installed_set.rs` checks it). Each module is re-exported, which
//! also links it, so its registrations are present.

// BEGIN installed plugins: generated from plugins/vendor/*/pulsar-plugin.toml
// by `UPDATE_INSTALLED_PLUGINS=1 cargo test -p installed_plugins --test installed_set`.
#[cfg(feature = "editor")]
pub use skeletal_animation_plugin;
#[cfg(feature = "editor")]
pub use asset_viewer_plugin;
#[cfg(feature = "editor")]
pub use blueprint_editor_plugin;
#[cfg(feature = "editor")]
pub use script_editor_plugin;
pub use decals_component;
#[cfg(feature = "editor")]
pub use decals_plugin;
pub use fog_component;
#[cfg(feature = "editor")]
pub use fog_plugin;
pub use foliage_component;
#[cfg(feature = "editor")]
pub use foliage_plugin;
pub use gi_component;
#[cfg(feature = "editor")]
pub use gi_plugin;
pub use input_plugin;
pub use lens_component;
#[cfg(feature = "editor")]
pub use lens_plugin;
pub use lights_component;
#[cfg(feature = "editor")]
pub use lights_plugin;
pub use materials_component;
#[cfg(feature = "editor")]
pub use materials_plugin;
#[cfg(feature = "editor")]
pub use plugin_matter;
pub use meshes_component;
#[cfg(feature = "editor")]
pub use meshes_plugin;
pub use particles_component;
#[cfg(feature = "editor")]
pub use particles_plugin;
pub use physics_plugin;
pub use portals_component;
#[cfg(feature = "editor")]
pub use portals_plugin;
pub use post_component;
#[cfg(feature = "editor")]
pub use post_plugin;
#[cfg(feature = "editor")]
pub use render_bps_plugin;
pub use sky_component;
#[cfg(feature = "editor")]
pub use sky_plugin;
pub use splines_component;
#[cfg(feature = "editor")]
pub use splines_plugin;
pub use sprites_component;
#[cfg(feature = "editor")]
pub use sprites_plugin;
#[cfg(feature = "editor")]
pub use editor_table_plugin;
#[cfg(feature = "editor")]
pub use ui_editor_plugin;
pub use voxel_component;
#[cfg(feature = "editor")]
pub use voxel_plugin;
pub use water_component;
#[cfg(feature = "editor")]
pub use water_plugin;
// END installed plugins
