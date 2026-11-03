//! Particles (`pulsar.particles`): the editor plugin.
//!
//! The plugin's identity ([`ParticlesPlugin`]). The components, their methods,
//! rows and the renderer feature's registrations are the component crate's
//! (`particles_component`), which this crate links: the editor links this
//! crate (`use particles_plugin as _`) and knows no GPU particle emitters type. The
//! inspector edits the components through their reflected properties.

use plugin_editor_api::*;

/// Links the component crate, so its registrations come with this crate.
pub use particles_component as component;

/// The plugin's identity.
#[derive(Default)]
pub struct ParticlesPlugin;

impl ParticlesPlugin {
    /// The plugin id.
    pub const ID: &'static str = "pulsar.particles";
}

impl EditorPlugin for ParticlesPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            id: PluginId::new(Self::ID),
            name: "Particles".into(),
            version: "0.1.0".into(),
            author: "Pulsar Team".into(),
            description: "GPU particle emitters: their component, rows and the renderer feature that simulates and draws them (Corona)".into(),
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
        assert_eq!(ParticlesPlugin.metadata().id.as_str(), "pulsar.particles");
    }
}
