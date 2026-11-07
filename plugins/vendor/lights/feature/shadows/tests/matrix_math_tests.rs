//! Perspective and orthographic projection matrix math tests.
//!
//! Tests the right-handed wgpu/Vulkan projection conventions (clip depth 0..1).
//! Pure math — no GPU, no wgpu, no crate imports.

use std::f32::consts::PI;

const EPSILON: f32 = 1e-5;

// ── Perspective matrix (right-handed, depth 0..1) ────────────────────────────
//
// Column-major convention, applied as:  clip = M * view
//
//  [ f/aspect  0   0     0  ]
//  [   0       f   0     0  ]
//  [   0       0   A     B  ]
//  [   0       0  -1     0  ]
//
// where f = 1/tan(fov_y/2),  A = far/(near-far),  B = near*far/(near-far)
//
// For a point at view-space z = -d (d > 0):
//   clip_z  = A*(-d) + B
//   clip_w  = d
//   ndc_z   = clip_z / clip_w

fn perspective_rh(fov_y: f32, aspect: f32, near: f32, far: f32) -> [[f32; 4]; 4] {
    let f = 1.0 / (fov_y / 2.0).tan();
    let a = far / (near - far);
    let b = near * far / (near - far);
    [
        [f / aspect, 0.0, 0.0, 0.0],
        [0.0, f, 0.0, 0.0],
        [0.0, 0.0, a, b],
        [0.0, 0.0, -1.0, 0.0],
    ]
}

/// Project a view-space point through the perspective matrix.
/// Returns NDC (x, y, z) with z in [0, 1].
fn perspective_project(m: &[[f32; 4]; 4], vx: f32, vy: f32, vz: f32) -> (f32, f32, f32) {
    let cx = m[0][0] * vx + m[0][1] * vy + m[0][2] * vz + m[0][3];
    let cy = m[1][0] * vx + m[1][1] * vy + m[1][2] * vz + m[1][3];
    let cz = m[2][0] * vx + m[2][1] * vy + m[2][2] * vz + m[2][3];
    let cw = m[3][0] * vx + m[3][1] * vy + m[3][2] * vz + m[3][3];
    (cx / cw, cy / cw, cz / cw)
}

// ── Orthographic matrix (right-handed, depth 0..1) ───────────────────────────
//
//  [ 2/w  0    0         0           ]
//  [ 0    2/h  0         0           ]
//  [ 0    0    1/(n-f)   n/(n-f)     ]  ← maps (-near) → 0, (-far) → 1
//  [ 0    0    0         1           ]

