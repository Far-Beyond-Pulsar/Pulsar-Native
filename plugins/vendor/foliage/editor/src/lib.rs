//! Foliage (`pulsar.foliage`): the editor plugin.
//!
//! The plugin's identity ([`FoliagePlugin`]). The components, their methods,
//! rows and the renderer feature's registrations are the component crate's
//! (`foliage_component`), which this crate links: the editor links this
//! crate (`use foliage_plugin as _`) and knows no foliage type. The
//! inspector edits the components through their reflected properties.

use plugin_editor_api::*;

/// Links the component crate, so its registrations come with this crate.
pub use foliage_component as component;

/// The plugin's identity.
#[derive(Default)]
pub struct FoliagePlugin;

impl FoliagePlugin {
    /// The plugin id.
    pub const ID: &'static str = "pulsar.foliage";
}

impl EditorPlugin for FoliagePlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            id: PluginId::new(Self::ID),
            name: "Foliage".into(),
            version: "0.1.0".into(),
            author: "Pulsar Team".into(),
            description: "Foliage and the level's wind: their components, rows, settings and the renderer feature that places and draws them".into(),
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
        assert_eq!(FoliagePlugin.metadata().id.as_str(), "pulsar.foliage");
    }
}
