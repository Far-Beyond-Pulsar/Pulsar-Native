//! Bit-level packing primitives shared by the foliage GPU types.
//!
//! Every function here has a one-or-two-instruction WGSL equivalent
//! (`pack2x16unorm`, `pack4x8unorm`, `pack2x16float`, `unpack*`), and the placement
//! shader is expected to use the builtin rather than a hand-rolled port. These Rust
//! versions exist so the packing *contract* — which field lives in which half of which
//! `u32`, and what the quantisation endpoints are — is pinned by a test instead of by a
//! comment. If the WGSL and these functions ever disagree, the symptom is not a crash:
//! blades render at plausible-but-wrong positions, sizes or rotations, which reads as
//! "the placement shader is buggy" and costs a day to bisect. The round-trip tests at
//! the bottom of this file are the cheap version of that day.
//!
//! # Endpoint conventions
//!
//! Two different quantisation conventions are used deliberately and they are not
//! interchangeable:
//!
//! - **Unorm** ([`pack_unorm16`], [`pack_unorm8`]) divides by `2^n - 1`, so `0.0` and
//!   `1.0` both round-trip exactly. Used for tile-local position, scales and tint —
//!   values where both endpoints are meaningful and distinct.
//! - **Turns** ([`pack_yaw`]) divides by `2^n`, because an angle wraps: `0` and `2π` are
//!   the *same* orientation, so spending a code on both would waste a step and make the
//!   wrap point non-uniform. Using `65535` here would leave a visible seam of
//!   over-represented yaw near zero.

/// Quantise a `0.0..=1.0` value to 16-bit unorm.
///
/// Out-of-range inputs are clamped rather than wrapped: a candidate position that
/// escaped its tile by a hair should land on the tile edge, not teleport to the
/// opposite corner. Non-finite input yields `0`, because Rust's saturating float→int
/// cast maps NaN to zero — a blade at the tile origin is a containable artefact, an
/// out-of-bounds arena write is not.
#[inline]
pub fn pack_unorm16(value: f32) -> u16 {
    let clamped = if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    };
    (clamped * 65535.0 + 0.5) as u16
}

/// Inverse of [`pack_unorm16`]. Exact at both endpoints.
#[inline]
pub fn unpack_unorm16(packed: u16) -> f32 {
    packed as f32 * (1.0 / 65535.0)
}

/// Quantise a `0.0..=1.0` value to 8-bit unorm.
///
/// 8 bits is 1/255 ≈ 0.4% — below the threshold where a difference in blade height
/// scale or tint is visible against neighbouring blades, which is the only comparison a
/// viewer can actually make.
#[inline]
pub fn pack_unorm8(value: f32) -> u8 {
    let clamped = if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    };
    (clamped * 255.0 + 0.5) as u8
}

/// Inverse of [`pack_unorm8`]. Exact at both endpoints.
#[inline]
pub fn unpack_unorm8(packed: u8) -> f32 {
    packed as f32 * (1.0 / 255.0)
}

/// Quantise an angle in radians to 16 bits of turn.
///
/// The input is wrapped, not clamped — a yaw derived from a hash has no natural range
/// and clamping would pile every out-of-range blade onto one of two orientations, which
/// shows up as a field of grass with two suspicious grain directions. Non-finite input
/// yields `0`; there is no orientation that is a correct answer to NaN, and zero is at
/// least stable frame-to-frame so it cannot flicker.
#[inline]
pub fn pack_yaw(radians: f32) -> u16 {
    if !radians.is_finite() {
        return 0;
    }
    let turns = radians * (1.0 / std::f32::consts::TAU);
    let wrapped = turns - turns.floor();
    // `wrapped` is in [0, 1); the rounding step can reach 65536, which must wrap to 0
    // rather than saturate to 65535 — saturating would make the seam at 2π a half-step
    // wide instead of exact.
    (((wrapped * 65536.0 + 0.5) as u32) & 0xffff) as u16
}

/// Inverse of [`pack_yaw`], returning radians in `0.0..TAU`.
#[inline]
pub fn unpack_yaw(packed: u16) -> f32 {
    packed as f32 * (std::f32::consts::TAU / 65536.0)
}

