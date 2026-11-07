//! Bit-exact integer noise, mirrored by `shaders/noise.wgsl`.
//!
//! Terrain generators build their fields from these primitives so the CPU
//! (collision, ray casts, gameplay queries) and the GPU (streaming,
//! rendering) agree to the bit. Every operation is wrapping two's-complement
//! integer arithmetic; WGSL names are the same except `lerp` (`lerp_q16`)
//! and `scale` (`scale_q16`).
use glam::IVec3;

/// Noise unit (Q16, `NOISE_ONE` in WGSL): [`noise`] returns values in
/// about `[-ONE, ONE]`.
pub const ONE: i32 = 65_536;

/// Integer hash of a lattice point.
#[inline]
pub fn hash3(x: i32, y: i32, z: i32, seed: u32) -> u32 {
    let mut v = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (y as u32).wrapping_mul(0xd816_3841)
        ^ (z as u32).wrapping_mul(0xcb1a_b31f)
        ^ seed;
    v ^= v >> 16;
    v = v.wrapping_mul(0x7feb_352d);
    v ^= v >> 15;
    v = v.wrapping_mul(0x846c_a68b);
    v ^ (v >> 16)
}

#[inline]
fn grad(hash: u32, x: i32, y: i32, z: i32) -> i32 {
    let h = hash & 15;
    let u = if h < 8 { x } else { y };
    let v = if h < 4 {
        y
    } else if h == 12 || h == 14 {
        x
    } else {
        z
    };
    (if h & 1 == 0 { u } else { u.wrapping_neg() }).wrapping_add(if h & 2 == 0 {
        v
    } else {
        v.wrapping_neg()
    })
}

/// `a * w >> 16` for `|a| < 2^19`, `0 <= w <= 65536` with 32-bit intermediates.
#[inline]
pub fn mul16(a: i32, w: i32) -> i32 {
    a.wrapping_mul(w >> 8)
        .wrapping_add(a.wrapping_mul(w & 255) >> 8)
        >> 8
}

#[inline]
pub fn fade(t: i32) -> i32 {
    // 6t^5 - 15t^4 + 10t^3 in Q16.
    let tu = t as u32;
    let t2 = tu.wrapping_mul(tu) >> 16;
    let t3 = t2.wrapping_mul(tu) >> 16;
    let inner = (6 * t2 as i32).wrapping_sub(15 * t).wrapping_add(10 * ONE);
    mul16(t3 as i32, inner)
}

/// `a + (b - a) * w` for a Q16 weight `w` in `[0, ONE]`.
#[inline]
pub fn lerp(a: i32, b: i32, w: i32) -> i32 {
    a.wrapping_add(mul16(b.wrapping_sub(a), w))
}

/// Gradient noise on a lattice of spacing `2^shift` domain units, with
/// 16-bit fractions and output in about `[-ONE, ONE]`. Its gradient is at
/// most about 5.3 `ONE` per lattice spacing.
/// Bound on how fast [`noise`] changes: Q16 units per lattice unit
/// (`2^shift` input units) between any two points, plus
/// [`NOISE_ROUNDING`] (tested; the largest measured is about 2.8 units).
/// Mirrored in WGSL.
pub const NOISE_SLOPE: i32 = 4 * ONE;
pub const NOISE_ROUNDING: i32 = 16;

pub fn noise(p: IVec3, shift: u32, seed: u32) -> i32 {
    let mask = (1i32 << shift) - 1;
    let c = [p.x >> shift, p.y >> shift, p.z >> shift];
    let f = [p.x & mask, p.y & mask, p.z & mask].map(|v| {
        if shift >= 16 {
            v >> (shift - 16)
        } else {
            v << (16 - shift)
        }
    });
    let w = f.map(fade);
    let corner = |dx: i32, dy: i32, dz: i32| {
        grad(
            hash3(
                c[0].wrapping_add(dx),
                c[1].wrapping_add(dy),
                c[2].wrapping_add(dz),
                seed,
            ),
            f[0] - dx * ONE,
            f[1] - dy * ONE,
            f[2] - dz * ONE,
        )
    };
    let x00 = lerp(corner(0, 0, 0), corner(1, 0, 0), w[0]);
    let x10 = lerp(corner(0, 1, 0), corner(1, 1, 0), w[0]);
    let x01 = lerp(corner(0, 0, 1), corner(1, 0, 1), w[0]);
    let x11 = lerp(corner(0, 1, 1), corner(1, 1, 1), w[0]);
    let y0 = lerp(x00, x10, w[1]);
    let y1 = lerp(x01, x11, w[1]);
    lerp(y0, y1, w[2]).clamp(-ONE, ONE)
}

