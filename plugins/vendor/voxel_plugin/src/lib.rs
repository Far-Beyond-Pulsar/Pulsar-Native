//! Voxel worlds (`pulsar.voxel`): the plugin that owns the voxel terrain.
//!
//! - Components ([`component`]): `VoxelTerrainComponent` and
//!   `VoxelTerrainLayersComponent`, their scripting methods (blocks,
//!   brushes, ground queries, sculpting) and block events.
//! - Their runtime ([`runtime`]): one world per terrain, built off the game
//!   thread; the scene subsystem that keeps journals on their ground, skies
//!   on their air ([`sync`]) and publishes the worlds to the renderer
//!   (`helio_pass_voxel_planet::engine::VOXEL_FRAME`).
//! - File formats: the edit journal and kept chunks (in the component),
//!   ready-made worlds ([`presets`]).
//! - Its Helio feature, `helio_pass_voxel_planet::VoxelFeature`, and the
//!   feature's settings (`VOXEL_RESIDENCY`), registered with the engine's
//!   renderer and project settings below; and the terrain's material names
//!   for ground-material lists (foliage's ground).
//!
//! Everything registers through `inventory` once the crate is linked:
//! hosts link it (`extern crate voxel_plugin`) and know no voxel type.

use plugin_editor_api::*;

pub mod component;
mod component_runtime;
pub mod features;
pub mod generator_editor;
pub mod presets;
pub mod runtime;
pub mod sculpt;
pub mod stack_editor;
pub mod sync;
pub mod world;

pub use component::*;
pub use features::*;
/// The terrain's data formats: the edit journal, brushes, kept chunks.
pub use helio_voxel_data as data;

use helio_pass_voxel_planet::terrain::material;

/// The plugin's identity.
#[derive(Default)]
pub struct VoxelPlugin;

impl VoxelPlugin {
    /// The plugin id.
    pub const ID: &'static str = "pulsar.voxel";
}

impl EditorPlugin for VoxelPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            id: PluginId::new(Self::ID),
            name: "Voxel Worlds".into(),
            version: "0.1.0".into(),
            author: "Pulsar Team".into(),
            description: "Destructible voxel terrain: its components, runtime, file formats, \
                          ready-made worlds and the renderer feature that draws it"
                .into(),
        }
    }

    fn file_types(&self) -> Vec<FileTypeDefinition> {
        Vec::new()
    }

    fn editors(&self) -> Vec<EditorMetadata> {
        Vec::new()
    }
}

/// The voxel feature: the chunk terrain at the G-buffer, its sun, GI tracer,
/// foliage ground and water terrain.
fn render_features() -> helio_feature_api::FeatureSet {
    helio_feature_api::FeatureSet::new().with(helio_pass_voxel_planet::VoxelFeature)
}

/// The feature's settings from the project's voxel settings.
fn configure_renderer(config: helio::RendererConfig) -> helio::RendererConfig {
    use engine_backend::subsystems::render::helio_renderer::project_settings;
    config.with_feature(helio_pass_voxel_planet::VoxelSettings {
        residency: helio_pass_voxel_planet::chunks::ResidencyConfig::from_settings(
            &project_settings::resolved(&helio_pass_voxel_planet::settings::VOXEL_RESIDENCY),
        ),
    })
}

inventory::submit! {
    engine_backend::subsystems::render::RenderFeatureRegistration {
        features: render_features,
        configure: configure_renderer,
    }
}

inventory::submit! {
    pulsar_settings::project::rendering::SettingsGroupRegistration {
        group: &helio_pass_voxel_planet::settings::VOXEL_RESIDENCY,
    }
}

/// A terrain material's name (ids start at 1).
fn material_name(id: u32) -> Option<&'static str> {
    let index = (id as usize).checked_sub(1)?;
    material::NAMES.get(index).copied()
}

inventory::submit! {
    helio_component::GroundMaterialNames {
        name: material_name,
        id: material::from_name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_its_feature_and_settings() {
        assert_eq!(VoxelPlugin.metadata().id.as_str(), "pulsar.voxel");
        let features =
            engine_backend::subsystems::render::helio_renderer::render_features::plugin_features();
        assert!(
            features.ids().contains(&"helio.voxel"),
            "features: {:?}",
            features.ids()
        );
        assert!(
            pulsar_settings::project::rendering::groups()
                .iter()
                .any(|group| std::ptr::eq(
                    *group,
                    &helio_pass_voxel_planet::settings::VOXEL_RESIDENCY
                ))
        );
    }
}
