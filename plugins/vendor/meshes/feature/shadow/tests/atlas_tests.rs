//! Shadow atlas resolution, PCF, bias, and binding math tests.
//!
//! Pure math — no GPU, no wgpu, no crate imports.

const MAX_SHADOW_FACES: usize = 256;
const SHADOW_RES: u32 = 1024;
const FACE_BUF_STRIDE: u64 = 256;

// ── Resolution tradeoffs ──────────────────────────────────────────────────────

#[test]
fn resolution_512_per_face_bytes() {
    let bytes = 512u64 * 512 * 4; // Depth32Float = 4 bytes
    assert_eq!(bytes, 1_048_576); // 1 MiB
}

#[test]
fn resolution_1024_per_face_bytes() {
    let bytes = 1024u64 * 1024 * 4;
    assert_eq!(bytes, 4_194_304); // 4 MiB
}

#[test]
fn resolution_2048_per_face_bytes() {
    let bytes = 2048u64 * 2048 * 4;
    assert_eq!(bytes, 16_777_216); // 16 MiB
}

#[test]
fn atlas_512_total_bytes() {
    let total = 512u64 * 512 * 4 * MAX_SHADOW_FACES as u64;
    assert_eq!(total, 268_435_456); // 256 MiB
                                    // NOTE: The source doc comment "~256 MB at 1024 px" appears to apply 512-px math.
}

#[test]
fn atlas_1024_total_bytes() {
    let total = 1024u64 * 1024 * 4 * MAX_SHADOW_FACES as u64;
    assert_eq!(total, 1_073_741_824); // 1 GiB — the actual runtime cost
}

#[test]
fn atlas_2048_total_bytes() {
    let total = 2048u64 * 2048 * 4 * MAX_SHADOW_FACES as u64;
    assert_eq!(total, 4_294_967_296); // 4 GiB — prohibitive
}

#[test]
fn doubling_resolution_quadruples_memory() {
    let mem_512 = 512u64 * 512 * 4 * MAX_SHADOW_FACES as u64;
    let mem_1024 = 1024u64 * 1024 * 4 * MAX_SHADOW_FACES as u64;
    assert_eq!(mem_1024, mem_512 * 4);
}

// ── Texel size (size of one texel in UV space) ────────────────────────────────

#[test]
fn texel_size_at_512() {
    let texel: f32 = 1.0 / 512.0;
    assert!((texel - 0.001953125_f32).abs() < 1e-10);
}

#[test]
fn texel_size_at_1024() {
    let texel: f32 = 1.0 / SHADOW_RES as f32;
    assert!((texel - 0.0009765625_f32).abs() < 1e-10);
}

#[test]
fn texel_size_at_2048() {
    let texel: f32 = 1.0 / 2048.0;
    assert!((texel - 0.00048828125_f32).abs() < 1e-10);
}

#[test]
fn higher_resolution_gives_smaller_texels() {
    let texel_512 = 1.0_f32 / 512.0;
    let texel_1024 = 1.0_f32 / 1024.0;
    let texel_2048 = 1.0_f32 / 2048.0;
    assert!(texel_512 > texel_1024);
    assert!(texel_1024 > texel_2048);
}

// ── PCF (Percentage-Closer Filtering) kernel sizes ───────────────────────────

#[test]
fn pcf_3x3_sample_count() {
    let kernel = 3 * 3;
    assert_eq!(kernel, 9);
}

#[test]
fn pcf_5x5_sample_count() {
    let kernel = 5 * 5;
    assert_eq!(kernel, 25);
}

#[test]
fn pcf_7x7_sample_count() {
    let kernel = 7 * 7;
    assert_eq!(kernel, 49);
}

#[test]
fn pcf_kernel_radius_for_3x3() {
    // A 3×3 kernel has radius 1 (±1 in each axis).
    let radius: i32 = 1;
    let side = 2 * radius + 1;
    assert_eq!(side, 3);
    assert_eq!(side * side, 9);
}

#[test]
fn pcf_kernel_radius_for_5x5() {
    let radius: i32 = 2;
    let side = 2 * radius + 1;
    assert_eq!(side, 5);
    assert_eq!(side * side, 25);
}

