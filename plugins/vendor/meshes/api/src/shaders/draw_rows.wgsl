// The meshes' batch rows in WGSL (helio-mesh-api `DRAW_ROWS_SNIPPET`,
// `//!use mesh_draw_rows`), for what reads the object batch on the GPU: an
// instance's row, a draw group, the indirect draw a group is issued as, and
// the instance flags.

// `helio_mesh_api::GpuInstanceData` (224 bytes).
struct GpuInstanceData {
    transform:      mat4x4<f32>,
    normal_mat_0:   vec4<f32>,
    normal_mat_1:   vec4<f32>,
    normal_mat_2:   vec4<f32>,
    // Bounding sphere: frame-space centre, radius.
    bounds:         vec4<f32>,
    prev_model:     mat4x4<f32>,
    mesh_id:        u32,
    material_id:    u32,
    // `INSTANCE_FLAG_*` bits and the coordinate space
    // (`instance_coordinate_space`).
    flags:          u32,
    lightmap_index: u32,
    skin_vertex_offset: u32,
    bone_palette_offset: u32,
    bone_palette_count: u32,
    _skin_pad: u32,
}

// `helio_mesh_api::GpuDrawCall`: a draw group, its instances contiguous.
struct GpuDrawCall {
    index_count:    u32,
    first_index:    u32,
    vertex_offset:  i32,
    first_instance: u32,
    instance_count: u32,
}

// `helio_mesh_api::DrawIndexedIndirectArgs`.
struct DrawIndexedIndirect {
    index_count:    u32,
    instance_count: u32,
    first_index:    u32,
    base_vertex:    i32,
    first_instance: u32,
}

// `helio_mesh_api::INSTANCE_FLAG_*`.
const INSTANCE_FLAG_CASTS_SHADOW: u32 = 1u;
const INSTANCE_FLAG_RECEIVES_SHADOW: u32 = 2u;
const INSTANCE_FLAG_ALWAYS_VISIBLE: u32 = 4u;
const INSTANCE_FLAG_MOVABLE: u32 = 8u;
const INSTANCE_FLAG_CLASSIC_SUPPRESSED: u32 = 16u;
// `helio_mesh_api::INSTANCE_COORDINATE_SPACE_SHIFT`.
const INSTANCE_COORDINATE_SPACE_SHIFT: u32 = 8u;

// The coordinate space an instance lives in (`helio_mesh_api::coordinate_space`):
// its row in the frame's `coordinate_spaces`, 0 the frame's own.
fn instance_coordinate_space(flags: u32) -> u32 {
    return (flags >> INSTANCE_COORDINATE_SPACE_SHIFT) & 0xFFu;
}
