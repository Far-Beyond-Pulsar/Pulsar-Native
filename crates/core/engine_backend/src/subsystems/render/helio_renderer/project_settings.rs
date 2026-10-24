//! The project's renderer settings, read through Helio's description of
//! them (`helio_settings`; `pulsar_settings::project::rendering` registers
//! the same description). The editor viewport and the game build their
//! renderers from these.

use helio::helio_settings::{self, Group, Resolved};

/// `group`'s values in the project settings (defaults where unset or
/// invalid).
pub fn resolved(group: &'static Group) -> Resolved {
    let config = engine_state::settings::global_config();
    group.resolve(|key| {
        config
            .get(engine_state::settings::NS_PROJECT, group.key, key)
            .ok()
            .and_then(pulsar_settings::project::rendering::helio_value)
    })
}

/// The renderer's settings.
pub fn renderer() -> Resolved {
    resolved(&helio_settings::RENDERER)
}

/// Voxel worlds' residency settings.
pub fn voxels() -> Resolved {
    resolved(&helio_settings::VOXEL_RESIDENCY)
}

/// A renderer config with the project's settings.
pub fn renderer_config(
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    settings: &Resolved,
) -> helio::RendererConfig {
    helio::RendererConfig::new(width, height, format).with_settings(settings)
}

/// The TSR preset to render with: the project's, or with "Automatic" the
/// scene's preference (`scene`).
pub fn tsr_quality(
    settings: &Resolved,
    scene: Option<helio::TsrQuality>,
) -> Option<helio::TsrQuality> {
    helio::TsrSetting::from_settings(settings).resolve(scene)
}
