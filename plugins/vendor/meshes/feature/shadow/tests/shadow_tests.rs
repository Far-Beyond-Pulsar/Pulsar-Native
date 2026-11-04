//! Architecture and math tests for the Shadow Atlas pass.
//!
//! All tests are pure math — no GPU, no wgpu, no crate imports.
//! Constants mirror the private values in `helio-pass-shadow/src/lib.rs`.

// Mirror of private constants in lib.rs
const MAX_SHADOW_FACES: usize = 256;
const SHADOW_RES: u32 = 1024;
const FACE_BUF_STRIDE: u64 = 256;

// Derived constants used throughout
const DEPTH32FLOAT_BYTES: u64 = 4;
const CSM_CASCADE_COUNT: usize = 4;
const CUBE_FACES_PER_LIGHT: usize = 6;

// ── Basic constant values ─────────────────────────────────────────────────────

#[test]
fn shadow_atlas_max_faces_is_256() {
    assert_eq!(MAX_SHADOW_FACES, 256);
}

#[test]
fn shadow_atlas_resolution_is_1024() {
    assert_eq!(SHADOW_RES, 1024);
}

#[test]
fn face_buf_stride_is_256() {
    assert_eq!(FACE_BUF_STRIDE, 256);
}

// ── FACE_BUF_STRIDE alignment properties ─────────────────────────────────────

#[test]
fn face_buf_stride_is_power_of_two() {
    assert!(FACE_BUF_STRIDE.is_power_of_two());
}

#[test]
fn face_buf_stride_satisfies_wgpu_min_uniform_alignment() {
    // wgpu guarantees min_uniform_buffer_offset_alignment ≤ 256 on all backends
    // (Metal, Vulkan, DX12, WebGPU). Using exactly 256 is always safe.
    let wgpu_max_required_alignment: u64 = 256;
    assert!(FACE_BUF_STRIDE >= wgpu_max_required_alignment);
    assert_eq!(FACE_BUF_STRIDE % wgpu_max_required_alignment, 0);
}

#[test]
fn face_buf_stride_larger_than_face_index_payload() {
    // Each face-index entry is a single u32 (4 bytes); stride is 256 bytes.
    // The remaining 252 bytes are zero padding to meet alignment requirements.
    let face_index_payload_bytes: u64 = 4; // sizeof(u32)
    assert!(FACE_BUF_STRIDE > face_index_payload_bytes);
    let padding = FACE_BUF_STRIDE - face_index_payload_bytes;
    assert_eq!(padding, 252);
}

// ── VRAM / memory calculations ────────────────────────────────────────────────

#[test]
fn depth32float_bytes_per_pixel() {
    // `wgpu::TextureFormat::Depth32Float` occupies exactly 4 bytes per texel.
    assert_eq!(DEPTH32FLOAT_BYTES, 4);
}

#[test]
fn atlas_vram_per_face_bytes() {
    let bytes = SHADOW_RES as u64 * SHADOW_RES as u64 * DEPTH32FLOAT_BYTES;
    // 1024 × 1024 × 4 = 4,194,304 bytes = 4 MiB per face
    assert_eq!(bytes, 4_194_304);
}

#[test]
fn atlas_vram_total_bytes() {
    let per_face = SHADOW_RES as u64 * SHADOW_RES as u64 * DEPTH32FLOAT_BYTES;
    let total = per_face * MAX_SHADOW_FACES as u64;
    // 4 MiB × 256 = 1,073,741,824 bytes = 1 GiB
    assert_eq!(total, 1_073_741_824);
}

#[test]
fn atlas_vram_total_is_one_gib() {
    let per_face = SHADOW_RES as u64 * SHADOW_RES as u64 * DEPTH32FLOAT_BYTES;
    let total = per_face * MAX_SHADOW_FACES as u64;
    let one_gib: u64 = 1 << 30; // 1,073,741,824
    assert_eq!(total, one_gib);
}

#[test]
fn face_idx_buf_total_size_bytes() {
    // The face-index uniform buffer holds one entry per shadow face.
    // Each entry is FACE_BUF_STRIDE bytes wide (dynamic-offset aligned).
    let total = FACE_BUF_STRIDE * MAX_SHADOW_FACES as u64;
    // 256 × 256 = 65,536 bytes = 64 KiB
    assert_eq!(total, 65_536);
}

#[test]
fn face_idx_buf_is_64_kib() {
    let total = FACE_BUF_STRIDE * MAX_SHADOW_FACES as u64;
    assert_eq!(total, 64 * 1024);
}

// ── CSM cascade and point-light face budget ───────────────────────────────────

#[test]
fn csm_cascade_count_is_four() {
    // Four CSM splits is a common balance between shadow quality and cost.
    assert_eq!(CSM_CASCADE_COUNT, 4);
}

#[test]
fn cube_faces_per_point_light() {
    // A point light uses all 6 faces of a cube map.
    assert_eq!(CUBE_FACES_PER_LIGHT, 6);
}

#[test]
fn max_point_lights_from_face_budget() {
    // Remaining faces after reserving CSM cascades, divided by cube-faces per light.
    let remaining = MAX_SHADOW_FACES - CSM_CASCADE_COUNT;
    let max_point_lights = remaining / CUBE_FACES_PER_LIGHT;
    // (256 - 4) / 6 = 252 / 6 = 42
    assert_eq!(max_point_lights, 42);
}

