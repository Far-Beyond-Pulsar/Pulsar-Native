//! Structural layout tests for `BillboardInstance`.
//!
//! These tests document and enforce the memory layout guarantees that the GPU
//! pipeline relies on. `BillboardInstance` is `#[repr(C)]` so field order and
//! offsets are stable across compiler versions.

use bytemuck::{bytes_of, Zeroable};
use helio_pass_billboard::BillboardInstance;
use std::mem::{align_of, offset_of, size_of};

// ── Size ─────────────────────────────────────────────────────────────────────

#[test]
fn size_of_billboard_instance_is_48() {
    // 3 fields, each [f32; 4] = 16 bytes → 3 × 16 = 48 bytes.
    assert_eq!(size_of::<BillboardInstance>(), 48);
}

#[test]
fn size_equals_three_vec4_f32() {
    // Expressed as three WGSL vec4<f32> fields.
    assert_eq!(size_of::<BillboardInstance>(), 3 * 4 * size_of::<f32>());
}

#[test]
fn size_is_twelve_floats() {
    // The struct holds exactly 12 individual f32 values.
    assert_eq!(size_of::<BillboardInstance>(), 12 * size_of::<f32>());
}

#[test]
fn size_is_multiple_of_sixteen() {
    // GPU WGSL vec4 alignment: 48 % 16 == 0.
    assert_eq!(size_of::<BillboardInstance>() % 16, 0);
}

// ── Alignment ────────────────────────────────────────────────────────────────

#[test]
fn align_of_billboard_instance_is_four() {
    // Contains only f32 fields → alignment is 4 bytes.
    assert_eq!(align_of::<BillboardInstance>(), 4);
}

#[test]
fn size_divisible_by_alignment() {
    assert_eq!(
        size_of::<BillboardInstance>() % align_of::<BillboardInstance>(),
        0
    );
}

// ── Field offsets (repr(C) guarantees these) ─────────────────────────────────

#[test]
fn world_pos_is_at_offset_0() {
    assert_eq!(offset_of!(BillboardInstance, world_pos), 0);
}

#[test]
fn scale_flags_is_at_offset_16() {
    // After one [f32; 4] (16 bytes).
    assert_eq!(offset_of!(BillboardInstance, scale_flags), 16);
}

#[test]
fn color_is_at_offset_32() {
    // After two [f32; 4] fields (2 × 16 = 32 bytes).
    assert_eq!(offset_of!(BillboardInstance, color), 32);
}

#[test]
fn scale_flags_offset_equals_one_vec4() {
    assert_eq!(
        offset_of!(BillboardInstance, scale_flags),
        1 * 4 * size_of::<f32>()
    );
}

#[test]
fn color_offset_equals_two_vec4s() {
    assert_eq!(
        offset_of!(BillboardInstance, color),
        2 * 4 * size_of::<f32>()
    );
}

#[test]
fn fields_span_exactly_48_bytes() {
    // Last field starts at 32, is 16 bytes wide → total = 48.
    assert_eq!(
        offset_of!(BillboardInstance, color) + 4 * size_of::<f32>(),
        48
    );
}

// ── Construction and field access ────────────────────────────────────────────

#[test]
fn can_construct_billboard_instance_with_known_values() {
    let inst = BillboardInstance {
        world_pos: [1.0, 2.0, 3.0, 0.0],
        scale_flags: [4.0, 5.0, 0.0, 0.0],
        color: [1.0, 0.0, 0.0, 1.0],
    };
    assert_eq!(inst.world_pos[0], 1.0);
    assert_eq!(inst.world_pos[1], 2.0);
    assert_eq!(inst.world_pos[2], 3.0);
}

#[test]
fn world_pos_w_component_is_unused_pad() {
    // The fourth component of world_pos is documented as unused pad.
    let inst = BillboardInstance {
        world_pos: [0.0, 0.0, 0.0, 42.0],
        scale_flags: [1.0, 1.0, 0.0, 0.0],
        color: [1.0; 4],
    };
    assert_eq!(inst.world_pos[3], 42.0);
}

#[test]
fn scale_xy_are_first_two_scale_components() {
    let inst = BillboardInstance {
        world_pos: [0.0; 4],
        scale_flags: [2.5, 3.5, 0.0, 0.0],
        color: [1.0; 4],
    };
    assert_eq!(inst.scale_flags[0], 2.5); // scale_x
    assert_eq!(inst.scale_flags[1], 3.5); // scale_y
}

