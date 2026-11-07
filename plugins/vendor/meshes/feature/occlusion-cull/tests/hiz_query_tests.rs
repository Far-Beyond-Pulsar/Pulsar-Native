// Hi-Z mip-level selection math tests for helio-pass-occlusion-cull.
// All helpers defined locally — no crate imports required.

/// Number of mip levels needed for a texture of size `w × h`.
/// Equivalent to: floor(log2(max(w, h))) + 1.
fn mip_levels(w: u32, h: u32) -> u32 {
    let m = w.max(h);
    (u32::BITS - m.leading_zeros()).max(1)
}

/// Mip level that fully covers an object footprint of `footprint_w × footprint_h`
/// texels on the Hi-Z pyramid.
fn select_query_mip(footprint_w: u32, footprint_h: u32) -> u32 {
    (footprint_w.max(footprint_h) as f32).log2().ceil() as u32
}

// ── mip_levels — powers of two ───────────────────────────────────────────────

#[test]
fn mip_levels_1x1() {
    // 1 has 1 significant bit  →  1 mip level
    assert_eq!(mip_levels(1, 1), 1);
}

#[test]
fn mip_levels_2x2() {
    assert_eq!(mip_levels(2, 2), 2);
}

#[test]
fn mip_levels_4x4() {
    assert_eq!(mip_levels(4, 4), 3);
}

#[test]
fn mip_levels_8x8() {
    assert_eq!(mip_levels(8, 8), 4);
}

#[test]
fn mip_levels_16x16() {
    assert_eq!(mip_levels(16, 16), 5);
}

#[test]
fn mip_levels_32x32() {
    assert_eq!(mip_levels(32, 32), 6);
}

#[test]
fn mip_levels_64x64() {
    assert_eq!(mip_levels(64, 64), 7);
}

#[test]
fn mip_levels_128x128() {
    assert_eq!(mip_levels(128, 128), 8);
}

#[test]
fn mip_levels_256x256() {
    assert_eq!(mip_levels(256, 256), 9);
}

#[test]
fn mip_levels_512x512() {
    assert_eq!(mip_levels(512, 512), 10);
}

#[test]
fn mip_levels_1024x1024() {
    // 1024 = 2^10  →  11 mip levels (0 through 10)
    assert_eq!(mip_levels(1024, 1024), 11);
}

#[test]
fn mip_levels_2048x2048() {
    assert_eq!(mip_levels(2048, 2048), 12);
}

// ── mip_levels — non-square and non-power-of-two ─────────────────────────────

#[test]
fn mip_levels_1920x1080() {
    // max = 1920; 1920 = 0b11110000000 = 11 bits  →  11 mip levels
    assert_eq!(mip_levels(1920, 1080), 11);
}

#[test]
fn mip_levels_1080x1920_same_as_1920x1080() {
    // Symmetric: max is still 1920
    assert_eq!(mip_levels(1080, 1920), mip_levels(1920, 1080));
}

#[test]
fn mip_levels_512x1024() {
    // max = 1024  →  11 levels
    assert_eq!(mip_levels(512, 1024), 11);
}

#[test]
fn mip_levels_1024x512() {
    assert_eq!(mip_levels(1024, 512), 11);
}

#[test]
fn mip_levels_3x3() {
    // 3 = 0b11  → 2 bits  →  2 mip levels
    assert_eq!(mip_levels(3, 3), 2);
}

#[test]
fn mip_levels_5x5() {
    // 5 = 0b101  → 3 bits  →  3 mip levels
    assert_eq!(mip_levels(5, 5), 3);
}

#[test]
fn mip_levels_asymmetric_wide() {
    // max(800, 100) = 800; 800 = 0b1100100000 = 10 bits  →  10 mip levels
    assert_eq!(mip_levels(800, 100), 10);
}

#[test]
fn mip_levels_asymmetric_tall() {
    // same as wide by symmetry
    assert_eq!(mip_levels(100, 800), mip_levels(800, 100));
}

// ── select_query_mip — powers of two ─────────────────────────────────────────

#[test]
fn select_query_mip_1x1() {
    // log2(1) = 0.0  →  ceil = 0
    assert_eq!(select_query_mip(1, 1), 0);
}

#[test]
fn select_query_mip_2x2() {
    // log2(2) = 1.0  →  ceil = 1
    assert_eq!(select_query_mip(2, 2), 1);
}

#[test]
fn select_query_mip_4x4() {
    assert_eq!(select_query_mip(4, 4), 2);
}

#[test]
fn select_query_mip_8x8() {
    assert_eq!(select_query_mip(8, 8), 3);
}

#[test]
fn select_query_mip_16x16() {
    assert_eq!(select_query_mip(16, 16), 4);
}

#[test]
fn select_query_mip_32x32() {
    assert_eq!(select_query_mip(32, 32), 5);
}

#[test]
fn select_query_mip_64x64() {
    assert_eq!(select_query_mip(64, 64), 6);
}

// ── select_query_mip — non-power-of-two ──────────────────────────────────────

#[test]
fn select_query_mip_100x100() {
    // log2(100) ≈ 6.644  →  ceil = 7
    assert_eq!(select_query_mip(100, 100), 7);
}

#[test]
fn select_query_mip_100x50() {
    // max = 100  →  same result as 100×100
    assert_eq!(select_query_mip(100, 50), 7);
}

#[test]
fn select_query_mip_50x100() {
    // max = 100  →  symmetric
    assert_eq!(select_query_mip(50, 100), 7);
}

#[test]
fn select_query_mip_3x3() {
    // log2(3) ≈ 1.585  →  ceil = 2
    assert_eq!(select_query_mip(3, 3), 2);
}

#[test]
fn select_query_mip_5x3() {
    // max = 5, log2(5) ≈ 2.322  →  ceil = 3
    assert_eq!(select_query_mip(5, 3), 3);
}

#[test]
fn select_query_mip_1000x1() {
    // log2(1000) ≈ 9.965  →  ceil = 10
    assert_eq!(select_query_mip(1000, 1), 10);
}

// ── Relationship between the two functions ────────────────────────────────────

#[test]
fn select_query_mip_never_exceeds_mip_count_minus_one() {
    // For a 1024×1024 texture (11 mip levels, indices 0..=10),
    // a footprint of at most 1024 texels maps to mip 10.
    assert_eq!(select_query_mip(1024, 1024), 10);
    assert_eq!(mip_levels(1024, 1024), 11);
    assert!(select_query_mip(1024, 1024) < mip_levels(1024, 1024));
}

#[test]
fn higher_footprint_selects_higher_mip() {
    let mip_small = select_query_mip(4, 4);
    let mip_large = select_query_mip(64, 64);
    assert!(mip_large > mip_small);
}

#[test]
fn mip_levels_at_least_one_for_any_size() {
    assert!(mip_levels(1, 1) >= 1);
    assert!(mip_levels(1920, 1) >= 1);
}
