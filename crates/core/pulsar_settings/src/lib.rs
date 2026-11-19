//! Pulsar Engine settings registry.
//!
//! Call [`register_all_settings`] once at startup to populate the global
//! [`pulsar_config::ConfigManager`] with every built-in schema.

pub mod editor;
pub mod plugin;
pub mod project;

use pulsar_config::ConfigManager;

/// Register every built-in editor and project setting schema.
///
/// This must be called before any UI or subsystem reads from the config.
/// It also binds the plugin settings ([`plugin`]) to `cfg`.
pub fn register_all_settings(cfg: &'static ConfigManager) {
    plugin::bind(cfg);
    editor::register_all(cfg);
    project::register_all(cfg);
}
