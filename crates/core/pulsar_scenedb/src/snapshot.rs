use crate::liveness::LivenessMask;
use std::sync::atomic::{AtomicBool, Ordering};

/// A pinned, immutable copy of a cell's liveness words at capture time
/// (spec §9.2.1 double-buffered state mask). A revoked lease holder reads its
/// pinned snapshot while compaction proceeds against the live mask.
pub struct LivenessSnapshot {
    words: Vec<u64>,
    len: u32,
}

impl LivenessSnapshot {
    /// Capture `len` rows of `mask` into an owned snapshot. Copies exactly the
    /// `ceil(len/64)` words covering rows `0..len`, so `words()` satisfies the
    /// SIMD kernel contract (`liveness_words.len() == len.div_ceil(64)`).
    ///
    /// The caller must have already acquired through the Layer 2 phase-boundary
    /// Acquire fence (or equivalent); the Relaxed loads are correct only because
    /// that fence establishes happens-before for all prior `set_live`/`set_dead`
    /// writes. Calling `capture` without the barrier is a silent correctness bug.
    #[must_use]
    pub fn capture(mask: &LivenessMask, len: u32) -> Self {
        let n_words = (len as usize).div_ceil(64);
        debug_assert!(n_words <= mask.words().len(), "len exceeds mask capacity");
        let words = mask.words()[..n_words]
            .iter()
            .map(|w| w.load(Ordering::Relaxed))
            .collect();
        Self { words, len }
    }

    #[inline]
    #[must_use]
    pub fn is_live(&self, row: u32) -> bool {
        row < self.len && self.words[(row / 64) as usize] & (1u64 << (row % 64)) != 0
    }

    /// Number of live rows in `[0, len)`. Exact regardless of any stray bits
    /// beyond `len` in the final partial word.
    #[must_use]
    pub fn live_count(&self) -> u32 {
        let full_words = (self.len / 64) as usize;
        let remainder = self.len % 64;
        let mut count: u32 = self.words[..full_words].iter().map(|w| w.count_ones()).sum();
        if remainder > 0 {
            let mask = (1u64 << remainder) - 1;
            count += (self.words[full_words] & mask).count_ones();
        }
        count
    }

    /// Raw snapshot words (for SIMD scans against the pinned topology).
    #[must_use]
    pub fn words(&self) -> &[u64] {
        &self.words
    }
}

/// A one-shot revocation flag for a lease (spec §9.2.1). Set by Layer 2 when a
/// lease exceeds its timeout; the holder re-validates against live generations
/// on use after seeing it set.
pub struct RevocationFlag {
    revoked: AtomicBool,
}

impl RevocationFlag {
    #[must_use]
    pub fn new() -> Self {
        Self { revoked: AtomicBool::new(false) }
    }

    pub fn revoke(&self) {
        self.revoked.store(true, Ordering::Release);
    }

    #[must_use]
    pub fn is_revoked(&self) -> bool {
        self.revoked.load(Ordering::Acquire)
    }
}

impl Default for RevocationFlag {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::liveness::LivenessMask;

    #[test]
    fn snapshot_pins_liveness_at_capture_time() {
        let mask = LivenessMask::new(128);
        for i in 0..10 { mask.set_live(i); }
        let snap = LivenessSnapshot::capture(&mask, 10);
        // Mutate the live mask after the snapshot.
        mask.set_dead(3);
        // Snapshot still reflects capture-time state.
        assert!(snap.is_live(3), "snapshot is pinned");
        assert!(!mask.is_live(3), "live mask moved on");
        assert_eq!(snap.live_count(), 10);
    }

    #[test]
    fn live_count_ignores_stray_bits_beyond_len() {
        let mask = LivenessMask::new(128);
        for i in 0..5 { mask.set_live(i); }
        // Stray bits within the captured word but beyond len → must not count.
        mask.set_live(60);
        mask.set_live(61);
        let snap = LivenessSnapshot::capture(&mask, 10);
        assert_eq!(snap.live_count(), 5, "only rows [0,10) count");
    }

    #[test]
    fn revocation_flag_round_trips() {
        let rev = RevocationFlag::new();
        assert!(!rev.is_revoked());
        rev.revoke();
        assert!(rev.is_revoked());
    }
}
