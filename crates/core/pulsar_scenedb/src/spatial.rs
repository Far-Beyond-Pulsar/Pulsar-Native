use crate::cell::CellStorage;
use crate::handle::Handle;
use crate::page::{ColumnDesc, LayoutError};

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// User-column indices for the six bounds columns (spec §4.2: six separate
/// f32 arrays, not an array of structs).
const COL_MIN_X: usize = 0;
const COL_MAX_X: usize = 1;
const COL_MIN_Y: usize = 2;
const COL_MAX_Y: usize = 3;
const COL_MIN_Z: usize = 4;
const COL_MAX_Z: usize = 5;
/// Number of bounds columns; cell-type-specific columns start after these.
pub const SPATIAL_COLUMNS: usize = 6;

/// User-column index of the GPU-mirrored transform column on cells built
/// with [`SpatialCell::with_transform`].
pub const TRANSFORM_COLUMN: usize = SPATIAL_COLUMNS;

/// GPU-mirrored per-instance metadata (M3-α, C5 amendment): cull's
/// token→mesh link. Mirrors row-indexed, beside the transform column, to
/// `gpu::SceneGpuStore`'s instance-info SSBO via `write_instance_info` (the
/// GPU mirror itself lives in `gpu::scene_store` — this type is plain Pod
/// data and stays graphics-free, CONTRACTS C0).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct InstanceInfo {
    pub mesh_index: u32, // @0 — MeshRegistry index (LOD 0 entry per R9)
    pub flags: u32,      // @4 — bit 0 reserved: near-clip CPU twin (M3-β); rest 0
}
const _: () = assert!(std::mem::size_of::<InstanceInfo>() == 8);
unsafe impl crate::page::Pod for InstanceInfo {}

/// User-column index of the GPU-mirrored instance-info column on cells built
/// with [`SpatialCell::with_transform`] (C5 amendment, M3-α).
pub const INSTANCE_INFO_COLUMN: usize = SPATIAL_COLUMNS + 1; // 7

/// Six inward-normal frustum planes (spec §8.1 frustum query input).
#[derive(Copy, Clone, Debug)]
pub struct Frustum {
    pub planes: [[f32; 4]; 6],
}

/// CellStorage + spatial bounds columns + the §8 query.
///
/// `query_aabb` is the **scalar reference implementation**: M1b's SIMD paths
/// (AVX2/AVX-512/NEON) must produce bit-identical output buffers.
pub struct SpatialCell {
    storage: CellStorage,
}

impl SpatialCell {
    pub fn new(capacity: u32) -> Result<Self, LayoutError> {
        let columns = [ColumnDesc::of::<f32>(); SPATIAL_COLUMNS];
        Ok(Self {
            storage: CellStorage::new(&columns, capacity)?,
        })
    }

    pub fn alloc(&mut self, bounds: Aabb) -> Option<Handle> {
        let h = self.storage.alloc()?;
        let row = self.storage.row_of(h).unwrap() as usize;
        self.storage.user_column_mut::<f32>(COL_MIN_X)[row] = bounds.min[0];
        self.storage.user_column_mut::<f32>(COL_MAX_X)[row] = bounds.max[0];
        self.storage.user_column_mut::<f32>(COL_MIN_Y)[row] = bounds.min[1];
        self.storage.user_column_mut::<f32>(COL_MAX_Y)[row] = bounds.max[1];
        self.storage.user_column_mut::<f32>(COL_MIN_Z)[row] = bounds.min[2];
        self.storage.user_column_mut::<f32>(COL_MAX_Z)[row] = bounds.max[2];
        Some(h)
    }

    /// A spatial cell that also carries a token-registered `[f32; 16]`
    /// transform column (user column [`TRANSFORM_COLUMN`]) and a
    /// token-registered [`InstanceInfo`] column (user column
    /// [`INSTANCE_INFO_COLUMN`], M3-α C5 amendment — cull's token→mesh link)
    /// so it can be registered with `gpu::SceneGpuStore` (which resolves both
    /// mirrored columns token-keyed). Columns: `[6×f32 @0-5, [f32;16] @6,
    /// InstanceInfo @7]`. Stride: 6×4 + 64 + 8 = 96 B ≤ the C2 128 B ceiling.
    pub fn with_transform(capacity: u32) -> Result<Self, LayoutError> {
        let mut columns = [ColumnDesc::of::<f32>(); SPATIAL_COLUMNS + 2];
        columns[TRANSFORM_COLUMN] = ColumnDesc::of::<[f32; 16]>();
        columns[INSTANCE_INFO_COLUMN] = ColumnDesc::of::<InstanceInfo>();
        let mut storage = CellStorage::new(&columns, capacity)?;
        storage.register_token_column::<[f32; 16]>(TRANSFORM_COLUMN);
        storage.register_token_column::<InstanceInfo>(INSTANCE_INFO_COLUMN);
        Ok(Self { storage })
    }

