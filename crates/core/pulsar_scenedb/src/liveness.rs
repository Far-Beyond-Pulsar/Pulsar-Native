use std::sync::atomic::{AtomicU64, Ordering};

/// Atomic liveness bitmask — 1 bit per page element (spec §4.4, C2).
///
/// Mid-frame deletion flips a bit here; physical row removal is deferred to
/// frame-boundary compaction. Bits are set/cleared with relaxed RMW atomics:
/// cross-thread visibility of the *aggregate* mask is guaranteed by the
/// phase-boundary synchronization in Layer 2, not by per-bit ordering.
///
/// # Memory ordering contract
///
/// `set_live` / `set_dead` use `Relaxed` atomics intentionally. Correct
/// visibility to harvest-phase readers is NOT self-contained here; it depends
/// on a Layer 2 phase-boundary barrier emitting a **Release** fence after all
/// simulation-phase writes complete, and every harvest-phase reader acquiring
/// through an **Acquire** fence before calling `is_live`, `live_count`, or
/// `dead_rows`.
///
/// **§9.2.1: the fence is owned.** `pulsar_scenedb::gpu::phase` is the fence
/// owner — `SimulateB::end` emits the `Release` fence that publishes every
/// simulate-phase write to this mask; `HarvestPhase::end` and
/// `BoundaryPhase::retire` each emit the paired `Acquire` fence before any
/// boundary/harvest code observes it. Any caller that drives a frame through
/// that phase machine gets this ordering for free. The residual warning is
/// for callers who read or write `LivenessMask` OUTSIDE the phase machine
/// (there are none in-crate today): without going through
/// `SimulateB::end`/`HarvestPhase::end`/`BoundaryPhase::retire`, a harvest
/// reader on another core may still observe a stale word — a `Relaxed` load
/// may return any previously stored value. That remains a silent correctness
/// bug, not a compile error, for anyone who bypasses the phase machine.
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

    /// Marks `row` live. `row` must be `< capacity` (caller contract).
    #[inline]
    pub fn set_live(&self, row: u32) {
        debug_assert!((row / 64) < self.words.len() as u32, "row {row} out of range");
        self.words[(row / 64) as usize].fetch_or(1u64 << (row % 64), Ordering::Relaxed);
    }

    /// Marks `row` dead (deferred — physical removal happens at compaction).
    /// `row` must be `< capacity` (caller contract).
    #[inline]
    pub fn set_dead(&self, row: u32) {
        debug_assert!((row / 64) < self.words.len() as u32, "row {row} out of range");
        self.words[(row / 64) as usize].fetch_and(!(1u64 << (row % 64)), Ordering::Relaxed);
    }

    #[inline]
    pub fn is_live(&self, row: u32) -> bool {
        self.words[(row / 64) as usize].load(Ordering::Relaxed) & (1u64 << (row % 64)) != 0
    }

    /// Number of live elements. Must only be called in the harvest phase
    /// (no concurrent `set_live`/`set_dead`); the result is not a consistent
    /// snapshot if writers run concurrently.
    pub fn live_count(&self) -> u32 {
        self.words
            .iter()
            .map(|w| w.load(Ordering::Relaxed).count_ones())
            .sum()
    }

    /// Iterate dead row indices in `[0, len)` — the compaction work list.
    ///
    /// Harvest-phase only (no concurrent writers). `len` must not exceed the
    /// mask capacity rounded up to a multiple of 64.
    pub fn dead_rows(&self, len: u32) -> impl Iterator<Item = u32> + '_ {
        debug_assert!(len as usize <= self.words.len() * 64, "len {len} exceeds mask capacity");
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
        // join() above provides the happens-before that makes this live_count()
        // well-defined; in production the Layer 2 phase barrier plays that role.
        assert_eq!(m.live_count(), 0);
    }
}
