use crate::handle::Handle;
use crate::liveness::LivenessMask;
use crate::page::{ColumnDesc, LayoutError, Page, PageLayout};
use crate::registry::HandleRegistry;

/// Layer 1 storage for one cell: page + liveness + handle registry, wired
/// per spec §4.4 / CONTRACTS.md C1–C3.
///
/// Column 0 is always the implicit **slot-ID column** (`u32` owning slot per
/// row) — compaction reads it to fix the slot→row table after a swap. User
/// columns are addressed by their own index space (user column i = physical
/// column i + 1).
///
/// Mutation contract (enforced by Layer 2's phase machine in M2; by
/// discipline here): `alloc`/`free` during Simulate, `compact` only at the
/// frame boundary, reads during Harvest.
pub struct CellStorage {
    page: Page,
    liveness: LivenessMask,
    registry: HandleRegistry,
    user_column_count: usize,
}

impl CellStorage {
    pub fn new(user_columns: &[ColumnDesc], capacity: u32) -> Result<Self, LayoutError> {
        let mut columns = Vec::with_capacity(user_columns.len() + 1);
        columns.push(ColumnDesc::of::<u32>()); // slot-ID column
        columns.extend_from_slice(user_columns);
        let layout = PageLayout::new(&columns, capacity)?;
        Ok(Self {
            page: Page::new(&layout),
            liveness: LivenessMask::new(capacity),
            registry: HandleRegistry::new(),
            user_column_count: user_columns.len(),
        })
    }

    /// Allocate an element: claims a row, marks it live, issues a handle.
    pub fn alloc(&mut self) -> Option<Handle> {
        let row = self.page.push_row()?;
        let handle = self.registry.allocate(row);
        self.page.column_slice_mut::<u32>(0)[row as usize] = handle.index();
        self.liveness.set_live(row);
        Some(handle)
    }

    /// Mark an element dead. Physical removal is deferred to `compact()`.
    /// Returns false for stale/invalid handles.
    pub fn free(&mut self, handle: Handle) -> bool {
        let Some(row) = self.registry.row_of(handle) else {
            return false;
        };
        self.liveness.set_dead(row);
        self.registry.free(handle)
    }

    /// Frame-boundary swap-and-pop compaction (spec §4.4). Moves the last
    /// live row into each dead row, updates the moved element's slot→row
    /// entry via the slot-ID column, and shrinks the row frontier.
    pub fn compact(&mut self) {
        let mut len = self.page.len();
        let mut row = 0u32;
        while row < len {
            if self.liveness.is_live(row) {
                row += 1;
                continue;
            }
            // Shrink trailing dead rows first.
            while len > row + 1 && !self.liveness.is_live(len - 1) {
                len -= 1;
                self.page.pop_row();
            }
            if len == row + 1 {
                // The dead row is the tail. pop_row drops it; page.len() is now
                // `row`, which equals the surviving live count (rows 0..row are all
                // live). The local `len` is dead after `break`, so we don't decrement it.
                self.page.pop_row();
                break;
            }
            // Swap last (live) row into the hole, column by column.
            let last = len - 1;
            self.swap_rows(row, last);
            // Fix the moved element's slot→row mapping.
            let moved_slot = self.page.column_slice::<u32>(0)[row as usize];
            self.registry.set_row(moved_slot, row);
            self.liveness.set_live(row);
            self.liveness.set_dead(last);
            len -= 1;
            self.page.pop_row();
            row += 1;
        }
    }

    /// Byte-wise swap of two rows across every physical column.
    fn swap_rows(&mut self, a: u32, b: u32) {
        for col in 0..self.user_column_count + 1 {
            let desc_size = self.column_size(col);
            let base = self.page.column_ptr_mut(col);
            // SAFETY: rows a, b < capacity; regions are disjoint (a != b)
            // and within the column span.
            unsafe {
                std::ptr::swap_nonoverlapping(
                    base.add(a as usize * desc_size),
                    base.add(b as usize * desc_size),
                    desc_size,
                );
            }
        }
    }

    fn column_size(&self, col: usize) -> usize {
        self.page.layout().column_descs()[col].size as usize
    }

    // ── accessors ──────────────────────────────────────────────────────────

    #[inline]
    pub fn row_of(&self, handle: Handle) -> Option<u32> {
        self.registry.row_of(handle)
    }

    pub fn user_column<T: crate::page::Pod>(&self, user_col: usize) -> &[T] {
        self.page.column_slice::<T>(user_col + 1)
    }

    pub fn user_column_mut<T: crate::page::Pod>(&mut self, user_col: usize) -> &mut [T] {
        self.page.column_slice_mut::<T>(user_col + 1)
    }

