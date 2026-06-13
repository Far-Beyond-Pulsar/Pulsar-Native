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
/// `liveness_words.len()` must equal `(len + 63) / 64` — the words covering
/// exactly rows `0..len` (not the full page capacity).
///
/// Writes `out[r] = r` on hit, `NULL_ROW` on miss/dead, for `r in 0..len`.
/// Returns the hit count. `out.len()` must be >= `len`.
#[inline]
pub fn aabb_scan(q: &QueryBounds, cols: &Columns, liveness_words: &[u64], len: usize, out: &mut [u32]) -> u32 {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            // SAFETY: guarded by the runtime feature check.
            return unsafe { aabb_scan_avx2(q, cols, liveness_words, len, out) };
        }
    }
    aabb_scan_scalar(q, cols, liveness_words, len, out)
}

/// AVX2 backend for the AABB scan, processing 8 rows per iteration.
///
/// Produces bit-identical `out` buffers and hit counts to
/// [`aabb_scan_scalar`]. Uses ordered comparison predicates so a NaN bound
/// yields false, matching the scalar `<=`/`>=` reference.
///
/// # Safety
/// The caller must ensure the `avx2` target feature is available at runtime
/// (verify with `is_x86_feature_detected!("avx2")`). Both the [`aabb_scan`]
/// dispatcher and the property test guard the call this way.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub(crate) unsafe fn aabb_scan_avx2(
    q: &QueryBounds,
    cols: &Columns,
    liveness_words: &[u64],
    len: usize,
    out: &mut [u32],
) -> u32 {
    use std::arch::x86_64::*;
    debug_assert!(out.len() >= len);
    debug_assert_eq!(liveness_words.len(), len.div_ceil(64), "liveness_words must cover exactly rows 0..len");
    debug_assert!(cols.min_x.len() >= len && cols.max_x.len() >= len, "x columns shorter than len");
    debug_assert!(cols.min_y.len() >= len && cols.max_y.len() >= len, "y columns shorter than len");
    debug_assert!(cols.min_z.len() >= len && cols.max_z.len() >= len, "z columns shorter than len");

    // Broadcast query bounds. Ordered comparisons (_CMP_*_OQ) so a NaN bound
    // yields false — bit-identical to the scalar `<=`/`>=` reference.
    let qmaxx = _mm256_set1_ps(q.max[0]);
    let qminx = _mm256_set1_ps(q.min[0]);
    let qmaxy = _mm256_set1_ps(q.max[1]);
    let qminy = _mm256_set1_ps(q.min[1]);
    let qmaxz = _mm256_set1_ps(q.max[2]);
    let qminz = _mm256_set1_ps(q.min[2]);

    let mut hits = 0u32;
    let mut row = 0usize;
    // Process 8 rows per iteration.
    while row + 8 <= len {
        let minx = _mm256_loadu_ps(cols.min_x.as_ptr().add(row));
        let maxx = _mm256_loadu_ps(cols.max_x.as_ptr().add(row));
        let miny = _mm256_loadu_ps(cols.min_y.as_ptr().add(row));
        let maxy = _mm256_loadu_ps(cols.max_y.as_ptr().add(row));
        let minz = _mm256_loadu_ps(cols.min_z.as_ptr().add(row));
        let maxz = _mm256_loadu_ps(cols.max_z.as_ptr().add(row));

        // box.min <= q.max  AND  box.max >= q.min, per axis (ordered).
        let mx = _mm256_and_ps(_mm256_cmp_ps(minx, qmaxx, _CMP_LE_OQ), _mm256_cmp_ps(maxx, qminx, _CMP_GE_OQ));
        let my = _mm256_and_ps(_mm256_cmp_ps(miny, qmaxy, _CMP_LE_OQ), _mm256_cmp_ps(maxy, qminy, _CMP_GE_OQ));
        let mz = _mm256_and_ps(_mm256_cmp_ps(minz, qmaxz, _CMP_LE_OQ), _mm256_cmp_ps(maxz, qminz, _CMP_GE_OQ));
        let geo = _mm256_and_ps(_mm256_and_ps(mx, my), mz);
        // 8-bit mask, one bit per lane (1 = geometric hit).
        let mut mask = _mm256_movemask_ps(geo) as u32;
        // AND in liveness for these 8 rows.
        let lw = liveness_words[row / 64];
        let live8 = ((lw >> (row % 64)) & 0xFF) as u32;
        mask &= live8;

        // POPCNT the hit count once, then scatter row indices per lane.
        hits += mask.count_ones();
        for lane in 0..8usize {
            let r = row + lane;
            out[r] = if (mask >> lane) & 1 != 0 { r as u32 } else { NULL_ROW };
        }
        row += 8;
    }
    // Scalar tail. Because pages are 64-aligned and we step by 8, row%64 ∈
    // {0,8,...,56} in the SIMD loop, so the 8-bit liveness window never crosses
    // a word boundary above; the tail handles the remaining < 8 rows.
    while row < len {
        let live = liveness_words[row / 64] & (1u64 << (row % 64)) != 0;
        let visible = cols.min_x[row] <= q.max[0]
            && cols.max_x[row] >= q.min[0]
            && cols.min_y[row] <= q.max[1]
            && cols.max_y[row] >= q.min[1]
            && cols.min_z[row] <= q.max[2]
            && cols.max_z[row] >= q.min[2]
            && live;
        out[row] = if visible { hits += 1; row as u32 } else { NULL_ROW };
        row += 1;
    }
    hits
}

