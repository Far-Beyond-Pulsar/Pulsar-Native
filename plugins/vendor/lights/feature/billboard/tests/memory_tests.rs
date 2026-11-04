//! Memory representation and byte-encoding tests for `BillboardInstance`.
//!
//! These tests verify that the GPU-uploaded byte stream matches what the wgsl
//! shader expects. Any change to `BillboardInstance` that shifts bytes is a
//! breaking change for the shader.

use bytemuck::{bytes_of, cast_slice, Zeroable};
use helio_pass_billboard::BillboardInstance;
use std::mem::size_of;

// ── Byte-count fundamentals ──────────────────────────────────────────────────

#[test]
fn byte_slice_length_from_bytes_of_is_48() {
    let inst: BillboardInstance = Zeroable::zeroed();
    assert_eq!(bytes_of(&inst).len(), 48);
}

#[test]
fn size_of_billboard_instance_equals_bytes_of_len() {
    let inst: BillboardInstance = Zeroable::zeroed();
    assert_eq!(size_of::<BillboardInstance>(), bytes_of(&inst).len());
}

#[test]
fn size_of_is_divisible_by_four() {
    // All fields are f32 (4 bytes) → total is always a multiple of 4.
    assert_eq!(size_of::<BillboardInstance>() % 4, 0);
}

// ── Zero bytes produce zero floats ───────────────────────────────────────────

#[test]
fn all_zero_bytes_decode_to_zero_world_pos() {
    let z: BillboardInstance = Zeroable::zeroed();
    for component in z.world_pos.iter() {
        assert_eq!(*component, 0.0f32);
    }
}

#[test]
fn all_zero_bytes_decode_to_zero_scale_flags() {
    let z: BillboardInstance = Zeroable::zeroed();
    for component in z.scale_flags.iter() {
        assert_eq!(*component, 0.0f32);
    }
}

#[test]
fn all_zero_bytes_decode_to_zero_color() {
    let z: BillboardInstance = Zeroable::zeroed();
    for component in z.color.iter() {
        assert_eq!(*component, 0.0f32);
    }
}

// ── IEEE 754 encoding spot-checks ────────────────────────────────────────────

#[test]
fn f32_one_encodes_as_known_bytes() {
    // 1.0f32 in little-endian IEEE 754: [0x00, 0x00, 0x80, 0x3F].
    let expected = [0x00u8, 0x00, 0x80, 0x3F];
    assert_eq!(1.0f32.to_le_bytes(), expected);
}

#[test]
fn world_pos_x_bytes_match_f32_encoding() {
    let inst = BillboardInstance {
        world_pos: [1.0, 0.0, 0.0, 0.0],
        scale_flags: [0.0; 4],
        color: [0.0; 4],
    };
    let bytes = bytes_of(&inst);
    // world_pos.x is at offset 0 — first 4 bytes.
    let x_bytes: [u8; 4] = bytes[0..4].try_into().unwrap();
    assert_eq!(x_bytes, 1.0f32.to_le_bytes());
}

#[test]
fn color_r_byte_offset_is_32() {
    let inst = BillboardInstance {
        world_pos: [0.0; 4],
        scale_flags: [0.0; 4],
        color: [1.0, 0.0, 0.0, 0.0],
    };
    let bytes = bytes_of(&inst);
    // color starts at byte 32.
    let r_bytes: [u8; 4] = bytes[32..36].try_into().unwrap();
    assert_eq!(f32::from_le_bytes(r_bytes), 1.0f32);
}

#[test]
fn scale_flags_x_byte_offset_is_16() {
    let inst = BillboardInstance {
        world_pos: [0.0; 4],
        scale_flags: [2.0, 0.0, 0.0, 0.0],
        color: [0.0; 4],
    };
    let bytes = bytes_of(&inst);
    // scale_flags starts at byte 16.
    let sx_bytes: [u8; 4] = bytes[16..20].try_into().unwrap();
    assert_eq!(f32::from_le_bytes(sx_bytes), 2.0f32);
}

// ── Round-trip consistency ────────────────────────────────────────────────────

#[test]
fn bytes_round_trip_preserves_world_pos() {
    let orig = BillboardInstance {
        world_pos: [10.0, 20.0, 30.0, 0.0],
        scale_flags: [1.0, 1.0, 0.0, 0.0],
        color: [0.5, 0.5, 0.5, 1.0],
    };
    let bytes = bytes_of(&orig);
    let recovered: &BillboardInstance = bytemuck::from_bytes(bytes);
    assert_eq!(recovered.world_pos, orig.world_pos);
}

#[test]
fn bytes_round_trip_preserves_scale_flags() {
    let orig = BillboardInstance {
        world_pos: [0.0; 4],
        scale_flags: [3.14, 2.71, 1.0, 0.0],
        color: [0.0; 4],
    };
    let bytes = bytes_of(&orig);
    let recovered: &BillboardInstance = bytemuck::from_bytes(bytes);
    assert_eq!(recovered.scale_flags, orig.scale_flags);
}

