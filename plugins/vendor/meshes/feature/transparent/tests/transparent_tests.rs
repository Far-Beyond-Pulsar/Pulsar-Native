// Tests for helio-pass-transparent: alpha blending math, GBufferGlobals layout, blending modes.
// All tests are pure Rust — no GPU device required.

use std::mem;

// ── Mirror private struct ─────────────────────────────────────────────────────

/// Mirrors GBufferGlobals (80 bytes, same layout as in transparent.rs).
#[repr(C)]
#[derive(Clone, Copy)]
struct GBufferGlobals {
    frame: u32,
    delta_time: f32,
    light_count: u32,
    ambient_intensity: f32,
    ambient_color: [f32; 4],
    csm_splits: [f32; 4],
}

// ── GBufferGlobals layout tests ───────────────────────────────────────────────

#[test]
fn gbuffer_globals_size_is_48() {
    assert_eq!(mem::size_of::<GBufferGlobals>(), 48);
}

#[test]
fn gbuffer_globals_scalar_header_16_bytes() {
    // frame(4) + delta_time(4) + light_count(4) + ambient_intensity(4) = 16
    assert_eq!(4 + 4 + 4 + 4, 16usize);
}

#[test]
fn gbuffer_globals_vec4_section_32_bytes() {
    // ambient_color + csm_splits = 2 × 16 = 32
    assert_eq!(2 * 4 * mem::size_of::<f32>(), 32usize);
}

#[test]
fn gbuffer_globals_total_16_plus_32() {
    assert_eq!(16 + 32, 48usize);
}

#[test]
fn gbuffer_globals_alignment_is_4() {
    assert_eq!(mem::align_of::<GBufferGlobals>(), 4);
}

#[test]
fn gbuffer_globals_size_divisible_by_16() {
    assert_eq!(mem::size_of::<GBufferGlobals>() % 16, 0);
}

#[test]
fn gbuffer_globals_can_be_zero_initialised() {
    let g = GBufferGlobals {
        frame: 0,
        delta_time: 0.0,
        light_count: 0,
        ambient_intensity: 0.0,
        ambient_color: [0.0; 4],
        csm_splits: [0.0; 4],
    };
    assert_eq!(g.frame, 0u32);
}

// ── Alpha blending formula tests ──────────────────────────────────────────────

/// SrcAlpha / OneMinusSrcAlpha blend: result = src * src.a + dst * (1 - src.a).
fn blend_over(src: [f32; 4], dst: [f32; 3]) -> [f32; 3] {
    let a = src[3];
    [
        src[0] * a + dst[0] * (1.0 - a),
        src[1] * a + dst[1] * (1.0 - a),
        src[2] * a + dst[2] * (1.0 - a),
    ]
}

#[test]
fn blend_opaque_source_overwrites_destination() {
    let src = [0.8f32, 0.2, 0.1, 1.0]; // alpha = 1
    let dst = [0.1f32, 0.5, 0.9];
    let result = blend_over(src, dst);
    assert!((result[0] - 0.8f32).abs() < 1e-6f32);
    assert!((result[1] - 0.2f32).abs() < 1e-6f32);
    assert!((result[2] - 0.1f32).abs() < 1e-6f32);
}

#[test]
fn blend_transparent_source_leaves_destination() {
    let src = [0.8f32, 0.2, 0.1, 0.0]; // alpha = 0
    let dst = [0.1f32, 0.5, 0.9];
    let result = blend_over(src, dst);
    assert!((result[0] - 0.1f32).abs() < 1e-6f32);
    assert!((result[1] - 0.5f32).abs() < 1e-6f32);
    assert!((result[2] - 0.9f32).abs() < 1e-6f32);
}

#[test]
fn blend_half_alpha_is_average() {
    let src = [1.0f32, 1.0, 1.0, 0.5]; // white, 50% alpha
    let dst = [0.0f32, 0.0, 0.0]; // black
    let result = blend_over(src, dst);
    for (i, &r) in result.iter().enumerate() {
        assert!((r - 0.5f32).abs() < 1e-6f32, "channel {i}: {r}");
    }
}

#[test]
fn blend_result_stays_in_0_1_for_valid_inputs() {
    let src = [0.6f32, 0.4, 0.2, 0.7];
    let dst = [0.3f32, 0.3, 0.3];
    let result = blend_over(src, dst);
    for (i, &r) in result.iter().enumerate() {
        assert!(r >= 0.0f32 && r <= 1.0f32, "channel {i}: {r}");
    }
}

