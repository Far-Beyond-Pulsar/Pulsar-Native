//! Fog (`pulsar.fog`): the editor plugin.
//!
//! The plugin's identity ([`FogPlugin`]). The components, their methods,
//! rows and the renderer feature's registrations are the component crate's
//! (`fog_component`), which this crate links: the editor links this
//! crate (`use fog_plugin as _`) and knows no fog type. The
//! inspector edits the components through their reflected properties.

use plugin_editor_api::*;

/// Links the component crate, so its registrations come with this crate.
pub use fog_component as component;

/// The plugin's identity.
#[derive(Default)]
pub struct FogPlugin;

impl FogPlugin {
    /// The plugin id.
    pub const ID: &'static str = "pulsar.fog";
}

impl EditorPlugin for FogPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            id: PluginId::new(Self::ID),
            name: "Fog".into(),
            version: "0.1.0".into(),
            author: "Pulsar Team".into(),
            description: "Global and local fog media: their components, rows and the renderer feature that integrates and composites them (volumetric fog)".into(),
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
        assert_eq!(FogPlugin.metadata().id.as_str(), "pulsar.fog");
    }
}
