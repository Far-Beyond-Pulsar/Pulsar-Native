//! SceneDB-owned volumetric-fog parameters.
//!
//! Fog used to be available only as part of Helio's post-process uniform
//! resource.  That made it impossible for the scene database to own the
//! authored environment state.  This record is the persistent, packed row;
//! the froxel grids and per-frame light integration remain pass-owned
//! transient resources.

/// World-space participating medium. One world unit is one metre. Overlaps add.
/// `extinction` is sigma_a + sigma_s in m^-1, `albedo` is sigma_s / sigma_t,
/// and `emission` is scene-linear radiance emitted per metre (independent of density).
/// Mode: 0 uniform, 1 exponential height, 2 animated smoke with a height envelope.
/// Heights are along world Y, or above a planet (`planet`, `around_planet`).
/// Zeroed/deleted rows are inert. No camera-distance gates apply to these media.
#[derive(bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct GlobalFogRow {
    pub enabled: u32,
    pub mode: u32,
    pub extinction: f32,
    pub height_falloff: f32,
    pub height: f32,
    pub anisotropy: f32,
    pub _pad: [f32; 2],
    pub albedo: [f32; 3],
    pub _pad_albedo: f32,
    pub emission: [f32; 3],
    pub _pad_emission: f32,
    /// The planet heights are measured above: its centre (world metres) and
    /// radius (m), so `height` is an altitude over its surface; radius 0:
    /// heights along world Y. Planet centres are world anchors, which the
    /// pass rebases by the frame's origin.
    pub planet: [f32; 4],
}

// SAFETY: a `#[repr(C)]` bytemuck `Pod` row: no padding bytes, any bit
// pattern valid. Lets an engine's SceneDB component embed it.
unsafe impl pulsar_scenedb::Pod for GlobalFogRow {}

/// The buffer the renderer reads [`GlobalFogRow`]s from. Engines produce it
/// (a SceneDB component stored under this key, or a scene derivation).
pub const GLOBAL_FOG_MEDIA: helio_core::BufferContract<GlobalFogRow> =
    helio_core::BufferContract::new("global_fog_media");

impl GlobalFogRow {
    /// This medium's heights measured above a planet of `radius_m` centred at
    /// `center_m` (world metres), as `AtmosphereRow::around_planet`.
    pub fn around_planet(mut self, center_m: [f32; 3], radius_m: f64) -> Self {
        self.planet = [center_m[0], center_m[1], center_m[2], radius_m as f32];
        self
    }
}

impl Default for GlobalFogRow {
    fn default() -> Self {
        Self {
            enabled: 1,
            mode: 0,
            extinction: 0.02,
            height_falloff: 0.0,
            height: 0.0,
            anisotropy: 0.0,
            _pad: [0.0; 2],
            albedo: [1.0; 3],
            _pad_albedo: 0.0,
            emission: [0.0; 3],
            _pad_emission: 0.0,
            planet: [0.0; 4],
        }
    }
}

/// Axis-aligned volume, its bounds in the frame's space (relative to
/// `Renderer::world_origin`, as every position in a pass row). `medium` and `medium_planet` store raw bytes in
/// reflection-supported float arrays (not numeric float conversions), with the
/// GlobalFogRow layout (its first 16 words, then `planet`); use `new` /
/// `set_medium` instead of packing words manually. `edge_fade` is an
/// inward fade distance in metres; zero gives a hard boundary. No PP priority.
#[derive(bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct LocalFogVolumeRow {
    pub bounds_min: [f32; 4],
    pub bounds_max: [f32; 4],
    pub medium: [f32; 16],
    pub medium_planet: [f32; 4],
    pub edge_fade: f32,
    pub _pad: [f32; 3],
}

// SAFETY: a `#[repr(C)]` bytemuck `Pod` row: no padding bytes, any bit
// pattern valid. Lets an engine's SceneDB component embed it.
unsafe impl pulsar_scenedb::Pod for LocalFogVolumeRow {}

/// The buffer the renderer reads [`LocalFogVolumeRow`]s from. Engines produce it
/// (a SceneDB component stored under this key, or a scene derivation).
pub const LOCAL_FOG_MEDIA: helio_core::BufferContract<LocalFogVolumeRow> =
    helio_core::BufferContract::new("local_fog_media");

