//! The installed plugins: every plugin under `plugins/vendor` that
//! describes itself (`pulsar-plugin.toml`), as one crate to link. Each
//! plugin is reached through its root crate only, the one place it
//! registers what it contributes.
//!
//! - The editor links it with `editor`: every root with its `editor`
//!   feature (the roots' editor registrations), and the editor tools. An
//!   editor serves every project, so it has every installed plugin, and the
//!   project it opens enables some of them
//!   (`pulsar_plugins::activate_project`).
//! - A game built for a project links only that project's enabled roots,
//!   without `editor` (`engine_backend::services::core_project_builder`),
//!   not this.
//! - Tests that need every plugin (a game with all of them) link it without
//!   `editor`.
//!
//! No host names a plugin: what is here is generated from the descriptors
//! (`tests/installed_set.rs` checks it). Each root is re-exported, which
//! also links it, so its registrations are present.

// BEGIN installed plugins: generated from plugins/vendor/*/pulsar-plugin.toml
// by `UPDATE_INSTALLED_PLUGINS=1 cargo test -p installed_plugins --test installed_set`.
#[cfg(feature = "editor")]
pub use asset_viewer_plugin;
#[cfg(feature = "editor")]
pub use blueprint_editor_plugin;
pub use character_plugin;
pub use decals_plugin;
#[cfg(feature = "editor")]
pub use editor_table_plugin;
pub use fog_plugin;
pub use foliage_plugin;
pub use gi_plugin;
pub use input_plugin;
pub use lens_plugin;
pub use lights_plugin;
pub use materials_plugin;
pub use meshes_plugin;
pub use particles_plugin;
pub use physics_plugin;
#[cfg(feature = "editor")]
pub use plugin_matter;
#[cfg(feature = "editor")]
pub use plugin_typescript;
pub use portals_plugin;
pub use post_plugin;
#[cfg(feature = "editor")]
pub use render_bps_plugin;
pub use render_pipelines_plugin;
#[cfg(feature = "editor")]
pub use script_editor_plugin;
pub use skeletal_animation_plugin;
pub use sky_plugin;
pub use splines_plugin;
pub use sprites_plugin;
#[cfg(feature = "editor")]
pub use trait_editor_plugin;
#[cfg(feature = "editor")]
pub use ui_editor_plugin;
pub use voxel_plugin;
pub use water_plugin;
// END installed plugins