/// `n * amplitude / ONE` without overflow (`|n| <= 2^17`, `|amplitude| < 2^27`).
#[inline]
pub fn scale(n: i32, amplitude: i32) -> i32 {
    n.wrapping_mul(amplitude >> 16)
        .wrapping_add(n.wrapping_mul((amplitude & 0xffff) >> 4) >> 12)
}

/// Unit of the fine noise (Q24, `FINE_ONE` in WGSL).
pub const FINE_ONE: i32 = 1 << 24;

/// `a * b / 2^24` rounded to nearest, for `|a| < 2^28` and `|b| <= 2^24`:
/// the exact product (`mul_fine` in WGSL: one wide multiply on GPUs with
/// 64-bit integers).
#[inline]
pub fn mul_fine(a: i32, b: i32) -> i32 {
    mul_shr_signed(a, b, 24)
}

#[inline]
fn fade_q24(t: i32) -> i32 {
    let t2 = mul_fine(t, t);
    let t3 = mul_fine(t2, t);
    mul_fine(6 * t2 - 15 * t + 10 * FINE_ONE, t3)
}

/// [`noise`] with 24-bit fractions and output in about `[-FINE_ONE,
/// FINE_ONE]` (about 256 times [`noise`]). Low-frequency octaves whose
/// value scales large quantities (continents, mountain regions, domain
/// warp) use it: with 16-bit fractions they would be constant over tens of
/// metres and step between neighbouring cells.
pub fn noise_fine(p: IVec3, shift: u32, seed: u32) -> i32 {
    let mask = (1i32 << shift) - 1;
    let c = [p.x >> shift, p.y >> shift, p.z >> shift];
    let f = [p.x & mask, p.y & mask, p.z & mask].map(|v| {
        if shift >= 24 {
            v >> (shift - 24)
        } else {
            v << (24 - shift)
        }
    });
    let w = f.map(fade_q24);
    let corner = |dx: i32, dy: i32, dz: i32| {
        grad(
            hash3(
                c[0].wrapping_add(dx),
                c[1].wrapping_add(dy),
                c[2].wrapping_add(dz),
                seed,
            ),
            f[0] - dx * FINE_ONE,
            f[1] - dy * FINE_ONE,
            f[2] - dz * FINE_ONE,
        )
    };
    let lerp = |a: i32, b: i32, w: i32| a + mul_fine(b - a, w);
    let x00 = lerp(corner(0, 0, 0), corner(1, 0, 0), w[0]);
    let x10 = lerp(corner(0, 1, 0), corner(1, 1, 0), w[0]);
    let x01 = lerp(corner(0, 0, 1), corner(1, 0, 1), w[0]);
    let x11 = lerp(corner(0, 1, 1), corner(1, 1, 1), w[0]);
    let y0 = lerp(x00, x10, w[1]);
    let y1 = lerp(x01, x11, w[1]);
    lerp(y0, y1, w[2]).clamp(-FINE_ONE, FINE_ONE)
}

/// `sin(2 pi t)` for `t` in Q16 turns (wrapping) in Q16: a parabola with
/// one correction step (within 0.002). Mirrored as `sin_turns`.
#[inline]
pub fn sin_turns(t: i32) -> i32 {
    let x = t.wrapping_shl(16) >> 16;
    let xn = x * 2;
    let a = xn >> 2;
    let y = 4 * xn - 4 * ((a * a.abs()) >> 12);
    let b = y >> 2;
    y + mul16(((b * b.abs()) >> 12) - y, 14_746)
}

