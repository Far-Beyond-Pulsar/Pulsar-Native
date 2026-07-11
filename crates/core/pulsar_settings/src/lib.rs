//! Pulsar Engine settings registry.
//!
//! Call [`register_all_settings`] once at startup to populate the global
//! [`pulsar_config::ConfigManager`] with every built-in schema.

pub mod editor;
pub mod project;

use pulsar_config::ConfigManager;

/// Register every built-in editor and project setting schema.
///
/// This must be called before any UI or subsystem reads from the config.
pub fn register_all_settings(cfg: &'static ConfigManager) {
    editor::register_all(cfg);
    project::register_all(cfg);
}
