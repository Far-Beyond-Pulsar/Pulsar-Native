//! Post-Process Volumes (`pulsar.post`): the editor plugin.
//!
//! The plugin's identity ([`PostPlugin`]). The components, their methods,
//! rows and the renderer feature's registrations are the component crate's
//! (`post_component`), which this crate links: the editor links this
//! crate (`use post_plugin as _`) and knows no post-process type. The
//! inspector edits the components through their reflected properties.

use plugin_editor_api::*;

/// Links the component crate, so its registrations come with this crate.
pub use post_component as component;

/// The plugin's identity.
#[derive(Default)]
pub struct PostPlugin;

impl PostPlugin {
    /// The plugin id.
    pub const ID: &'static str = "pulsar.post";
}

impl EditorPlugin for PostPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            id: PluginId::new(Self::ID),
            name: "Post-Process Volumes".into(),
            version: "0.1.0".into(),
            author: "Pulsar Team".into(),
            description: "Post-process volumes and camera baselines: their components, rows and the renderer feature that blends them into the frame's post settings".into(),
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
        assert_eq!(PostPlugin.metadata().id.as_str(), "pulsar.post");
    }
}