/// Encode an `f32` as IEEE-754 binary16 bits, round-to-nearest-even.
///
/// Hand-rolled because `f16` is not stable in this toolchain and the `half` crate is not
/// a workspace dependency — and this crate's whole reason to exist is being buildable
/// and testable without pulling anything heavy in. The rounding mode matters: GPUs round
/// to nearest even in `pack2x16float`, and a truncating CPU encoder would produce blade
/// dimensions that differ from the shader's by one ULP, which is enough to make a
/// byte-for-byte placement determinism test fail against a CPU reference.
///
/// Overflow saturates to infinity and NaN is preserved as a quiet NaN, matching the
/// GPU. Callers that cannot tolerate infinity must range-check before encoding.
pub fn f32_to_f16_bits(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let mantissa = bits & 0x007f_ffff;
    let raw_exponent = ((bits >> 23) & 0xff) as i32;

    if raw_exponent == 0xff {
        // Infinity keeps a zero mantissa; any NaN collapses to one canonical quiet NaN.
        return if mantissa != 0 {
            sign | 0x7e00
        } else {
            sign | 0x7c00
        };
    }

    // Rebias from f32's 127 to f16's 15.
    let exponent = raw_exponent - (127 - 15);

    if exponent >= 0x1f {
        return sign | 0x7c00;
    }

    if exponent <= 0 {
        // Subnormal territory. Below 2^-25 nothing survives rounding.
        if exponent < -10 {
            return sign;
        }
        let with_implicit_one = mantissa | 0x0080_0000;
        let shift = (14 - exponent) as u32; // 14..=24
        let truncated = (with_implicit_one >> shift) as u16;
        let round_bit = 1u32 << (shift - 1);
        let round_up = (with_implicit_one & round_bit) != 0
            && ((with_implicit_one & (round_bit - 1)) != 0 || (truncated & 1) != 0);
        return sign | (truncated + u16::from(round_up));
    }

    let truncated = ((exponent as u16) << 10) | ((mantissa >> 13) as u16);
    let round_bit = 0x0000_1000u32;
    let round_up =
        (mantissa & round_bit) != 0 && ((mantissa & (round_bit - 1)) != 0 || (truncated & 1) != 0);
    // A carry out of the mantissa increments the exponent field for free, and a carry
    // out of the exponent lands exactly on the infinity encoding. Both are correct.
    sign | (truncated + u16::from(round_up))
}

/// Decode IEEE-754 binary16 bits to `f32`. Lossless — every f16 is exactly representable.
pub fn f16_bits_to_f32(bits: u16) -> f32 {
    let sign_bit = ((bits & 0x8000) as u32) << 16;
    let exponent = ((bits >> 10) & 0x1f) as u32;
    let mantissa = (bits & 0x03ff) as u32;

    if exponent == 0 {
        // Zero and subnormals: value = mantissa * 2^-24, computed directly rather than
        // by renormalising, because the multiply is exact and the loop is not obviously so.
        let magnitude = mantissa as f32 * (1.0 / 16_777_216.0);
        return if sign_bit != 0 { -magnitude } else { magnitude };
    }

    if exponent == 0x1f {
        return f32::from_bits(sign_bit | 0x7f80_0000 | (mantissa << 13));
    }

    f32::from_bits(sign_bit | ((exponent + (127 - 15)) << 23) | (mantissa << 13))
}

/// Pack two `f32` as two `f16` into one `u32`: `low` in bits 0..16, `high` in bits 16..32.
///
/// Byte-identical to WGSL's `pack2x16float(vec2(low, high))`.
#[inline]
pub fn pack_f16x2(low: f32, high: f32) -> u32 {
    (f32_to_f16_bits(low) as u32) | ((f32_to_f16_bits(high) as u32) << 16)
}