#[test]
fn screen_scale_flag_is_z_component_of_scale_flags() {
    // scale_flags[2] == 1.0 means the billboard uses screen-space scaling.
    let screen_space = BillboardInstance {
        world_pos: [0.0; 4],
        scale_flags: [1.0, 1.0, 1.0, 0.0],
        color: [1.0; 4],
    };
    let world_space = BillboardInstance {
        world_pos: [0.0; 4],
        scale_flags: [1.0, 1.0, 0.0, 0.0],
        color: [1.0; 4],
    };
    assert_eq!(screen_space.scale_flags[2], 1.0);
    assert_eq!(world_space.scale_flags[2], 0.0);
}

#[test]
fn color_rgba_components_in_correct_order() {
    let inst = BillboardInstance {
        world_pos: [0.0; 4],
        scale_flags: [1.0, 1.0, 0.0, 0.0],
        color: [1.0, 0.5, 0.25, 0.75],
    };
    assert_eq!(inst.color[0], 1.0); // R
    assert_eq!(inst.color[1], 0.5); // G
    assert_eq!(inst.color[2], 0.25); // B
    assert_eq!(inst.color[3], 0.75); // A
}

#[test]
fn alpha_zero_makes_transparent_billboard() {
    let inst = BillboardInstance {
        world_pos: [0.0; 4],
        scale_flags: [1.0, 1.0, 0.0, 0.0],
        color: [1.0, 1.0, 1.0, 0.0],
    };
    assert_eq!(inst.color[3], 0.0);
}

// ── Mutation ─────────────────────────────────────────────────────────────────

#[test]
fn can_mutate_world_pos_x() {
    let mut inst = BillboardInstance {
        world_pos: [0.0; 4],
        scale_flags: [1.0; 4],
        color: [1.0; 4],
    };
    inst.world_pos[0] = 99.0;
    assert_eq!(inst.world_pos[0], 99.0);
}

#[test]
fn can_mutate_color_alpha_independently() {
    let mut inst = BillboardInstance {
        world_pos: [0.0; 4],
        scale_flags: [1.0; 4],
        color: [1.0, 1.0, 1.0, 1.0],
    };
    inst.color[3] = 0.0;
    assert_eq!(inst.color[3], 0.0);
    // Other channels unaffected.
    assert_eq!(inst.color[0], 1.0);
}

// ── Copy / Clone ─────────────────────────────────────────────────────────────

#[test]
fn billboard_instance_is_copy() {
    let a = BillboardInstance {
        world_pos: [1.0; 4],
        scale_flags: [2.0; 4],
        color: [3.0; 4],
    };
    let b = a; // copy, not move
    assert_eq!(b.world_pos, a.world_pos);
    assert_eq!(b.scale_flags, a.scale_flags);
    assert_eq!(b.color, a.color);
}

#[test]
fn billboard_instance_is_clone() {
    let a = BillboardInstance {
        world_pos: [1.0; 4],
        scale_flags: [2.0; 4],
        color: [3.0; 4],
    };
    let b = a.clone();
    assert_eq!(b.world_pos, a.world_pos);
    assert_eq!(b.color, a.color);
}

// ── bytemuck: zeroed / bytes_of ───────────────────────────────────────────────

#[test]
fn zeroed_instance_has_zero_world_pos() {
    let z: BillboardInstance = Zeroable::zeroed();
    assert_eq!(z.world_pos, [0.0f32; 4]);
}

#[test]
fn zeroed_instance_has_zero_scale_flags() {
    let z: BillboardInstance = Zeroable::zeroed();
    assert_eq!(z.scale_flags, [0.0f32; 4]);
}

#[test]
fn zeroed_instance_has_zero_color() {
    let z: BillboardInstance = Zeroable::zeroed();
    assert_eq!(z.color, [0.0f32; 4]);
}

#[test]
fn zeroed_instance_bytes_are_all_zero() {
    let z: BillboardInstance = Zeroable::zeroed();
    assert!(bytes_of(&z).iter().all(|&b| b == 0));
}

#[test]
fn bytes_of_produces_correct_length() {
    let inst = BillboardInstance {
        world_pos: [0.0; 4],
        scale_flags: [0.0; 4],
        color: [0.0; 4],
    };
    assert_eq!(bytes_of(&inst).len(), 48);
}

#[test]
fn bytes_of_matches_size_of() {
    let inst: BillboardInstance = Zeroable::zeroed();
    assert_eq!(bytes_of(&inst).len(), size_of::<BillboardInstance>());
}
