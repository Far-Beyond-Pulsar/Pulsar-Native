use crate::handle::Handle;

/// Sentinel row meaning "slot has no live row" (also the null token, C4).
pub const NULL_ROW: u32 = 0xFFFF_FFFF;

/// Slot allocator + generation validator + slot→row indirection (spec §3,
/// CONTRACTS.md C1). One instance per cell.
///
/// Invariants:
/// - `generations[slot]` is the live generation if the slot is allocated,
///   or the generation a recycled allocation *will get* if free.
/// - `slot_to_row[slot] == NULL_ROW` iff the slot is unallocated.
/// - A slot whose generation reaches `u32::MAX` on free is permanently
///   retired (never pushed back to the free pool) — see spec §3.2. Its
///   `generations[slot]` entry stays at `u32::MAX` as a tombstone.
pub struct HandleRegistry {
    generations: Vec<u32>,
    slot_to_row: Vec<u32>,
    free: Vec<u32>,
    retired_count: u32,
}

impl HandleRegistry {
    pub fn new() -> Self {
        Self {
            generations: Vec::new(),
            slot_to_row: Vec::new(),
            free: Vec::new(),
            retired_count: 0,
        }
    }

    /// Allocate a slot pointing at `row`. Returns the new live handle.
    pub fn allocate(&mut self, row: u32) -> Handle {
        if let Some(slot) = self.free.pop() {
            let gen = self.generations[slot as usize];
            self.slot_to_row[slot as usize] = row;
            return Handle::new(slot, gen);
        }
        let slot = self.generations.len() as u32;
        self.generations.push(1);
        self.slot_to_row.push(row);
        Handle::new(slot, 1)
    }

    /// Free a live handle. Returns false for stale/invalid/double-free.
    /// The slot's generation is bumped immediately; if it would reach
    /// u32::MAX the slot is permanently retired instead of pooled.
    pub fn free(&mut self, handle: Handle) -> bool {
        if !self.is_live(handle) {
            return false;
        }
        let slot = handle.index() as usize;
        self.slot_to_row[slot] = NULL_ROW;
        debug_assert!(
            handle.generation() < u32::MAX,
            "generation overflow on slot {}: a slot reaching u32::MAX must already be retired",
            handle.index()
        );
        let next = handle.generation() + 1;
        self.generations[slot] = next;
        if next == u32::MAX {
            self.retired_count += 1;
        } else {
            self.free.push(handle.index());
        }
        true
    }

    /// Current row for a handle, validating the generation. None if stale,
    /// invalid, or freed.
    #[inline]
    pub fn row_of(&self, handle: Handle) -> Option<u32> {
        if !self.is_live(handle) {
            return None;
        }
        Some(self.slot_to_row[handle.index() as usize])
    }

    #[inline]
    pub fn is_live(&self, handle: Handle) -> bool {
        if !handle.is_valid() {
            return false;
        }
        let slot = handle.index() as usize;
        slot < self.generations.len()
            && self.generations[slot] == handle.generation()
            && self.slot_to_row[slot] != NULL_ROW
    }

    /// Redirect a slot to a new row. Called by frame-boundary compaction
    /// when swap-and-pop moves an element (spec §4.4).
    ///
    /// Callers must guarantee `slot` was allocated and is currently live.
    #[inline]
    pub fn set_row(&mut self, slot: u32, row: u32) {
        debug_assert!(
            (slot as usize) < self.slot_to_row.len(),
            "set_row: slot {} out of range (len={})",
            slot,
            self.slot_to_row.len()
        );
        debug_assert!(
            self.slot_to_row[slot as usize] != NULL_ROW,
            "set_row: slot {} is not live (freed or never allocated)",
            slot
        );
        self.slot_to_row[slot as usize] = row;
    }

    /// Read-only view of the generation array (uploaded to the VRAM
    /// validation buffer in Layer 2/3). Retired slots hold `u32::MAX`;
    /// `is_live` still rejects them via the slot→row `NULL_ROW` check.
    pub fn generations(&self) -> &[u32] {
        &self.generations
    }

    pub fn retired_count(&self) -> u32 {
        self.retired_count
    }

    /// Test hook: force a slot's stored generation (used to exercise the
    /// u32::MAX retirement path without 4.3 B iterations).
    #[cfg(test)]
    pub(crate) fn force_generation(&mut self, slot: u32, gen: u32) {
        self.generations[slot as usize] = gen;
    }
}

impl Default for HandleRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocate_starts_at_generation_one() {
        let mut reg = HandleRegistry::new();
        let h = reg.allocate(7);
        assert_eq!(h.generation(), 1);
        assert_eq!(reg.row_of(h), Some(7));
    }

    #[test]
    fn stale_handle_rejected_after_free() {
        let mut reg = HandleRegistry::new();
        let h1 = reg.allocate(0);
        assert!(reg.free(h1));
        assert_eq!(reg.row_of(h1), None, "stale handle must not resolve");
        let h2 = reg.allocate(0);
        assert_eq!(h2.index(), h1.index(), "slot is recycled");
        assert_eq!(h2.generation(), h1.generation() + 1);
        assert_eq!(reg.row_of(h2), Some(0));
    }

    #[test]
    fn double_free_rejected() {
        let mut reg = HandleRegistry::new();
        let h = reg.allocate(0);
        assert!(reg.free(h));
        assert!(!reg.free(h), "second free of the same handle must fail");
    }

    #[test]
    fn invalid_handle_never_resolves() {
        let reg = HandleRegistry::new();
        assert_eq!(reg.row_of(Handle::INVALID), None);
    }

    #[test]
    fn slot_retired_at_generation_max() {
        let mut reg = HandleRegistry::new();
        let h = reg.allocate(0);
        let slot = h.index();
        reg.force_generation(slot, u32::MAX - 1); // test hook
        let h = Handle::new(slot, u32::MAX - 1);
        assert!(reg.free(h));
        // Recycling this slot would need gen u32::MAX → permanently retired.
        let h2 = reg.allocate(0);
        assert_ne!(h2.index(), slot, "retired slot must never be reissued");
        assert_eq!(h2.generation(), 1, "a fresh slot starts at generation 1");
    }

    #[test]
    fn set_row_redirects_lookup() {
        let mut reg = HandleRegistry::new();
        let h = reg.allocate(5);
        reg.set_row(h.index(), 2);
        assert_eq!(reg.row_of(h), Some(2));
    }
}
