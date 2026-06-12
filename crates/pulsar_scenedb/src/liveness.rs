use std::sync::atomic::{AtomicU64, Ordering};

/// Atomic liveness bitmask — 1 bit per page element (spec §4.4, C2).
///
/// Mid-frame deletion flips a bit here; physical row removal is deferred to
/// frame-boundary compaction. Bits are set/cleared with relaxed RMW atomics:
/// cross-thread visibility of the *aggregate* mask is guaranteed by the
/// phase-boundary synchronization in Layer 2, not by per-bit ordering.
pub struct LivenessMask {
    words: Vec<AtomicU64>,
}

impl LivenessMask {
    pub fn new(capacity: u32) -> Self {
        let n_words = capacity.div_ceil(64) as usize;
        Self {
            words: (0..n_words).map(|_| AtomicU64::new(0)).collect(),
        }
    }

    #[inline]
    pub fn set_live(&self, row: u32) {
        self.words[(row / 64) as usize].fetch_or(1u64 << (row % 64), Ordering::Relaxed);
    }

    #[inline]
    pub fn set_dead(&self, row: u32) {
        self.words[(row / 64) as usize].fetch_and(!(1u64 << (row % 64)), Ordering::Relaxed);
    }

    #[inline]
    pub fn is_live(&self, row: u32) -> bool {
        self.words[(row / 64) as usize].load(Ordering::Relaxed) & (1u64 << (row % 64)) != 0
    }

    pub fn live_count(&self) -> u32 {
        self.words
            .iter()
            .map(|w| w.load(Ordering::Relaxed).count_ones())
            .sum()
    }

    /// Iterate dead row indices in `[0, len)` — the compaction work list.
    pub fn dead_rows(&self, len: u32) -> impl Iterator<Item = u32> + '_ {
        (0..len).filter(move |&row| !self.is_live(row))
    }

    /// Raw word access (uploaded alongside columns for GPU-side liveness).
    pub fn words(&self) -> &[AtomicU64] {
        &self.words
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_mask_is_all_dead() {
        let m = LivenessMask::new(256);
        assert_eq!(m.live_count(), 0);
        assert!(!m.is_live(0));
    }

    #[test]
    fn mark_live_and_dead() {
        let m = LivenessMask::new(256);
        m.set_live(3);
        m.set_live(64); // second word
        m.set_live(255);
        assert!(m.is_live(3) && m.is_live(64) && m.is_live(255));
        assert_eq!(m.live_count(), 3);
        m.set_dead(64);
        assert!(!m.is_live(64));
        assert_eq!(m.live_count(), 2);
    }

    #[test]
    fn dead_rows_iterates_marked_only() {
        let m = LivenessMask::new(128);
        for i in 0..10 {
            m.set_live(i);
        }
        m.set_dead(2);
        m.set_dead(7);
        let dead: Vec<u32> = m.dead_rows(10).collect();
        assert_eq!(dead, vec![2, 7]);
    }

    #[test]
    fn concurrent_marking_is_safe() {
        use std::sync::Arc;
        let m = Arc::new(LivenessMask::new(1024));
        for i in 0..1024 {
            m.set_live(i);
        }
        let handles: Vec<_> = (0..8)
            .map(|t| {
                let m = Arc::clone(&m);
                std::thread::spawn(move || {
                    for i in (t..1024).step_by(8) {
                        m.set_dead(i as u32);
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(m.live_count(), 0);
    }
}
