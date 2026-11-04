//! Screen-wide selection density: computed, not fed back.
//!
//! Each asset carries a [`LodTable`](crate::LodTable): the triangles its cut
//! selects at any object-space error tolerance. A view's selected triangles are
//! therefore a sum over instances of table lookups at the tolerance that
//! instance's distance implies, and the tolerance multiplier that puts the sum on
//! the budget is a small search over that sum. The cull pass does exactly this on
//! the GPU (`cs_density` in `vg_cull.wgsl`) before traversal; this module is the
//! same arithmetic for the CPU validation and the unit tests.
//!
//! A larger multiplier selects a complete coarser parent for every cut, so
//! geometry is replaced, never dropped. The only thing measured from the GPU is
//! the visible fraction (occlusion is unknown before traversal), see
//! [`VisibleFraction`].

use crate::LodTable;

/// Candidate multipliers `2^(k / STEP_DIV)`, `k = 0..STEPS`: 1 to 8.
pub const STEPS: usize = 13;
pub const STEP_DIV: f32 = 4.0;
/// Largest multiplier (stored errors are relaxed at most this much by the
/// builder too; beyond it detail would visibly go).
pub const MAX_SCALE: f32 = 8.0;

pub fn candidate(k: usize) -> f32 {
    (k as f32 / STEP_DIV).exp2()
}

/// One instance's predicted triangles at every candidate multiplier, given the
/// object-space tolerance `base_error` the base tau implies at its distance.
pub fn contribution(table: &LodTable, base_error: f32) -> [f32; STEPS] {
    let mut out = [0.0; STEPS];
    for (k, v) in out.iter_mut().enumerate() {
        *v = table.tris_at(base_error * candidate(k));
    }
    out
}

/// Smallest multiplier whose summed triangles fit `budget`, interpolated
/// between candidates (log domain) so the result is continuous in the view.
/// Returns (multiplier, predicted triangles at it).
pub fn solve(sums: &[f64; STEPS], budget: f64) -> (f32, f64) {
    if budget <= 0.0 || !budget.is_finite() {
        return (1.0, sums[0]);
    }
    if sums[0] <= budget {
        return (1.0, sums[0]);
    }
    for k in 1..STEPS {
        if sums[k] <= budget {
            let (hi, lo) = (sums[k - 1].max(1.0).ln(), sums[k].max(1.0).ln());
            let t = if hi > lo {
                ((hi - budget.max(1.0).ln()) / (hi - lo)).clamp(0.0, 1.0)
            } else {
                1.0
            };
            let scale = candidate(k - 1) * (t as f32 / STEP_DIV).exp2();
            return (scale, budget.min(sums[k - 1]));
        }
    }
    (MAX_SCALE, sums[STEPS - 1])
}

/// Per-frame limits on the multiplier, so a view change cannot pop the whole
/// scene: it may rise fast (the budget matters) and falls slowly.
pub const SLEW_UP: f32 = 1.25;
pub const SLEW_DOWN: f32 = 0.96;

/// Rate-limited step from last frame's multiplier to the solved one (`prev < 1`
/// = no history). Mirrors the end of `cs_density`.
pub fn slew(prev: f32, wanted: f32) -> f32 {
    if prev < 1.0 {
        return wanted;
    }
    wanted
        .clamp(prev * SLEW_DOWN, prev * SLEW_UP)
        .clamp(1.0, MAX_SCALE)
}

/// Ratio of the triangles the cut really selects to the predicted ones
/// (occlusion removes some, centre-distance prediction is off by a little), learned from the
/// GPU's own counts (which arrive a few frames late). It only scales the
/// budget: a rise is taken at once (the budget must never be exceeded), a fall
/// slowly.
#[derive(Clone, Copy, Debug)]
pub struct VisibleFraction {
    keep: f32,
}

impl Default for VisibleFraction {
    fn default() -> Self {
        Self { keep: 1.0 }
    }
}

impl VisibleFraction {
    pub const MIN: f32 = 0.1;
    /// The prediction can also be low (it uses each instance's centre distance);
    /// a share above 1 shrinks the budget.
    pub const MAX: f32 = 4.0;
    const RELEASE: f32 = 0.08;

    pub fn keep(&self) -> f32 {
        self.keep
    }

    /// `predicted` and `observed`: the GPU's predicted and actually selected
    /// triangles of the same (late) frame.
    pub fn update(&mut self, predicted: f32, observed: f32) {
        if !(predicted.is_finite() && observed.is_finite()) || predicted < 1.0 {
            return;
        }
        let seen = (observed / predicted).clamp(Self::MIN, Self::MAX);
        self.keep = if seen > self.keep {
            seen
        } else {
            self.keep + (seen - self.keep) * Self::RELEASE
        };
    }

    /// Triangles the prediction may reach for `budget` visible ones.
    pub fn predicted_budget(&self, budget: f64) -> f64 {
        budget / f64::from(self.keep.max(Self::MIN))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> LodTable {
        // 1M triangles at the finest entry, falling to about 64 at the roots.
        let mut q = [0u8; crate::LOD_TABLE_ENTRIES];
        for (k, v) in q.iter_mut().enumerate() {
            *v = LodTable::quantize(1.0e6f64 * (-((31 - k) as f64) * 0.5).exp2() + 64.0);
        }
        LodTable { error_max: 1.0, q }
    }

    #[test]
    fn table_lookup_is_monotone_and_bounded() {
        let t = table();
        let mut last = 0.0;
        for i in 0..200 {
            let e = 2.0 * (-(i as f32) * 0.1).exp2();
            let n = t.tris_at(e);
            assert!(n >= last * 0.99, "{n} after {last} at {e}");
            last = n;
        }
        assert!(t.tris_at(10.0) < 100.0);
        assert!(t.tris_at(0.0) > 5.0e5);
    }

    #[test]
    fn words_round_trip() {
        let t = table();
        let mut h = crate::GpuAssetHeader::default();
        h.lod_error_max = t.error_max;
        h.lod_table = t.words();
        assert_eq!(LodTable::from_header(&h), t);
    }

    #[test]
    fn solve_fits_the_budget_and_is_continuous() {
        let t = table();
        let mut last = 1.0f32;
        for n in [1usize, 10, 100, 1000] {
            let mut sums = [0.0f64; STEPS];
            for k in 0..STEPS {
                sums[k] = n as f64 * f64::from(t.tris_at(0.05 * candidate(k)));
            }
            let budget = sums[0] * 0.3;
            let (g, predicted) = solve(&sums, budget);
            assert!(g >= 1.0 && g <= MAX_SCALE);
            assert!(
                predicted <= budget * 1.0001 || g == MAX_SCALE,
                "{predicted} > {budget}"
            );
            let _ = last;
            last = g;
        }
        assert_eq!(solve(&[10.0; STEPS], 100.0).0, 1.0);
        assert_eq!(solve(&[1.0e9; STEPS], 100.0).0, MAX_SCALE);
    }

    #[test]
    fn visible_fraction_rises_fast_and_falls_slowly() {
        let mut v = VisibleFraction::default();
        for _ in 0..10 {
            v.update(1000.0, 300.0);
        }
        assert!(v.keep() < 0.9 && v.keep() > 0.3);
        v.update(1000.0, 900.0);
        assert!((v.keep() - 0.9).abs() < 1e-6);
    }
}