/// [`noise_fine`] and its gradient. The value is bit-identical to
/// [`noise_fine`]; the gradient is the exact derivative of the interpolant
/// in `FINE_ONE` per lattice spacing on each axis (at most about 5.3
/// `FINE_ONE`; zero where the value is clamped). Mirrored by
/// `noise_fine_grad` in `noise.wgsl`.
pub fn noise_fine_grad(p: IVec3, shift: u32, seed: u32) -> (i32, IVec3) {
    let mask = (1i32 << shift) - 1;
    let c = [p.x >> shift, p.y >> shift, p.z >> shift];
    let f = [p.x & mask, p.y & mask, p.z & mask].map(|v| {
        if shift >= 24 {
            v >> (shift - 24)
        } else {
            v << (24 - shift)
        }
    });
    let w = f.map(fade_q24);
    // fade'(t) = 30 t^2 (1 - t)^2, at most 1.875 FINE_ONE.
    let dw = f.map(|t| {
        let u = FINE_ONE - t;
        30 * mul_fine(mul_fine(t, t), mul_fine(u, u))
    });
    let lerp = |a: i32, b: i32, w: i32| a + mul_fine(b - a, w);
    // Corner values and the corner gradients' components (-1, 0 or 1).
    let mut v = [0i32; 8];
    let mut g = [[0i32; 3]; 8];
    for (index, (vc, gc)) in v.iter_mut().zip(g.iter_mut()).enumerate() {
        let (dx, dy, dz) = (
            (index & 1) as i32,
            ((index >> 1) & 1) as i32,
            (index >> 2) as i32,
        );
        let h = hash3(
            c[0].wrapping_add(dx),
            c[1].wrapping_add(dy),
            c[2].wrapping_add(dz),
            seed,
        );
        *vc = grad(
            h,
            f[0] - dx * FINE_ONE,
            f[1] - dy * FINE_ONE,
            f[2] - dz * FINE_ONE,
        );
        *gc = [
            grad(h, FINE_ONE, 0, 0),
            grad(h, 0, FINE_ONE, 0),
            grad(h, 0, 0, FINE_ONE),
        ];
    }
    let x00 = lerp(v[0], v[1], w[0]);
    let x10 = lerp(v[2], v[3], w[0]);
    let x01 = lerp(v[4], v[5], w[0]);
    let x11 = lerp(v[6], v[7], w[0]);
    let y0 = lerp(x00, x10, w[1]);
    let y1 = lerp(x01, x11, w[1]);
    let value = lerp(y0, y1, w[2]);
    if value.abs() > FINE_ONE {
        return (value.clamp(-FINE_ONE, FINE_ONE), IVec3::ZERO);
    }
    let tri = |k: [i32; 8]| {
        let y0 = lerp(lerp(k[0], k[1], w[0]), lerp(k[2], k[3], w[0]), w[1]);
        let y1 = lerp(lerp(k[4], k[5], w[0]), lerp(k[6], k[7], w[0]), w[1]);
        lerp(y0, y1, w[2])
    };
    let along_x = lerp(
        lerp(v[1] - v[0], v[3] - v[2], w[1]),
        lerp(v[5] - v[4], v[7] - v[6], w[1]),
        w[2],
    );
    let along_y = lerp(x10 - x00, x11 - x01, w[2]);
    let along_z = y1 - y0;
    let gradient = IVec3::new(
        mul_fine(along_x, dw[0]) + tri(g.map(|c| c[0])),
        mul_fine(along_y, dw[1]) + tri(g.map(|c| c[1])),
        mul_fine(along_z, dw[2]) + tri(g.map(|c| c[2])),
    );
    (value, gradient)
}

pub const Q30: u32 = 1 << 30;

/// `(a * b) >> s` of the exact 64-bit product (`mul_shr` in WGSL computes
/// it with 16-bit limbs). The caller keeps the result within 32 bits.
#[inline]
pub fn mul_shr(a: u32, b: u32, s: u32) -> u32 {
    ((u64::from(a) * u64::from(b)) >> s) as u32
}

