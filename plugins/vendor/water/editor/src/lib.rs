//! Water (`pulsar.water`): the editor plugin.
//!
//! The plugin's identity ([`WaterPlugin`]). The components, their methods,
//! rows and the renderer feature's registrations are the component crate's
//! (`water_component`), which this crate links: the editor links this
//! crate (through `installed_plugins`, as an installed plugin's editor module) and knows no water type. The
//! inspector edits the components through their reflected properties.

/// The plugin's level editor UI, registered through the level editor's
/// extension points (`ui_level_editor::extend`).
pub mod level_editor;

use plugin_editor_api::*;

/// Links the component crate, so its registrations come with this crate.
pub use water_component as component;

/// The plugin's identity.
#[derive(Default)]
pub struct WaterPlugin;

impl WaterPlugin {
    /// The plugin id.
    pub const ID: &'static str = "pulsar.water";
}

impl EditorPlugin for WaterPlugin {
    fn metadata(&self) -> PluginMetadata {
        // The plugin's identity is its descriptor's (pulsar-plugin.toml).
        let descriptor = water_component::plugin_descriptor();
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
        assert_eq!(WaterPlugin.metadata().id.as_str(), "pulsar.water");
    }
}
