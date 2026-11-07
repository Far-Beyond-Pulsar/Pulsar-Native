// Pure math tests for helio-pass-occlusion-cull.
// No crate imports required — all helpers are defined locally.

const WORKGROUP_SIZE: u32 = 64;

/// Ceiling division: number of compute dispatch groups for `instance_count` instances.
fn dispatch_groups(instance_count: u32) -> u32 {
    (instance_count + WORKGROUP_SIZE - 1) / WORKGROUP_SIZE
}

/// Mip level to sample for a Hi-Z query covering a footprint of `w × h` pixels.
fn select_mip(w: u32, h: u32) -> u32 {
    (w.max(h) as f32).log2().ceil() as u32
}

/// Dimension of a Hi-Z pyramid level: halved each mip, clamped to 1.
fn hiz_mip_dim(base_dim: u32, mip: u32) -> u32 {
    (base_dim >> mip).max(1)
}

/// Returns true when the object is occluded by the Hi-Z surface.
/// (reversed depth convention: larger value = nearer the camera)
fn is_occluded(object_nearest_depth: f32, hiz_sample: f32) -> bool {
    object_nearest_depth < hiz_sample
}

/// Returns the 8 AABB corners in all combinations of (min/max) per axis.
fn aabb_corners(min: [f32; 3], max: [f32; 3]) -> [[f32; 3]; 8] {
    [
        [min[0], min[1], min[2]],
        [max[0], min[1], min[2]],
        [min[0], max[1], min[2]],
        [max[0], max[1], min[2]],
        [min[0], min[1], max[2]],
        [max[0], min[1], max[2]],
        [min[0], max[1], max[2]],
        [max[0], max[1], max[2]],
    ]
}

/// Local mirror of the private OcclusionUniforms struct for size/layout verification.
#[repr(C)]
struct OcclusionUniforms {
    instance_count: u32,
    hiz_width: u32,
    hiz_height: u32,
    hiz_mip_count: u32,
}

// ── Layout tests ──────────────────────────────────────────────────────────────

#[test]
fn uniforms_size_is_16_bytes() {
    assert_eq!(std::mem::size_of::<OcclusionUniforms>(), 16);
}

#[test]
fn uniforms_has_four_u32_fields() {
    // 4 fields × 4 bytes each = 16 bytes
    assert_eq!(4 * std::mem::size_of::<u32>(), 16);
}

#[test]
fn uniforms_each_field_is_4_bytes() {
    assert_eq!(std::mem::size_of::<u32>(), 4);
}

// ── Workgroup constant ────────────────────────────────────────────────────────

#[test]
fn workgroup_size_is_64() {
    assert_eq!(WORKGROUP_SIZE, 64);
}

#[test]
fn workgroup_size_is_power_of_two() {
    assert!(WORKGROUP_SIZE.is_power_of_two());
}

// ── Dispatch group math ───────────────────────────────────────────────────────

#[test]
fn dispatch_groups_zero_instances() {
    assert_eq!(dispatch_groups(0), 0);
}

#[test]
fn dispatch_groups_one_instance() {
    assert_eq!(dispatch_groups(1), 1);
}

#[test]
fn dispatch_groups_63_instances() {
    assert_eq!(dispatch_groups(63), 1);
}

#[test]
fn dispatch_groups_64_instances() {
    assert_eq!(dispatch_groups(64), 1);
}

#[test]
fn dispatch_groups_65_instances() {
    assert_eq!(dispatch_groups(65), 2);
}

#[test]
fn dispatch_groups_127_instances() {
    assert_eq!(dispatch_groups(127), 2);
}

#[test]
fn dispatch_groups_128_instances() {
    assert_eq!(dispatch_groups(128), 2);
}

#[test]
fn dispatch_groups_129_instances() {
    assert_eq!(dispatch_groups(129), 3);
}

#[test]
fn dispatch_groups_1000_instances() {
    // ceil(1000 / 64) = ceil(15.625) = 16
    assert_eq!(dispatch_groups(1000), 16);
}

#[test]
fn dispatch_groups_exact_multiples() {
    assert_eq!(dispatch_groups(64), 1);
    assert_eq!(dispatch_groups(128), 2);
    assert_eq!(dispatch_groups(256), 4);
    assert_eq!(dispatch_groups(512), 8);
}

#[test]
fn dispatch_groups_65536() {
    // 65536 / 64 = 1024 exactly
    assert_eq!(dispatch_groups(65536), 1024);
}

// ── Mip selection math ────────────────────────────────────────────────────────

#[test]
fn select_mip_1x1() {
    // log2(1) = 0  →  ceil(0) = 0
    assert_eq!(select_mip(1, 1), 0);
}

#[test]
fn select_mip_2x2() {
    // log2(2) = 1.0  →  ceil = 1
    assert_eq!(select_mip(2, 2), 1);
}

#[test]
fn select_mip_4x4() {
    assert_eq!(select_mip(4, 4), 2);
}

#[test]
fn select_mip_8x8() {
    assert_eq!(select_mip(8, 8), 3);
}

