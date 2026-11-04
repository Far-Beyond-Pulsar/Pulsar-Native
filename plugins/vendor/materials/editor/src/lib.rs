//! Materials (`pulsar.materials`): the editor plugin.
//!
//! The plugin's identity ([`MaterialsPlugin`]). The components and every
//! registration are the component crate's (`materials_component`), which this
//! crate links: the editor links this crate (`use materials_plugin as _`).

use plugin_editor_api::*;

/// Links the component crate, so its registrations come with this crate.
pub use materials_component as component;

/// The plugin's identity.
#[derive(Default)]
pub struct MaterialsPlugin;

impl MaterialsPlugin {
    /// The plugin id.
    pub const ID: &'static str = "pulsar.materials";
}

impl EditorPlugin for MaterialsPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            id: PluginId::new(Self::ID),
            name: "Materials".into(),
            version: "0.1.0".into(),
            author: "Pulsar Team".into(),
            description: "Materials: the material row and texture table every kind draws with, the PBR evaluation and the Radiant material templates".into(),
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
        assert_eq!(MaterialsPlugin.metadata().id.as_str(), "pulsar.materials");
    }
}
