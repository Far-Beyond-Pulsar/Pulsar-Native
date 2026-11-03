//! Portals (`pulsar.portals`): the editor plugin.
//!
//! The plugin's identity ([`PortalsPlugin`]). The components, their methods,
//! rows and the renderer feature's registrations are the component crate's
//! (`portals_component`), which this crate links: the editor links this
//! crate (`use portals_plugin as _`) and knows no portals type. The
//! inspector edits the components through their reflected properties.
//!
//! The portal diagnostics overlay is still the level editor's
//! (`ui_level_editor`): it pairs portals through the component crate and
//! publishes what the overlay draws as the overlay's view payload.

use plugin_editor_api::*;

/// Links the component crate, so its registrations come with this crate.
pub use portals_component as component;

/// The plugin's identity.
#[derive(Default)]
pub struct PortalsPlugin;

impl PortalsPlugin {
    /// The plugin id.
    pub const ID: &'static str = "pulsar.portals";
}

impl EditorPlugin for PortalsPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            id: PluginId::new(Self::ID),
            name: "Portals".into(),
            version: "0.1.0".into(),
            author: "Pulsar Team".into(),
            description: "Openings that show another place: their component, pairing and projection, settings and the renderer features that cull and draw through them".into(),
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
        assert_eq!(PortalsPlugin.metadata().id.as_str(), "pulsar.portals");
    }
}