fn ortho_rh(width: f32, height: f32, near: f32, far: f32) -> [[f32; 4]; 4] {
    let a = 1.0 / (near - far);
    let b = near * a; // = near / (near - far)
    [
        [2.0 / width, 0.0, 0.0, 0.0],
        [0.0, 2.0 / height, 0.0, 0.0],
        [0.0, 0.0, a, b],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

/// Project a view-space point through the orthographic matrix (no division needed).
fn ortho_project(m: &[[f32; 4]; 4], vx: f32, vy: f32, vz: f32) -> (f32, f32, f32) {
    let cx = m[0][0] * vx + m[0][3];
    let cy = m[1][1] * vy + m[1][3];
    let cz = m[2][2] * vz + m[2][3];
    (cx, cy, cz)
}

// ── Perspective matrix: diagonal entries ──────────────────────────────────────

#[test]
fn perspective_fov90_aspect1_scale_xy_is_1() {
    // fov_y = 90°, aspect = 1 → f = 1/tan(45°) = 1.0
    let m = perspective_rh(PI / 2.0, 1.0, 0.1, 100.0);
    assert!((m[0][0] - 1.0).abs() < EPSILON, "m[0][0]={}", m[0][0]);
    assert!((m[1][1] - 1.0).abs() < EPSILON, "m[1][1]={}", m[1][1]);
}

#[test]
fn perspective_fov90_aspect2_x_scale() {
    // aspect = 2 → [0][0] = f/aspect = 1/2 = 0.5
    let m = perspective_rh(PI / 2.0, 2.0, 0.1, 100.0);
    assert!((m[0][0] - 0.5).abs() < EPSILON, "m[0][0]={}", m[0][0]);
}

#[test]
fn perspective_fov60_aspect1_y_scale() {
    // fov_y = 60° → f = 1/tan(30°) = sqrt(3) ≈ 1.7321
    let m = perspective_rh(PI / 3.0, 1.0, 0.1, 100.0);
    let expected = 1.0 / (PI / 6.0).tan(); // cot(30°) = sqrt(3)
    assert!((m[1][1] - expected).abs() < EPSILON, "m[1][1]={}", m[1][1]);
}

#[test]
fn perspective_bottom_row() {
    // Row 3: [0, 0, -1, 0] — the key row for homogeneous division.
    let m = perspective_rh(PI / 2.0, 1.0, 0.1, 100.0);
    assert_eq!(m[3][0], 0.0);
    assert_eq!(m[3][1], 0.0);
    assert!((m[3][2] - (-1.0)).abs() < EPSILON);
    assert_eq!(m[3][3], 0.0);
}

// ── Perspective matrix: near-plane depth mapping ──────────────────────────────

#[test]
fn perspective_near_plane_maps_to_zero() {
    // A point exactly on the near plane (view z = -near) must map to NDC z = 0.
    let near = 0.1_f32;
    let far = 100.0_f32;
    let m = perspective_rh(PI / 2.0, 1.0, near, far);
    let (_, _, ndc_z) = perspective_project(&m, 0.0, 0.0, -near);
    assert!(ndc_z.abs() < EPSILON, "ndc_z={}", ndc_z);
}

#[test]
fn perspective_far_plane_maps_to_one() {
    // A point exactly on the far plane (view z = -far) must map to NDC z = 1.
    let near = 0.1_f32;
    let far = 100.0_f32;
    let m = perspective_rh(PI / 2.0, 1.0, near, far);
    let (_, _, ndc_z) = perspective_project(&m, 0.0, 0.0, -far);
    assert!((ndc_z - 1.0).abs() < EPSILON, "ndc_z={}", ndc_z);
}

#[test]
fn perspective_depth_is_monotonically_increasing() {
    // NDC z increases from 0 at the near plane to 1 at the far plane.
    let near = 0.1_f32;
    let far = 100.0_f32;
    let m = perspective_rh(PI / 2.0, 1.0, near, far);
    let zs = [0.1_f32, 1.0, 10.0, 50.0, 100.0];
    let ndcs: Vec<f32> = zs
        .iter()
        .map(|&d| perspective_project(&m, 0.0, 0.0, -d).2)
        .collect();
    for w in ndcs.windows(2) {
        assert!(w[1] > w[0], "depth not increasing: {} > {}", w[0], w[1]);
    }
}

// ── Perspective matrix: clip-space symmetry ───────────────────────────────────

#[test]
fn perspective_symmetric_x_maps_to_plus_minus_one() {
    // With fov=90° and aspect=1, x=±near at view z=-near should → NDC x=±1.
    let near = 1.0_f32;
    let far = 100.0_f32;
    let m = perspective_rh(PI / 2.0, 1.0, near, far);
    let (nx, _, _) = perspective_project(&m, near, 0.0, -near);
    assert!((nx - 1.0).abs() < EPSILON, "nx={}", nx);
    let (nx_neg, _, _) = perspective_project(&m, -near, 0.0, -near);
    assert!((nx_neg - (-1.0)).abs() < EPSILON, "nx_neg={}", nx_neg);
}

#[test]
fn perspective_center_ray_is_zero() {
    // A ray along the view axis (x=0, y=0) maps to NDC x=0, y=0.
    let m = perspective_rh(PI / 2.0, 1.0, 0.1, 100.0);
    let (nx, ny, _) = perspective_project(&m, 0.0, 0.0, -10.0);
    assert!(nx.abs() < EPSILON);
    assert!(ny.abs() < EPSILON);
}

// ── Perspective matrix: near-far parameter requirements ──────────────────────

#[test]
fn perspective_near_must_be_positive() {
    // near ≤ 0 causes degenerate (infinite or NaN) projection.
    let near: f32 = 0.001;
    assert!(near > 0.0);
}

#[test]
fn perspective_far_must_exceed_near() {
    let near: f32 = 0.1;
    let far: f32 = 100.0;
    assert!(far > near);
}

#[test]
fn perspective_a_coefficient_is_negative() {
    // A = far / (near - far); since far > near, (near - far) < 0, so A < 0.
    let near = 0.1_f32;
    let far = 100.0_f32;
    let a = far / (near - far);
    assert!(a < 0.0, "A={}", a);
}

#[test]
fn perspective_b_coefficient_is_negative() {
    // B = near * far / (near - far); same sign argument as A.
    let near = 0.1_f32;
    let far = 100.0_f32;
    let b = near * far / (near - far);
    assert!(b < 0.0, "B={}", b);
}

// ── Orthographic matrix: scale entries ───────────────────────────────────────

#[test]
fn ortho_x_scale_is_two_over_width() {
    let m = ortho_rh(10.0, 10.0, 0.0, 100.0);
    assert!((m[0][0] - 0.2).abs() < EPSILON, "m[0][0]={}", m[0][0]);
}

#[test]
fn ortho_y_scale_is_two_over_height() {
    let m = ortho_rh(10.0, 8.0, 0.0, 100.0);
    assert!((m[1][1] - 0.25).abs() < EPSILON, "m[1][1]={}", m[1][1]);
}

#[test]
fn ortho_preserves_far_row() {
    // Orthographic matrices have no perspective divide: last row = [0,0,0,1].
    let m = ortho_rh(10.0, 10.0, 0.0, 100.0);
    assert_eq!(m[3], [0.0, 0.0, 0.0, 1.0]);
}

// ── Orthographic matrix: near/far depth mapping ───────────────────────────────

#[test]
fn ortho_near_plane_maps_to_zero() {
    // view z = -near → ndc_z = 0
    let near = 0.0_f32;
    let far = 100.0_f32;
    let m = ortho_rh(10.0, 10.0, near, far);
    let (_, _, ndc_z) = ortho_project(&m, 0.0, 0.0, -near);
    assert!(ndc_z.abs() < EPSILON, "ndc_z={}", ndc_z);
}

#[test]
fn ortho_far_plane_maps_to_one() {
    // view z = -far → ndc_z = 1
    let near = 0.0_f32;
    let far = 100.0_f32;
    let m = ortho_rh(10.0, 10.0, near, far);
    let (_, _, ndc_z) = ortho_project(&m, 0.0, 0.0, -far);
    assert!((ndc_z - 1.0).abs() < EPSILON, "ndc_z={}", ndc_z);
}

#[test]
fn ortho_midpoint_maps_to_half() {
    // view z = -(near + far) / 2 → ndc_z = 0.5
    let near = 0.0_f32;
    let far = 100.0_f32;
    let m = ortho_rh(10.0, 10.0, near, far);
    let mid_z = -(near + far) / 2.0;
    let (_, _, ndc_z) = ortho_project(&m, 0.0, 0.0, mid_z);
    assert!((ndc_z - 0.5).abs() < EPSILON, "ndc_z={}", ndc_z);
}

#[test]
fn ortho_depth_is_linear() {
    // Unlike perspective, orthographic depth is a linear function of view z.
    let m = ortho_rh(10.0, 10.0, 0.0, 100.0);
    // Check linearity by verifying three evenly-spaced z values give evenly-spaced ndc_z.
    let z0 = ortho_project(&m, 0.0, 0.0, 0.0).2; // z_view = 0
    let z1 = ortho_project(&m, 0.0, 0.0, -50.0).2; // z_view = -50
    let z2 = ortho_project(&m, 0.0, 0.0, -100.0).2; // z_view = -100
    let d01 = z1 - z0;
    let d12 = z2 - z1;
    assert!((d01 - d12).abs() < EPSILON, "d01={} d12={}", d01, d12);
}

// ── Identity matrix ───────────────────────────────────────────────────────────

fn identity() -> [[f32; 4]; 4] {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

fn det4(m: &[[f32; 4]; 4]) -> f32 {
    // Expand along first row using cofactors.
    let minor = |r0: usize, r1: usize, r2: usize, c0: usize, c1: usize, c2: usize| -> f32 {
        m[r0][c0] * (m[r1][c1] * m[r2][c2] - m[r1][c2] * m[r2][c1])
            - m[r0][c1] * (m[r1][c0] * m[r2][c2] - m[r1][c2] * m[r2][c0])
            + m[r0][c2] * (m[r1][c0] * m[r2][c1] - m[r1][c1] * m[r2][c0])
    };
    m[0][0] * minor(1, 2, 3, 1, 2, 3) - m[0][1] * minor(1, 2, 3, 0, 2, 3)
        + m[0][2] * minor(1, 2, 3, 0, 1, 3)
        - m[0][3] * minor(1, 2, 3, 0, 1, 2)
}

#[test]
fn identity_matrix_determinant_is_one() {
    let d = det4(&identity());
    assert!((d - 1.0).abs() < EPSILON, "det={}", d);
}

#[test]
fn identity_diagonal_is_all_ones() {
    let m = identity();
    for i in 0..4 {
        assert_eq!(m[i][i], 1.0);
    }
}

#[test]
fn identity_off_diagonal_is_zero() {
    let m = identity();
    for i in 0..4 {
        for j in 0..4 {
            if i != j {
                assert_eq!(m[i][j], 0.0);
            }
        }
    }
}