#[test]
fn pcf_max_radius_5_texels_at_1024_in_uv_space() {
    // 5-texel PCF radius expressed in UV space at 1024px.
    let radius_uv = 5.0_f32 / SHADOW_RES as f32;
    assert!((radius_uv - 0.00488281_f32).abs() < 1e-5);
}

// ── Shadow bias ───────────────────────────────────────────────────────────────

#[test]
fn shadow_bias_typical_minimum() {
    let bias_min: f32 = 0.0001;
    assert!(bias_min > 0.0);
    assert!(bias_min < 0.01);
}

#[test]
fn shadow_bias_typical_maximum() {
    let bias_max: f32 = 0.005;
    assert!(bias_max > 0.0);
    assert!(bias_max < 0.1);
}

#[test]
fn shadow_bias_is_positive() {
    // Bias is added to the stored depth so the surface does not shadow itself.
    let bias: f32 = 0.001;
    assert!(bias > 0.0);
}

#[test]
fn normal_offset_bias_depends_on_texel_size() {
    // Slope-scale bias should scale with texel size.
    let texel = 1.0_f32 / SHADOW_RES as f32;
    let slope_bias = 2.0 * texel;
    assert!(slope_bias > 0.0);
    assert!((slope_bias - 0.001953125_f32).abs() < 1e-7);
}

// ── Light-space (shadow) matrices ────────────────────────────────────────────

#[test]
fn light_space_matrix_is_4x4_f32() {
    // Shadow projection matrix: 4×4 × sizeof(f32) = 64 bytes.
    let size = 4 * 4 * std::mem::size_of::<f32>();
    assert_eq!(size, 64);
}

#[test]
fn shadow_matrices_buffer_for_all_faces() {
    let matrix_bytes = 4 * 4 * std::mem::size_of::<f32>();
    let total = matrix_bytes * MAX_SHADOW_FACES;
    assert_eq!(total, 16_384); // 16 KiB
}

#[test]
fn shadow_matrices_buffer_is_16_kib() {
    let matrix_bytes = 4 * 4 * std::mem::size_of::<f32>();
    let total = matrix_bytes * MAX_SHADOW_FACES;
    assert_eq!(total, 16 * 1024);
}

// ── wgpu bind group / dynamic offset ─────────────────────────────────────────

#[test]
fn dynamic_offset_stride_equals_face_buf_stride() {
    // The dynamic offset passed to set_bind_group = face * FACE_BUF_STRIDE.
    for face in 0..16_usize {
        let offset = face as u64 * FACE_BUF_STRIDE;
        assert_eq!(offset % 256, 0, "face {} offset not 256-aligned", face);
    }
}

#[test]
fn face_buf_total_covers_all_faces() {
    let total = FACE_BUF_STRIDE * MAX_SHADOW_FACES as u64;
    assert_eq!(total, 65_536);
}

// ── Perspective shadow properties ────────────────────────────────────────────

#[test]
fn shadow_near_plane_must_be_positive() {
    let near: f32 = 0.05;
    assert!(near > 0.0);
}

#[test]
fn shadow_far_must_exceed_near() {
    let near: f32 = 0.05;
    let far: f32 = 100.0;
    assert!(far > near);
}

#[test]
fn shadow_far_near_ratio() {
    // For a typical room-scale scene.
    let near: f32 = 0.1;
    let far: f32 = 100.0;
    assert!(far / near >= 10.0);
}

#[test]
fn face_viewport_count_matches_atlas_layers() {
    // One viewport (render-target view) per atlas layer.
    let viewport_count = MAX_SHADOW_FACES;
    assert_eq!(viewport_count, 256);
}

// ── Atlas face index uniqueness ───────────────────────────────────────────────

#[test]
fn all_face_offsets_are_unique() {
    // Each face maps to a distinct byte offset in face_idx_buf.
    let offsets: Vec<u64> = (0..MAX_SHADOW_FACES)
        .map(|f| f as u64 * FACE_BUF_STRIDE)
        .collect();
    let mut sorted = offsets.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), MAX_SHADOW_FACES);
}
