//! SceneDB-owned decal records. Visible lists and screen-space work are
//! transient state owned by this pass.

#[derive(bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct DecalRow {
    pub transform: [f32; 16],
    pub color: [f32; 4],
    pub albedo_texture_index: u32,
    pub normal_texture_index: u32,
    pub roughness_texture_index: u32,
    pub metalness_texture_index: u32,
    pub blend_mode: u32,
    pub decal_type: u32,
    pub fade_time: f32,
    pub fade_start_delay: f32,
    pub age: f32,
    pub normal_adapt: u32,
    pub _pad0: f32,
    pub _pad1: f32,
}

// SAFETY: a `#[repr(C)]` bytemuck `Pod` row: no padding bytes, any bit
// pattern valid. Lets an engine's SceneDB component embed it.
unsafe impl pulsar_scenedb::Pod for DecalRow {}

/// The buffer the renderer reads [`DecalRow`]s from. Engines produce it
/// (a SceneDB component stored under this key, or a scene derivation).
pub const DECALS: helio_core::BufferContract<DecalRow> = helio_core::BufferContract::new("decals");

impl From<crate::GpuDecal> for DecalRow {
    fn from(v: crate::GpuDecal) -> Self {
        bytemuck::cast(v)
    }
}
impl From<DecalRow> for crate::GpuDecal {
    fn from(v: DecalRow) -> Self {
        bytemuck::cast(v)
    }
}

#[cfg(test)]
mod tests {
    use super::DecalRow;
    #[test]
    fn layout_matches_gpu_abi() {
        assert_eq!(
            std::mem::size_of::<DecalRow>(),
            std::mem::size_of::<crate::GpuDecal>()
        );
    }
}
#[cfg(test)]
mod lifecycle_tests {
    use super::DecalRow;
}
