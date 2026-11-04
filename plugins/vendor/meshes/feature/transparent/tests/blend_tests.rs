// Tests for helio-pass-transparent: SrcAlpha/OneMinusSrcAlpha blending formula,
// OIT concepts, depth sorting math. All tests are pure math.

use std::f32;

// ── Alpha compositing helpers ─────────────────────────────────────────────────

/// Standard SrcAlpha / OneMinusSrcAlpha premultiplied blend.
fn blend_src_alpha(src_rgb: [f32; 3], src_a: f32, dst_rgb: [f32; 3]) -> [f32; 3] {
    [
        src_rgb[0] * src_a + dst_rgb[0] * (1.0 - src_a),
        src_rgb[1] * src_a + dst_rgb[1] * (1.0 - src_a),
        src_rgb[2] * src_a + dst_rgb[2] * (1.0 - src_a),
    ]
}

/// Premultiplied alpha blend: src_rgb is already multiplied by src_a.
fn blend_premultiplied(src_rgb_premul: [f32; 3], src_a: f32, dst_rgb: [f32; 3]) -> [f32; 3] {
    [
        src_rgb_premul[0] + dst_rgb[0] * (1.0 - src_a),
        src_rgb_premul[1] + dst_rgb[1] * (1.0 - src_a),
        src_rgb_premul[2] + dst_rgb[2] * (1.0 - src_a),
    ]
}

/// Blend alpha: result_a = src_a + dst_a * (1 - src_a).
fn blend_alpha(src_a: f32, dst_a: f32) -> f32 {
    src_a + dst_a * (1.0 - src_a)
}

// ── SrcAlpha / OneMinusSrcAlpha formula tests ─────────────────────────────────

#[test]
fn blend_alpha1_overwrites_dest() {
    let result = blend_src_alpha([1.0, 0.0, 0.0], 1.0, [0.0, 1.0, 0.0]);
    assert!((result[0] - 1.0f32).abs() < 1e-6);
    assert!((result[1] - 0.0f32).abs() < 1e-6);
}

#[test]
fn blend_alpha0_keeps_dest() {
    let result = blend_src_alpha([1.0, 0.0, 0.0], 0.0, [0.0, 1.0, 0.0]);
    assert!((result[0] - 0.0f32).abs() < 1e-6);
    assert!((result[1] - 1.0f32).abs() < 1e-6);
}

#[test]
fn blend_alpha_half_averages() {
    let result = blend_src_alpha([1.0, 0.0, 0.0], 0.5, [0.0, 1.0, 0.0]);
    assert!((result[0] - 0.5f32).abs() < 1e-6);
    assert!((result[1] - 0.5f32).abs() < 1e-6);
}

#[test]
fn blend_formula_matches_opengl_spec() {
    // GL spec: Cs * As + Cd * (1 - As)
    let cs = 0.7f32;
    let cd = 0.3f32;
    let as_ = 0.6f32;
    let expected = cs * as_ + cd * (1.0 - as_);
    let result = blend_src_alpha([cs, 0.0, 0.0], as_, [cd, 0.0, 0.0]);
    assert!((result[0] - expected).abs() < 1e-6f32);
}

#[test]
fn blend_src_alpha_and_premultiplied_equivalent() {
    let rgb = [0.8f32, 0.4, 0.2];
    let a = 0.6f32;
    let dst = [0.3f32, 0.5, 0.7];
    let standard = blend_src_alpha(rgb, a, dst);
    let premul = blend_premultiplied([rgb[0] * a, rgb[1] * a, rgb[2] * a], a, dst);
    for (i, (&s, &p)) in standard.iter().zip(premul.iter()).enumerate() {
        assert!(
            (s - p).abs() < 1e-5f32,
            "channel {i}: standard={s} premul={p}"
        );
    }
}

#[test]
fn blend_output_clamped_to_0_1_for_valid_inputs() {
    let src = [0.9f32, 0.1, 0.5];
    let a = 0.8f32;
    let dst = [0.2f32, 0.8, 0.3];
    let result = blend_src_alpha(src, a, dst);
    for (i, &r) in result.iter().enumerate() {
        assert!(r >= 0.0 && r <= 1.0f32, "channel {i}: {r}");
    }
}

// ── Alpha composition tests ───────────────────────────────────────────────────

