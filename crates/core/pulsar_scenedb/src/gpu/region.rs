//! Size-class region pools (design Rev 2 §2/§7): fixed-size regions of the
//! global row/slot spaces, O(1) alloc/free, with serial-pinned free — the
//! ONLY serial pinning in M2b (§6.1; row-granularity harvest pins are
//! forbidden by design).

use std::collections::VecDeque;

/// Hard region-exhaustion errors (§8): surfaced to the caller, never a realloc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionError {
    RowsExhausted,
    SlotsExhausted,
}

pub struct RegionPool {
    base_offset: u32,
    region_size: u32,
    count: u32,
    free: Vec<u32>,
    /// (region_base, submission serial) — reusable once the serial completes.
    pinned: VecDeque<(u32, u64)>,
}

impl RegionPool {
    pub fn new(base_offset: u32, region_size: u32, count: u32) -> Self {
        // LIFO free list; reverse so the first alloc returns the lowest base
        // (deterministic tests, better locality).
        let free = (0..count).rev().map(|i| base_offset + i * region_size).collect();
        Self { base_offset, region_size, count, free, pinned: VecDeque::new() }
    }

    pub fn alloc(&mut self) -> Option<u32> {
        self.free.pop()
    }

    /// Queue a region for reuse once `serial` completes (§4.1 eviction).
    pub fn free_pinned(&mut self, base: u32, serial: u64) {
        debug_assert!(
            base >= self.base_offset
                && (base - self.base_offset) % self.region_size == 0
                && (base - self.base_offset) / self.region_size < self.count,
            "region base {base} does not belong to this pool"
        );
        debug_assert!(
            !self.free.contains(&base) && !self.pinned.iter().any(|&(b, _)| b == base),
            "double free of region {base}"
        );
        self.pinned.push_back((base, serial));
    }

    /// Recycle every pinned region whose serial is complete. Returns count.
    pub fn drain_completed(&mut self, completed: u64) -> u32 {
        let mut drained = 0;
        // Serials are not guaranteed monotone across cells; scan the whole queue.
        let mut i = 0;
        while i < self.pinned.len() {
            if self.pinned[i].1 <= completed {
                let (base, _) = self.pinned.remove(i).unwrap();
                self.free.push(base);
                drained += 1;
            } else {
                i += 1;
            }
        }
        drained
    }

    pub fn region_size(&self) -> u32 {
        self.region_size
    }

    pub fn free_count(&self) -> u32 {
        self.free.len() as u32
    }

    /// Total number of regions this pool manages (allocated + free + pinned).
    pub fn total_regions(&self) -> u32 {
        self.count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_exhausts_then_none() {
        let mut p = RegionPool::new(1000, 256, 2);
        let a = p.alloc().unwrap();
        let b = p.alloc().unwrap();
        assert_ne!(a, b);
        for base in [a, b] {
            assert!(base == 1000 || base == 1256, "bases offset by region_size from base_offset");
        }
        assert_eq!(p.alloc(), None, "exhausted pool");
    }

    #[test]
    fn pinned_free_returns_only_after_serial_completes() {
        let mut p = RegionPool::new(0, 256, 1);
        let a = p.alloc().unwrap();
        p.free_pinned(a, 5);
        assert_eq!(p.alloc(), None, "pinned region not reusable");
        assert_eq!(p.drain_completed(4), 0, "serial incomplete");
        assert_eq!(p.drain_completed(5), 1);
        assert_eq!(p.alloc(), Some(a), "region recycled after completion");
    }
}