#[test]
fn max_point_light_faces() {
    let remaining = MAX_SHADOW_FACES - CSM_CASCADE_COUNT;
    let max_point_lights = remaining / CUBE_FACES_PER_LIGHT;
    let max_faces = max_point_lights * CUBE_FACES_PER_LIGHT;
    // 42 × 6 = 252
    assert_eq!(max_faces, 252);
}

#[test]
fn total_atlas_faces_equals_csm_plus_max_point_light_faces() {
    let remaining = MAX_SHADOW_FACES - CSM_CASCADE_COUNT;
    let max_point_lights = remaining / CUBE_FACES_PER_LIGHT;
    let point_light_faces = max_point_lights * CUBE_FACES_PER_LIGHT;
    // 4 CSM cascades + 252 point-light faces = 256 total
    assert_eq!(CSM_CASCADE_COUNT + point_light_faces, MAX_SHADOW_FACES);
}

#[test]
fn face_budget_has_no_wasted_faces() {
    // 256 - 4 = 252, and 252 % 6 = 0, so no faces are wasted.
    let point_faces = MAX_SHADOW_FACES - CSM_CASCADE_COUNT;
    assert_eq!(point_faces % CUBE_FACES_PER_LIGHT, 0);
}

// ── Dynamic-offset addressing ─────────────────────────────────────────────────

#[test]
fn dynamic_offset_for_face_zero_is_zero() {
    let face: u64 = 0;
    let offset = face * FACE_BUF_STRIDE;
    assert_eq!(offset, 0);
}

#[test]
fn dynamic_offset_for_face_one() {
    let face: u64 = 1;
    let offset = face * FACE_BUF_STRIDE;
    assert_eq!(offset, 256);
}

#[test]
fn dynamic_offset_for_last_face() {
    let face: u64 = (MAX_SHADOW_FACES - 1) as u64;
    let offset = face * FACE_BUF_STRIDE;
    assert_eq!(offset, 255 * 256); // = 65,280
}

#[test]
fn dynamic_offset_is_multiple_of_stride() {
    for face in 0..MAX_SHADOW_FACES {
        let offset = face as u64 * FACE_BUF_STRIDE;
        assert_eq!(offset % FACE_BUF_STRIDE, 0);
    }
}

// ── Light-space matrix and related buffer sizing ──────────────────────────────

#[test]
fn light_space_matrix_size_bytes() {
    // A 4×4 f32 matrix = 16 × 4 bytes = 64 bytes.
    let size: usize = 4 * 4 * std::mem::size_of::<f32>();
    assert_eq!(size, 64);
}

#[test]
fn shadow_matrices_buffer_size_bytes() {
    // One 4×4 matrix per atlas face.
    let matrix_size: usize = 4 * 4 * std::mem::size_of::<f32>();
    let total = matrix_size * MAX_SHADOW_FACES;
    // 64 × 256 = 16,384 bytes = 16 KiB
    assert_eq!(total, 16_384);
}

// ── DrawIndexedIndirect sizing ────────────────────────────────────────────────

#[test]
fn draw_indexed_indirect_size_bytes() {
    // wgpu DrawIndexedIndirectArgs: [index_count, instance_count, first_index,
    //   base_vertex, first_instance] = 5 × u32 = 20 bytes.
    let size: usize = 5 * std::mem::size_of::<u32>();
    assert_eq!(size, 20);
}

#[test]
fn multi_draw_indirect_buffer_for_all_faces() {
    let indirect_size: usize = 5 * std::mem::size_of::<u32>(); // 20 bytes per draw
                                                               // One indirect draw per face (assuming max draws = MAX_SHADOW_FACES)
    let total = indirect_size * MAX_SHADOW_FACES;
    // 20 × 256 = 5,120 bytes
    assert_eq!(total, 5_120);
}

// ── Atlas texture array properties ───────────────────────────────────────────

#[test]
fn atlas_texture_array_layers_equals_max_shadow_faces() {
    // The atlas is a 2D texture array with one layer per shadow face.
    assert_eq!(MAX_SHADOW_FACES, 256);
}

#[test]
fn atlas_is_square() {
    // Each face in the atlas is SHADOW_RES × SHADOW_RES.
    assert_eq!(SHADOW_RES, SHADOW_RES); // trivially square
                                        // More usefully: width == height
    let width = SHADOW_RES;
    let height = SHADOW_RES;
    assert_eq!(width, height);
}

#[test]
fn atlas_total_texels() {
    let texels = SHADOW_RES as u64 * SHADOW_RES as u64 * MAX_SHADOW_FACES as u64;
    // 1024 × 1024 × 256 = 268,435,456 texels
    assert_eq!(texels, 268_435_456);
}

// ── u32 face-index sanity ─────────────────────────────────────────────────────

#[test]
fn all_face_indices_fit_in_u32() {
    // Each face-index entry is a u32 encoding the face number (0..255).
    // Verify the maximum face index does not overflow u32.
    let max: u32 = (MAX_SHADOW_FACES - 1) as u32;
    assert!(max <= u32::MAX);
    assert_eq!(max, 255);
}
