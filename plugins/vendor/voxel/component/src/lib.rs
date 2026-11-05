//! Voxel worlds (`pulsar.voxel`): the component crate of the plugin that
//! owns the voxel terrain.
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
//! - The registrations of the plugin's Helio feature,
//!   `helio_pass_voxel_planet::VoxelFeature`, and the feature's settings
//!   (`VOXEL_RESIDENCY`) with the engine's renderer and project settings
//!   below; and the terrain's material names for ground-material lists
//!   (foliage's ground).
//!
//! Everything registers through `inventory` once the crate is linked: hosts link it
//! without naming it (a game built for a project links the runtime modules
//! of the plugins its `Pulsar.toml` enables, the editor every installed
//! plugin through `installed_plugins`; `pulsar_plugins`) and know no voxel type.

// The plugin's descriptor (`../pulsar-plugin.toml`): linking this crate
// installs the plugin; a project enables it or not (`pulsar_plugins`).
pulsar_plugins::installed!("../pulsar-plugin.toml");

pub mod component;
mod component_runtime;
pub mod features;
pub mod presets;
pub mod runtime;
pub mod sculpt;
pub mod sync;
pub mod world;

pub use component::*;
pub use features::*;
/// The terrain's data formats: the edit journal, brushes, kept chunks.
pub use helio_voxel_data as data;

use helio_pass_voxel_planet::terrain::material;

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
        origin: pulsar_plugins::origin!(),
        features: render_features,
        configure: configure_renderer,
    }
}

inventory::submit! {
    pulsar_settings::project::rendering::SettingsGroupRegistration {
        origin: pulsar_plugins::origin!(),
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
        origin: pulsar_plugins::origin!(),
        name: material_name,
        id: material::from_name,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn registers_its_feature_and_settings() {
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
