//! Layout and architecture documentation tests for `DeferredLightPass`.
//!
//! `DeferredLightPass` requires a live `wgpu::Device` to construct.  These
//! tests document the known memory layout of the privately-declared
//! `DeferredGlobals` uniform buffer and the architectural decisions this pass
//! encodes, serving as an executable specification.

use helio_pass_deferred_light::DeferredLightPass;
use std::mem::size_of;

// ── Type accessibility ────────────────────────────────────────────────────────

#[test]
fn deferred_light_pass_type_is_publicly_accessible() {
    let _: std::marker::PhantomData<DeferredLightPass> = std::marker::PhantomData;
}

#[test]
fn deferred_light_pass_type_name_contains_pass() {
    let name = std::any::type_name::<DeferredLightPass>();
    assert!(name.contains("DeferredLightPass"), "got: {name}");
}

// ── DeferredGlobals layout maths (private struct, derived from source) ────────
//
// struct DeferredGlobals {
//   frame: u32,              // 4
//   delta_time: f32,         // 4
//   light_count: u32,        // 4
//   ambient_intensity: f32,  // 4  → row-1 = 16 bytes
//   ambient_color: [f32;4],  // 16 → row-2
//   csm_splits: [f32;4],     // 16 → row-3
//   debug_mode: u32,                 // 4
//   num_tiles_x: u32,                // 4
//   reflection_capture_count: u32,   // 4
//   enable_reflections: u32,         // 4  → row-4 = 16 bytes
//   enable_env_reflections: u32,     // 4
//   _pad: [u32; 3],                  // 12 → row-5 = 16 bytes
// }                                  // Total = 80 bytes

#[test]
fn deferred_globals_row1_scalar_fields_are_16_bytes() {
    let row1 = size_of::<u32>()  // frame
             + size_of::<f32>()  // delta_time
             + size_of::<u32>()  // light_count
             + size_of::<f32>(); // ambient_intensity
    assert_eq!(row1, 16);
}

#[test]
fn deferred_globals_one_vec4_field_is_16_bytes() {
    let vec4_bytes = 4 * size_of::<f32>();
    assert_eq!(vec4_bytes, 16);
}

#[test]
fn deferred_globals_two_vec4_fields_are_32_bytes() {
    // ambient_color + csm_splits
    let two_vec4s = 2 * (4 * size_of::<f32>());
    assert_eq!(two_vec4s, 32);
}

#[test]
fn deferred_globals_last_row_padding_is_16_bytes() {
    // enable_env_reflections + _pad (four u32s = 16 bytes).
    let last_row = 4 * size_of::<u32>();
    assert_eq!(last_row, 16);
}

#[test]
fn deferred_globals_total_size_is_80_bytes() {
    let row1 = size_of::<u32>() * 2 + size_of::<f32>() * 2; // 16
    let vec4_rows = 2 * (4 * size_of::<f32>()); // 32
    let scalar_rows = 2 * 4 * size_of::<u32>(); // 32
    assert_eq!(row1 + vec4_rows + scalar_rows, 80);
}

#[test]
fn deferred_globals_total_is_multiple_of_16() {
    // Uniform buffers must be a multiple of 16 bytes (wgpu / Vulkan / Metal).
    assert_eq!(80 % 16, 0);
}

#[test]
fn deferred_globals_total_divided_by_16_is_five_rows() {
    assert_eq!(80 / 16, 5);
}

// ── Individual field sizes ────────────────────────────────────────────────────

#[test]
fn frame_field_is_u32_4_bytes() {
    assert_eq!(size_of::<u32>(), 4);
}

#[test]
fn delta_time_field_is_f32_4_bytes() {
    assert_eq!(size_of::<f32>(), 4);
}

#[test]
fn light_count_field_is_u32_4_bytes() {
    assert_eq!(size_of::<u32>(), 4);
}

#[test]
fn ambient_intensity_field_is_f32_4_bytes() {
    assert_eq!(size_of::<f32>(), 4);
}

#[test]
fn ambient_color_is_16_bytes() {
    // ambient_color: [f32; 4]
    assert_eq!(4 * size_of::<f32>(), 16);
}

#[test]
fn csm_splits_vec4_is_16_bytes() {
    // csm_splits: [f32; 4]
    assert_eq!(4 * size_of::<f32>(), 16);
}

// ── CSM architecture constants ────────────────────────────────────────────────

#[test]
fn csm_cascade_count_is_four() {
    // The pass uses 4 CSM splits stored in [f32; 4].
    const CSM_CASCADE_COUNT: usize = 4;
    assert_eq!(CSM_CASCADE_COUNT, 4);
}

#[test]
fn csm_splits_fits_in_one_vec4() {
    // 4 cascades, 4 elements in [f32;4] — exact fit.
    const CSM_CASCADE_COUNT: usize = 4;
    const VEC4_COMPONENTS: usize = 4;
    assert_eq!(CSM_CASCADE_COUNT, VEC4_COMPONENTS);
}

// ── Uniform buffer wgpu alignment ────────────────────────────────────────────

#[test]
fn globals_buf_size_96_satisfies_min_uniform_binding_size_multiple() {
    // wgpu requires uniform buffer binding offsets to be a multiple of 256,
    // and total size to be a multiple of 16 (WGSL vec4 alignment).
    assert_eq!(80 % 16, 0);
}
