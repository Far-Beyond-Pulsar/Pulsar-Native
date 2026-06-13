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
    /// Capture `len` rows of `mask` into an owned snapshot (relaxed loads;
    /// the caller holds the phase barrier).
    #[must_use]
    pub fn capture(mask: &LivenessMask, len: u32) -> Self {
        let words = mask.words().iter().map(|w| w.load(Ordering::Relaxed)).collect();
        Self { words, len }
    }

    #[inline]
    #[must_use]
    pub fn is_live(&self, row: u32) -> bool {
        row < self.len && self.words[(row / 64) as usize] & (1u64 << (row % 64)) != 0
    }

    #[must_use]
    pub fn live_count(&self) -> u32 {
        self.words.iter().map(|w| w.count_ones()).sum::<u32>().min(self.len)
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
    fn revocation_flag_round_trips() {
        let rev = RevocationFlag::new();
        assert!(!rev.is_revoked());
        rev.revoke();
        assert!(rev.is_revoked());
    }
}
