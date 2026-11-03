//! Sprites (`pulsar.sprites`): the editor plugin.
//!
//! The plugin's identity ([`SpritesPlugin`]). The components, their methods,
//! rows and the renderer feature's registrations are the component crate's
//! (`sprites_component`), which this crate links: the editor links this
//! crate (`use sprites_plugin as _`) and knows no 2D sprites type. The
//! inspector edits the components through their reflected properties.
//!
//! The sprite placement helper is still the level editor's
//! (`ui_level_editor::core::scene_edit::sprite`), attaching the component.

use plugin_editor_api::*;

/// Links the component crate, so its registrations come with this crate.
pub use sprites_component as component;

/// The plugin's identity.
#[derive(Default)]
pub struct SpritesPlugin;

impl SpritesPlugin {
    /// The plugin id.
    pub const ID: &'static str = "pulsar.sprites";
}

impl EditorPlugin for SpritesPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            id: PluginId::new(Self::ID),
            name: "Sprites".into(),
            version: "0.1.0".into(),
            author: "Pulsar Team".into(),
            description: "2D sprites drawn over the scene: their component, rows, image binding and the renderer feature that culls, batches and draws them".into(),
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
        assert_eq!(SpritesPlugin.metadata().id.as_str(), "pulsar.sprites");
    }
}
