//! The fog media and the volumetric settings rows as SceneDB components, for standalone hosts
//! (examples, tests): each carries its contract row's exact fields
//! (`same_layout!`), so SceneDB mirrors it into the buffer the passes read.
//! Engines derive these rows from their own components instead. Behind the
//! `host-rows` feature.

use helio_components::same_layout;
use pulsar_scenedb_derive::SceneStore;

/// World-space participating medium. One world unit is one metre. Overlaps add.
/// `extinction` is sigma_a + sigma_s in m^-1, `albedo` is sigma_s / sigma_t,
/// and `emission` is scene-linear radiance emitted per metre (independent of density).
/// Mode: 0 uniform, 1 exponential height, 2 animated smoke with a height envelope.
/// Heights are along world Y, or above a planet (`planet`, `around_planet`).
/// Zeroed/deleted rows are inert. No camera-distance gates apply to these media.
#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "global_fog_media")]
pub struct GlobalFogComponent {
    #[gpu]
    pub enabled: u32,
    #[gpu]
    pub mode: u32,
    #[gpu]
    pub extinction: f32,
    #[gpu]
    pub height_falloff: f32,
    #[gpu]
    pub height: f32,
    #[gpu]
    pub anisotropy: f32,
    #[gpu]
    pub _pad: [f32; 2],
    #[gpu]
    pub albedo: [f32; 3],
    #[gpu]
    pub _pad_albedo: f32,
    #[gpu]
    pub emission: [f32; 3],
    #[gpu]
    pub _pad_emission: f32,
    /// The planet heights are measured above: its centre (world metres) and
    /// radius (m), so `height` is an altitude over its surface; radius 0:
    /// heights along world Y. Planet centres are world anchors, which the
    /// pass rebases by the frame's origin.
    #[gpu]
    pub planet: [f32; 4],
}

same_layout!(GlobalFogComponent, crate::GlobalFogRow, crate::GLOBAL_FOG_MEDIA, "global_fog_media"; enabled, mode, extinction, height_falloff, height, anisotropy, _pad, albedo, _pad_albedo, emission, _pad_emission, planet);

/// Axis-aligned volume, its bounds in the frame's space (relative to
/// `Renderer::world_origin`, as every position in a pass row). `medium` and `medium_planet` store raw bytes in
/// reflection-supported float arrays (not numeric float conversions), with the
/// GlobalFogComponent layout (its first 16 words, then `planet`); use `new` /
/// `set_medium` instead of packing words manually. `edge_fade` is an
/// inward fade distance in metres; zero gives a hard boundary. No PP priority.
#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "local_fog_media")]
pub struct LocalFogVolumeComponent {
    #[gpu]
    pub bounds_min: [f32; 4],
    #[gpu]
    pub bounds_max: [f32; 4],
    #[gpu]
    pub medium: [f32; 16],
    #[gpu]
    pub medium_planet: [f32; 4],
    #[gpu]
    pub edge_fade: f32,
    #[gpu]
    pub _pad: [f32; 3],
}

same_layout!(LocalFogVolumeComponent, crate::LocalFogVolumeRow, crate::LOCAL_FOG_MEDIA, "local_fog_media"; bounds_min, bounds_max, medium, medium_planet, edge_fade, _pad);

/// GPU-resolved per-view scalability. Match `view_id` to the u32 bits in
/// Camera.jitter_frame.w; u32::MAX is a fallback for every view. Exact matches
/// beat fallbacks, and the lowest entity row wins ties. `active=0` is a tombstone;
/// `enabled=0` on an active row explicitly disables rendering for that view.
/// Quality 0: 96x54x64; quality 1: 192x108x128 (aspect adjusted within budget).
/// Increase `history_epoch` for a camera cut or explicit history invalidation.
#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "volumetric_fog_settings")]
pub struct VolumetricFogSettingsComponent {
    #[gpu]
    pub active: u32,
    #[gpu]
    pub view_id: u32,
    #[gpu]
    pub quality: u32,
    #[gpu]
    pub enabled: u32,
    #[gpu]
    pub max_distance: f32,
    #[gpu]
    pub light_max_distance: f32,
    #[gpu]
    pub temporal_blend: f32,
    #[gpu]
    /// Relative density change at which history starts to be discarded (a
    /// medium appearing or vanishing). Lighting changes never reject: samples
    /// are jittered, so they differ by design and the blend integrates them.
    pub history_rejection: f32,
    #[gpu]
    pub light_samples: u32,
    #[gpu]
    pub history_epoch: u32,
    #[gpu]
    pub _pad: [f32; 2],
}

same_layout!(VolumetricFogSettingsComponent, crate::VolumetricFogSettingsRow, crate::VOLUMETRIC_FOG_SETTINGS, "volumetric_fog_settings"; active, view_id, quality, enabled, max_distance, light_max_distance, temporal_blend, history_rejection, light_samples, history_epoch, _pad);

impl Default for GlobalFogComponent {
    fn default() -> Self {
        crate::GlobalFogRow::default().into()
    }
}

impl Default for VolumetricFogSettingsComponent {
    fn default() -> Self {
        crate::VolumetricFogSettingsRow::default().into()
    }
}

impl GlobalFogComponent {
    pub fn around_planet(self, center_m: [f32; 3], radius_m: f64) -> Self {
        crate::GlobalFogRow::from(self)
            .around_planet(center_m, radius_m)
            .into()
    }
}
