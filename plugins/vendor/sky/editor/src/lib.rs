//! Sky (`pulsar.sky`): the editor plugin.
//!
//! The plugin's identity ([`SkyPlugin`]). The component, its methods, row,
//! level rule and the renderer feature's registrations are the component
//! crate's (`sky_component`), which this crate links: the editor links this
//! crate (through `installed_plugins`, as an installed plugin's editor module) and knows no sky type. World Settings and
//! the inspector edit the component through its reflected properties.

// The level editor UI's text (`locales/`).
rust_i18n::i18n!("locales", fallback = "en");

/// The plugin's level editor UI, registered through the level editor's
/// extension points (`ui_level_editor::extend`).
pub mod level_editor;

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
        // The plugin's identity is its descriptor's (pulsar-plugin.toml).
        let descriptor = sky_component::plugin_descriptor();
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
        assert_eq!(SkyPlugin.metadata().id.as_str(), "pulsar.sky");
    }
}
