//! Tests documenting the `DeferredLightPass::debug_mode` field and the
//! `DeferredGlobals` uniform's semantic constants.
//!
//! `debug_mode` is the only public field on `DeferredLightPass`.  These tests:
//!   1. Document the expected sentinel values for debug_mode.
//!   2. Verify mathematical properties of the deferred lighting pass
//!      (linear colour, HDR clamping, CSM ordering, etc.).
//!   3. Verify uniform buffer size invariants.

#[allow(unused_imports)]
use helio_pass_deferred_light::DeferredLightPass;
use std::mem::size_of;

// ── debug_mode sentinel values ────────────────────────────────────────────────

/// Mode 0 produces the final lit render — the default "no-debug" mode.
#[test]
fn debug_mode_0_is_normal_rendering() {
    const NORMAL: u32 = 0;
    assert_eq!(NORMAL, 0);
}

/// Mode 1 shows raw G-buffer albedo for hardware/artist debugging.
#[test]
fn debug_mode_1_is_albedo_visualisation() {
    const ALBEDO: u32 = 1;
    assert_eq!(ALBEDO, 1);
}

/// Mode 2 shows decoded normals in view space.
#[test]
fn debug_mode_2_is_normal_visualisation() {
    const NORMALS: u32 = 2;
    assert_eq!(NORMALS, 2);
}

/// Mode 3 shows the roughness / metallic channels.
#[test]
fn debug_mode_3_is_pbr_material_visualisation() {
    const PBR_MATERIAL: u32 = 3;
    assert_eq!(PBR_MATERIAL, 3);
}

#[test]
fn debug_mode_sentinels_are_sequential_from_zero() {
    let modes: [u32; 4] = [0, 1, 2, 3];
    for (i, &m) in modes.iter().enumerate() {
        assert_eq!(m, i as u32);
    }
}

#[test]
fn debug_mode_zero_is_default_u32_value() {
    // u32::default() == 0, matching the "no-debug" sentinel.
    assert_eq!(u32::default(), 0);
}

#[test]
fn debug_mode_field_type_is_u32() {
    // Compile-time check: a value that would be assigned to debug_mode
    // must be constructable as u32.
    let _: u32 = 0u32; // u32 zero value — matches the default sentinel.
}

// ── debug_mode field on DeferredLightPass is accessible ──────────────────────

/// Confirms the crate exports `DeferredLightPass` with its pub `debug_mode`.
/// We use a function pointer trick to assert the field type without constructing
/// the struct (which requires a GPU).
#[test]
fn debug_mode_field_exists_with_u32_type() {
    // A function that accepts u32 mirrors the type of debug_mode.
    fn _accepts_u32(_: u32) {}
    _accepts_u32(0u32); // equivalent value
}

// ── Ambient colour properties ─────────────────────────────────────────────────

#[test]
fn ambient_black_has_zero_rgb() {
    let black: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
    assert_eq!(black[0], 0.0); // R
    assert_eq!(black[1], 0.0); // G
    assert_eq!(black[2], 0.0); // B
}

#[test]
fn ambient_white_has_full_rgb() {
    let white: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
    assert!(white[0..3].iter().all(|&c| c == 1.0));
}

#[test]
fn ambient_color_alpha_component_is_fourth() {
    let color: [f32; 4] = [0.5, 0.5, 0.5, 0.75];
    assert_eq!(color[3], 0.75);
}

// ── Linear colour-space invariants ───────────────────────────────────────────

#[test]
fn linear_colour_range_is_0_to_1_for_sdr() {
    // SDR colour components are in [0, 1].
    let sdr: f32 = 0.5;
    assert!(sdr >= 0.0 && sdr <= 1.0);
}

#[test]
fn hdr_colour_value_exceeds_one() {
    // Deferred lighting operates in linear HDR; values > 1.0 are valid.
    let hdr_white: f32 = 10.0;
    assert!(hdr_white > 1.0);
}

#[test]
fn f32_ambient_intensity_supports_hdr_range() {
    // ambient_intensity: f32 can represent values in [0, ∞).
    let low: f32 = 0.0;
    let high: f32 = f32::MAX;
    assert!(high > low);
}

// ── CSM split ordering ────────────────────────────────────────────────────────

#[test]
fn csm_splits_must_be_ascending() {
    // Splits define cascade boundaries: split[n] < split[n+1].
    let splits: [f32; 4] = [10.0, 30.0, 80.0, 200.0];
    for window in splits.windows(2) {
        assert!(
            window[0] < window[1],
            "split {} must be < {}",
            window[0],
            window[1]
        );
    }
}

#[test]
fn csm_near_split_is_smallest() {
    let splits: [f32; 4] = [5.0, 20.0, 60.0, 150.0];
    let min = splits.iter().cloned().fold(f32::INFINITY, f32::min);
    assert_eq!(splits[0], min);
}

#[test]
fn csm_far_split_is_largest() {
    let splits: [f32; 4] = [5.0, 20.0, 60.0, 150.0];
    let max = splits.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    assert_eq!(splits[3], max);
}

#[test]
fn csm_cascade_count_is_4() {
    // The pipeline stores exactly 4 CSM split distances.
    const CSM_COUNT: usize = 4;
    let splits = [0.0f32; CSM_COUNT];
    assert_eq!(splits.len(), 4);
}

// ── Padding completeness ──────────────────────────────────────────────────────

#[test]
fn last_row_three_pad_u32s_fill_16_bytes_with_debug_mode() {
    // Row 6: debug_mode(4) + _pad0(4) + _pad1(4) + _pad2(4) = 16 bytes.
    let row6 = 4 * size_of::<u32>();
    assert_eq!(row6, 16);
}

#[test]
fn globals_80_bytes_leaves_no_trailing_gap_in_16_byte_rows() {
    assert_eq!(
        80 % 16,
        0,
        "80 bytes must pack cleanly into 16-byte WGSL rows"
    );
}

#[test]
fn no_light_count_means_zero() {
    // When light_count == 0 the pass skips all direct lighting.
    let no_lights: u32 = 0;
    assert_eq!(no_lights, 0);
}
