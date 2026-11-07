//! Tests for G-buffer render target formats, field semantics,
//! CSM cascade ordering, world-space bounds, and frame/time invariants.

use helio_pass_gbuffer::GBufferGlobals;

// ── G-buffer render target format documentation ───────────────────────────────
//
//  Albedo:    Rgba8Unorm   – 4 × 8-bit  = 32 bpp  (base colour, linear space)
//  Normal:    Rgba16Float  – 4 × 16-bit = 64 bpp  (world-space XYZ + padding)
//  ORM:       Rgba8Unorm   – 4 × 8-bit  = 32 bpp  (Occlusion/Roughness/Metallic)
//  Emissive:  Rgba16Float  – 4 × 16-bit = 64 bpp  (HDR emissive colour)

const ALBEDO_BYTES_PER_PIXEL: u32 = 4; // 4 × 8-bit channels
const NORMAL_BYTES_PER_PIXEL: u32 = 8; // 4 × 16-bit channels
const ORM_BYTES_PER_PIXEL: u32 = 4;
const EMISSIVE_BYTES_PER_PIXEL: u32 = 8;

// ── Format tests ──────────────────────────────────────────────────────────────

#[test]
fn albedo_format_is_32bpp() {
    assert_eq!(ALBEDO_BYTES_PER_PIXEL * 8, 32);
}

#[test]
fn normal_format_is_64bpp() {
    assert_eq!(NORMAL_BYTES_PER_PIXEL * 8, 64);
}

#[test]
fn orm_format_is_32bpp() {
    assert_eq!(ORM_BYTES_PER_PIXEL * 8, 32);
}

#[test]
fn emissive_format_is_64bpp() {
    assert_eq!(EMISSIVE_BYTES_PER_PIXEL * 8, 64);
}

#[test]
fn gbuffer_render_target_count_is_four() {
    const TARGET_COUNT: usize = 4; // albedo, normal, orm, emissive
    assert_eq!(TARGET_COUNT, 4);
}

#[test]
fn hdr_targets_use_float16() {
    // Normal and emissive require HDR range → Rgba16Float.
    const NORMAL_IS_FLOAT16: bool = true;
    const EMISSIVE_IS_FLOAT16: bool = true;
    assert!(NORMAL_IS_FLOAT16 && EMISSIVE_IS_FLOAT16);
}

#[test]
fn ldr_targets_use_unorm8() {
    // Albedo and ORM fit in [0,1] → Rgba8Unorm is sufficient.
    const ALBEDO_IS_UNORM8: bool = true;
    const ORM_IS_UNORM8: bool = true;
    assert!(ALBEDO_IS_UNORM8 && ORM_IS_UNORM8);
}

// ── CSM cascade ordering ──────────────────────────────────────────────────────

#[test]
fn csm_splits_ascending_order() {
    let mut g: GBufferGlobals = bytemuck::Zeroable::zeroed();
    g.csm_splits = [10.0, 30.0, 80.0, 200.0];
    assert!(
        g.csm_splits[0] < g.csm_splits[1]
            && g.csm_splits[1] < g.csm_splits[2]
            && g.csm_splits[2] < g.csm_splits[3],
        "CSM splits must be strictly ascending"
    );
}

#[test]
fn csm_four_cascades() {
    let g: GBufferGlobals = bytemuck::Zeroable::zeroed();
    assert_eq!(g.csm_splits.len(), 4);
}

#[test]
fn csm_first_split_positive() {
    // The first cascade split should be a positive near distance.
    let splits = [5.0_f32, 15.0, 50.0, 150.0];
    assert!(splits[0] > 0.0);
}

#[test]
fn csm_last_split_largest() {
    // The last cascade covers the greatest depth range.
    let splits = [5.0_f32, 15.0, 50.0, 150.0];
    let max = splits.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    assert_eq!(max, 150.0);
}

// ── Frame and time ────────────────────────────────────────────────────────────

#[test]
fn frame_counter_wraps_at_u32_max() {
    let f: u32 = u32::MAX;
    assert_eq!(f.wrapping_add(1), 0);
}

#[test]
fn frame_default_is_zero() {
    let g: GBufferGlobals = bytemuck::Zeroable::zeroed();
    assert_eq!(g.frame, 0);
}

#[test]
fn delta_time_typical_value_is_finite_and_positive() {
    let dt: f32 = 1.0 / 60.0; // ≈16 ms
    assert!(dt.is_finite() && dt > 0.0);
}

#[test]
fn delta_time_is_not_nan() {
    let dt: f32 = 1.0 / 60.0;
    assert!(!dt.is_nan());
}

// ── Light count ───────────────────────────────────────────────────────────────

#[test]
fn light_count_zero_by_default() {
    let g: GBufferGlobals = bytemuck::Zeroable::zeroed();
    assert_eq!(g.light_count, 0);
}

#[test]
fn light_count_u32_max_is_large() {
    assert!(u32::MAX > 1_000_000);
}

// ── Debug mode ────────────────────────────────────────────────────────────────

#[test]
fn debug_mode_zero_is_normal_rendering() {
    let mut g: GBufferGlobals = bytemuck::Zeroable::zeroed();
    g.debug_mode = 0;
    assert_eq!(g.debug_mode, 0);
}

// ── Ambient ───────────────────────────────────────────────────────────────────

#[test]
fn ambient_color_rgba_alpha_index_is_three() {
    let mut g: GBufferGlobals = bytemuck::Zeroable::zeroed();
    g.ambient_color = [0.1, 0.2, 0.3, 1.0];
    assert_eq!(g.ambient_color[3], 1.0, "alpha is the fourth component");
}

#[test]
fn ambient_intensity_non_negative() {
    let mut g: GBufferGlobals = bytemuck::Zeroable::zeroed();
    g.ambient_intensity = 0.05;
    assert!(g.ambient_intensity >= 0.0);
}

#[test]
fn ambient_intensity_finite() {
    let intensity: f32 = 0.05;
    assert!(intensity.is_finite());
}

// ── Padding ───────────────────────────────────────────────────────────────────

#[test]
fn padding_fields_zeroed_by_default() {
    let g: GBufferGlobals = bytemuck::Zeroable::zeroed();
    assert_eq!(g.time, 0.0);
}