    /// Spec §8.2 predicate over all physical rows, writing positionally
    /// aligned row tokens into `out` (spec §8.3): `out[row] = row` on hit,
    /// `NULL_ROW` on miss/dead. `out.len()` must be ≥ `rows_in_use()`.
    /// Returns the hit count. Allocates nothing (spec §8.1).
    ///
    /// Entries `out[rows_in_use()..]` (when `out` is larger than the live
    /// frontier) are **left unchanged** — not zeroed or sentinel-filled.
    /// Re-using an oversized scratch buffer across frames is safe as long as
    /// the caller only reads `out[0..rows_in_use()]`. M1b SIMD paths must
    /// replicate this: no full-buffer clear is performed.
    ///
    /// # Float semantics
    ///
    /// All comparisons use Rust's `<=`/`>=`, which are IEEE 754 **ordered**
    /// comparisons: a NaN bound makes every comparison false, so the row is a
    /// miss. M1b SIMD paths must use **ordered** comparison predicates
    /// (e.g. `_CMP_LE_OQ`/`_CMP_GE_OQ` in AVX, `fcmle`/`fcmge` in NEON) — not
    /// unordered variants — to stay bit-identical to this reference.
    ///
    /// Allocates a per-call liveness `Vec<u64>` snapshot. The §8.1 no-allocation
    /// contract is honored end-to-end by [`Self::query_aabb_in`]: harvest calls
    /// that with words captured into a `Scratchpad` buffer (M2b). This wrapper
    /// captures into a local `Vec` and delegates — kept for callers outside the
    /// harvest path (tests, tools) that don't need the no-alloc discipline.
    pub fn query_aabb(&self, q: &Aabb, out: &mut [u32]) -> u32 {
        let len = self.storage.rows_in_use() as usize;
        let n_words = len.div_ceil(64);
        let words: Vec<u64> = self
            .storage
            .liveness()
            .words()
            .iter()
            .take(n_words)
            .map(|w| w.load(std::sync::atomic::Ordering::Relaxed))
            .collect();
        self.query_aabb_in(q, &words, out)
    }

    /// Identical to [`Self::query_aabb`] but scans against caller-provided
    /// liveness words instead of capturing its own snapshot (spec §8.1
    /// no-allocation query path). `liveness_words` must cover rows `0..len`
    /// (`liveness_words.len() == rows_in_use().div_ceil(64)`); typically
    /// produced by `LivenessSnapshot::capture_words` into a `Scratchpad`
    /// u64 buffer. `out.len()` must be ≥ `rows_in_use()`.
    ///
    /// C4 frame-validity contract: `liveness_words` must be captured in the
    /// SAME frame phase as this query, after the phase-boundary fence, and the
    /// returned row tokens are valid for the issuing frame only — compaction
    /// at the boundary invalidates both. A stale under-sized words slice fails
    /// fast (index panic in the kernel), never silently; freshness itself is
    /// the caller's contract (the harvest pipeline recaptures per cell per
    /// frame).
    pub fn query_aabb_in(&self, q: &Aabb, liveness_words: &[u64], out: &mut [u32]) -> u32 {
        let len = self.storage.rows_in_use() as usize;
        assert!(out.len() >= len, "scratch buffer too small");
        debug_assert_eq!(liveness_words.len(), len.div_ceil(64));
        let min_x = &self.storage.user_column::<f32>(COL_MIN_X)[..len];
        let max_x = &self.storage.user_column::<f32>(COL_MAX_X)[..len];
        let min_y = &self.storage.user_column::<f32>(COL_MIN_Y)[..len];
        let max_y = &self.storage.user_column::<f32>(COL_MAX_Y)[..len];
        let min_z = &self.storage.user_column::<f32>(COL_MIN_Z)[..len];
        let max_z = &self.storage.user_column::<f32>(COL_MAX_Z)[..len];
        let qb = crate::simd::QueryBounds { min: q.min, max: q.max };
        let cols = crate::simd::Columns { min_x, max_x, min_y, max_y, min_z, max_z };
        crate::simd::aabb_scan(&qb, &cols, liveness_words, len, out)
    }

