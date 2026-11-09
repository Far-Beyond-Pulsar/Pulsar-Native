pub mod graphics;
pub mod rendering;
pub mod streaming;

use pulsar_config::ConfigManager;

/// A plugin's project settings namespace (the meshes' `project.virtual_geometry`):
/// registered with the rest, while the plugin is enabled. The plugin's
/// root crate submits one; this crate names no plugin's settings.
pub struct ProjectSettingsRegistration {
    /// The package that submitted it (`pulsar_plugins::origin!()`).
    pub origin: &'static str,
    /// Stable id of the registration within its plugin (`"meshes.import"`): what
    /// the plugin catalog lists it by.
    pub name: &'static str,
    pub register: fn(&'static ConfigManager),
}

impl pulsar_plugins::Contribution for ProjectSettingsRegistration {
    const CATEGORY: &'static str = "settings.project";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        self.name.to_string()
    }
}

pulsar_plugins::registration!(ProjectSettingsRegistration);

pub fn register_all(cfg: &'static ConfigManager) {
    graphics::register(cfg);
    rendering::register(cfg);
    streaming::register(cfg);
    for registration in pulsar_plugins::registered::<ProjectSettingsRegistration>() {
        (registration.register)(cfg);
    }
}
