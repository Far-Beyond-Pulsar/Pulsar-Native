//! Tests for `GBufferGlobals` memory layout, field offsets, alignment,
//! and bytemuck Pod/Zeroable trait implementations.

use helio_pass_gbuffer::GBufferGlobals;

// ── Size and alignment ────────────────────────────────────────────────────────

#[test]
fn gbuffer_globals_size_is_64() {
    // 4+4+4+4 = 16, four [f32;4] = 64, four u32 padding = 16 → 96 bytes
    assert_eq!(std::mem::size_of::<GBufferGlobals>(), 64);
}

#[test]
fn gbuffer_globals_alignment_is_4() {
    assert_eq!(std::mem::align_of::<GBufferGlobals>(), 4);
}

// ── Field offsets ─────────────────────────────────────────────────────────────

#[test]
fn frame_field_offset_is_0() {
    assert_eq!(std::mem::offset_of!(GBufferGlobals, frame), 0);
}

#[test]
fn delta_time_field_offset_is_4() {
    assert_eq!(std::mem::offset_of!(GBufferGlobals, delta_time), 4);
}

#[test]
fn light_count_field_offset_is_8() {
    assert_eq!(std::mem::offset_of!(GBufferGlobals, light_count), 8);
}

#[test]
fn ambient_intensity_field_offset_is_12() {
    assert_eq!(std::mem::offset_of!(GBufferGlobals, ambient_intensity), 12);
}

#[test]
fn ambient_color_field_offset_is_16() {
    assert_eq!(std::mem::offset_of!(GBufferGlobals, ambient_color), 16);
}

#[test]
fn csm_splits_field_offset_is_32() {
    assert_eq!(std::mem::offset_of!(GBufferGlobals, csm_splits), 32);
}

#[test]
fn debug_mode_field_offset_is_48() {
    assert_eq!(std::mem::offset_of!(GBufferGlobals, debug_mode), 48);
}

// ── bytemuck: Zeroable ────────────────────────────────────────────────────────

#[test]
fn zeroable_produces_zeroed_scalar_fields() {
    let g: GBufferGlobals = bytemuck::Zeroable::zeroed();
    assert_eq!(g.frame, 0);
    assert_eq!(g.delta_time, 0.0);
    assert_eq!(g.light_count, 0);
    assert_eq!(g.ambient_intensity, 0.0);
    assert_eq!(g.debug_mode, 0);
}

#[test]
fn zeroable_produces_zeroed_array_fields() {
    let g: GBufferGlobals = bytemuck::Zeroable::zeroed();
    assert_eq!(g.ambient_color, [0.0; 4]);
    assert_eq!(g.csm_splits, [0.0; 4]);
}

#[test]
fn zeroed_bytes_are_all_zero() {
    let g: GBufferGlobals = bytemuck::Zeroable::zeroed();
    let bytes: &[u8] = bytemuck::bytes_of(&g);
    assert!(
        bytes.iter().all(|&b| b == 0),
        "all bytes must be zero after Zeroable::zeroed()"
    );
}

// ── bytemuck: Pod roundtrip ───────────────────────────────────────────────────

#[test]
fn pod_bytes_of_length_is_64() {
    let g: GBufferGlobals = bytemuck::Zeroable::zeroed();
    assert_eq!(bytemuck::bytes_of(&g).len(), 64);
}

#[test]
fn pod_cast_roundtrip_preserves_frame() {
    let mut g: GBufferGlobals = bytemuck::Zeroable::zeroed();
    g.frame = 99;
    let bytes: &[u8] = bytemuck::bytes_of(&g);
    let recovered: &GBufferGlobals = bytemuck::from_bytes(bytes);
    assert_eq!(recovered.frame, 99);
}

#[test]
fn pod_cast_roundtrip_preserves_light_count() {
    let mut g: GBufferGlobals = bytemuck::Zeroable::zeroed();
    g.light_count = 512;
    let bytes: &[u8] = bytemuck::bytes_of(&g);
    let recovered: &GBufferGlobals = bytemuck::from_bytes(bytes);
    assert_eq!(recovered.light_count, 512);
}

#[test]
fn pod_cast_roundtrip_preserves_csm_splits() {
    let mut g: GBufferGlobals = bytemuck::Zeroable::zeroed();
    g.csm_splits = [5.0, 20.0, 70.0, 180.0];
    let bytes: &[u8] = bytemuck::bytes_of(&g);
    let recovered: &GBufferGlobals = bytemuck::from_bytes(bytes);
    assert_eq!(recovered.csm_splits, [5.0, 20.0, 70.0, 180.0]);
}

// ── repr(C) contiguity ────────────────────────────────────────────────────────

#[test]
fn repr_c_frame_and_delta_time_are_contiguous() {
    let frame_end = std::mem::offset_of!(GBufferGlobals, frame) + std::mem::size_of::<u32>();
    let dt_start = std::mem::offset_of!(GBufferGlobals, delta_time);
    assert_eq!(frame_end, dt_start);
}

#[test]
fn repr_c_light_count_and_ambient_intensity_are_contiguous() {
    let lc_end = std::mem::offset_of!(GBufferGlobals, light_count) + std::mem::size_of::<u32>();
    let ai_start = std::mem::offset_of!(GBufferGlobals, ambient_intensity);
    assert_eq!(lc_end, ai_start);
}

// ── Field widths ──────────────────────────────────────────────────────────────

#[test]
fn ambient_color_has_four_components() {
    let g: GBufferGlobals = bytemuck::Zeroable::zeroed();
    assert_eq!(g.ambient_color.len(), 4);
}

#[test]
fn csm_splits_has_four_cascades() {
    let g: GBufferGlobals = bytemuck::Zeroable::zeroed();
    assert_eq!(g.csm_splits.len(), 4);
}

// ── Copy / Clone ──────────────────────────────────────────────────────────────

#[test]
fn struct_is_copy() {
    let g: GBufferGlobals = bytemuck::Zeroable::zeroed();
    let g2 = g; // Copy
    assert_eq!(g.frame, g2.frame);
}
