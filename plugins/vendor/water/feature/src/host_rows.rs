//! The water volume and hitbox rows as SceneDB components, for standalone hosts
//! (examples, tests): each carries its contract row's exact fields
//! (`same_layout!`), so SceneDB mirrors it into the buffer the passes read.
//! Engines derive these rows from their own components instead. Behind the
//! `host-rows` feature.

use helio_components::same_layout;
use pulsar_scenedb_derive::SceneStore;

#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "water_volumes")]
pub struct WaterVolumeComponent {
    #[gpu]
    pub bounds_min: [f32; 4],
    #[gpu]
    pub bounds_max: [f32; 4],
    #[gpu]
    pub wave_params: [f32; 4],
    #[gpu]
    pub wave_direction: [f32; 4],
    #[gpu]
    pub water_color: [f32; 4],
    #[gpu]
    pub extinction: [f32; 4],
    #[gpu]
    pub reflection_refraction: [f32; 4],
    #[gpu]
    pub caustics_params: [f32; 4],
    #[gpu]
    pub fog_params: [f32; 4],
    #[gpu]
    pub sim_params: [f32; 4],
    #[gpu]
    pub shadow_params: [f32; 4],
    #[gpu]
    pub sun_direction: [f32; 4],
    #[gpu]
    pub ssr_params: [f32; 4],
    #[gpu]
    pub sim_dynamics: [f32; 4],
    #[gpu]
    pub wind_params: [f32; 4],
    #[gpu]
    pub _pad6: [f32; 4],
}

same_layout!(WaterVolumeComponent, crate::WaterVolumeRow, crate::WATER_VOLUMES, "water_volumes"; bounds_min, bounds_max, wave_params, wave_direction, water_color, extinction, reflection_refraction, caustics_params, fog_params, sim_params, shadow_params, sun_direction, ssr_params, sim_dynamics, wind_params, _pad6);

#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "water_hitboxes")]
pub struct WaterHitboxComponent {
    #[gpu]
    pub old_min: [f32; 4],
    #[gpu]
    pub old_max: [f32; 4],
    #[gpu]
    pub new_min: [f32; 4],
    #[gpu]
    pub new_max: [f32; 4],
    #[gpu]
    pub params: [f32; 4],
}

same_layout!(WaterHitboxComponent, crate::WaterHitboxRow, crate::WATER_HITBOXES, "water_hitboxes"; old_min, old_max, new_min, new_max, params);

impl From<crate::GpuWaterVolume> for WaterVolumeComponent {
    fn from(value: crate::GpuWaterVolume) -> Self {
        crate::WaterVolumeRow::from(value).into()
    }
}

impl From<WaterVolumeComponent> for crate::GpuWaterVolume {
    fn from(value: WaterVolumeComponent) -> Self {
        crate::WaterVolumeRow::from(value).into()
    }
}

impl From<crate::GpuWaterHitbox> for WaterHitboxComponent {
    fn from(value: crate::GpuWaterHitbox) -> Self {
        crate::WaterHitboxRow::from(value).into()
    }
}

impl From<WaterHitboxComponent> for crate::GpuWaterHitbox {
    fn from(value: WaterHitboxComponent) -> Self {
        crate::WaterHitboxRow::from(value).into()
    }
}