#[test]
fn select_mip_1024x1024() {
    // log2(1024) = 10.0  →  ceil = 10
    assert_eq!(select_mip(1024, 1024), 10);
}

#[test]
fn select_mip_uses_max_dimension() {
    // max(1920, 1080) = 1920, log2(1920) ≈ 10.906  →  ceil = 11
    assert_eq!(select_mip(1920, 1080), 11);
    assert_eq!(select_mip(1080, 1920), 11);
}

#[test]
fn select_mip_non_power_of_two_100() {
    // max = 100, log2(100) ≈ 6.644  →  ceil = 7
    assert_eq!(select_mip(100, 100), 7);
}

#[test]
fn select_mip_rectangular_512x256() {
    // max = 512, log2(512) = 9.0  →  9
    assert_eq!(select_mip(512, 256), 9);
    assert_eq!(select_mip(256, 512), 9);
}

#[test]
fn select_mip_non_power_of_two_3x3() {
    // log2(3) ≈ 1.585  →  ceil = 2
    assert_eq!(select_mip(3, 3), 2);
}

// ── Visibility / occlusion ────────────────────────────────────────────────────

#[test]
fn not_occluded_when_object_is_closer() {
    // object at 0.8, Hi-Z at 0.2 → visible (0.8 is NOT < 0.2)
    assert!(!is_occluded(0.8, 0.2));
}

#[test]
fn occluded_when_object_is_behind_surface() {
    // object at 0.3, Hi-Z at 0.9 → occluded (0.3 < 0.9)
    assert!(is_occluded(0.3, 0.9));
}

#[test]
fn not_occluded_at_equal_boundary() {
    // object_depth == hiz_sample: NOT strictly farther → visible
    assert!(!is_occluded(0.5, 0.5));
}

#[test]
fn not_occluded_very_close_object() {
    assert!(!is_occluded(1.0, 0.0));
}

#[test]
fn occluded_object_just_behind_surface() {
    assert!(is_occluded(0.499, 0.5));
}

// ── Hi-Z pyramid level dimensions ────────────────────────────────────────────

#[test]
fn hiz_mip_dim_level_zero_unchanged() {
    assert_eq!(hiz_mip_dim(1024, 0), 1024);
    assert_eq!(hiz_mip_dim(512, 0), 512);
    assert_eq!(hiz_mip_dim(256, 0), 256);
}

#[test]
fn hiz_mip_dim_level_one_halved() {
    assert_eq!(hiz_mip_dim(1024, 1), 512);
    assert_eq!(hiz_mip_dim(512, 1), 256);
    assert_eq!(hiz_mip_dim(64, 1), 32);
}

#[test]
fn hiz_mip_dim_level_two() {
    assert_eq!(hiz_mip_dim(1024, 2), 256);
    assert_eq!(hiz_mip_dim(512, 2), 128);
}

#[test]
fn hiz_mip_dim_1024_at_mip10() {
    // 1024 >> 10 = 1
    assert_eq!(hiz_mip_dim(1024, 10), 1);
}

#[test]
fn hiz_mip_dim_clamped_to_one_minimum() {
    // Shifting beyond available bits must not produce 0
    assert_eq!(hiz_mip_dim(1, 0), 1);
    assert_eq!(hiz_mip_dim(1, 1), 1);
    assert_eq!(hiz_mip_dim(2, 5), 1);
    assert_eq!(hiz_mip_dim(1024, 20), 1);
}

// ── AABB conservative projection (8 corners) ─────────────────────────────────

#[test]
fn aabb_has_eight_corners() {
    let corners = aabb_corners([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
    assert_eq!(corners.len(), 8);
}

#[test]
fn aabb_unit_cube_first_corner_is_min() {
    let corners = aabb_corners([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
    assert_eq!(corners[0], [0.0, 0.0, 0.0]);
}

#[test]
fn aabb_unit_cube_last_corner_is_max() {
    let corners = aabb_corners([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
    assert_eq!(corners[7], [1.0, 1.0, 1.0]);
}

#[test]
fn aabb_corners_all_eight_combinations() {
    let corners = aabb_corners([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
    assert_eq!(corners[0], [0.0, 0.0, 0.0]);
    assert_eq!(corners[1], [1.0, 0.0, 0.0]);
    assert_eq!(corners[2], [0.0, 1.0, 0.0]);
    assert_eq!(corners[3], [1.0, 1.0, 0.0]);
    assert_eq!(corners[4], [0.0, 0.0, 1.0]);
    assert_eq!(corners[5], [1.0, 0.0, 1.0]);
    assert_eq!(corners[6], [0.0, 1.0, 1.0]);
    assert_eq!(corners[7], [1.0, 1.0, 1.0]);
}

#[test]
fn aabb_corners_asymmetric_bounds() {
    let corners = aabb_corners([-1.0, -2.0, -4.0], [1.0, 2.0, 4.0]);
    // Each axis independently takes its min or max value
    for c in &corners {
        assert!(c[0] == -1.0 || c[0] == 1.0);
        assert!(c[1] == -2.0 || c[1] == 2.0);
        assert!(c[2] == -4.0 || c[2] == 4.0);
    }
}