/// Scalar reference. The §8.2 predicate with ordered IEEE comparisons,
/// liveness ANDed last. M1b SIMD arms must match this bit-for-bit.
pub(crate) fn aabb_scan_scalar(
    q: &QueryBounds,
    cols: &Columns,
    liveness_words: &[u64],
    len: usize,
    out: &mut [u32],
) -> u32 {
    debug_assert!(out.len() >= len);
    debug_assert_eq!(liveness_words.len(), len.div_ceil(64), "liveness_words must cover exactly rows 0..len");
    debug_assert!(cols.min_x.len() >= len && cols.max_x.len() >= len, "x columns shorter than len");
    debug_assert!(cols.min_y.len() >= len && cols.max_y.len() >= len, "y columns shorter than len");
    debug_assert!(cols.min_z.len() >= len && cols.max_z.len() >= len, "z columns shorter than len");
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

/// Six frustum planes, each `[nx, ny, nz, d]` with inward normal; a point `p`
/// is inside the plane iff `nx*px + ny*py + nz*pz + d >= 0`.
#[derive(Copy, Clone)]
pub struct FrustumPlanes {
    pub planes: [[f32; 4]; 6],
}

/// Scalar frustum scan. A box passes iff, for every plane, its positive
/// vertex (the corner farthest along the inward normal) is inside. Writes
/// `out[r] = r` on pass, `NULL_ROW` on cull/dead. Returns the pass count.
/// `liveness_words.len()` must equal `(len + 63) / 64`.
pub(crate) fn frustum_scan_scalar(
    f: &FrustumPlanes,
    cols: &Columns,
    liveness_words: &[u64],
    len: usize,
    out: &mut [u32],
) -> u32 {
    debug_assert!(out.len() >= len);
    debug_assert_eq!(liveness_words.len(), len.div_ceil(64), "liveness_words must cover exactly rows 0..len");
    debug_assert!(cols.min_x.len() >= len && cols.max_x.len() >= len, "x columns shorter than len");
    debug_assert!(cols.min_y.len() >= len && cols.max_y.len() >= len, "y columns shorter than len");
    debug_assert!(cols.min_z.len() >= len && cols.max_z.len() >= len, "z columns shorter than len");
    let mut hits = 0u32;
    for row in 0..len {
        let live = liveness_words[row / 64] & (1u64 << (row % 64)) != 0;
        let bmin = [cols.min_x[row], cols.min_y[row], cols.min_z[row]];
        let bmax = [cols.max_x[row], cols.max_y[row], cols.max_z[row]];
        let mut inside = live;
        let mut p = 0;
        // Short-circuit on first failing plane (scalar only; the AVX2 arm in
        // Task 6 evaluates all 6 planes and ANDs the masks — result is
        // identical, plane order irrelevant).
        while inside && p < 6 {
            let pl = f.planes[p];
            // Positive vertex: pick max-projection corner per axis.
            let px = if pl[0] >= 0.0 { bmax[0] } else { bmin[0] };
            let py = if pl[1] >= 0.0 { bmax[1] } else { bmin[1] };
            let pz = if pl[2] >= 0.0 { bmax[2] } else { bmin[2] };
            if pl[0] * px + pl[1] * py + pl[2] * pz + pl[3] < 0.0 {
                inside = false; // positive vertex behind plane → fully outside
            }
            p += 1;
        }
        out[row] = if inside { hits += 1; row as u32 } else { NULL_ROW };
    }
    hits
}

/// Runtime-dispatched frustum scan (scalar for now; AVX2 arm in Task 6).
#[inline]
pub fn frustum_scan(f: &FrustumPlanes, cols: &Columns, liveness_words: &[u64], len: usize, out: &mut [u32]) -> u32 {
    frustum_scan_scalar(f, cols, liveness_words, len, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frustum_scalar_keeps_inside_culls_outside_and_dead() {
        // 3 rows: inside box, box outside the x<=10 plane, dead box.
        let min_x = [0.0f32, 100.0, 0.0]; let max_x = [1.0f32, 101.0, 1.0];
        let min_y = [0.0f32; 3]; let max_y = [1.0f32; 3];
        let min_z = [0.0f32; 3]; let max_z = [1.0f32; 3];
        let live = 0b011u64; // rows 0,1 live; row 2 dead
        let planes = [
            [1.0, 0.0, 0.0, 10.0], [-1.0, 0.0, 0.0, 10.0],
            [0.0, 1.0, 0.0, 10.0], [0.0, -1.0, 0.0, 10.0],
            [0.0, 0.0, 1.0, 10.0], [0.0, 0.0, -1.0, 10.0],
        ];
        let f = FrustumPlanes { planes };
        let cols = Columns { min_x: &min_x, max_x: &max_x, min_y: &min_y, max_y: &max_y, min_z: &min_z, max_z: &max_z };
        let mut out = [0u32; 3];
        let hits = frustum_scan_scalar(&f, &cols, &[live], 3, &mut out);
        // row 0 inside → 0; row 1 (x=100) culled by x<=10 → NULL; row 2 dead → NULL.
        assert_eq!(out, [0, crate::registry::NULL_ROW, crate::registry::NULL_ROW]);
        assert_eq!(hits, 1);
    }

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

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn avx2_matches_scalar_bit_for_bit() {
        if !is_x86_feature_detected!("avx2") {
            eprintln!("AVX2 not available on this host; skipping");
            return;
        }
        use rand::{Rng, SeedableRng};
        let mut rng = rand::rngs::StdRng::seed_from_u64(0xA7F2 ^ 0x5CEDB);
        for _ in 0..200 {
            let len = rng.gen_range(0..=300usize);
            let gen_col = |rng: &mut rand::rngs::StdRng| (0..len).map(|_| rng.gen_range(-100.0f32..100.0)).collect::<Vec<_>>();
            let min_x = gen_col(&mut rng); let max_x: Vec<f32> = min_x.iter().map(|&m| m + rng.gen_range(0.0..10.0)).collect();
            let min_y = gen_col(&mut rng); let max_y: Vec<f32> = min_y.iter().map(|&m| m + rng.gen_range(0.0..10.0)).collect();
            let min_z = gen_col(&mut rng); let max_z: Vec<f32> = min_z.iter().map(|&m| m + rng.gen_range(0.0..10.0)).collect();
            let n_words = (len + 63) / 64;
            let words: Vec<u64> = (0..n_words).map(|_| rng.gen::<u64>()).collect();
            let q = QueryBounds {
                min: [rng.gen_range(-100.0..100.0), rng.gen_range(-100.0..100.0), rng.gen_range(-100.0..100.0)],
                max: [rng.gen_range(-100.0..100.0), rng.gen_range(-100.0..100.0), rng.gen_range(-100.0..100.0)],
            };
            let cols = Columns { min_x: &min_x, max_x: &max_x, min_y: &min_y, max_y: &max_y, min_z: &min_z, max_z: &max_z };
            let mut out_s = vec![0u32; len];
            let mut out_v = vec![0u32; len];
            let hs = aabb_scan_scalar(&q, &cols, &words, len, &mut out_s);
            // SAFETY: guarded by the runtime feature check above.
            let hv = unsafe { aabb_scan_avx2(&q, &cols, &words, len, &mut out_v) };
            assert_eq!(out_s, out_v, "AVX2 diverged from scalar at len={len}");
            assert_eq!(hs, hv);
        }
    }
}