#[test]
fn bytes_round_trip_preserves_color() {
    let orig = BillboardInstance {
        world_pos: [0.0; 4],
        scale_flags: [0.0; 4],
        color: [1.0, 0.5, 0.25, 0.75],
    };
    let bytes = bytes_of(&orig);
    let recovered: &BillboardInstance = bytemuck::from_bytes(bytes);
    assert_eq!(recovered.color, orig.color);
}

// ── NaN / special float values survive byte round-trips ──────────────────────

#[test]
fn f32_nan_survives_byte_round_trip() {
    let inst = BillboardInstance {
        world_pos: [f32::NAN, 0.0, 0.0, 0.0],
        scale_flags: [0.0; 4],
        color: [0.0; 4],
    };
    let bytes = bytes_of(&inst);
    let recovered: &BillboardInstance = bytemuck::from_bytes(bytes);
    // NaN != NaN by definition; check bit pattern survives.
    assert_eq!(
        inst.world_pos[0].to_bits(),
        recovered.world_pos[0].to_bits()
    );
}

#[test]
fn f32_infinity_survives_byte_round_trip() {
    let inst = BillboardInstance {
        world_pos: [f32::INFINITY, f32::NEG_INFINITY, 0.0, 0.0],
        scale_flags: [0.0; 4],
        color: [0.0; 4],
    };
    let bytes = bytes_of(&inst);
    let recovered: &BillboardInstance = bytemuck::from_bytes(bytes);
    assert_eq!(recovered.world_pos[0], f32::INFINITY);
    assert_eq!(recovered.world_pos[1], f32::NEG_INFINITY);
}

#[test]
fn negative_f32_survives_byte_round_trip() {
    let inst = BillboardInstance {
        world_pos: [-1.0, -2.5, -100.0, 0.0],
        scale_flags: [0.0; 4],
        color: [0.0; 4],
    };
    let bytes = bytes_of(&inst);
    let recovered: &BillboardInstance = bytemuck::from_bytes(bytes);
    assert_eq!(recovered.world_pos, inst.world_pos);
}

// ── Array / slice casting ─────────────────────────────────────────────────────

#[test]
fn cast_slice_works_for_instance_array() {
    let instances: [BillboardInstance; 3] =
        [Zeroable::zeroed(), Zeroable::zeroed(), Zeroable::zeroed()];
    let bytes: &[u8] = cast_slice(&instances);
    assert_eq!(bytes.len(), 3 * 48);
}

#[test]
fn two_instances_occupy_96_contiguous_bytes() {
    let instances: [BillboardInstance; 2] = [Zeroable::zeroed(), Zeroable::zeroed()];
    let bytes: &[u8] = cast_slice(&instances);
    assert_eq!(bytes.len(), 96);
}

#[test]
fn instance_array_is_tightly_packed() {
    // No padding between array elements in a C array.
    assert_eq!(
        size_of::<[BillboardInstance; 4]>(),
        4 * size_of::<BillboardInstance>()
    );
}

#[test]
fn instance_array_size_scales_linearly() {
    for n in 1usize..=8 {
        assert_eq!(
            size_of::<BillboardInstance>() * n,
            std::mem::size_of_val(
                &vec![
                    BillboardInstance {
                        world_pos: [0.0; 4],
                        scale_flags: [0.0; 4],
                        color: [0.0; 4]
                    };
                    n
                ][..]
            )
        );
    }
}

// ── Determinism ───────────────────────────────────────────────────────────────

#[test]
fn equal_instances_produce_equal_byte_slices() {
    let a = BillboardInstance {
        world_pos: [1.0, 2.0, 3.0, 0.0],
        scale_flags: [1.5, 2.5, 0.0, 0.0],
        color: [0.8, 0.2, 0.4, 1.0],
    };
    let b = a;
    assert_eq!(bytes_of(&a), bytes_of(&b));
}

#[test]
fn different_world_pos_produces_different_bytes() {
    let a = BillboardInstance {
        world_pos: [1.0, 0.0, 0.0, 0.0],
        scale_flags: [1.0; 4],
        color: [1.0; 4],
    };
    let b = BillboardInstance {
        world_pos: [2.0, 0.0, 0.0, 0.0],
        ..a
    };
    assert_ne!(bytes_of(&a), bytes_of(&b));
}

#[test]
fn different_colors_produce_different_bytes() {
    let a = BillboardInstance {
        world_pos: [0.0; 4],
        scale_flags: [1.0; 4],
        color: [1.0, 0.0, 0.0, 1.0],
    };
    let b = BillboardInstance {
        color: [0.0, 1.0, 0.0, 1.0],
        ..a
    };
    assert_ne!(bytes_of(&a), bytes_of(&b));
}