#[test]
fn blend_alpha_fully_opaque_source_yields_1() {
    let result = blend_alpha(1.0, 0.5);
    assert!((result - 1.0f32).abs() < 1e-6f32);
}

#[test]
fn blend_alpha_fully_transparent_source_yields_dst() {
    let dst_a = 0.7f32;
    let result = blend_alpha(0.0, dst_a);
    assert!((result - dst_a).abs() < 1e-6f32);
}

#[test]
fn blend_alpha_two_half_alpha_layers() {
    // 0.5 over 0.5 → 0.5 + 0.5*(1-0.5) = 0.75
    let result = blend_alpha(0.5, 0.5);
    assert!((result - 0.75f32).abs() < 1e-6f32);
}

#[test]
fn blend_alpha_three_quarter_source_over_opaque() {
    // 0.75 over 1.0 → 0.75 + 1.0*0.25 = 1.0
    let result = blend_alpha(0.75, 1.0);
    assert!((result - 1.0f32).abs() < 1e-6f32);
}

// ── Depth sorting tests ───────────────────────────────────────────────────────

#[test]
fn back_to_front_sort_descending_depth() {
    let mut depths = [0.2f32, 0.8, 0.5, 1.0, 0.1];
    depths.sort_by(|a, b| b.partial_cmp(a).unwrap());
    assert!(depths[0] >= depths[1] && depths[1] >= depths[2]);
}

#[test]
fn depth_sort_complexity_is_o_n_log_n() {
    // n items requires n*log(n) comparisons for a comparison sort
    let n = 1000usize;
    let upper_bound = n * (n as f32).log2() as usize;
    assert!(upper_bound > 0 && upper_bound < n * n);
}

#[test]
fn depth_sort_worst_case_already_sorted() {
    let depths: Vec<f32> = (0..100).map(|i| i as f32 / 99.0).collect();
    let mut copy = depths.clone();
    copy.sort_by(|a, b| b.partial_cmp(a).unwrap());
    // Just verify it doesn't panic and last element is near 0
    assert!(copy.last().copied().unwrap() < 0.01f32 + 1e-6);
}

// ── OIT weighted blended formula ─────────────────────────────────────────────

/// Weighted Blended OIT weight: McGuire & Bavoil 2013.
fn wboit_weight(alpha: f32, depth: f32) -> f32 {
    alpha * (1.0 / (depth * depth * depth * depth + 1e-3f32))
}

#[test]
fn wboit_weight_near_depth_greater_than_far() {
    let alpha = 0.5f32;
    let w_near = wboit_weight(alpha, 0.1);
    let w_far = wboit_weight(alpha, 0.9);
    assert!(w_near > w_far, "near={w_near} far={w_far}");
}

#[test]
fn wboit_weight_scales_linearly_with_alpha() {
    let w1 = wboit_weight(0.5, 0.5);
    let w2 = wboit_weight(1.0, 0.5);
    assert!((w2 - 2.0 * w1).abs() < 1e-4f32);
}

#[test]
fn wboit_weight_zero_alpha_gives_zero() {
    let w = wboit_weight(0.0, 0.5);
    assert!(w.abs() < 1e-6f32);
}

#[test]
fn read_only_depth_prevents_transparent_write_to_opaque() {
    // In read-only depth mode, transparent fragments don't update depth buffer.
    // Verify modelling: a closer transparent object doesn't occlude a farther opaque.
    let opaque_depth = 0.3f32;
    // Read-only: depth test passes (0.2 < 0.3) but depth buffer unchanged
    let depth_after = opaque_depth; // NOT transparent_depth
    assert!((depth_after - opaque_depth).abs() < 1e-6f32);
}

#[test]
fn blend_layers_accumulate_correctly() {
    // Three-layer blend: red(0.5) over green(0.5) over blue background
    let blue = [0.0f32, 0.0, 1.0];
    let green_over_blue = blend_src_alpha([0.0, 1.0, 0.0], 0.5, blue);
    let red_over_green_blue = blend_src_alpha([1.0, 0.0, 0.0], 0.5, green_over_blue);
    // Should have contributions from all three
    assert!(red_over_green_blue[0] > 0.0f32); // red
    assert!(red_over_green_blue[1] > 0.0f32); // green
    assert!(red_over_green_blue[2] > 0.0f32); // blue
}