/// Inverse of [`pack_f16x2`]. Byte-identical to WGSL's `unpack2x16float`.
#[inline]
pub fn unpack_f16x2(packed: u32) -> [f32; 2] {
    [
        f16_bits_to_f32((packed & 0xffff) as u16),
        f16_bits_to_f32((packed >> 16) as u16),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unorm16_round_trips_every_code() {
        for code in 0..=u16::MAX {
            let decoded = unpack_unorm16(code);
            assert!(
                (0.0..=1.0).contains(&decoded),
                "code {code} left the unit range"
            );
            assert_eq!(
                pack_unorm16(decoded),
                code,
                "unorm16 code {code} did not survive a round trip"
            );
        }
        assert_eq!(pack_unorm16(0.0), 0);
        assert_eq!(pack_unorm16(1.0), u16::MAX);
    }

    #[test]
    fn unorm8_round_trips_every_code() {
        for code in 0..=u8::MAX {
            assert_eq!(pack_unorm8(unpack_unorm8(code)), code);
        }
        assert_eq!(pack_unorm8(0.0), 0);
        assert_eq!(pack_unorm8(1.0), u8::MAX);
    }

    #[test]
    fn unorm_clamps_out_of_range_and_non_finite() {
        assert_eq!(pack_unorm16(-5.0), 0);
        assert_eq!(pack_unorm16(5.0), u16::MAX);
        assert_eq!(pack_unorm16(f32::NAN), 0);
        assert_eq!(pack_unorm16(f32::INFINITY), 0);
        assert_eq!(pack_unorm8(-5.0), 0);
        assert_eq!(pack_unorm8(5.0), u8::MAX);
        assert_eq!(pack_unorm8(f32::NAN), 0);
    }

    #[test]
    fn yaw_round_trips_every_code() {
        for code in 0..=u16::MAX {
            let radians = unpack_yaw(code);
            assert_eq!(
                pack_yaw(radians),
                code,
                "yaw code {code} did not round trip"
            );
        }
    }

    #[test]
    fn yaw_wraps_instead_of_clamping() {
        const STEP: f32 = std::f32::consts::TAU / 65536.0;
        assert_eq!(pack_yaw(0.0), 0);
        assert_eq!(pack_yaw(std::f32::consts::TAU), 0, "2π must alias to 0");
        assert_eq!(pack_yaw(-std::f32::consts::TAU), 0);
        assert_eq!(pack_yaw(4.0 * std::f32::consts::TAU), 0);
        // One step below a full turn is the last code, not a clamped duplicate.
        assert_eq!(pack_yaw(std::f32::consts::TAU - STEP), u16::MAX);
        // Negative angles land in the upper half of the code range, not at zero.
        assert_eq!(pack_yaw(-STEP), u16::MAX);
        assert_eq!(pack_yaw(f32::NAN), 0);
        assert_eq!(pack_yaw(f32::NEG_INFINITY), 0);
    }

    #[test]
    fn yaw_quantisation_error_stays_under_half_a_step() {
        const STEP: f32 = std::f32::consts::TAU / 65536.0;
        let mut worst = 0.0f32;
        for i in 0..4096u32 {
            let angle = i as f32 * (std::f32::consts::TAU / 4096.0);
            let error = (unpack_yaw(pack_yaw(angle)) - angle).abs();
            worst = worst.max(error.min((std::f32::consts::TAU - error).abs()));
        }
        assert!(
            worst <= STEP * 0.5 + 1.0e-6,
            "worst yaw error {worst} exceeds half a step"
        );
    }

    #[test]
    fn f16_round_trips_exactly_representable_values() {
        // Every finite f16 decodes to an f32 that re-encodes to the same bits. This is
        // the property the placement determinism test leans on.
        for code in 0..=u16::MAX {
            let exponent = (code >> 10) & 0x1f;
            if exponent == 0x1f {
                continue; // inf/NaN handled separately
            }
            let decoded = f16_bits_to_f32(code);
            assert!(
                decoded.is_finite(),
                "code {code:#06x} decoded to non-finite"
            );
            assert_eq!(
                f32_to_f16_bits(decoded),
                code,
                "f16 code {code:#06x} did not round trip"
            );
        }
    }

    #[test]
    fn f16_handles_boundaries_and_specials() {
        assert_eq!(f16_bits_to_f32(0x0000), 0.0);
        assert_eq!(f16_bits_to_f32(0x3c00), 1.0);
        assert_eq!(f16_bits_to_f32(0xbc00), -1.0);
        assert_eq!(f16_bits_to_f32(0x0001), 2.0f32.powi(-24)); // min subnormal
        assert_eq!(f16_bits_to_f32(0x0400), 2.0f32.powi(-14)); // min normal
        assert_eq!(f16_bits_to_f32(0x7bff), 65504.0); // max finite

        assert_eq!(f32_to_f16_bits(0.0), 0x0000);
        assert_eq!(f32_to_f16_bits(-0.0), 0x8000);
        assert_eq!(f32_to_f16_bits(1.0), 0x3c00);
        assert_eq!(f32_to_f16_bits(65504.0), 0x7bff);
        // Overflow saturates to infinity rather than wrapping to a small number.
        assert_eq!(f32_to_f16_bits(70_000.0), 0x7c00);
        assert_eq!(f32_to_f16_bits(f32::INFINITY), 0x7c00);
        assert_eq!(f32_to_f16_bits(f32::NEG_INFINITY), 0xfc00);
        assert!(f16_bits_to_f32(f32_to_f16_bits(f32::NAN)).is_nan());
        // Underflow flushes to a signed zero, not to a wrong-signed denormal.
        assert_eq!(f32_to_f16_bits(1.0e-10), 0x0000);
        assert_eq!(f32_to_f16_bits(-1.0e-10), 0x8000);
        // Ties round to even: 2^-25 sits exactly between 0 and the min subnormal.
        assert_eq!(f32_to_f16_bits(2.0f32.powi(-25)), 0x0000);
        assert_eq!(f32_to_f16_bits(2.0f32.powi(-25) * 1.5), 0x0001);
    }

    #[test]
    fn f16_relative_error_is_within_binary16_precision() {
        // Everything the type descriptor stores as f16 lives in this range: blade
        // dimensions in metres, cosines, and LOD distances out to a few hundred metres.
        for i in 1..20_000u32 {
            let value = i as f32 * 0.0625;
            let error = (f16_bits_to_f32(f32_to_f16_bits(value)) - value).abs();
            assert!(
                error <= value * 5.0e-4,
                "f16 relative error {error} at {value} exceeds binary16 precision"
            );
        }
    }

    #[test]
    fn f16x2_packs_low_half_first() {
        let packed = pack_f16x2(1.0, 2.0);
        assert_eq!(
            packed & 0xffff,
            0x3c00,
            "low half must hold the first argument"
        );
        assert_eq!(
            packed >> 16,
            0x4000,
            "high half must hold the second argument"
        );
        assert_eq!(unpack_f16x2(packed), [1.0, 2.0]);
        assert_eq!(unpack_f16x2(pack_f16x2(0.0, 0.0)), [0.0, 0.0]);
        assert_eq!(unpack_f16x2(pack_f16x2(-3.5, 65504.0)), [-3.5, 65504.0]);
    }
}
