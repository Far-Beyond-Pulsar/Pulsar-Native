use crate::cell_type::{GenericColumnDesc, RegisteredCellType};
use crate::component::ComponentId;
use crate::handle::Handle;
use crate::liveness::LivenessMask;
use crate::page::{ColumnDesc, GenericColumn, LayoutError, Page, PageLayout};
use crate::registry::HandleRegistry;
use crate::token::TypeToken;
use std::any::TypeId;

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
    /// Token id → user-column index for Pod columns, populated only via
    /// `from_cell_type`.
    token_index: Vec<(ComponentId, usize)>,
    /// Generic-column (ComponentId, TypeId) → generic-column-index mapping,
    /// also populated via `from_cell_type`.
    generic_token_index: Vec<(ComponentId, TypeId, usize)>,
    /// Row pin bitmask (M2a §5): a set bit means the row is in the
    /// pin-by-serial deferred-retirement window — excluded from compaction
    /// even though liveness already marked it dead. One bit per row, packed
    /// 64 rows per word.
    pins: Vec<u64>,
}

/// In-flight retirement record (M2a §5). Produced by `mark_pending_retire`,
/// consumed by `commit_retire` once the submission serial completes.
pub(crate) struct PendingRetire {
    pub slot: u32,
    pub row: u32,
    pub next_gen: u32,
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
            token_index: Vec::new(),
            generic_token_index: Vec::new(),
            pins: vec![0u64; capacity.div_ceil(64) as usize],
        })
    }

    /// Build a cell from a registered cell type (token-keyed). Preferred over
    /// `new` for typed call sites.  Creates both Pod and generic columns.
    pub fn from_cell_type(
        cell_type: &RegisteredCellType,
        capacity: u32,
    ) -> Result<Self, crate::page::LayoutError> {
        let descs = cell_type.user_descs();
        let mut storage = Self::new(&descs, capacity)?;
        storage.token_index = cell_type
            .token_ids()
            .into_iter()
            .enumerate()
            .map(|(idx, id)| (id, idx))
            .collect();
        // Build generic columns from the registered cell type.
        for (gen_idx, entry) in cell_type.generic_entries.iter().enumerate() {
            let col = (entry.construct)(capacity);
            storage.page.push_generic_column(col);
            storage.generic_token_index.push((entry.component_id, entry.type_id, gen_idx));
        }
        Ok(storage)
    }

    /// Register a token→user-column mapping on a positionally-constructed
    /// cell so `column_for::<T>()` resolves (M2b-β: `SpatialCell` carries six
    /// same-type bounds columns that CellType's type-keyed tokens cannot
    /// express, plus one token-keyed transform column for the GPU mirror).
    /// # Aliasing hazard (audit B, GAP-2; β Task 1 review note)
    ///
    /// Both guards below are `debug_assert!`, not real checks — in a release
    /// build, `register_token_column::<T>(wrong_col)` registers a token to
    /// whatever column `wrong_col` happens to be, with NO error at
    /// registration time. `Page::assert_column`'s real `assert_eq!` on size
    /// still catches a *size* mismatch on first read, but if two distinct
    /// `Pod` types happen to share the same byte size (plausible: many
    /// 64/128-byte structs), a wrong-column registration silently
    /// reinterprets one type's bytes as another's — no panic anywhere, ever.
    ///
    /// The caller must guarantee `user_col` genuinely addresses a column of
    /// element type `T` and is not ALSO addressed positionally elsewhere
    /// (e.g. via `user_column::<U>(user_col)` for some other `U`) — this
    /// method does not and cannot verify that on its own. Today's only call
    /// site (`SpatialCell::with_transform`, `spatial.rs`) is self-consistent
    /// and safe by construction; this is a real weakening of the
    /// token-keyed-column type-safety story for any FUTURE in-crate caller,
    /// since this method is `pub(crate)` and the hole is silent. Documented,
    /// deferred, narrow blast radius today (progress.md, β Task 1 review:
    /// "register_token_column has no misuse guard against aliasing a
    /// positional column (safe at today's only call site;
    /// discipline-guarded)").
    pub(crate) fn register_token_column<T: crate::page::Pod + 'static>(&mut self, user_col: usize) {
        let id = TypeToken::of::<T>().id();
        debug_assert!(
            !self.token_index.iter().any(|(tid, _)| *tid == id),
            "token already registered on this cell"
        );
        debug_assert_eq!(
            self.page.layout().column_descs()[user_col + 1].size as usize,
            std::mem::size_of::<T>(),
            "token type size does not match the column stride"
        );
        self.token_index.push((id, user_col));
    }

    /// Typed column access by token (resolves token → user-column index).
    /// Returns None if the token isn't a column of this cell.
    pub fn column_for<T: crate::page::Pod + 'static>(&self) -> Option<&[T]> {
        let id = TypeToken::of::<T>().id();
        let idx = self
            .token_index
            .iter()
            .find(|(tid, _)| *tid == id)
            .map(|(_, i)| *i)?;
        Some(self.user_column::<T>(idx))
    }

    /// For GPU-mirrored columns, write through `gpu::SceneGpuStore::write_transform`
    /// instead — raw writes here bypass dirty tracking and leave VRAM stale
    /// (enforced by the M2b phase machine; convention until then).
    pub fn column_for_mut<T: crate::page::Pod + 'static>(&mut self) -> Option<&mut [T]> {
        let id = TypeToken::of::<T>().id();
        let idx = self
            .token_index
            .iter()
            .find(|(tid, _)| *tid == id)
            .map(|(_, i)| *i)?;
        Some(self.user_column_mut::<T>(idx))
    }

    /// Typed read access to a generic (non-Pod) column by type.  Returns
    /// `None` if this cell doesn't carry a column of type `T` (or if `T`
    /// is Pod — use [`column_for`] for those).
    pub fn column_for_generic<T: 'static>(&self) -> Option<&GenericColumn<T>> {
        let type_id = TypeId::of::<T>();
        let idx = self
            .generic_token_index
            .iter()
            .find(|(_, tid, _)| *tid == type_id)
            .map(|(_, _, i)| *i)?;
        self.page.generic_column::<T>(idx)
    }

    /// Typed write access to a generic (non-Pod) column by type.
    pub fn column_for_generic_mut<T: 'static>(&mut self) -> Option<&mut GenericColumn<T>> {
        let type_id = TypeId::of::<T>();
        let idx = self
            .generic_token_index
            .iter()
            .find(|(_, tid, _)| *tid == type_id)
            .map(|(_, _, i)| *i)?;
        self.page.generic_column_mut::<T>(idx)
    }

    /// Return the raw backing bytes of a Pod column identified by
    /// `ComponentId`.  Only Pod columns support raw-byte access (for GPU
    /// sync); returns `None` for generic columns or unknown IDs.
    pub fn column_raw_bytes(&self, id: ComponentId) -> Option<&[u8]> {
        let idx = self
            .token_index
            .iter()
            .find(|(tid, _)| *tid == id)
            .map(|(_, i)| *i)?;
        let rows = self.page.len();
        Some(self.page.column_raw_bytes(idx + 1, rows))
    }

    /// Physical column 0 — the slot-ID column (one owning slot per row).
    /// Read by the GPU layer to maintain the row-indexed global-slot mirror
    /// (design Rev 2 §2; C6 GPU handle validation).
    pub(crate) fn slot_column(&self) -> &[u32] {
        self.page.column_slice::<u32>(0)
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
        debug_assert!(
            !self.is_row_pinned(row),
            "free() on a pending-retire row — use the deferred path end-to-end"
        );
        self.liveness.set_dead(row);
        self.registry.free(handle)
    }

    #[inline]
    pub(crate) fn is_row_pinned(&self, row: u32) -> bool {
        self.pins[(row / 64) as usize] & (1u64 << (row % 64)) != 0
    }

    fn pin_row(&mut self, row: u32) {
        self.pins[(row / 64) as usize] |= 1u64 << (row % 64);
    }

    fn unpin_row(&mut self, row: u32) {
        self.pins[(row / 64) as usize] &= !(1u64 << (row % 64));
    }

    /// Begin deferred retirement (M2a §5): liveness-dead (excluded from new
    /// harvests) and pinned (excluded from compaction), but the registry is
    /// untouched — the handle intentionally still resolves by row during the
    /// in-flight window. None for stale handles or already-pending rows.
    pub(crate) fn mark_pending_retire(&mut self, handle: Handle) -> Option<PendingRetire> {
        let row = self.registry.row_of(handle)?;
        if self.is_row_pinned(row) {
            return None;
        }
        self.liveness.set_dead(row);
        self.pin_row(row);
        Some(PendingRetire {
            slot: handle.index(),
            row,
            next_gen: handle.generation() + 1,
        })
    }

    /// Complete deferred retirement: unpin the row (compactable) and run the
    /// registry tail (generation bump + slot pooling). The caller must have
    /// written `pending.next_gen` to the VRAM generation buffer FIRST (C6).
    pub(crate) fn commit_retire(&mut self, pending: PendingRetire) {
        self.unpin_row(pending.row);
        let new_gen = self.registry.commit_retire(pending.slot);
        debug_assert_eq!(
            new_gen, pending.next_gen,
            "generation drift between mark and commit"
        );
    }

    /// Frame-boundary swap-and-pop compaction (spec §4.4). Public form of
    /// [`compact_report`] without move observation.
    ///
    /// For cells mirrored by a `gpu::SceneGpuStore`, call `SceneGpuStore::compact_all`
    /// instead — direct compaction skips dirty-marking of moved rows.
    pub fn compact(&mut self) {
        self.compact_report(|_, _| {});
    }

    /// Compaction that reports every `(from_row, to_row)` move so the GPU
    /// layer can mark destination rows dirty (M2a §4). Pinned rows (in-flight
    /// retirement, M2a §5) are neither swapped away nor filled into, and a
    /// pinned tail stops the pop frontier: holes behind it persist until the
    /// pin clears — `retire()` runs before `compact()` at the boundary, so
    /// steady-state pins are already drained.
    pub(crate) fn compact_report(&mut self, mut on_move: impl FnMut(u32, u32)) {
        let mut len = self.page.len();
        let mut row = 0u32;
        while row < len {
            if self.liveness.is_live(row) || self.is_row_pinned(row) {
                row += 1;
                continue;
            }
            // Shrink trailing dead rows first (stop at pinned rows — they
            // cannot pop). set_dead keeps the ≥len-all-dead invariant that
            // M1b's GPU liveness upload relies on.
            while len > row + 1 && !self.liveness.is_live(len - 1) && !self.is_row_pinned(len - 1) {
                len -= 1;
                self.liveness.set_dead(len);
                self.page.pop_row();
            }
            if len == row + 1 {
                // The dead row is the tail. pop_row drops it; page.len() is now
                // `row`, which equals the surviving live count (rows 0..row are all
                // live). The local `len` is dead after `break`, so we don't decrement it.
                self.page.pop_row();
                break;
            }
            let last = len - 1;
            if !self.liveness.is_live(last) {
                // `last` is pinned (the shrink loop above consumed every
                // unpinned-dead tail row). Nothing past it can move or pop
                // this frame; the hole at `row` persists until unpin.
                break;
            }
            // Swap last (live) row into the hole, column by column.
            self.swap_rows(row, last);
            // Fix the moved element's slot→row mapping.
            let moved_slot = self.page.column_slice::<u32>(0)[row as usize];
            self.registry.set_row(moved_slot, row);
            self.liveness.set_live(row);
            self.liveness.set_dead(last);
            on_move(last, row);
            len -= 1;
            self.page.pop_row();
            row += 1;
        }
    }

    /// Byte-wise swap of two rows across every physical column and every
    /// generic column.
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
        // Swap generic column entries.
        for gc in self.page.generic_columns_mut() {
            gc.swap(a, b);
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

    /// For GPU-mirrored columns, write through `gpu::SceneGpuStore::write_transform`
    /// instead — raw writes here bypass dirty tracking and leave VRAM stale
    /// (enforced by the M2b phase machine; convention until then).
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

    #[test]
    fn pending_retire_keeps_handle_resolvable_but_not_live() {
        let mut c = cell();
        let h = c.alloc().unwrap();
        let p = c.mark_pending_retire(h).unwrap();
        assert_eq!(p.slot, h.index());
        assert_eq!(p.next_gen, h.generation() + 1);
        // In-flight window: row still resolvable (GPU's last harvest is valid)…
        assert_eq!(c.row_of(h), Some(p.row));
        assert!(c.is_row_pinned(p.row));
        // …but excluded from liveness (won't appear in new harvests).
        assert_eq!(c.live_count(), 0);
        // Double-mark is rejected.
        assert!(c.mark_pending_retire(h).is_none());
    }

    #[test]
    fn commit_retire_rejects_stale_handle_and_recycles_slot() {
        let mut c = cell();
        let h = c.alloc().unwrap();
        let p = c.mark_pending_retire(h).unwrap();
        let row = p.row;
        c.commit_retire(p);
        assert!(!c.is_row_pinned(row));
        assert_eq!(c.row_of(h), None, "stale after commit");
        let h2 = c.alloc().unwrap();
        assert_eq!(h2.index(), h.index(), "slot recycled only after commit");
        assert_eq!(h2.generation(), h.generation() + 1);
    }

    #[test]
    fn compact_reports_moves() {
        let mut c = cell();
        let hs: Vec<_> = (0..4).map(|_| c.alloc().unwrap()).collect();
        c.free(hs[1]);
        let mut moves = Vec::new();
        c.compact_report(|from, to| moves.push((from, to)));
        assert_eq!(moves, vec![(3, 1)], "last live row fills the hole");
        assert_eq!(c.rows_in_use(), 3);
    }

    #[test]
    fn pinned_row_survives_compaction_in_place() {
        let mut c = cell();
        let ha = c.alloc().unwrap();
        let hb = c.alloc().unwrap();
        let hc = c.alloc().unwrap();
        let row_b = c.row_of(hb).unwrap();
        let p = c.mark_pending_retire(hb).unwrap(); // dead but pinned
        c.free(ha); // dead, unpinned → compactable hole at row 0
        c.compact();
        // Physical survival: hc (moved into row 0) + pinned hb = 2 rows. A
        // pin-ignoring compaction would tail-pop hb's row without touching
        // the registry mapping, so row_of alone cannot catch that.
        assert_eq!(c.rows_in_use(), 2, "pinned row physically survives compaction");
        // Pinned row untouched at its original index; its bytes are preserved.
        assert!(c.is_row_pinned(row_b));
        assert_eq!(c.row_of(hb), Some(row_b), "pinned row not moved");
        // hc filled ha's hole:
        assert_eq!(c.row_of(hc), Some(0));
        // After commit, a second compact reclaims the row.
        c.commit_retire(p);
        c.compact();
        assert_eq!(c.rows_in_use(), 1);
    }

    #[test]
    fn pinned_tail_blocks_pop_leaving_hole() {
        let mut c = cell();
        let ha = c.alloc().unwrap();
        let hb = c.alloc().unwrap(); // tail row 1
        let _p = c.mark_pending_retire(hb).unwrap(); // pinned tail
        c.free(ha); // hole at row 0
        c.compact();
        // Neither the pinned tail nor the hole can move this frame.
        assert_eq!(c.rows_in_use(), 2, "hole persists behind a pinned tail");
    }

    #[test]
    fn token_keyed_column_access() {
        use crate::cell_type::CellType;
        use crate::token::TypeToken;
        let ct = CellType::new("xy")
            .with(TypeToken::of::<f32>())
            .build()
            .unwrap();
        let mut c = CellStorage::from_cell_type(&ct, 16).unwrap();
        let h = c.alloc().unwrap();
        let row = c.row_of(h).unwrap() as usize;
        c.column_for_mut::<f32>().unwrap()[row] = 9.0;
        assert_eq!(c.column_for::<f32>().unwrap()[row], 9.0);
        // u64 is not a user column of this cell type → None.
        assert!(c.column_for::<u64>().is_none());
    }
}