    pub fn live_count(&self) -> u32 {
        self.liveness.live_count()
    }

    /// Physical rows currently occupied (live + not-yet-compacted dead).
    pub fn rows_in_use(&self) -> u32 {
        self.page.len()
    }

    pub fn liveness(&self) -> &LivenessMask {
        &self.liveness
    }

    pub fn registry(&self) -> &HandleRegistry {
        &self.registry
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::ColumnDesc;

    /// One user column: f32 "x position" (column index 1; column 0 is the
    /// implicit slot-ID column).
    fn cell() -> CellStorage {
        CellStorage::new(&[ColumnDesc::of::<f32>()], 256).unwrap()
    }

    #[test]
    fn alloc_writes_and_reads_through_handle() {
        let mut c = cell();
        let h = c.alloc().unwrap();
        let row = c.row_of(h).unwrap();
        c.user_column_mut::<f32>(0)[row as usize] = 7.5;
        assert_eq!(c.user_column::<f32>(0)[row as usize], 7.5);
    }

    #[test]
    fn free_is_deferred_until_compact() {
        let mut c = cell();
        let h = c.alloc().unwrap();
        assert_eq!(c.live_count(), 1);
        assert!(c.free(h));
        // Row still physically present (deferred), but handle is dead and
        // the element no longer counts as live.
        assert_eq!(c.live_count(), 0);
        assert_eq!(c.row_of(h), None);
        assert_eq!(c.rows_in_use(), 1, "physical removal deferred");
        c.compact();
        assert_eq!(c.rows_in_use(), 0);
    }

    #[test]
    fn handles_survive_compaction_rows_do_not() {
        let mut c = cell();
        let ha = c.alloc().unwrap();
        let hb = c.alloc().unwrap();
        let hc = c.alloc().unwrap();
        // Write distinct values keyed by handle.
        for (h, v) in [(ha, 1.0f32), (hb, 2.0), (hc, 3.0)] {
            let row = c.row_of(h).unwrap() as usize;
            c.user_column_mut::<f32>(0)[row] = v;
        }
        let hb_row_before = c.row_of(hb).unwrap();
        c.free(hb);
        c.compact(); // swap-and-pop: hc moves into hb's old row
        // hc's handle still resolves to hc's data:
        let hc_row = c.row_of(hc).unwrap();
        assert_eq!(c.user_column::<f32>(0)[hc_row as usize], 3.0);
        // and it moved into the vacated row:
        assert_eq!(hc_row, hb_row_before, "swap-and-pop fills the hole");
        // ha untouched:
        let ha_row = c.row_of(ha).unwrap();
        assert_eq!(c.user_column::<f32>(0)[ha_row as usize], 1.0);
        assert_eq!(c.rows_in_use(), 2);
    }

    #[test]
    fn alloc_after_compact_reuses_rows_and_slots() {
        let mut c = cell();
        let h1 = c.alloc().unwrap();
        c.free(h1);
        c.compact();
        let h2 = c.alloc().unwrap();
        assert_eq!(h2.index(), h1.index(), "slot recycled");
        assert!(h2.generation() > h1.generation());
        assert_eq!(c.row_of(h2), Some(0), "row 0 reused");
        assert_eq!(c.row_of(h1), None, "old handle stays dead");
    }

    #[test]
    fn full_cell_returns_none() {
        let mut c = CellStorage::new(&[ColumnDesc::of::<f32>()], 1).unwrap();
        assert!(c.alloc().is_some());
        assert!(c.alloc().is_none());
    }

    #[test]
    fn compact_handles_multiple_holes_including_tail() {
        let mut c = cell();
        let hs: Vec<_> = (0..6).map(|_| c.alloc().unwrap()).collect();
        for (i, &h) in hs.iter().enumerate() {
            let row = c.row_of(h).unwrap() as usize;
            c.user_column_mut::<f32>(0)[row] = i as f32;
        }
        // Kill rows 1, 3, and the tail row 5.
        c.free(hs[1]);
        c.free(hs[3]);
        c.free(hs[5]);
        c.compact();
        assert_eq!(c.rows_in_use(), 3);
        for &(i, h) in &[(0usize, hs[0]), (2, hs[2]), (4, hs[4])] {
            let row = c.row_of(h).unwrap() as usize;
            assert_eq!(c.user_column::<f32>(0)[row], i as f32, "survivor {i} intact");
        }
        for &h in &[hs[1], hs[3], hs[5]] {
            assert_eq!(c.row_of(h), None);
        }
    }
}
