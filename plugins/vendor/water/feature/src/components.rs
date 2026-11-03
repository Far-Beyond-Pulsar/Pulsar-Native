//! SceneDB-owned water inputs. Simulation textures and interaction results are
//! transient and remain owned by the water simulation pass.

#[derive(bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct WaterVolumeRow {
    pub bounds_min: [f32; 4],
    pub bounds_max: [f32; 4],
    pub wave_params: [f32; 4],
    pub wave_direction: [f32; 4],
    pub water_color: [f32; 4],
    pub extinction: [f32; 4],
    pub reflection_refraction: [f32; 4],
    pub caustics_params: [f32; 4],
    pub fog_params: [f32; 4],
    pub sim_params: [f32; 4],
    pub shadow_params: [f32; 4],
    pub sun_direction: [f32; 4],
    pub ssr_params: [f32; 4],
    pub sim_dynamics: [f32; 4],
    pub wind_params: [f32; 4],
    pub _pad6: [f32; 4],
}

// SAFETY: a `#[repr(C)]` bytemuck `Pod` row: no padding bytes, any bit
// pattern valid. Lets an engine's SceneDB component embed it.
unsafe impl pulsar_scenedb::Pod for WaterVolumeRow {}

/// The buffer the renderer reads [`WaterVolumeRow`]s from. Engines produce it
/// (a SceneDB component stored under this key, or a scene derivation).
pub const WATER_VOLUMES: helio_core::BufferContract<WaterVolumeRow> =
    helio_core::BufferContract::new("water_volumes");
#[derive(bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct WaterHitboxRow {
    pub old_min: [f32; 4],
    pub old_max: [f32; 4],
    pub new_min: [f32; 4],
    pub new_max: [f32; 4],
    pub params: [f32; 4],
}

// SAFETY: a `#[repr(C)]` bytemuck `Pod` row: no padding bytes, any bit
// pattern valid. Lets an engine's SceneDB component embed it.
unsafe impl pulsar_scenedb::Pod for WaterHitboxRow {}

/// The buffer the renderer reads [`WaterHitboxRow`]s from. Engines produce it
/// (a SceneDB component stored under this key, or a scene derivation).
pub const WATER_HITBOXES: helio_core::BufferContract<WaterHitboxRow> =
    helio_core::BufferContract::new("water_hitboxes");

impl From<crate::GpuWaterVolume> for WaterVolumeRow {
    fn from(v: crate::GpuWaterVolume) -> Self {
        bytemuck::cast(v)
    }
}
impl From<WaterVolumeRow> for crate::GpuWaterVolume {
    fn from(v: WaterVolumeRow) -> Self {
        bytemuck::cast(v)
    }
}
impl From<crate::GpuWaterHitbox> for WaterHitboxRow {
    fn from(v: crate::GpuWaterHitbox) -> Self {
        bytemuck::cast(v)
    }
}
impl From<WaterHitboxRow> for crate::GpuWaterHitbox {
    fn from(v: WaterHitboxRow) -> Self {
        bytemuck::cast(v)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn layout_matches_gpu_abi() {
        assert_eq!(std::mem::size_of::<WaterVolumeRow>(), 256);
        assert_eq!(std::mem::size_of::<WaterHitboxRow>(), 80);
    }
}
#[cfg(test)]
mod lifecycle_tests {
    use super::{WaterHitboxRow, WaterVolumeRow};
}
