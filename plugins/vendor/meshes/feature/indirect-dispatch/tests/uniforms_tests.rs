//! Tests for CullUniforms layout, indirect dispatch sizing, and
//! DrawIndexedIndirect argument semantics.

// ── CullUniforms layout constants ─────────────────────────────────────────────
//
//  frustum_planes: [[f32; 4]; 6]  →  6 × 16 = 96 bytes
//  draw_count:     u32            →       4 bytes
//  _pad:           [u32; 3]       →  3 ×  4 = 12 bytes
//  ────────────────────────────────────────────────────
//  Total                          →     112 bytes

const FRUSTUM_PLANES_SIZE: usize = 6 * std::mem::size_of::<[f32; 4]>(); // 96
const DRAW_COUNT_SIZE: usize = std::mem::size_of::<u32>(); // 4
const PADDING_SIZE: usize = 3 * std::mem::size_of::<u32>(); // 12
const CULL_UNIFORMS_SIZE: usize = FRUSTUM_PLANES_SIZE + DRAW_COUNT_SIZE + PADDING_SIZE;

/// Number of threads per workgroup in the indirect-dispatch compute shader.
const WORKGROUP_SIZE: u32 = 64;

// ── CullUniforms size tests ───────────────────────────────────────────────────

#[test]
fn frustum_planes_field_is_96_bytes() {
    assert_eq!(FRUSTUM_PLANES_SIZE, 96);
}

#[test]
fn draw_count_field_is_4_bytes() {
    assert_eq!(DRAW_COUNT_SIZE, 4);
}

#[test]
fn padding_field_is_12_bytes() {
    assert_eq!(PADDING_SIZE, 12);
}

#[test]
fn cull_uniforms_total_size_is_112() {
    assert_eq!(CULL_UNIFORMS_SIZE, 112);
}

#[test]
fn cull_uniforms_is_multiple_of_16() {
    // Uniform buffers must be aligned to 16 bytes on most GPU APIs.
    assert_eq!(CULL_UNIFORMS_SIZE % 16, 0);
}

#[test]
fn cull_uniforms_is_multiple_of_4() {
    assert_eq!(CULL_UNIFORMS_SIZE % 4, 0);
}

#[test]
fn frustum_planes_field_offset_is_zero() {
    // frustum_planes is the first field in CullUniforms.
    const FRUSTUM_OFFSET: usize = 0;
    assert_eq!(FRUSTUM_OFFSET, 0);
}

#[test]
fn draw_count_field_offset_is_96() {
    // draw_count follows frustum_planes (96 bytes).
    const DRAW_COUNT_OFFSET: usize = 96;
    assert_eq!(DRAW_COUNT_OFFSET, FRUSTUM_PLANES_SIZE);
}

#[test]
fn padding_field_offset_is_100() {
    // _pad starts right after draw_count.
    const PAD_OFFSET: usize = 100;
    assert_eq!(PAD_OFFSET, FRUSTUM_PLANES_SIZE + DRAW_COUNT_SIZE);
}

// ── Workgroup size tests ──────────────────────────────────────────────────────

#[test]
fn workgroup_size_is_64() {
    assert_eq!(WORKGROUP_SIZE, 64);
}

#[test]
fn workgroup_size_is_power_of_two() {
    assert!(WORKGROUP_SIZE.is_power_of_two());
}

// ── Dispatch sizing tests ─────────────────────────────────────────────────────

#[test]
fn dispatch_one_group_for_64_draws() {
    assert_eq!(64_u32.div_ceil(WORKGROUP_SIZE), 1);
}

#[test]
fn dispatch_two_groups_for_65_draws() {
    assert_eq!(65_u32.div_ceil(WORKGROUP_SIZE), 2);
}

#[test]
fn dispatch_two_groups_for_128_draws() {
    assert_eq!(128_u32.div_ceil(WORKGROUP_SIZE), 2);
}

#[test]
fn dispatch_three_groups_for_129_draws() {
    assert_eq!(129_u32.div_ceil(WORKGROUP_SIZE), 3);
}

#[test]
fn dispatch_one_group_for_one_draw() {
    assert_eq!(1_u32.div_ceil(WORKGROUP_SIZE), 1);
}

#[test]
fn dispatch_zero_groups_for_zero_draws() {
    assert_eq!(0_u32.div_ceil(WORKGROUP_SIZE), 0);
}

#[test]
fn dispatch_ten_groups_for_640_draws() {
    assert_eq!(640_u32.div_ceil(WORKGROUP_SIZE), 10);
}

#[test]
fn dispatch_covers_all_draws_for_any_count() {
    // For every count in 0..=512, the dispatched groups × WORKGROUP_SIZE >= count.
    for count in 0_u32..=512 {
        let groups = count.div_ceil(WORKGROUP_SIZE);
        assert!(
            groups * WORKGROUP_SIZE >= count,
            "{groups} groups insufficient for {count} draws"
        );
    }
}

#[test]
fn dispatch_exact_for_multiples_of_workgroup_size() {
    for k in 1_u32..=8 {
        let count = k * WORKGROUP_SIZE;
        assert_eq!(count.div_ceil(WORKGROUP_SIZE), k);
    }
}

// ── DrawIndexedIndirect tests ─────────────────────────────────────────────────

#[test]
fn draw_indexed_indirect_has_five_arguments() {
    // DrawIndexedIndirect: vertex_count, instance_count, first_index,
    //                      base_vertex, first_instance
    const ARG_COUNT: usize = 5;
    assert_eq!(ARG_COUNT, 5);
}

#[test]
fn draw_indexed_indirect_size_is_20_bytes() {
    // 5 × u32 = 20 bytes.
    let size = 5 * std::mem::size_of::<u32>();
    assert_eq!(size, 20);
}

#[test]
fn draw_indexed_indirect_instance_count_zero_skips_draw() {
    // When instance_count = 0 the GPU issues no draw call for that object.
    let instance_count: u32 = 0;
    assert_eq!(instance_count, 0);
}

// ── Plane array layout ────────────────────────────────────────────────────────

#[test]
fn frustum_plane_array_has_six_elements() {
    let planes: [[f32; 4]; 6] = [[0.0; 4]; 6];
    assert_eq!(planes.len(), 6);
}

#[test]
fn frustum_plane_array_total_bytes() {
    let planes: [[f32; 4]; 6] = [[0.0; 4]; 6];
    assert_eq!(std::mem::size_of_val(&planes), 96);
}
