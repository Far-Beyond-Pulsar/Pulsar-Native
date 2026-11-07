//! Splines (`pulsar.splines`): the editor plugin.
//!
//! The plugin's identity ([`SplinesPlugin`]). The components, their methods,
//! rows and the renderer feature's registrations are the component crate's
//! (`splines_component`), which this crate links: the editor links this
//! crate (through `installed_plugins`, as an installed plugin's editor module) and knows no splines type. The
//! inspector edits the components through their reflected properties.
//!
//! The spline tool mode, its panel and the assistant's spline tools are
//! this crate's ([`level_editor`]), acting through the component's methods.

// The level editor UI's text (`locales/`).
rust_i18n::i18n!("locales", fallback = "en");

/// The plugin's level editor UI, registered through the level editor's
/// extension points (`ui_level_editor::extend`).
pub mod level_editor;

use plugin_editor_api::*;

/// Links the component crate, so its registrations come with this crate.
pub use splines_component as component;

/// The plugin's identity.
#[derive(Default)]
pub struct SplinesPlugin;

impl SplinesPlugin {
    /// The plugin id.
    pub const ID: &'static str = "pulsar.splines";
}

impl EditorPlugin for SplinesPlugin {
    fn metadata(&self) -> PluginMetadata {
        // The plugin's identity is its descriptor's (pulsar-plugin.toml).
        let descriptor = splines_component::plugin_descriptor();
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
        assert_eq!(SplinesPlugin.metadata().id.as_str(), "pulsar.splines");
    }
}
