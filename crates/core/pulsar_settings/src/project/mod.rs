//! Project settings: the graphics, rendering and streaming groups. `register_all` adds them
//! to a `ConfigManager`.

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
        let before = owners(cfg);
        (registration.register)(cfg);
        for owner in owners(cfg).difference(&before) {
            record_origin(owner, registration.origin);
        }
    }
}

fn owners(cfg: &ConfigManager) -> std::collections::BTreeSet<String> {
    cfg.list_owners(NS)
        .into_iter()
        .map(|owner| owner.join("/"))
        .collect()
}

/// The namespace of every project setting.
pub const NS: &str = "project";

static ORIGINS: std::sync::LazyLock<
    std::sync::RwLock<std::collections::BTreeMap<String, &'static str>>,
> = std::sync::LazyLock::new(Default::default);

/// Remember that a plugin's registration added project owner `owner`.
pub(crate) fn record_origin(owner: &str, origin: &'static str) {
    if let Ok(mut origins) = ORIGINS.write() {
        origins.insert(owner.to_string(), origin);
    }
}

/// The package whose registration added the project settings owner `owner`
/// (`pulsar_plugins::origin!()` of a plugin), or `None` for the engine's own
/// project settings. The Project Settings window files a plugin's owners
/// under that plugin.
pub fn owner_origin(owner: &str) -> Option<&'static str> {
    ORIGINS.read().ok()?.get(owner).copied()
}
