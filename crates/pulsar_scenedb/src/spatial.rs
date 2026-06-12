use crate::cell::CellStorage;
use crate::handle::Handle;
use crate::page::{ColumnDesc, LayoutError};
use crate::registry::NULL_ROW;

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
    /// (e.g. `_CMP_LE_OS`/`_CMP_GE_OS` in AVX, `fcmle`/`fcmge` in NEON) — not
    /// unordered variants — to stay bit-identical to this reference.
    pub fn query_aabb(&self, q: &Aabb, out: &mut [u32]) -> u32 {
        let len = self.storage.rows_in_use() as usize;
        assert!(out.len() >= len, "scratch buffer too small");
        let min_x = &self.storage.user_column::<f32>(COL_MIN_X)[..len];
        let max_x = &self.storage.user_column::<f32>(COL_MAX_X)[..len];
        let min_y = &self.storage.user_column::<f32>(COL_MIN_Y)[..len];
        let max_y = &self.storage.user_column::<f32>(COL_MAX_Y)[..len];
        let min_z = &self.storage.user_column::<f32>(COL_MIN_Z)[..len];
        let max_z = &self.storage.user_column::<f32>(COL_MAX_Z)[..len];
        let liveness = self.storage.liveness();

        let mut hits = 0u32;
        for row in 0..len {
            // Scalar reference: one atomic liveness load per row. M1b SIMD
            // should instead load liveness.words()[row / 64] once per 64-row
            // block and extract per-lane mask bits from the cached u64.
            let visible = min_x[row] <= q.max[0]
                && max_x[row] >= q.min[0]
                && min_y[row] <= q.max[1]
                && max_y[row] >= q.min[1]
                && min_z[row] <= q.max[2]
                && max_z[row] >= q.min[2]
                && liveness.is_live(row as u32);
            out[row] = if visible {
                hits += 1;
                row as u32
            } else {
                NULL_ROW
            };
        }
        hits
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
            assert_eq!(hits, expected);
            assert_eq!(n, expected.len());
        }
    }
}