    /// Frustum query (§8.1). Same positional-token output contract as
    /// `query_aabb` (`out[r] = r` on pass, `NULL_ROW` on cull/dead;
    /// `out[rows_in_use()..]` untouched).
    ///
    /// Float semantics: `dot >= 0.0` is an ordered comparison — a NaN plane
    /// normal makes the dot NaN, which compares false, so the row is culled.
    /// M1b SIMD arms must use ordered predicates (`_CMP_GE_OQ` in AVX2).
    ///
    /// Allocates a per-call liveness `Vec<u64>` snapshot; harvest uses
    /// [`Self::query_frustum_in`] with `Scratchpad` words instead — §8.1.
    pub fn query_frustum(&self, f: &Frustum, out: &mut [u32]) -> u32 {
        let len = self.storage.rows_in_use() as usize;
        let n_words = len.div_ceil(64);
        let words: Vec<u64> = self.storage.liveness().words().iter().take(n_words)
            .map(|w| w.load(std::sync::atomic::Ordering::Relaxed)).collect();
        self.query_frustum_in(f, &words, out)
    }

    /// Identical to [`Self::query_frustum`] but scans against caller-provided
    /// liveness words instead of capturing its own snapshot (spec §8.1
    /// no-allocation query path). `liveness_words` must cover rows `0..len`
    /// (`liveness_words.len() == rows_in_use().div_ceil(64)`). `out.len()`
    /// must be ≥ `rows_in_use()`.
    ///
    /// C4 frame-validity contract: `liveness_words` must be captured in the
    /// SAME frame phase as this query, after the phase-boundary fence, and the
    /// returned row tokens are valid for the issuing frame only — compaction
    /// at the boundary invalidates both. A stale under-sized words slice fails
    /// fast (index panic in the kernel), never silently; freshness itself is
    /// the caller's contract (the harvest pipeline recaptures per cell per
    /// frame).
    pub fn query_frustum_in(&self, f: &Frustum, liveness_words: &[u64], out: &mut [u32]) -> u32 {
        let len = self.storage.rows_in_use() as usize;
        assert!(out.len() >= len, "scratch buffer too small");
        debug_assert_eq!(liveness_words.len(), len.div_ceil(64));
        let min_x = &self.storage.user_column::<f32>(COL_MIN_X)[..len];
        let max_x = &self.storage.user_column::<f32>(COL_MAX_X)[..len];
        let min_y = &self.storage.user_column::<f32>(COL_MIN_Y)[..len];
        let max_y = &self.storage.user_column::<f32>(COL_MAX_Y)[..len];
        let min_z = &self.storage.user_column::<f32>(COL_MIN_Z)[..len];
        let max_z = &self.storage.user_column::<f32>(COL_MAX_Z)[..len];
        let fp = crate::simd::FrustumPlanes { planes: f.planes };
        let cols = crate::simd::Columns { min_x, max_x, min_y, max_y, min_z, max_z };
        crate::simd::frustum_scan(&fp, &cols, liveness_words, len, out)
    }

    // ── delegation ─────────────────────────────────────────────────────────

    pub fn free(&mut self, handle: Handle) -> bool {
        self.storage.free(handle)
    }

    pub fn compact(&mut self) {
        self.storage.compact()
    }

    pub fn row_of(&self, handle: Handle) -> Option<u32> {
        self.storage.row_of(handle)
    }

    pub fn rows_in_use(&self) -> u32 {
        self.storage.rows_in_use()
    }

    pub fn live_count(&self) -> u32 {
        self.storage.live_count()
    }

    pub fn storage(&self) -> &CellStorage {
        &self.storage
    }

