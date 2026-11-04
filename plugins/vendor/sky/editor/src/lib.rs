//! Sky (`pulsar.sky`): the editor plugin.
//!
//! The plugin's identity ([`SkyPlugin`]). The component, its methods, row,
//! level rule and the renderer feature's registrations are the component
//! crate's (`sky_component`), which this crate links: the editor links this
//! crate (`use sky_plugin as _`) and knows no sky type. World Settings and
//! the inspector edit the component through its reflected properties.

use plugin_editor_api::*;

/// Links the component crate, so its registrations come with this crate.
pub use sky_component as component;

/// The plugin's identity.
#[derive(Default)]
pub struct SkyPlugin;

impl SkyPlugin {
    /// The plugin id.
    pub const ID: &'static str = "pulsar.sky";
}

impl EditorPlugin for SkyPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            id: PluginId::new(Self::ID),
            name: "Sky".into(),
            version: "0.1.0".into(),
            author: "Pulsar Team".into(),
            description: "The level's sky: the atmosphere component (air, sun disc, clouds and weather), its row and the renderer feature that draws it and publishes the sky's lighting inputs".into(),
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
        assert_eq!(SkyPlugin.metadata().id.as_str(), "pulsar.sky");
    }
}
