pub mod graphics;
pub mod rendering;
pub mod streaming;

use pulsar_config::ConfigManager;

/// A plugin's project settings namespace (the meshes' `project.virtual_geometry`):
/// registered with the rest, while the plugin is enabled. The plugin's
/// component crate submits one; this crate names no plugin's settings.
pub struct ProjectSettingsRegistration {
    /// The package that submitted it (`pulsar_plugins::origin!()`).
    pub origin: &'static str,
    pub register: fn(&'static ConfigManager),
}

inventory::collect!(ProjectSettingsRegistration);

impl pulsar_plugins::Registration for ProjectSettingsRegistration {
    fn origin(&self) -> &'static str {
        self.origin
    }
}

pub fn register_all(cfg: &'static ConfigManager) {
    graphics::register(cfg);
    rendering::register(cfg);
    streaming::register(cfg);
    for registration in pulsar_plugins::registered::<ProjectSettingsRegistration>() {
        (registration.register)(cfg);
    }
}