    pub fn storage_mut(&mut self) -> &mut CellStorage {
        &mut self.storage
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::LivenessSnapshot;

    fn aabb(min: [f32; 3], max: [f32; 3]) -> Aabb {
        Aabb { min, max }
    }

    #[test]
    fn query_writes_token_per_row_position() {
        let mut c = SpatialCell::new(256).unwrap();
        let ha = c.alloc(aabb([0.0, 0.0, 0.0], [1.0, 1.0, 1.0])).unwrap();
        let _hb = c.alloc(aabb([10.0, 10.0, 10.0], [11.0, 11.0, 11.0])).unwrap();
        let hc = c.alloc(aabb([0.5, 0.5, 0.5], [2.0, 2.0, 2.0])).unwrap();

        let mut out = vec![0u32; c.rows_in_use() as usize];
        let n = c.query_aabb(&aabb([0.0, 0.0, 0.0], [3.0, 3.0, 3.0]), &mut out);

        assert_eq!(n, 2, "two hits");
        // Positional alignment (spec §8.3): out[row] = row for hits,
        // NULL_ROW sentinel for misses.
        assert_eq!(out[c.row_of(ha).unwrap() as usize], c.row_of(ha).unwrap());
        assert_eq!(out[1], crate::registry::NULL_ROW, "miss row holds sentinel");
        assert_eq!(out[c.row_of(hc).unwrap() as usize], c.row_of(hc).unwrap());
    }

    #[test]
    fn dead_elements_excluded() {
        let mut c = SpatialCell::new(256).unwrap();
        let h = c.alloc(aabb([0.0; 3], [1.0; 3])).unwrap();
        c.free(h);
        let mut out = vec![0u32; c.rows_in_use() as usize];
        let n = c.query_aabb(&aabb([-1.0; 3], [2.0; 3]), &mut out);
        assert_eq!(n, 0);
        assert_eq!(out[0], crate::registry::NULL_ROW);
    }

    #[test]
    fn touching_boxes_intersect() {
        // Spec §8.2 predicate uses ≤/≥ — shared faces count as overlap.
        let mut c = SpatialCell::new(256).unwrap();
        c.alloc(aabb([1.0, 0.0, 0.0], [2.0, 1.0, 1.0])).unwrap();
        let mut out = vec![0u32; 1];
        let n = c.query_aabb(&aabb([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]), &mut out);
        assert_eq!(n, 1, "face contact at x=1 is a hit");
    }

    #[test]
    fn property_matches_naive_reference() {
        use rand::{Rng, SeedableRng};
        let mut rng = rand::rngs::StdRng::seed_from_u64(0x5CE_DB);
        let mut c = SpatialCell::new(1024).unwrap();
        let mut boxes = Vec::new();
        for _ in 0..1000 {
            let min: [f32; 3] = std::array::from_fn(|_| rng.gen_range(-100.0..100.0));
            let ext: [f32; 3] = std::array::from_fn(|_| rng.gen_range(0.0..10.0));
            let max = [min[0] + ext[0], min[1] + ext[1], min[2] + ext[2]];
            let b = aabb(min, max);
            c.alloc(b).unwrap();
            boxes.push(b);
        }
        for _ in 0..50 {
            let qmin: [f32; 3] = std::array::from_fn(|_| rng.gen_range(-100.0..100.0));
            let qext: [f32; 3] = std::array::from_fn(|_| rng.gen_range(0.0..50.0));
            let q = aabb(qmin, [qmin[0] + qext[0], qmin[1] + qext[1], qmin[2] + qext[2]]);
            let mut out = vec![0u32; c.rows_in_use() as usize];
            let n = c.query_aabb(&q, &mut out) as usize;
            let expected: Vec<u32> = boxes
                .iter()
                .enumerate()
                .filter(|(_, b)| {
                    b.min[0] <= q.max[0] && b.max[0] >= q.min[0]
                        && b.min[1] <= q.max[1] && b.max[1] >= q.min[1]
                        && b.min[2] <= q.max[2] && b.max[2] >= q.min[2]
                })
                .map(|(i, _)| i as u32)
                .collect();
            let hits: Vec<u32> = out
                .iter()
                .copied()
                .filter(|&t| t != crate::registry::NULL_ROW)
                .collect();
            // Ordered-vector comparison is valid here ONLY because no free/
            // compact runs in this test, so physical row order == insertion
            // order. If a future variant adds compaction, compare as sets
            // (row order would then differ from insertion order).
            assert_eq!(hits, expected);
            assert_eq!(n, expected.len());
        }
    }

    fn unit_box(at: [f32; 3]) -> Aabb {
        Aabb { min: at, max: [at[0] + 1.0, at[1] + 1.0, at[2] + 1.0] }
    }

    #[test]
    fn with_transform_exposes_token_keyed_mat4_column() {
        let mut c = SpatialCell::with_transform(64).unwrap();
        let h = c.alloc(aabb([0.0; 3], [1.0; 3])).unwrap();
        let row = c.row_of(h).unwrap() as usize;
        c.storage_mut().column_for_mut::<[f32; 16]>().unwrap()[row] = [7.0; 16];
        assert_eq!(c.storage().column_for::<[f32; 16]>().unwrap()[row], [7.0; 16]);
        // Bounds columns unaffected and still positional:
        assert_eq!(c.storage().user_column::<f32>(0)[row], 0.0);
    }

    /// M3-α T4: `with_transform` also exposes a token-keyed `InstanceInfo`
    /// column (user column [`INSTANCE_INFO_COLUMN`]) alongside the mat4
    /// transform column, independently addressable and unaffected by writes
    /// to the other columns — proves `register_token_column` wires BOTH
    /// tokens correctly on the same positionally-constructed cell.
    #[test]
    fn with_transform_exposes_token_keyed_instance_info_column() {
        let mut c = SpatialCell::with_transform(64).unwrap();
        let h = c.alloc(aabb([0.0; 3], [1.0; 3])).unwrap();
        let row = c.row_of(h).unwrap() as usize;
        c.storage_mut().column_for_mut::<[f32; 16]>().unwrap()[row] = [7.0; 16];
        c.storage_mut().column_for_mut::<InstanceInfo>().unwrap()[row] =
            InstanceInfo { mesh_index: 42, flags: 1 };
        assert_eq!(
            c.storage().column_for::<InstanceInfo>().unwrap()[row],
            InstanceInfo { mesh_index: 42, flags: 1 }
        );
        // Independent of the transform column written just above:
        assert_eq!(c.storage().column_for::<[f32; 16]>().unwrap()[row], [7.0; 16]);
        // Bounds columns unaffected and still positional:
        assert_eq!(c.storage().user_column::<f32>(0)[row], 0.0);
    }

    #[test]
    fn query_in_variants_match_allocating_queries() {
        let mut c = SpatialCell::new(256).unwrap();
        for i in 0..40 {
            let f = i as f32;
            c.alloc(aabb([f, 0.0, 0.0], [f + 0.5, 1.0, 1.0])).unwrap();
        }
        let q = aabb([3.0, 0.0, 0.0], [20.0, 1.0, 1.0]);
        let len = c.rows_in_use() as usize;
        let mut out_a = vec![0u32; len];
        let mut out_b = vec![0u32; len];
        let n_a = c.query_aabb(&q, &mut out_a);
        let mut words = vec![0u64; len.div_ceil(64)];
        let nw = LivenessSnapshot::capture_words(c.storage().liveness(), len as u32, &mut words);
        let n_b = c.query_aabb_in(&q, &words[..nw], &mut out_b);
        assert_eq!((n_a, &out_a), (n_b, &out_b), "in-variant is bit-identical");
    }

    #[test]
    fn frustum_keeps_inside_culls_outside() {
        let mut c = SpatialCell::new(64).unwrap();
        let _inside = c.alloc(unit_box([0.0, 0.0, 0.0])).unwrap();
        let _outside = c.alloc(unit_box([100.0, 0.0, 0.0])).unwrap();
        // Six planes of an axis-aligned box [-10,10]^3, inward normals.
        // Plane: (nx,ny,nz,d) with point inside iff n·p + d >= 0.
        let planes = [
            [1.0, 0.0, 0.0, 10.0],   // x >= -10
            [-1.0, 0.0, 0.0, 10.0],  // x <= 10
            [0.0, 1.0, 0.0, 10.0],
            [0.0, -1.0, 0.0, 10.0],
            [0.0, 0.0, 1.0, 10.0],
            [0.0, 0.0, -1.0, 10.0],
        ];
        let f = Frustum { planes };
        let mut out = vec![0u32; c.rows_in_use() as usize];
        let n = c.query_frustum(&f, &mut out);
        assert_eq!(n, 1, "only the box at origin is inside");
        assert_eq!(out[0], 0);
        assert_eq!(out[1], crate::registry::NULL_ROW);
    }
}
