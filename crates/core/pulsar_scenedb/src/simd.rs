use crate::registry::NULL_ROW;

/// Query AABB in the kernel's own scalar layout (min/max per axis).
#[derive(Copy, Clone)]
pub(crate) struct QueryBounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// Borrowed bounds columns for one cell, sliced to the row count.
pub(crate) struct Columns<'a> {
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
pub(crate) fn aabb_scan(q: &QueryBounds, cols: &Columns, liveness_words: &[u64], len: usize, out: &mut [u32]) -> u32 {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            // SAFETY: guarded by the runtime feature check.
            return unsafe { aabb_scan_avx2(q, cols, liveness_words, len, out) };
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: NEON is mandatory on aarch64 (ARMv8-A Advanced SIMD).
        return unsafe { aabb_scan_neon(q, cols, liveness_words, len, out) };
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

/// NEON backend for the AABB scan, processing 4 rows per iteration
/// (128-bit NEON registers, each holding 4 × f32).
///
/// Produces bit-identical `out` buffers and hit counts to
/// [`aabb_scan_scalar`]. Uses ordered comparison predicates (`vcle`/`vcge`)
/// so a NaN bound yields false, matching the scalar `<=`/`>=` reference.
///
/// # Safety
/// Caller must ensure the `neon` target feature is available (mandatory on
/// aarch64 — ARMv8-A Advanced SIMD is always present).
#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
pub(crate) unsafe fn aabb_scan_neon(
    q: &QueryBounds,
    cols: &Columns,
    liveness_words: &[u64],
    len: usize,
    out: &mut [u32],
) -> u32 {
    use std::arch::aarch64::*;
    debug_assert!(out.len() >= len);
    debug_assert_eq!(liveness_words.len(), len.div_ceil(64), "liveness_words must cover exactly rows 0..len");
    debug_assert!(cols.min_x.len() >= len && cols.max_x.len() >= len, "x columns shorter than len");
    debug_assert!(cols.min_y.len() >= len && cols.max_y.len() >= len, "y columns shorter than len");
    debug_assert!(cols.min_z.len() >= len && cols.max_z.len() >= len, "z columns shorter than len");

    let qmaxx = vdupq_n_f32(q.max[0]);
    let qminx = vdupq_n_f32(q.min[0]);
    let qmaxy = vdupq_n_f32(q.max[1]);
    let qminy = vdupq_n_f32(q.min[1]);
    let qmaxz = vdupq_n_f32(q.max[2]);
    let qminz = vdupq_n_f32(q.min[2]);

    let mut hits = 0u32;
    let mut row = 0usize;
    // Process 4 rows per iteration (128-bit NEON).
    while row + 4 <= len {
        let minx = vld1q_f32(cols.min_x.as_ptr().add(row));
        let maxx = vld1q_f32(cols.max_x.as_ptr().add(row));
        let miny = vld1q_f32(cols.min_y.as_ptr().add(row));
        let maxy = vld1q_f32(cols.max_y.as_ptr().add(row));
        let minz = vld1q_f32(cols.min_z.as_ptr().add(row));
        let maxz = vld1q_f32(cols.max_z.as_ptr().add(row));

        // box.min <= q.max  AND  box.max >= q.min, per axis (ordered).
        let mx = vandq_u32(vcleq_f32(minx, qmaxx), vcgeq_f32(maxx, qminx));
        let my = vandq_u32(vcleq_f32(miny, qmaxy), vcgeq_f32(maxy, qminy));
        let mz = vandq_u32(vcleq_f32(minz, qmaxz), vcgeq_f32(maxz, qminz));
        let geo = vandq_u32(vandq_u32(mx, my), mz);

        // 4-bit mask, one bit per lane (1 = geometric hit).
        let shr = vshrq_n_u32(geo, 31);
        let mut mask = vgetq_lane_u32(shr, 0)
                     | (vgetq_lane_u32(shr, 1) << 1)
                     | (vgetq_lane_u32(shr, 2) << 2)
                     | (vgetq_lane_u32(shr, 3) << 3);
        // AND in liveness for these 4 rows.
        let lw = liveness_words[row / 64];
        let live4 = ((lw >> (row % 64)) & 0b1111) as u32;
        mask &= live4;

        hits += mask.count_ones();
        for lane in 0..4usize {
            let r = row + lane;
            out[r] = if (mask >> lane) & 1 != 0 { r as u32 } else { NULL_ROW };
        }
        row += 4;
    }
    // Scalar tail. Same reasoning as AVX2: row%64 ∈ {0,4,...,60} in the SIMD
    // loop, never crossing a word boundary; tail handles the remaining < 4 rows.
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
pub(crate) struct FrustumPlanes {
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

/// AVX2 backend for the frustum scan, processing 8 rows per iteration.
///
/// Produces bit-identical `out` buffers and pass counts to
/// [`frustum_scan_scalar`]. The dot-product association
/// `((nx*px + ny*py) + nz*pz) + d` and ordered (`_CMP_GE_OQ`) comparisons
/// exactly mirror the scalar reference; no FMA contraction is used so the
/// rounding matches.
///
/// # Safety
/// Caller must ensure the `avx2` target feature is available (the dispatcher
/// and tests guard with `is_x86_feature_detected!("avx2")`).
///
/// The dot product MUST use separate `_mm256_mul_ps`/`_mm256_add_ps` (never
/// `_mm256_fmadd_ps`): FMA's single rounding would diverge from the scalar
/// reference's mul-then-add and break the bit-for-bit contract.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub(crate) unsafe fn frustum_scan_avx2(
    f: &FrustumPlanes,
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
    let mut hits = 0u32;
    let mut row = 0usize;
    while row + 8 <= len {
        let minx = _mm256_loadu_ps(cols.min_x.as_ptr().add(row));
        let maxx = _mm256_loadu_ps(cols.max_x.as_ptr().add(row));
        let miny = _mm256_loadu_ps(cols.min_y.as_ptr().add(row));
        let maxy = _mm256_loadu_ps(cols.max_y.as_ptr().add(row));
        let minz = _mm256_loadu_ps(cols.min_z.as_ptr().add(row));
        let maxz = _mm256_loadu_ps(cols.max_z.as_ptr().add(row));
        // inside accumulator: all-ones, ANDed by each plane's "inside" mask.
        let mut inside = _mm256_castsi256_ps(_mm256_set1_epi32(-1));
        for pl in f.planes.iter() {
            let nx = _mm256_set1_ps(pl[0]);
            let ny = _mm256_set1_ps(pl[1]);
            let nz = _mm256_set1_ps(pl[2]);
            let d = _mm256_set1_ps(pl[3]);
            // positive vertex per axis: nx>=0 ? maxx : minx (blend on the sign
            // mask of the plane normal component, ordered GE matching scalar).
            let selx = _mm256_cmp_ps(nx, _mm256_setzero_ps(), _CMP_GE_OQ);
            let sely = _mm256_cmp_ps(ny, _mm256_setzero_ps(), _CMP_GE_OQ);
            let selz = _mm256_cmp_ps(nz, _mm256_setzero_ps(), _CMP_GE_OQ);
            let px = _mm256_blendv_ps(minx, maxx, selx);
            let py = _mm256_blendv_ps(miny, maxy, sely);
            let pz = _mm256_blendv_ps(minz, maxz, selz);
            // dot = ((nx*px + ny*py) + nz*pz) + d
            // Association MUST match the scalar reference exactly —
            // `((a+b)+c)+d` — because f32 addition is not associative and the
            // property test asserts bit-identical results. Separate mul+add
            // (no FMA contraction) matches the scalar path's rounding.
            let dot = _mm256_add_ps(
                _mm256_add_ps(
                    _mm256_add_ps(_mm256_mul_ps(nx, px), _mm256_mul_ps(ny, py)),
                    _mm256_mul_ps(nz, pz),
                ),
                d,
            );
            // inside-this-plane iff dot >= 0 (ordered).
            let inplane = _mm256_cmp_ps(dot, _mm256_setzero_ps(), _CMP_GE_OQ);
            inside = _mm256_and_ps(inside, inplane);
        }
        let mut mask = _mm256_movemask_ps(inside) as u32;
        let lw = liveness_words[row / 64];
        mask &= ((lw >> (row % 64)) & 0xFF) as u32;
        hits += mask.count_ones();
        for lane in 0..8usize {
            let r = row + lane;
            out[r] = if (mask >> lane) & 1 != 0 { r as u32 } else { NULL_ROW };
        }
        row += 8;
    }
    // Scalar tail (identical predicate to frustum_scan_scalar). Its short-circuit
    // is equivalent to the SIMD body's all-planes AND — order-independent.
    while row < len {
        let live = liveness_words[row / 64] & (1u64 << (row % 64)) != 0;
        let bmin = [cols.min_x[row], cols.min_y[row], cols.min_z[row]];
        let bmax = [cols.max_x[row], cols.max_y[row], cols.max_z[row]];
        let mut inside = live;
        let mut p = 0;
        while inside && p < 6 {
            let pl = f.planes[p];
            let px = if pl[0] >= 0.0 { bmax[0] } else { bmin[0] };
            let py = if pl[1] >= 0.0 { bmax[1] } else { bmin[1] };
            let pz = if pl[2] >= 0.0 { bmax[2] } else { bmin[2] };
            if pl[0] * px + pl[1] * py + pl[2] * pz + pl[3] < 0.0 { inside = false; }
            p += 1;
        }
        out[row] = if inside { hits += 1; row as u32 } else { NULL_ROW };
        row += 1;
    }
    hits
}

/// NEON backend for the frustum scan, processing 4 rows per iteration
/// (128-bit NEON registers, each holding 4 × f32).
///
/// Produces bit-identical `out` buffers and pass counts to
/// [`frustum_scan_scalar`]. The dot-product association
/// `((nx*px + ny*py) + nz*pz) + d` and ordered (`vcge`) comparisons
/// exactly mirror the scalar reference; no FMA contraction is used so the
/// rounding matches.
///
/// # Safety
/// Caller must ensure the `neon` target feature is available (mandatory on
/// aarch64 — ARMv8-A Advanced SIMD is always present).
///
/// The dot product MUST use separate `vmulq_f32`/`vaddq_f32` (never
/// `vfmaq_f32`): FMA's single rounding would diverge from the scalar
/// reference's mul-then-add and break the bit-for-bit contract.
#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
pub(crate) unsafe fn frustum_scan_neon(
    f: &FrustumPlanes,
    cols: &Columns,
    liveness_words: &[u64],
    len: usize,
    out: &mut [u32],
) -> u32 {
    use std::arch::aarch64::*;
    debug_assert!(out.len() >= len);
    debug_assert_eq!(liveness_words.len(), len.div_ceil(64), "liveness_words must cover exactly rows 0..len");
    debug_assert!(cols.min_x.len() >= len && cols.max_x.len() >= len, "x columns shorter than len");
    debug_assert!(cols.min_y.len() >= len && cols.max_y.len() >= len, "y columns shorter than len");
    debug_assert!(cols.min_z.len() >= len && cols.max_z.len() >= len, "z columns shorter than len");
    let mut hits = 0u32;
    let mut row = 0usize;
    while row + 4 <= len {
        let minx = vld1q_f32(cols.min_x.as_ptr().add(row));
        let maxx = vld1q_f32(cols.max_x.as_ptr().add(row));
        let miny = vld1q_f32(cols.min_y.as_ptr().add(row));
        let maxy = vld1q_f32(cols.max_y.as_ptr().add(row));
        let minz = vld1q_f32(cols.min_z.as_ptr().add(row));
        let maxz = vld1q_f32(cols.max_z.as_ptr().add(row));
        // inside accumulator: all-ones, ANDed by each plane's "inside" mask.
        let mut inside = vdupq_n_u32(0xFFFFFFFF);
        for pl in f.planes.iter() {
            let nx = vdupq_n_f32(pl[0]);
            let ny = vdupq_n_f32(pl[1]);
            let nz = vdupq_n_f32(pl[2]);
            let d = vdupq_n_f32(pl[3]);
            // positive vertex per axis: nx>=0 ? maxx : minx (blend on the sign
            // mask of the plane normal component, ordered GE matching scalar).
            let selx = vcgeq_f32(nx, vdupq_n_f32(0.0));
            let sely = vcgeq_f32(ny, vdupq_n_f32(0.0));
            let selz = vcgeq_f32(nz, vdupq_n_f32(0.0));
            let px = vbslq_f32(selx, maxx, minx);
            let py = vbslq_f32(sely, maxy, miny);
            let pz = vbslq_f32(selz, maxz, minz);
            // dot = ((nx*px + ny*py) + nz*pz) + d
            // Association MUST match the scalar reference exactly —
            // `((a+b)+c)+d` — because f32 addition is not associative. Separate
            // mul+add (no FMA contraction) matches the scalar path's rounding.
            let dot = vaddq_f32(
                vaddq_f32(
                    vaddq_f32(vmulq_f32(nx, px), vmulq_f32(ny, py)),
                    vmulq_f32(nz, pz),
                ),
                d,
            );
            // inside-this-plane iff dot >= 0 (ordered).
            let inplane = vcgeq_f32(dot, vdupq_n_f32(0.0));
            inside = vandq_u32(inside, inplane);
        }
        // 4-bit mask, one bit per lane (1 = geometric pass).
        let shr = vshrq_n_u32(inside, 31);
        let mut mask = vgetq_lane_u32(shr, 0)
                     | (vgetq_lane_u32(shr, 1) << 1)
                     | (vgetq_lane_u32(shr, 2) << 2)
                     | (vgetq_lane_u32(shr, 3) << 3);
        let lw = liveness_words[row / 64];
        mask &= ((lw >> (row % 64)) & 0b1111) as u32;
        hits += mask.count_ones();
        for lane in 0..4usize {
            let r = row + lane;
            out[r] = if (mask >> lane) & 1 != 0 { r as u32 } else { NULL_ROW };
        }
        row += 4;
    }
    // Scalar tail (identical predicate to frustum_scan_scalar). Its short-circuit
    // is equivalent to the SIMD body's all-planes AND — order-independent.
    while row < len {
        let live = liveness_words[row / 64] & (1u64 << (row % 64)) != 0;
        let bmin = [cols.min_x[row], cols.min_y[row], cols.min_z[row]];
        let bmax = [cols.max_x[row], cols.max_y[row], cols.max_z[row]];
        let mut inside = live;
        let mut p = 0;
        while inside && p < 6 {
            let pl = f.planes[p];
            let px = if pl[0] >= 0.0 { bmax[0] } else { bmin[0] };
            let py = if pl[1] >= 0.0 { bmax[1] } else { bmin[1] };
            let pz = if pl[2] >= 0.0 { bmax[2] } else { bmin[2] };
            if pl[0] * px + pl[1] * py + pl[2] * pz + pl[3] < 0.0 { inside = false; }
            p += 1;
        }
        out[row] = if inside { hits += 1; row as u32 } else { NULL_ROW };
        row += 1;
    }
    hits
}

/// Runtime-dispatched frustum scan. Selects AVX2 when available, else NEON
/// on aarch64; all backends produce bit-identical `out` buffers (scalar is
/// the reference).
#[inline]
pub(crate) fn frustum_scan(f: &FrustumPlanes, cols: &Columns, liveness_words: &[u64], len: usize, out: &mut [u32]) -> u32 {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            // SAFETY: guarded by the runtime feature check.
            return unsafe { frustum_scan_avx2(f, cols, liveness_words, len, out) };
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: NEON is mandatory on aarch64 (ARMv8-A Advanced SIMD).
        return unsafe { frustum_scan_neon(f, cols, liveness_words, len, out) };
    }
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
            let n_words = len.div_ceil(64);
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

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn avx2_frustum_matches_scalar() {
        if !is_x86_feature_detected!("avx2") { return; }
        use rand::{Rng, SeedableRng};
        let mut rng = rand::rngs::StdRng::seed_from_u64(0xF2057);
        for _ in 0..200 {
            let len = rng.gen_range(0..=300usize);
            let col = |rng: &mut rand::rngs::StdRng| (0..len).map(|_| rng.gen_range(-50.0f32..50.0)).collect::<Vec<_>>();
            let min_x = col(&mut rng); let max_x: Vec<f32> = min_x.iter().map(|&m| m + rng.gen_range(0.0..5.0)).collect();
            let min_y = col(&mut rng); let max_y: Vec<f32> = min_y.iter().map(|&m| m + rng.gen_range(0.0..5.0)).collect();
            let min_z = col(&mut rng); let max_z: Vec<f32> = min_z.iter().map(|&m| m + rng.gen_range(0.0..5.0)).collect();
            let words: Vec<u64> = (0..len.div_ceil(64)).map(|_| rng.gen::<u64>()).collect();
            let mut planes = [[0.0f32; 4]; 6];
            for pl in &mut planes { for v in pl.iter_mut() { *v = rng.gen_range(-20.0..20.0); } }
            let f = FrustumPlanes { planes };
            let cols = Columns { min_x: &min_x, max_x: &max_x, min_y: &min_y, max_y: &max_y, min_z: &min_z, max_z: &max_z };
            let mut a = vec![0u32; len]; let mut b = vec![0u32; len];
            let ha = frustum_scan_scalar(&f, &cols, &words, len, &mut a);
            // SAFETY: guarded by the runtime feature check above.
            let hb = unsafe { frustum_scan_avx2(&f, &cols, &words, len, &mut b) };
            assert_eq!(a, b, "AVX2 frustum diverged at len={len}");
            assert_eq!(ha, hb);
        }
    }

    #[test]
    #[cfg(target_arch = "aarch64")]
    fn neon_aabb_matches_scalar_bit_for_bit() {
        use rand::{Rng, SeedableRng};
        let mut rng = rand::rngs::StdRng::seed_from_u64(0xA7F2 ^ 0x5CEDB);
        for _ in 0..200 {
            let len = rng.gen_range(0..=300usize);
            let gen_col = |rng: &mut rand::rngs::StdRng| (0..len).map(|_| rng.gen_range(-100.0f32..100.0)).collect::<Vec<_>>();
            let min_x = gen_col(&mut rng); let max_x: Vec<f32> = min_x.iter().map(|&m| m + rng.gen_range(0.0..10.0)).collect();
            let min_y = gen_col(&mut rng); let max_y: Vec<f32> = min_y.iter().map(|&m| m + rng.gen_range(0.0..10.0)).collect();
            let min_z = gen_col(&mut rng); let max_z: Vec<f32> = min_z.iter().map(|&m| m + rng.gen_range(0.0..10.0)).collect();
            let n_words = len.div_ceil(64);
            let words: Vec<u64> = (0..n_words).map(|_| rng.gen::<u64>()).collect();
            let q = QueryBounds {
                min: [rng.gen_range(-100.0..100.0), rng.gen_range(-100.0..100.0), rng.gen_range(-100.0..100.0)],
                max: [rng.gen_range(-100.0..100.0), rng.gen_range(-100.0..100.0), rng.gen_range(-100.0..100.0)],
            };
            let cols = Columns { min_x: &min_x, max_x: &max_x, min_y: &min_y, max_y: &max_y, min_z: &min_z, max_z: &max_z };
            let mut out_s = vec![0u32; len];
            let mut out_v = vec![0u32; len];
            let hs = aabb_scan_scalar(&q, &cols, &words, len, &mut out_s);
            // SAFETY: NEON is mandatory on aarch64.
            let hv = unsafe { aabb_scan_neon(&q, &cols, &words, len, &mut out_v) };
            assert_eq!(out_s, out_v, "NEON diverged from scalar at len={len}");
            assert_eq!(hs, hv);
        }
    }

    #[test]
    #[cfg(target_arch = "aarch64")]
    fn neon_frustum_matches_scalar() {
        use rand::{Rng, SeedableRng};
        let mut rng = rand::rngs::StdRng::seed_from_u64(0xF2057);
        for _ in 0..200 {
            let len = rng.gen_range(0..=300usize);
            let col = |rng: &mut rand::rngs::StdRng| (0..len).map(|_| rng.gen_range(-50.0f32..50.0)).collect::<Vec<_>>();
            let min_x = col(&mut rng); let max_x: Vec<f32> = min_x.iter().map(|&m| m + rng.gen_range(0.0..5.0)).collect();
            let min_y = col(&mut rng); let max_y: Vec<f32> = min_y.iter().map(|&m| m + rng.gen_range(0.0..5.0)).collect();
            let min_z = col(&mut rng); let max_z: Vec<f32> = min_z.iter().map(|&m| m + rng.gen_range(0.0..5.0)).collect();
            let words: Vec<u64> = (0..len.div_ceil(64)).map(|_| rng.gen::<u64>()).collect();
            let mut planes = [[0.0f32; 4]; 6];
            for pl in &mut planes { for v in pl.iter_mut() { *v = rng.gen_range(-20.0..20.0); } }
            let f = FrustumPlanes { planes };
            let cols = Columns { min_x: &min_x, max_x: &max_x, min_y: &min_y, max_y: &max_y, min_z: &min_z, max_z: &max_z };
            let mut a = vec![0u32; len]; let mut b = vec![0u32; len];
            let ha = frustum_scan_scalar(&f, &cols, &words, len, &mut a);
            // SAFETY: NEON is mandatory on aarch64.
            let hb = unsafe { frustum_scan_neon(&f, &cols, &words, len, &mut b) };
            assert_eq!(a, b, "NEON frustum diverged at len={len}");
            assert_eq!(ha, hb);
        }
    }
}
