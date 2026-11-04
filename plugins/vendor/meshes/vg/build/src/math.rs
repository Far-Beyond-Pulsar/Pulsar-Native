//! Small vector / sphere / Morton helpers.
//!
//! Everything here is plain `[f32; 3]` math so the builder stays free of a
//! math-crate dependency and its float behaviour is easy to reason about for
//! the determinism guarantee.

pub type V3 = [f32; 3];

#[inline]
pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
#[inline]
pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
#[inline]
pub fn scale(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
#[inline]
pub fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
#[inline]
pub fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
#[inline]
pub fn length(a: V3) -> f32 {
    dot(a, a).sqrt()
}
#[inline]
pub fn dist(a: V3, b: V3) -> f32 {
    length(sub(a, b))
}
/// Normalize; a zero vector stays zero (callers treat that as "no direction").
#[inline]
pub fn normalize(a: V3) -> V3 {
    let l = length(a);
    if l > 0.0 {
        scale(a, 1.0 / l)
    } else {
        [0.0; 3]
    }
}

/// Bit-exact position key. `+ 0.0` folds `-0.0` into `0.0` so two vertices
/// that compare equal as floats are equal as keys (the DAG welds by position).
#[inline]
pub fn pos_key(p: V3) -> [u32; 3] {
    [
        (p[0] + 0.0).to_bits(),
        (p[1] + 0.0).to_bits(),
        (p[2] + 0.0).to_bits(),
    ]
}

#[inline]
pub fn edge_key(a: u32, b: u32) -> u64 {
    let (lo, hi) = if a < b { (a, b) } else { (b, a) };
    ((lo as u64) << 32) | hi as u64
}

// ── spheres ────────────────────────────────────────────────────────────────

/// Slack applied whenever a sphere is grown so f32 rounding can never leave a
/// contained sphere poking out. Containment is an invariant the runtime relies
/// on, so we err on the side of loose.
const SPHERE_SLACK: f32 = 1.0 + 4e-6;

/// Smallest sphere containing both (conservative: slightly inflated).
pub fn sphere_union(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let ca = [a[0], a[1], a[2]];
    let cb = [b[0], b[1], b[2]];
    let d = dist(ca, cb);
    if d + b[3] <= a[3] {
        return a;
    }
    if d + a[3] <= b[3] {
        return b;
    }
    let r = (d + a[3] + b[3]) * 0.5;
    let t = if d > 0.0 { (r - a[3]) / d } else { 0.0 };
    let c = add(ca, scale(sub(cb, ca), t));
    [c[0], c[1], c[2], r * SPHERE_SLACK + 1e-12]
}

/// Does `outer` contain `inner`, allowing a relative tolerance for float noise.
pub fn sphere_contains(outer: [f32; 4], inner: [f32; 4], tol: f32) -> bool {
    let d = dist(
        [outer[0], outer[1], outer[2]],
        [inner[0], inner[1], inner[2]],
    );
    d + inner[3] <= outer[3] * (1.0 + tol) + tol * 1e-3
}

/// AABB-centred bounding sphere of a point set (not minimal, but cheap and
/// deterministic).
pub fn bounding_sphere<I: Iterator<Item = V3> + Clone>(points: I) -> [f32; 4] {
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for p in points.clone() {
        for i in 0..3 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    if lo[0] > hi[0] {
        return [0.0; 4];
    }
    let c = scale(add(lo, hi), 0.5);
    let mut r2 = 0.0f32;
    for p in points {
        let d = sub(p, c);
        r2 = r2.max(dot(d, d));
    }
    [c[0], c[1], c[2], r2.sqrt() * SPHERE_SLACK]
}

// ── Morton ─────────────────────────────────────────────────────────────────

fn expand_bits(mut v: u64) -> u64 {
    v &= 0x1f_ffff;
    v = (v | v << 32) & 0x1f00000000ffff;
    v = (v | v << 16) & 0x1f0000ff0000ff;
    v = (v | v << 8) & 0x100f00f00f00f00f;
    v = (v | v << 4) & 0x10c30c30c30c30c3;
    v = (v | v << 2) & 0x1249249249249249;
    v
}

/// Frame (origin, inverse uniform extent) that maps a point set into [0,1]^3.
pub fn morton_frame<I: Iterator<Item = V3>>(points: I) -> (V3, f32) {
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for p in points {
        for i in 0..3 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    if lo[0] > hi[0] {
        return ([0.0; 3], 1.0);
    }
    let ext = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(hi[2] - lo[2]);
    (lo, if ext > 0.0 { 1.0 / ext } else { 1.0 })
}

pub fn morton3(p: V3, frame: (V3, f32)) -> u64 {
    let q = |i: usize| (((p[i] - frame.0[i]) * frame.1).clamp(0.0, 1.0) * 2_097_151.0) as u64;
    expand_bits(q(0)) | expand_bits(q(1)) << 1 | expand_bits(q(2)) << 2
}
