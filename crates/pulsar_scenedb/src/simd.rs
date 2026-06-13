use crate::registry::NULL_ROW;

/// Query AABB in the kernel's own scalar layout (min/max per axis).
#[derive(Copy, Clone)]
pub struct QueryBounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// Borrowed bounds columns for one cell, sliced to the row count.
pub struct Columns<'a> {
    pub min_x: &'a [f32],
    pub max_x: &'a [f32],
    pub min_y: &'a [f32],
    pub max_y: &'a [f32],
    pub min_z: &'a [f32],
    pub max_z: &'a [f32],
}

/// Runtime-dispatched AABB scan. Selects the best available backend; all
/// backends produce bit-identical `out` buffers (the scalar arm is the
/// reference). `liveness_words` is the raw `LivenessMask` word slice;
/// `len` is the physical row count.
///
/// Writes `out[r] = r` on hit, `NULL_ROW` on miss/dead, for `r in 0..len`.
/// Returns the hit count. `out.len()` must be >= `len`.
#[inline]
pub fn aabb_scan(q: &QueryBounds, cols: &Columns, liveness_words: &[u64], len: usize, out: &mut [u32]) -> u32 {
    // Scalar-only in this task. Task 4 adds the AVX2 branch here; AVX-512/NEON
    // remain routed to scalar (correct, not yet optimized — scoping note 2).
    aabb_scan_scalar(q, cols, liveness_words, len, out)
}

/// Scalar reference. The §8.2 predicate with ordered IEEE comparisons,
/// liveness ANDed last. M1b SIMD arms must match this bit-for-bit.
pub fn aabb_scan_scalar(
    q: &QueryBounds,
    cols: &Columns,
    liveness_words: &[u64],
    len: usize,
    out: &mut [u32],
) -> u32 {
    debug_assert!(out.len() >= len);
    let mut hits = 0u32;
    for row in 0..len {
        let live = liveness_words[row / 64] & (1u64 << (row % 64)) != 0;
        let visible = cols.min_x[row] <= q.max[0]
            && cols.max_x[row] >= q.min[0]
            && cols.min_y[row] <= q.max[1]
            && cols.max_y[row] >= q.min[1]
            && cols.min_z[row] <= q.max[2]
            && cols.max_z[row] >= q.min[2]
            && live;
        out[row] = if visible {
            hits += 1;
            row as u32
        } else {
            NULL_ROW
        };
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_arm_matches_manual_predicate() {
        // Six columns, 5 rows. Build by hand and compare against the kernel.
        let min_x = [0.0f32, 10.0, 0.5, -5.0, 100.0];
        let max_x = [1.0f32, 11.0, 2.0, -4.0, 101.0];
        let min_y = [0.0f32; 5];
        let max_y = [1.0f32; 5];
        let min_z = [0.0f32; 5];
        let max_z = [1.0f32; 5];
        let live = 0b11111u64; // all live
        let q = QueryBounds { min: [0.0, 0.0, 0.0], max: [3.0, 3.0, 3.0] };
        let cols = Columns { min_x: &min_x, max_x: &max_x, min_y: &min_y, max_y: &max_y, min_z: &min_z, max_z: &max_z };
        let mut out = [0u32; 5];
        let hits = aabb_scan_scalar(&q, &cols, &[live], 5, &mut out);
        // rows 0 (0..1), 2 (0.5..2) intersect [0,3]; rows 1,3,4 don't.
        assert_eq!(out, [0, crate::registry::NULL_ROW, 2, crate::registry::NULL_ROW, crate::registry::NULL_ROW]);
        assert_eq!(hits, 2);
    }

    #[test]
    fn dead_rows_excluded_by_liveness_word() {
        let min_x = [0.0f32, 0.0];
        let max_x = [1.0f32, 1.0];
        let min_y = [0.0f32; 2];
        let max_y = [1.0f32; 2];
        let min_z = [0.0f32; 2];
        let max_z = [1.0f32; 2];
        let live = 0b01u64; // row 0 live, row 1 dead
        let q = QueryBounds { min: [0.0; 3], max: [1.0; 3] };
        let cols = Columns { min_x: &min_x, max_x: &max_x, min_y: &min_y, max_y: &max_y, min_z: &min_z, max_z: &max_z };
        let mut out = [0u32; 2];
        let hits = aabb_scan_scalar(&q, &cols, &[live], 2, &mut out);
        assert_eq!(out, [0, crate::registry::NULL_ROW]);
        assert_eq!(hits, 1);
    }
}
