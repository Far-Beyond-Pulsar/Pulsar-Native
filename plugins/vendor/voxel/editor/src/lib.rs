//! Voxel worlds (`pulsar.voxel`): the editor plugin.
//!
//! The plugin's identity ([`VoxelPlugin`]) and the voxel terrain's inspector
//! editors: the generator picker ([`generator_editor`]) and the layer stack
//! ([`stack_editor`]), registered through `inventory` once the crate is
//! linked. The components, their methods and runtime, the file formats and
//! the renderer feature's registrations are the component crate's
//! (`voxel_component`), which this crate links: the editor links this crate
//! (through `installed_plugins`, as an installed plugin's editor module) and knows no voxel type.
//!
//! The sculpt tool and the terrain panel are still the level editor's
//! (`ui_level_editor`), acting through the component's methods.

use plugin_editor_api::*;

pub mod generator_editor;
pub mod stack_editor;

/// The plugin's identity.
#[derive(Default)]
pub struct VoxelPlugin;

impl VoxelPlugin {
    /// The plugin id.
    pub const ID: &'static str = "pulsar.voxel";
}

impl EditorPlugin for VoxelPlugin {
    fn metadata(&self) -> PluginMetadata {
        // The plugin's identity is its descriptor's (pulsar-plugin.toml).
        let descriptor = voxel_component::plugin_descriptor();
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
        assert_eq!(VoxelPlugin.metadata().id.as_str(), "pulsar.voxel");
    }
}
