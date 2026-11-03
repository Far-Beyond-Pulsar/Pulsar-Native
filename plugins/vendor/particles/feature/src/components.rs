/// The authored Corona emitter record. The pass owns the schema; SceneDB owns
/// its packed CPU column and GPU projection.
#[derive(bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct CoronaEmitterRow {
    pub transform: [[f32; 4]; 4],
    pub emit_params: [f32; 4],
    pub size_params: [f32; 4],
    pub start_color: [f32; 4],
    pub end_color: [f32; 4],
    pub velocity: [f32; 4],
    pub velocity_variation: [f32; 4],
    pub extras: [f32; 4],
    pub texture_index: i32,
    pub particle_offset: u32,
    pub particle_count: u32,
    pub spawn_cursor: u32,
    pub _pad: [f32; 12],
}

// SAFETY: a `#[repr(C)]` bytemuck `Pod` row: no padding bytes, any bit
// pattern valid. Lets an engine's SceneDB component embed it.
unsafe impl pulsar_scenedb::Pod for CoronaEmitterRow {}

/// The buffer the renderer reads [`CoronaEmitterRow`]s from. Engines produce it
/// (a SceneDB component stored under this key, or a scene derivation).
pub const CORONA_EMITTERS: helio_core::BufferContract<CoronaEmitterRow> =
    helio_core::BufferContract::new("corona_emitters");

impl From<crate::GpuCoronaEmitter> for CoronaEmitterRow {
    fn from(value: crate::GpuCoronaEmitter) -> Self {
        bytemuck::cast(value)
    }
}

impl From<CoronaEmitterRow> for crate::GpuCoronaEmitter {
    fn from(value: CoronaEmitterRow) -> Self {
        bytemuck::cast(value)
    }
}

#[cfg(test)]
mod tests {
    use super::CoronaEmitterRow;

    #[test]
    fn scene_record_matches_corona_gpu_abi() {
        assert_eq!(
            std::mem::size_of::<CoronaEmitterRow>(),
            std::mem::size_of::<crate::GpuCoronaEmitter>()
        );
        assert_eq!(std::mem::align_of::<CoronaEmitterRow>(), 4);
        // corona.wgsl's `EmitterDef`.
        assert_eq!(std::mem::size_of::<crate::GpuCoronaEmitter>(), 240);
    }
}