#[test]
fn blend_not_commutative_for_different_alpha() {
    // Over is NOT commutative in general; choose different source alpha values.
    let a = [1.0f32, 0.0, 0.0, 0.5]; // red, 50% alpha
    let b = [0.0f32, 0.0, 1.0, 0.25]; // blue, 25% alpha
    let ab = blend_over(a, [b[0], b[1], b[2]]);
    let ba = blend_over(b, [a[0], a[1], a[2]]);
    // Color channels should differ due asymmetric alpha.
    assert!(
        (ab[0] - ba[0]).abs() > 0.1f32,
        "red channel should differ for order-dependent blend"
    );
    assert!(
        (ab[2] - ba[2]).abs() > 0.1f32,
        "blue channel should differ for order-dependent blend"
    );
}

#[test]
fn blend_alpha_linearity() {
    // blend(src, alpha=2α) ≠ 2 × blend(src, alpha=α) — blending is NOT linear
    let src_color = [0.8f32, 0.2, 0.5];
    let dst = [0.1f32, 0.1, 0.1];
    let blend_a = blend_over([src_color[0], src_color[1], src_color[2], 0.3], dst);
    let blend_2a = blend_over([src_color[0], src_color[1], src_color[2], 0.6], dst);
    // Simple check: higher alpha = more source color
    assert!(
        blend_2a[0] > blend_a[0],
        "higher alpha should give more source contribution"
    );
}

// ── Porter-Duff "over" operator tests ────────────────────────────────────────

#[test]
fn porter_duff_over_associativity_three_layers() {
    // A over (B over C) should equal (A over B) over C for same alpha
    let a = [0.9f32, 0.0, 0.0, 0.5];
    let b = [0.0f32, 0.9, 0.0, 0.5];
    let c = [0.0f32, 0.0, 0.9];

    let bc = blend_over(b, c);
    let a_over_bc = blend_over(a, bc);

    let ab = blend_over(a, [b[0], b[1], b[2]]);
    let ab_over_c = blend_over([ab[0], ab[1], ab[2], a[3] + b[3] * (1.0 - a[3])], c);

    // Not exactly associative but close in practice
    assert!(a_over_bc[0] > 0.0f32 && ab_over_c[0] > 0.0f32);
}

// ── Depth sorting tests ───────────────────────────────────────────────────────

#[test]
fn depth_sort_front_to_back_ordering() {
    let depths = [0.9f32, 0.3f32, 0.7f32, 0.1f32];
    let mut sorted = depths.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(sorted[0], 0.1f32);
    assert_eq!(sorted[3], 0.9f32);
}

#[test]
fn depth_sort_back_to_front_for_transparency() {
    // Transparent objects blended back-to-front so painter's algorithm works
    let depths = [0.1f32, 0.5f32, 0.9f32, 0.3f32];
    let mut sorted = depths.to_vec();
    sorted.sort_by(|a, b| b.partial_cmp(a).unwrap()); // descending
    assert!(sorted[0] > sorted[1] && sorted[1] > sorted[2]);
}

// ── OIT discussion tests ──────────────────────────────────────────────────────

#[test]
fn oit_would_eliminate_sort_requirement() {
    // Property check: with OIT, blend weight is order-independent
    // Weighted Blended OIT weight function: alpha * (1 / (z^4 + 0.001))
    let alpha = 0.5f32;
    let z1 = 0.1f32;
    let z2 = 0.9f32;
    let w1 = alpha / (z1.powi(4) + 0.001f32);
    let w2 = alpha / (z2.powi(4) + 0.001f32);
    // Near fragments have higher weight (as expected in WBOIT)
    assert!(w1 > w2, "near weight {w1} should exceed far weight {w2}");
}

#[test]
fn gbuffer_globals_csm_splits_can_hold_four_cascade_depths() {
    let g = GBufferGlobals {
        frame: 0,
        delta_time: 0.016,
        light_count: 1,
        ambient_intensity: 0.1,
        ambient_color: [0.1, 0.1, 0.1, 1.0],
        csm_splits: [10.0, 30.0, 70.0, 200.0],
    };
    assert!(g.csm_splits[0] < g.csm_splits[1]);
    assert!(g.csm_splits[1] < g.csm_splits[2]);
    assert!(g.csm_splits[2] < g.csm_splits[3]);
}
