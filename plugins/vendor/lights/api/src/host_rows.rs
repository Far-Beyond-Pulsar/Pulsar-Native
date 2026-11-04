//! The light list's row as a SceneDB component, for standalone hosts (examples, tests):
//! it carries its contract row's exact fields (`same_layout!`), so SceneDB
//! mirrors it into the buffer the passes read. Engines derive the rows from
//! their own components instead. Behind the `host-rows` feature.

use helio_components::same_layout;
use pulsar_scenedb_derive::SceneStore;

/// A placed light, authored as a SceneDB component. Field-for-field
/// identical to `GpuLight` (enforced by the size/align test
/// below) so conversion is a zero-cost `bytemuck::cast`.
#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "scene_lights")]
pub struct LightComponent {
    #[gpu]
    pub position_range: [f32; 4],
    #[gpu]
    pub direction_outer: [f32; 4],
    #[gpu]
    pub color_intensity: [f32; 4],
    #[gpu]
    pub shadow_index: u32,
    #[gpu]
    pub light_type: u32,
    #[gpu]
    pub inner_angle: f32,
    #[gpu]
    pub _pad: u32,
    #[gpu]
    pub god_rays_enabled: u32,
    #[gpu]
    pub god_rays_density: f32,
    #[gpu]
    pub god_rays_weight: f32,
    #[gpu]
    pub god_rays_decay: f32,
    #[gpu]
    pub god_rays_exposure: f32,
    #[gpu]
    pub flare_enabled: u32,
    #[gpu]
    pub flare_type: u32,
    #[gpu]
    pub flare_intensity: f32,
    #[gpu]
    pub flare_scale: f32,
    #[gpu]
    pub flare_tint_r: f32,
    #[gpu]
    pub flare_tint_g: f32,
    #[gpu]
    pub flare_tint_b: f32,
    #[gpu]
    pub ies_profile_index: i32,
    #[gpu]
    pub light_function_index: i32,
    #[gpu]
    pub ies_angle_scale: f32,
    #[gpu]
    pub ies_angle_offset: f32,
}
same_layout!(LightComponent, crate::LightRow, crate::SCENE_LIGHTS, "scene_lights"; position_range, direction_outer, color_intensity, shadow_index, light_type, inner_angle, _pad, god_rays_enabled, god_rays_density, god_rays_weight, god_rays_decay, god_rays_exposure, flare_enabled, flare_type, flare_intensity, flare_scale, flare_tint_r, flare_tint_g, flare_tint_b, ies_profile_index, light_function_index, ies_angle_scale, ies_angle_offset);

impl From<crate::GpuLight> for LightComponent {
    fn from(value: crate::GpuLight) -> Self {
        crate::LightRow::from(value).into()
    }
}

impl From<LightComponent> for crate::GpuLight {
    fn from(value: LightComponent) -> Self {
        crate::LightRow::from(value).into()
    }
}
