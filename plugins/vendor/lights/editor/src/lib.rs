//! Lights (`pulsar.lights`): the editor plugin.
//!
//! The plugin's identity ([`LightsPlugin`]). The components and every
//! registration are the component crate's (`lights_component`), which this
//! crate links: the editor links this crate (`use lights_plugin as _`).

use plugin_editor_api::*;

/// Links the component crate, so its registrations come with this crate.
pub use lights_component as component;

/// The plugin's identity.
#[derive(Default)]
pub struct LightsPlugin;

impl LightsPlugin {
    /// The plugin id.
    pub const ID: &'static str = "pulsar.lights";
}

impl EditorPlugin for LightsPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            id: PluginId::new(Self::ID),
            name: "Lights".into(),
            version: "0.1.0".into(),
            author: "Pulsar Team".into(),
            description: "Lights and reflection captures: the light list, shadow views, the lighting models, IES profiles and light icons".into(),
        }
    }

    fn file_types(&self) -> Vec<FileTypeDefinition> {
        Vec::new()
    }

    fn editors(&self) -> Vec<EditorMetadata> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_the_plugin_id() {
        assert_eq!(LightsPlugin.metadata().id.as_str(), "pulsar.lights");
    }
}