/// `1 / sqrt(d)` in Q30 for `d` in `[1, 4)` (Q30, up to `2^32 - 1`):
/// Newton from a linear guess, five fixed steps.
#[inline]
fn rsqrt_wide(d: u32) -> u32 {
    let mut y = 1_342_177_280u32 - mul_shr(d, 204_010_946, 30);
    for _ in 0..5 {
        let dy2 = mul_shr(d, mul_shr(y, y, 30), 30);
        y = mul_shr(y, 3 * Q30 - dy2, 31);
    }
    y
}

/// `a b / 2^s` rounded to nearest (halves up) of the exact signed 64-bit
/// product, wrapping (the caller keeps it within 32 bits): one wide
/// multiply, an add and a shift on GPUs with 64-bit integers. Rounding
/// toward zero needed sign handling, a fifth of the field's GPU time;
/// rounding down biased sums of products.
#[inline]
pub fn mul_shr_signed(a: i32, b: i32, s: u32) -> i32 {
    ((i64::from(a) * i64::from(b)).wrapping_add(((1u64 << s) >> 1) as i64) >> s) as i32
}

/// `v / |v|` in Q30 to within a few units (zero for zero), for any `v`:
/// exact 64-bit squares and a Q30 reciprocal square root. Mirrored by
/// `unit_q30` in `noise.wgsl`.
pub fn unit_q30(v: IVec3) -> IVec3 {
    let a = [v.x.unsigned_abs(), v.y.unsigned_abs(), v.z.unsigned_abs()];
    let m = a[0] | a[1] | a[2];
    if m == 0 {
        return IVec3::ZERO;
    }
    // Largest component in [2^29, 2^30).
    let bits = 32 - m.leading_zeros();
    let a = a.map(|x| {
        if bits > 30 {
            x >> (bits - 30)
        } else {
            x << (30 - bits)
        }
    });
    let mut s = a.iter().fold(0u32, |sum, &x| sum + mul_shr(x, x, 30));
    let wide = s < Q30;
    if wide {
        s <<= 2;
    }
    let r = rsqrt_wide(s);
    let u = a.map(|x| (mul_shr(x, r, 30) << u32::from(wide)) as i32);
    IVec3::new(
        if v.x < 0 { -u[0] } else { u[0] },
        if v.y < 0 { -u[1] } else { u[1] },
        if v.z < 0 { -u[2] } else { u[2] },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No two points' noises differ by more than [`NOISE_SLOPE`] per
    /// lattice unit between them (and a few units of rounding), at every
    /// lattice size: generation skips cells a cave cannot reach with it.
    /// The largest measured is about 2.8 units per lattice unit.
    #[test]
    fn noise_slope_is_bounded() {
        let mut seed = 1u64;
        let mut next = |m: i32| {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((seed >> 33) as i64).rem_euclid(i64::from(m)) as i32
        };
        for shift in [4u32, 8, 10, 14, 20] {
            let mut worst = 0.0f64;
            for _ in 0..400_000 {
                let p = IVec3::new(
                    next(1 << 24) - (1 << 23),
                    next(1 << 24) - (1 << 23),
                    next(1 << 24) - (1 << 23),
                );
                let r = [1, 4, 32, 256, 4096][next(5) as usize];
                let d = IVec3::new(
                    next(2 * r + 1) - r,
                    next(2 * r + 1) - r,
                    next(2 * r + 1) - r,
                );
                let (a, b) = (noise(p, shift, 7), noise(p + d, shift, 7));
                let allowed = f64::from(NOISE_SLOPE) * d.as_dvec3().length()
                    / f64::from(1u32 << shift)
                    + f64::from(NOISE_ROUNDING);
                worst = worst.max(f64::from((a - b).abs()) - allowed);
            }
            assert!(worst <= 0.0, "shift {shift}: {worst} units past the bound");
        }
    }

    #[test]
    fn noise_is_bounded_and_continuous_at_lattice_points() {
        let mut max = 0;
        for x in -300..300 {
            let p = IVec3::new(x * 37, x * -11 + 5, x * 3);
            let v = noise(p, 6, 99);
            max = max.max(v.abs());
            let a = noise(IVec3::new(64 * x, 0, 0), 6, 5);
            let b = noise(IVec3::new(64 * x + 1, 0, 0), 6, 5);
            assert!((a - b).abs() < 6400, "{a} {b}");
        }
        assert!(max > 16000 && max <= 65536, "{max}");
    }

    #[test]
    fn fine_noise_matches_noise_and_never_steps() {
        for shift in [8u32, 16, 22, 26, 29] {
            // Two domain units at the gradient bound (5.3 per lattice spacing)
            // plus rounding: the 16-bit noise steps far beyond this.
            let bound =
                (2.0 * 5.3 * f64::from(FINE_ONE) / f64::from(1u32 << shift)).ceil() as i32 + 8;
            for x in -2000..2000 {
                // Mid-cell: at a lattice point the noise may be flat along x.
                let p = IVec3::new(
                    (1 << (shift - 1)) + x * 2 + 1,
                    99_614_720,
                    (1 << (shift - 2)) - 81,
                );
                let fine = noise_fine(p, shift, 7);
                let coarse = noise(p, shift, 7) * 256;
                // Agrees with the 16-bit noise to within its truncation error.
                assert!(
                    (fine - coarse).abs() <= 16 * 256,
                    "shift {shift} x {x}: fine {fine} coarse {coarse}"
                );
                let step = (noise_fine(p + IVec3::X * 2, shift, 7) - fine).abs();
                assert!(step <= bound, "shift {shift} x {x}: step {step} > {bound}");
            }
        }
        assert_eq!(mul_fine(-(1 << 27), 1 << 23), -(1 << 26));
        assert_eq!(mul_fine(12_345_678, FINE_ONE), 12_345_678);
    }

    #[test]
    fn fine_gradient_matches_value_and_differences() {
        for shift in [6u32, 10, 16, 21] {
            let step = 1i32 << shift.saturating_sub(6);
            let mut worst = 0f64;
            for x in -400..400 {
                let p = IVec3::new(x * 977 + 13, 99_614_720 + x * 311, -5_975_683 - x * 53);
                let (value, gradient) = noise_fine_grad(p, shift, 99);
                assert_eq!(value, noise_fine(p, shift, 99));
                for (axis, d) in [IVec3::X, IVec3::Y, IVec3::Z].into_iter().enumerate() {
                    let ahead = noise_fine(p + d * step, shift, 99);
                    let behind = noise_fine(p - d * step, shift, 99);
                    // Central difference per lattice spacing.
                    let numeric = f64::from(ahead - behind) / (2.0 * f64::from(step))
                        * f64::from(1u32 << shift);
                    let error = (numeric - f64::from(gradient[axis])).abs() / f64::from(FINE_ONE);
                    worst = worst.max(error);
                }
            }
            assert!(
                worst < 0.05,
                "shift {shift}: gradient off by {worst} per lattice spacing"
            );
        }
    }

    #[test]
    fn integer_helpers_match_their_definitions() {
        for t in (-200_000..200_000).step_by(137) {
            let exact = (f64::from(t) / 65_536.0 * std::f64::consts::TAU).sin() * 65_536.0;
            assert!(
                (f64::from(sin_turns(t)) - exact).abs() < 140.0,
                "sin {t}: {} vs {exact}",
                sin_turns(t)
            );
        }
    }

    #[test]
    fn noise_is_continuous_at_fine_steps() {
        for shift in [8u32, 12, 16, 19, 20, 24] {
            let mut worst = 0;
            for x in -600_000..-300_000 {
                let p = IVec3::new(x * 3 + 7, 99_614_720, -5_975_683);
                let d = (noise(p, shift, 12345) - noise(p + IVec3::X * 2, shift, 12345)).abs();
                worst = worst.max(d);
            }
            // Two domain units at the measured gradient bound (5.3 per lattice
            // spacing) plus rounding.
            let bound = (2.0 * 5.3 * 65536.0 / f64::from(1u32 << shift)).ceil() as i32 + 8;
            assert!(worst <= bound, "shift {shift}: {worst} > {bound}");
        }
    }
}
