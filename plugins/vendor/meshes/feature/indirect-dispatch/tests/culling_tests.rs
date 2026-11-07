//! Tests for the frustum culling mathematics used by IndirectDispatchPass.
//!
//! A frustum plane is stored as `[nx, ny, nz, d]` where `(nx,ny,nz)` is the
//! inward-facing unit normal and `d` is the plane offset from the origin.
//! A sphere (centre, radius) is visible if:
//!   dot(plane.xyz, centre) + d >= -radius

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Returns `true` if the sphere is fully or partially inside the half-space
/// defined by `plane = [nx, ny, nz, d]`.
fn sphere_vs_plane(plane: [f32; 4], center: [f32; 3], radius: f32) -> bool {
    let dist = plane[0] * center[0] + plane[1] * center[1] + plane[2] * center[2] + plane[3];
    dist >= -radius
}

/// Returns `true` if the sphere passes all 6 frustum planes.
fn sphere_in_frustum(planes: &[[f32; 4]; 6], center: [f32; 3], radius: f32) -> bool {
    planes.iter().all(|&p| sphere_vs_plane(p, center, radius))
}

// ── Box frustum: x∈[-1,1], y∈[-1,1], z∈[-1,100] ─────────────────────────────

const NEAR_PLANE: [f32; 4] = [0.0, 0.0, 1.0, 1.0]; // z >= -1
const FAR_PLANE: [f32; 4] = [0.0, 0.0, -1.0, 100.0]; // z <=  100
const LEFT_PLANE: [f32; 4] = [1.0, 0.0, 0.0, 1.0]; // x >= -1
const RIGHT_PLANE: [f32; 4] = [-1.0, 0.0, 0.0, 1.0]; // x <=  1
const BOTTOM_PLANE: [f32; 4] = [0.0, 1.0, 0.0, 1.0]; // y >= -1
const TOP_PLANE: [f32; 4] = [0.0, -1.0, 0.0, 1.0]; // y <=  1

const BOX_FRUSTUM: [[f32; 4]; 6] = [
    NEAR_PLANE,
    FAR_PLANE,
    LEFT_PLANE,
    RIGHT_PLANE,
    BOTTOM_PLANE,
    TOP_PLANE,
];

// ── Tests ─────────────────────────────────────────────────────────────────────

#[test]
fn frustum_has_six_planes() {
    assert_eq!(BOX_FRUSTUM.len(), 6);
}

#[test]
fn sphere_at_origin_is_inside() {
    assert!(sphere_in_frustum(&BOX_FRUSTUM, [0.0, 0.0, 0.0], 0.5));
}

#[test]
fn point_at_origin_is_inside() {
    assert!(sphere_in_frustum(&BOX_FRUSTUM, [0.0, 0.0, 0.0], 0.0));
}

#[test]
fn sphere_beyond_far_plane_is_outside() {
    // Centre at z=200, radius 1 → behind the far plane (z<=100).
    assert!(!sphere_vs_plane(FAR_PLANE, [0.0, 0.0, 200.0], 1.0));
}

#[test]
fn sphere_just_inside_far_plane() {
    assert!(sphere_vs_plane(FAR_PLANE, [0.0, 0.0, 99.0], 0.0));
}

#[test]
fn sphere_tangent_to_near_plane_from_inside() {
    // Centre at z=-1 (on the near plane), radius 0 → right on boundary → inside.
    assert!(sphere_vs_plane(NEAR_PLANE, [0.0, 0.0, -1.0], 0.0));
}

#[test]
fn sphere_behind_near_plane_is_outside() {
    // Centre at z=-2, radius 0 → completely behind the near plane.
    assert!(!sphere_vs_plane(NEAR_PLANE, [0.0, 0.0, -2.0], 0.0));
}

#[test]
fn sphere_straddling_near_plane_is_visible() {
    // Centre at z=-1.5, radius 1 → intersects the near plane → visible.
    assert!(sphere_vs_plane(NEAR_PLANE, [0.0, 0.0, -1.5], 1.0));
}

#[test]
fn sphere_outside_left_plane_is_culled() {
    assert!(!sphere_vs_plane(LEFT_PLANE, [-2.0, 0.0, 0.0], 0.0));
}

#[test]
fn sphere_inside_left_plane() {
    assert!(sphere_vs_plane(LEFT_PLANE, [0.0, 0.0, 0.0], 0.5));
}

#[test]
fn point_on_left_plane_is_inside() {
    // Distance = 0, radius = 0 → 0 >= 0 → inside.
    assert!(sphere_vs_plane(LEFT_PLANE, [-1.0, 0.0, 0.0], 0.0));
}

#[test]
fn sphere_beyond_right_plane_is_culled() {
    // x=2 → dist = -1*2 + 1 = -1 < 0 = -radius.
    assert!(!sphere_vs_plane(RIGHT_PLANE, [2.0, 0.0, 0.0], 0.0));
}

#[test]
fn sphere_above_top_plane_is_culled() {
    // y=2 → dist = -1*2 + 1 = -1 < -0.5 → culled.
    assert!(!sphere_vs_plane(TOP_PLANE, [0.0, 2.0, 0.0], 0.5));
}

#[test]
fn sphere_far_off_axis_fails_whole_frustum() {
    // Object at x=1000 fails the right-plane test.
    assert!(!sphere_in_frustum(&BOX_FRUSTUM, [1000.0, 0.0, 0.0], 1.0));
}

#[test]
fn sphere_near_corner_is_visible() {
    // Centre at (-0.9, -0.9, 0), radius 0.05 → inside all planes.
    assert!(sphere_in_frustum(&BOX_FRUSTUM, [-0.9, -0.9, 0.0], 0.05));
}

#[test]
fn all_box_frustum_normals_are_unit_length() {
    for plane in &BOX_FRUSTUM {
        let len = (plane[0] * plane[0] + plane[1] * plane[1] + plane[2] * plane[2]).sqrt();
        assert!(
            (len - 1.0).abs() < 1e-6,
            "plane {plane:?} normal length is {len}, expected 1.0"
        );
    }
}

#[test]
fn near_plane_normal_is_unit() {
    let n = NEAR_PLANE;
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    assert!((len - 1.0).abs() < 1e-6);
}

#[test]
fn far_plane_normal_is_unit() {
    let n = FAR_PLANE;
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    assert!((len - 1.0).abs() < 1e-6);
}

#[test]
fn frustum_plane_struct_is_four_floats() {
    let plane: [f32; 4] = LEFT_PLANE;
    assert_eq!(plane.len(), 4);
}

#[test]
fn frustum_plane_size_is_sixteen_bytes() {
    assert_eq!(std::mem::size_of::<[f32; 4]>(), 16);
}

#[test]
fn large_sphere_spanning_entire_frustum_is_visible() {
    // A sphere with radius=200 centred at origin engulfs the whole box.
    assert!(sphere_in_frustum(&BOX_FRUSTUM, [0.0, 0.0, 0.0], 200.0));
}

#[test]
fn sphere_just_outside_multiple_planes_is_culled() {
    // Object at (0, 0, 200) with radius 50 is still outside the far plane.
    assert!(!sphere_in_frustum(&BOX_FRUSTUM, [0.0, 0.0, 200.0], 50.0));
}
