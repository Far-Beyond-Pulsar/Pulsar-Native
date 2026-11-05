//! Lights (`pulsar.lights`): the editor plugin.
//!
//! The plugin's identity ([`LightsPlugin`]). The components and every
//! registration are the component crate's (`lights_component`), which this
//! crate links: the editor links this crate (through `installed_plugins`, as an installed plugin's editor module).

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
        // The plugin's identity is its descriptor's (pulsar-plugin.toml).
        let descriptor = lights_component::plugin_descriptor();
        PluginMetadata {
            id: PluginId::new(descriptor.id),
            name: descriptor.name,
            version: "0.1.0".into(),
            author: "Pulsar Team".into(),
            description: descriptor.description,
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
