//! Camera Lens (`pulsar.lens`): the editor plugin.
//!
//! The plugin's identity ([`LensPlugin`]). The components, their methods,
//! rows and the renderer feature's registrations are the component crate's
//! (`lens_component`), which this crate links: the editor links this
//! crate (`use lens_plugin as _`) and knows no camera lenses type. The
//! inspector edits the components through their reflected properties.

use plugin_editor_api::*;

/// Links the component crate, so its registrations come with this crate.
pub use lens_component as component;

/// The plugin's identity.
#[derive(Default)]
pub struct LensPlugin;

impl LensPlugin {
    /// The plugin id.
    pub const ID: &'static str = "pulsar.lens";
}

impl EditorPlugin for LensPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            id: PluginId::new(Self::ID),
            name: "Camera Lens".into(),
            version: "0.1.0".into(),
            author: "Pulsar Team".into(),
            description: "A camera's physically based lens: its component, presets, rows and the renderer features that resolve the image through it and draw its flare".into(),
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
        assert_eq!(LensPlugin.metadata().id.as_str(), "pulsar.lens");
    }
}
