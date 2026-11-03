//! The post-process volume and camera baseline rows as SceneDB components, for standalone hosts
//! (examples, tests): each carries its contract row's exact fields
//! (`same_layout!`), so SceneDB mirrors it into the buffer the passes read.
//! Engines derive these rows from their own components instead. Behind the
//! `host-rows` feature.

use helio_components::same_layout;
use pulsar_scenedb_derive::SceneStore;

#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "post_process_volumes")]
pub struct PostProcessVolumeComponent {
    #[gpu]
    pub bounds_min: [f32; 4],
    #[gpu]
    pub bounds_max: [f32; 4],
    #[gpu]
    pub priority: f32,
    #[gpu]
    pub blend_radius: f32,
    /// Zero means "inactive" -- `postprocess.wgsl`'s `cs_volume_blend`
    /// skips a row with `blend_weight <= 0.0` before ever evaluating its
    /// (otherwise degenerate, all-zero) bounds, which is what makes an
    /// unused row safely inert. See `MAX_PP_VOLUMES`'s doc (helio-pass-
    /// postprocess) for the fixed-capacity iteration this enables.
    #[gpu]
    pub blend_weight: f32,
    #[gpu]
    pub unbound: u32,
    /// `PostProcessProperty` override bits, words 0..3 (`vec4<u32>` in WGSL).
    /// Scalars rather than `[u32; 4]`: reflection registers only specific
    /// array shapes, and this row derives reflection through `SceneStore`.
    #[gpu]
    pub override_mask_0: u32,
    #[gpu]
    pub override_mask_1: u32,
    #[gpu]
    pub override_mask_2: u32,
    #[gpu]
    pub override_mask_3: u32,
    /// `crate::GpuPostProcessUniforms`'s raw bytes, split at byte 464:
    /// reflection registers `[u32; 116]` and `[f32; 16]` but no 132-word
    /// array. `settings` is bytes 0..464 (including the first eight words of
    /// the lens block); `lens` is the remaining 64 bytes as raw bits, not
    /// numeric floats. The packed derive needs `pulsar_scenedb::Pod` per
    /// field, which `GpuPostProcessUniforms` lacks; `bytemuck::cast` between
    /// the outer structs only needs equal size -- see `layout_matches_gpu_abi`.
    #[gpu]
    pub settings: [u32; 116],
    #[gpu]
    pub lens: [f32; 16],
}

same_layout!(PostProcessVolumeComponent, crate::PostProcessVolumeRow, crate::POST_PROCESS_VOLUMES, "post_process_volumes"; bounds_min, bounds_max, priority, blend_radius, blend_weight, unbound, override_mask_0, override_mask_1, override_mask_2, override_mask_3, settings, lens);

/// Optional per-view baseline: the camera's own post-process settings,
/// including lens flare. The first enabled row matching the camera's view id
/// (u32 bits of `jitter_frame.w`) wins; with none, the resolver's project
/// defaults apply. PP volumes then override on top. Removing the component
/// clears its packed row. `settings`/`lens` split the uniforms exactly as
/// [`PostProcessVolumeComponent`] does.
#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "camera_postprocess")]
pub struct CameraPostProcessComponent {
    #[gpu]
    pub view_id: u32,
    #[gpu]
    pub enabled: u32,
    #[gpu]
    pub _pad0: u32,
    #[gpu]
    pub _pad1: u32,
    #[gpu]
    pub settings: [u32; 116],
    #[gpu]
    pub lens: [f32; 16],
}

same_layout!(CameraPostProcessComponent, crate::CameraPostProcessRow, crate::CAMERA_POSTPROCESS, "camera_postprocess"; view_id, enabled, _pad0, _pad1, settings, lens);

impl From<helio_pass_postprocess::GpuPostProcessVolume> for PostProcessVolumeComponent {
    fn from(value: helio_pass_postprocess::GpuPostProcessVolume) -> Self {
        crate::PostProcessVolumeRow::from(value).into()
    }
}

impl From<PostProcessVolumeComponent> for helio_pass_postprocess::GpuPostProcessVolume {
    fn from(value: PostProcessVolumeComponent) -> Self {
        crate::PostProcessVolumeRow::from(value).into()
    }
}
