//! Decals (`pulsar.decals`): the editor plugin.
//!
//! The plugin's identity ([`DecalsPlugin`]). The components, their methods,
//! rows and the renderer feature's registrations are the component crate's
//! (`decals_component`), which this crate links: the editor links this
//! crate (`use decals_plugin as _`) and knows no decals type. The
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
        PluginMetadata {
            id: PluginId::new(Self::ID),
            name: "Decals".into(),
            version: "0.1.0".into(),
            author: "Pulsar Team".into(),
            description: "Decals projected onto surfaces: their component, rows, image binding and the renderer feature that draws them".into(),
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
