//! Decals (`pulsar.decals`): the editor plugin.
//!
//! The plugin's identity ([`DecalsPlugin`]). The components, their methods,
//! rows and the renderer feature's registrations are the component crate's
//! (`decals_component`), which this crate links: the editor links this
//! crate (through `installed_plugins`, as an installed plugin's editor module) and knows no decals type. The
//! inspector edits the components through their reflected properties.

use plugin_editor_api::*;

/// Links the component crate, so its registrations come with this crate.
pub use decals_component as component;

/// The plugin's identity.
#[derive(Default)]
pub struct DecalsPlugin;

impl DecalsPlugin {
    /// The plugin id.
    pub const ID: &'static str = "pulsar.decals";
}

impl EditorPlugin for DecalsPlugin {
    fn metadata(&self) -> PluginMetadata {
        // The plugin's identity is its descriptor's (pulsar-plugin.toml).
        let descriptor = decals_component::plugin_descriptor();
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
        assert_eq!(DecalsPlugin.metadata().id.as_str(), "pulsar.decals");
    }
}
