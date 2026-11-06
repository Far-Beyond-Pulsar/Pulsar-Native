//! SceneDB-owned post-process volume records.
//!
//! One coarse editor-facing `PostProcessVolumeRow` (bounds, priority,
//! blend weight, and every exposure/tonemap/fog/color-grading knob) authors
//! exactly one packed row here -- there is no separate "settings" component,
//! the whole editor property set is this one GPU-facing struct, field for
//! field identical to `helio_pass_postprocess::GpuPostProcessVolume`'s ABI.

#[derive(bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct PostProcessVolumeRow {
    pub bounds_min: [f32; 4],
    pub bounds_max: [f32; 4],
    pub priority: f32,
    pub blend_radius: f32,
    /// Zero means "inactive" -- `postprocess.wgsl`'s `cs_volume_blend`
    /// skips a row with `blend_weight <= 0.0` before ever evaluating its
    /// (otherwise degenerate, all-zero) bounds, which is what makes an
    /// unused row safely inert. See `MAX_PP_VOLUMES`'s doc (helio-pass-
    /// postprocess) for the fixed-capacity iteration this enables.
    pub blend_weight: f32,
    pub unbound: u32,
    /// `PostProcessProperty` override bits, words 0..3 (`vec4<u32>` in WGSL).
    /// Scalars rather than `[u32; 4]`: reflection registers only specific
    /// array shapes, and this row derives reflection through `SceneStore`.
    pub override_mask_0: u32,
    pub override_mask_1: u32,
    pub override_mask_2: u32,
    pub override_mask_3: u32,
    /// `helio_pass_postprocess::GpuPostProcessUniforms`'s raw bytes (464):
    /// the packed derive needs `pulsar_scenedb::Pod` per field, which
    /// `GpuPostProcessUniforms` lacks; `bytemuck::cast` between the outer
    /// structs only needs equal size -- see `layout_matches_gpu_abi`.
    pub settings: [u32; 116],
}

// SAFETY: a `#[repr(C)]` bytemuck `Pod` row: no padding bytes, any bit
// pattern valid. Lets an engine's SceneDB component embed it.
unsafe impl pulsar_scenedb::Pod for PostProcessVolumeRow {}

/// The buffer the renderer reads [`PostProcessVolumeRow`]s from. Engines produce it
/// (a SceneDB component stored under this key, or a scene derivation).
pub const POST_PROCESS_VOLUMES: helio_core::BufferContract<PostProcessVolumeRow> =
    helio_core::BufferContract::new("post_process_volumes");
const _: () = assert!(
    std::mem::size_of::<[u32; 116]>()
        == std::mem::size_of::<helio_pass_postprocess::GpuPostProcessUniforms>()
);

impl From<helio_pass_postprocess::GpuPostProcessVolume> for PostProcessVolumeRow {
    fn from(v: helio_pass_postprocess::GpuPostProcessVolume) -> Self {
        bytemuck::cast(v)
    }
}
impl From<PostProcessVolumeRow> for helio_pass_postprocess::GpuPostProcessVolume {
    fn from(v: PostProcessVolumeRow) -> Self {
        bytemuck::cast(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_split_round_trips_every_byte() {
        let mut settings = helio_pass_postprocess::PostProcessSettings::default();
        settings.lens_flare.enabled = true;
        settings.lens_flare.ghost_count = 7;
        settings.lens_flare.field_margin = 0.25;
        settings.bloom_intensity = 0.125;
        let gpu = settings.to_gpu();
        let camera = CameraPostProcessRow::new(3, &settings);
        assert_eq!(
            bytemuck::bytes_of(&camera.settings()),
            bytemuck::bytes_of(&gpu)
        );
        assert_eq!(camera.view_id, 3);

        let mut descriptor = helio_pass_postprocess::PostProcessVolumeDescriptor::default();
        descriptor.settings = settings;
        descriptor.override_mask = [1, 2, 3, 4];
        let row = PostProcessVolumeRow::from(descriptor.to_gpu());
        assert_eq!(
            [
                row.override_mask_0,
                row.override_mask_1,
                row.override_mask_2,
                row.override_mask_3
            ],
            [1, 2, 3, 4]
        );
        let back = helio_pass_postprocess::GpuPostProcessVolume::from(row);
        assert_eq!(bytemuck::bytes_of(&back.settings), bytemuck::bytes_of(&gpu));
    }

    #[test]
    fn layout_matches_gpu_abi() {
        // `bytemuck::cast` is a by-value reinterpret (like `mem::transmute`):
        // it requires equal size, not equal alignment, so only size is
        // checked here -- see `settings`'s field doc for why the two
        // structs' internal shapes differ (nested struct vs. flat `u32`s).
        assert_eq!(
            std::mem::size_of::<PostProcessVolumeRow>(),
            std::mem::size_of::<helio_pass_postprocess::GpuPostProcessVolume>()
        );
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::PostProcessVolumeRow;
}

/// Optional per-view baseline: the camera's own post-process settings,
/// including lens flare. The first enabled row matching the camera's view id
/// (u32 bits of `jitter_frame.w`) wins; with none, the resolver's project
/// defaults apply. PP volumes then override on top. Removing the component
/// clears its packed row. `settings` holds the uniforms' bytes as
/// [`PostProcessVolumeRow`]'s does.
#[derive(bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct CameraPostProcessRow {
    pub view_id: u32,
    pub enabled: u32,
    pub _pad0: u32,
    pub _pad1: u32,
    pub settings: [u32; 116],
}

// SAFETY: a `#[repr(C)]` bytemuck `Pod` row: no padding bytes, any bit
// pattern valid. Lets an engine's SceneDB component embed it.
unsafe impl pulsar_scenedb::Pod for CameraPostProcessRow {}

/// The buffer the renderer reads [`CameraPostProcessRow`]s from. Engines produce it
/// (a SceneDB component stored under this key, or a scene derivation).
pub const CAMERA_POSTPROCESS: helio_core::BufferContract<CameraPostProcessRow> =
    helio_core::BufferContract::new("camera_postprocess");
impl CameraPostProcessRow {
    pub fn new(view_id: u32, settings: &helio_pass_postprocess::PostProcessSettings) -> Self {
        Self::from_gpu(view_id, &settings.to_gpu())
    }
    pub fn from_gpu(
        view_id: u32,
        settings: &helio_pass_postprocess::GpuPostProcessUniforms,
    ) -> Self {
        Self {
            view_id,
            enabled: 1,
            _pad0: 0,
            _pad1: 0,
            settings: bytemuck::cast(*settings),
        }
    }
    pub fn settings(&self) -> helio_pass_postprocess::GpuPostProcessUniforms {
        bytemuck::cast(self.settings)
    }
}
const _: () = assert!(std::mem::size_of::<CameraPostProcessRow>() == 480);
const _: () = assert!(std::mem::offset_of!(CameraPostProcessRow, settings) == 16);
