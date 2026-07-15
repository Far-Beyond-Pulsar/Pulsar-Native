//! Per-cell row dirty bitmask (design Rev 2 §2): dirty state lives beside the
//! CELL, not inside the global buffer — the same atomic-word shape as M1's
//! LivenessMask. Relaxed ordering per the M2a contract: the frame-boundary
//! join provides the happens-before edge between column writes and sync.

use std::sync::atomic::{AtomicU64, Ordering};

pub struct DirtyMask {
    words: Vec<AtomicU64>,
    capacity: u32,
}

impl DirtyMask {
    pub fn new(capacity: u32) -> Self {
        let words = (0..capacity.div_ceil(64)).map(|_| AtomicU64::new(0)).collect();
        Self { words, capacity }
    }

    #[inline]
    pub fn mark(&self, row: u32) {
        debug_assert!(row < self.capacity, "row {row} beyond mask capacity {}", self.capacity);
        self.words[(row / 64) as usize].fetch_or(1u64 << (row % 64), Ordering::Relaxed);
    }

    #[inline]
    pub fn is_marked(&self, row: u32) -> bool {
        self.words[(row / 64) as usize].load(Ordering::Relaxed) & (1u64 << (row % 64)) != 0
    }

    /// Mark rows `0..rows` (promotion warm-up: full-region resync, §4.1).
    pub fn mark_range(&self, rows: u32) {
        for row in 0..rows {
            self.mark(row);
        }
    }

    pub fn clear_all(&self) {
        for w in &self.words {
            w.store(0, Ordering::Relaxed);
        }
    }

    pub fn capacity(&self) -> u32 {
        self.capacity
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_marks_and_clears() {
        let m = DirtyMask::new(130);
        m.mark(0);
        m.mark(129);
        assert!(m.is_marked(0) && m.is_marked(129) && !m.is_marked(64));
        m.clear_all();
        assert!(!m.is_marked(0) && !m.is_marked(129));
        m.mark_range(65);
        assert!(m.is_marked(64) && !m.is_marked(65));
    }
}
