//! GI (`pulsar.gi`): the editor plugin.
//!
//! The plugin's identity ([`GiPlugin`]). The renderer feature's and its
//! settings' registrations are the component crate's (`gi_component`),
//! which this crate links: the editor links this crate (`use gi_plugin as
//! _`) and knows no GI type. The project settings window edits GI's
//! settings group like any other.

use plugin_editor_api::*;

/// Links the component crate, so its registrations come with this crate.
pub use gi_component as component;

/// The plugin's identity.
#[derive(Default)]
pub struct GiPlugin;

impl GiPlugin {
    /// The plugin id.
    pub const ID: &'static str = "pulsar.gi";
}

impl EditorPlugin for GiPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            id: PluginId::new(Self::ID),
            name: "Global Illumination".into(),
            version: "0.1.0".into(),
            author: "Pulsar Team".into(),
            description: "World-space diffuse GI: the probe clipmap the scene's tracers feed, and its settings".into(),
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
        assert_eq!(GiPlugin.metadata().id.as_str(), "pulsar.gi");
    }
}