impl LocalFogVolumeRow {
    pub fn new(bounds_min: [f32; 3], bounds_max: [f32; 3], medium: GlobalFogRow) -> Self {
        let mut volume = Self {
            bounds_min: [bounds_min[0], bounds_min[1], bounds_min[2], 0.0],
            bounds_max: [bounds_max[0], bounds_max[1], bounds_max[2], 0.0],
            medium: [0.0; 16],
            medium_planet: [0.0; 4],
            edge_fade: 0.0,
            _pad: [0.0; 3],
        };
        volume.set_medium(medium);
        volume
    }
    pub fn set_medium(&mut self, medium: GlobalFogRow) {
        let words: [f32; 20] = bytemuck::cast(medium);
        self.medium.copy_from_slice(&words[..16]);
        self.medium_planet.copy_from_slice(&words[16..]);
    }
    pub fn medium(&self) -> GlobalFogRow {
        let mut words = [0.0f32; 20];
        words[..16].copy_from_slice(&self.medium);
        words[16..].copy_from_slice(&self.medium_planet);
        bytemuck::cast(words)
    }
}

/// GPU-resolved per-view scalability. Match `view_id` to the u32 bits in
/// Camera.jitter_frame.w; u32::MAX is a fallback for every view. Exact matches
/// beat fallbacks, and the lowest entity row wins ties. `active=0` is a tombstone;
/// `enabled=0` on an active row explicitly disables rendering for that view.
/// Quality 0: 96x54x64; quality 1: 192x108x128 (aspect adjusted within budget).
/// Increase `history_epoch` for a camera cut or explicit history invalidation.
#[derive(bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct VolumetricFogSettingsRow {
    pub active: u32,
    pub view_id: u32,
    pub quality: u32,
    pub enabled: u32,
    pub max_distance: f32,
    pub light_max_distance: f32,
    pub temporal_blend: f32,
    /// Relative density change at which history starts to be discarded (a
    /// medium appearing or vanishing). Lighting changes never reject: samples
    /// are jittered, so they differ by design and the blend integrates them.
    pub history_rejection: f32,
    pub light_samples: u32,
    pub history_epoch: u32,
    pub _pad: [f32; 2],
}

// SAFETY: a `#[repr(C)]` bytemuck `Pod` row: no padding bytes, any bit
// pattern valid. Lets an engine's SceneDB component embed it.
unsafe impl pulsar_scenedb::Pod for VolumetricFogSettingsRow {}

/// The buffer the renderer reads [`VolumetricFogSettingsRow`]s from. Engines produce it
/// (a SceneDB component stored under this key, or a scene derivation).
pub const VOLUMETRIC_FOG_SETTINGS: helio_core::BufferContract<VolumetricFogSettingsRow> =
    helio_core::BufferContract::new("volumetric_fog_settings");

impl Default for VolumetricFogSettingsRow {
    fn default() -> Self {
        Self {
            active: 1,
            view_id: u32::MAX,
            quality: 0,
            enabled: 1,
            max_distance: 1000.0,
            light_max_distance: 1000.0,
            temporal_blend: 0.05,
            history_rejection: 0.8,
            light_samples: 0,
            history_epoch: 0,
            _pad: [0.0; 2],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_component_layouts_and_local_medium_round_trip() {
        assert_eq!(std::mem::size_of::<GlobalFogRow>(), 80);
        assert_eq!(std::mem::size_of::<LocalFogVolumeRow>(), 128);
        assert_eq!(std::mem::offset_of!(GlobalFogRow, planet), 64);
        assert_eq!(std::mem::offset_of!(LocalFogVolumeRow, medium_planet), 96);
        assert_eq!(std::mem::offset_of!(LocalFogVolumeRow, medium), 32);
        assert_eq!(std::mem::offset_of!(GlobalFogRow, albedo), 32);
        assert_eq!(std::mem::offset_of!(GlobalFogRow, emission), 48);
        assert_eq!(std::mem::size_of::<VolumetricFogSettingsRow>(), 48);
        let mut medium = GlobalFogRow {
            mode: 2,
            anisotropy: -0.8,
            emission: [1.0, 2.0, 3.0],
            ..Default::default()
        }
        .around_planet([1.0, 2.0, 3.0], 500.0);
        let mut local = LocalFogVolumeRow::new([-1.0; 3], [1.0; 3], medium);
        assert_eq!(local.medium(), medium);
        medium.extinction = 4.0;
        local.set_medium(medium);
        assert_eq!(local.medium(), medium);
    }

    #[test]
    fn tier_dimensions_preserve_aspect_and_bound_allocations() {
        assert_eq!(crate::froxel_dimensions(1920, 1080, 0), [96, 54, 64]);
        assert_eq!(crate::froxel_dimensions(1920, 1080, 1), [192, 108, 128]);
        for (w, h) in [
            (0, 0),
            (1080, 1920),
            (1920, 800),
            (2560, 1600),
            (1, u32::MAX),
            (u32::MAX, 1),
        ] {
            let high = crate::froxel_dimensions(w, h, 1);
            let low = crate::froxel_dimensions(w, h, 0);
            assert!(high[0] <= 192 && high[1] <= 108);
            assert!(low.iter().all(|v| *v > 0));
            assert_eq!(low.map(|v| v * 2), high);
        }
    }
}
