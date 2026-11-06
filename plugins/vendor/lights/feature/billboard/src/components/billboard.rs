/// The authoritative SceneDB record consumed by the billboard pass.
///
/// The packed layout is intentional: the shader reads the three fields as one
/// 48-byte instance record. `World::insert`/`get_mut` update this record and
/// SceneDB owns the deferred GPU upload.
#[derive(bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct BillboardRow {
    /// Where the billboard is, in the frame's space, as every position in a
    /// pass row: metres from the frame's world origin
    /// (`Renderer::world_origin`).
    pub world_pos: [f32; 4],
    /// xy: bounds the source image fits inside, preserving its aspect ratio;
    /// z > 0.5: multiply size by view depth; w: reserved.
    pub scale_flags: [f32; 4],
    pub color: [f32; 4],
}

// SAFETY: a `#[repr(C)]` bytemuck `Pod` row: no padding bytes, any bit
// pattern valid. Lets an engine's SceneDB component embed it.
unsafe impl pulsar_scenedb::Pod for BillboardRow {}

/// The buffer the renderer reads [`BillboardRow`]s from. Engines produce it
/// (a SceneDB component stored under this key, or a scene derivation).
pub const BILLBOARD_INSTANCES: helio_core::BufferContract<BillboardRow> =
    helio_core::BufferContract::new("billboard_instances");
