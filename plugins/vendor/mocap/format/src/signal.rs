//! Signal tools over take columns: smoothing, gap fill, quaternion hygiene.

use std::f32::consts::TAU;

/// The One-Euro filter (Casiez et al.): a low-pass whose cutoff rises with the
/// signal's speed, so slow motion is smooth and fast motion lags little. The
/// standard filter for live mocap.
#[derive(Clone, Debug)]
pub struct OneEuro {
    /// Cutoff in Hz at rest: lower is smoother.
    pub min_cutoff: f32,
    /// How fast the cutoff rises with speed: higher is more responsive.
    pub beta: f32,
    /// Cutoff for the speed estimate, Hz.
    pub derivative_cutoff: f32,
    previous: Option<(f32, f32)>,
}

fn smoothing(cutoff: f32, dt: f32) -> f32 {
    let tau = 1.0 / (TAU * cutoff.max(1e-3));
    1.0 / (1.0 + tau / dt.max(1e-6))
}

impl OneEuro {
    /// A filter with the given tuning.
    pub fn new(min_cutoff: f32, beta: f32) -> Self {
        Self { min_cutoff, beta, derivative_cutoff: 1.0, previous: None }
    }

    /// Forgets history (a gap, a seek).
    pub fn reset(&mut self) {
        self.previous = None;
    }

    /// Filters `x` sampled `dt` seconds after the last call. NaN passes
    /// through and resets the filter.
    pub fn filter(&mut self, x: f32, dt: f32) -> f32 {
        if !x.is_finite() {
            self.previous = None;
            return x;
        }
        let Some((previous_x, previous_dx)) = self.previous else {
            self.previous = Some((x, 0.0));
            return x;
        };
        let raw_dx = (x - previous_x) / dt.max(1e-6);
        let dx = previous_dx + smoothing(self.derivative_cutoff, dt) * (raw_dx - previous_dx);
        let cutoff = self.min_cutoff + self.beta * dx.abs();
        let filtered = previous_x + smoothing(cutoff, dt) * (x - previous_x);
        self.previous = Some((filtered, dx));
        filtered
    }
}

/// A One-Euro filter on a quaternion: components are filtered on the hemisphere
/// of the previous output and the result is renormalised.
#[derive(Clone, Debug)]
pub struct QuatFilter {
    axes: [OneEuro; 4],
    last: Option<[f32; 4]>,
}

impl QuatFilter {
    /// A filter with the given tuning.
    pub fn new(min_cutoff: f32, beta: f32) -> Self {
        Self { axes: std::array::from_fn(|_| OneEuro::new(min_cutoff, beta)), last: None }
    }

    /// Filters `q` (x, y, z, w).
    pub fn filter(&mut self, q: [f32; 4], dt: f32) -> [f32; 4] {
        if q.iter().any(|v| !v.is_finite()) {
            self.last = None;
            self.axes.iter_mut().for_each(OneEuro::reset);
            return q;
        }
        let aligned = match self.last {
            Some(last) if dot(last, q) < 0.0 => q.map(|v| -v),
            _ => q,
        };
        let mut out = [0.0; 4];
        for (i, axis) in self.axes.iter_mut().enumerate() {
            out[i] = axis.filter(aligned[i], dt);
        }
        let out = normalise(out);
        self.last = Some(out);
        out
    }
}

fn dot(a: [f32; 4], b: [f32; 4]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// The conjugate (inverse of a unit quaternion).
pub fn conjugate(q: [f32; 4]) -> [f32; 4] {
    [-q[0], -q[1], -q[2], q[3]]
}

/// The Hamilton product `a * b` (apply `b` first, then `a`), as (x, y, z, w).
pub fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

/// Normalised lerp from `a` to `b` on the shorter arc.
pub fn nlerp(a: [f32; 4], b: [f32; 4], f: f32) -> [f32; 4] {
    let b = if dot(a, b) < 0.0 { b.map(|v| -v) } else { b };
    normalise(std::array::from_fn(|i| a[i] + (b[i] - a[i]) * f))
}

/// `q` scaled to unit length (identity for a zero quaternion).
pub fn normalise(q: [f32; 4]) -> [f32; 4] {
    let length = dot(q, q).sqrt();
    if length > 1e-9 {
        q.map(|v| v / length)
    } else {
        [0.0, 0.0, 0.0, 1.0]
    }
}

/// Flips quaternions so each is on the hemisphere of the one before, which
/// makes a column pair of the same rotation continuous. Gaps are skipped.
pub fn make_continuous(quats: &mut [[f32; 4]]) {
    let mut last: Option<[f32; 4]> = None;
    for q in quats.iter_mut() {
        if q.iter().any(|v| !v.is_finite()) {
            continue;
        }
        if last.is_some_and(|l| dot(l, *q) < 0.0) {
            *q = q.map(|v| -v);
        }
        last = Some(*q);
    }
}

/// Linearly fills runs of NaN no longer than `max_gap` that have a finite
/// sample on both sides; returns how many samples were filled.
pub fn fill_gaps(column: &mut [f32], max_gap: usize) -> usize {
    let mut filled = 0;
    let mut i = 0;
    while i < column.len() {
        if column[i].is_finite() {
            i += 1;
            continue;
        }
        let start = i;
        while i < column.len() && !column[i].is_finite() {
            i += 1;
        }
        if start == 0 || i == column.len() || i - start > max_gap {
            continue;
        }
        let (a, b) = (column[start - 1], column[i]);
        for (k, slot) in column[start..i].iter_mut().enumerate() {
            *slot = a + (b - a) * (k + 1) as f32 / (i - start + 1) as f32;
        }
        filled += i - start;
    }
    filled
}

/// The column's value at fractional frame `t`; NaN when either neighbour is
/// a gap or `t` is outside the column.
pub fn sample_linear(column: &[f32], t: f64) -> f32 {
    if column.is_empty() || t < 0.0 || t > (column.len() - 1) as f64 {
        return f32::NAN;
    }
    let i = t.floor() as usize;
    let f = (t - i as f64) as f32;
    let a = column[i];
    let b = column[(i + 1).min(column.len() - 1)];
    a + (b - a) * f
}

/// The rotation at fractional frame `t` from four component columns:
/// normalised lerp on the shorter arc.
pub fn sample_quat(columns: [&[f32]; 4], t: f64) -> [f32; 4] {
    let a = columns.map(|c| sample_linear(c, t.floor()));
    let b = columns.map(|c| sample_linear(c, t.ceil()));
    let f = (t - t.floor()) as f32;
    let b = if dot(a, b) < 0.0 { b.map(|v| -v) } else { b };
    normalise(std::array::from_fn(|i| a[i] + (b[i] - a[i]) * f))
}
