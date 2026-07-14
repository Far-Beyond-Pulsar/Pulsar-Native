use std::sync::atomic::{AtomicU64, Ordering};

/// Number of concurrent read-lease slots per cell (spec §9.2, matches the
/// bitmask width). Not bound to thread identity — acquired from a pool, so
/// dynamic pools / work-stealing / nesting all work.
pub const LEASE_SLOTS: usize = 64;

/// Per-cell atomic lease bitmask. A reader acquires a slot for the duration of
/// a query; the frame-boundary compaction checks `any_held()` is false before
/// swap-and-pop (enforced by Layer 2's phase machine in M2).
pub struct LeaseMask {
    bits: AtomicU64,
}

/// RAII lease guard — releases its slot on drop.
pub struct Lease<'a> {
    mask: &'a LeaseMask,
    slot: u32,
}

impl LeaseMask {
    #[must_use]
    pub fn new() -> Self {
        Self { bits: AtomicU64::new(0) }
    }

    /// Acquire a free lease slot, or None if the pool is exhausted.
    pub fn acquire(&self) -> Option<Lease<'_>> {
        loop {
            let cur = self.bits.load(Ordering::Acquire);
            if cur == u64::MAX {
                return None; // all 64 slots held
            }
            let slot = cur.trailing_ones(); // first 0 bit
            let bit = 1u64 << slot;
            if self
                .bits
                .compare_exchange_weak(cur, cur | bit, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Some(Lease { mask: self, slot });
            }
        }
    }

    #[must_use]
    pub fn any_held(&self) -> bool {
        self.bits.load(Ordering::Acquire) != 0
    }

    fn release(&self, slot: u32) {
        self.bits.fetch_and(!(1u64 << slot), Ordering::AcqRel);
    }
}

impl Default for LeaseMask {
    fn default() -> Self {
        Self::new()
    }
}

impl Lease<'_> {
    #[inline]
    #[must_use]
    pub fn slot(&self) -> u32 {
        self.slot
    }
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        self.mask.release(self.slot);
    }
}

/// Thread-local scratchpad with the 8-frame / 50% decay policy (spec §9.1).
/// Holds reusable query buffers so the harvest path never touches the heap
/// mid-frame after warm-up.
///
/// Note: M1b provides only a `u32` buffer (`get_u32`) for query token output.
/// The M2 harvest path also needs `u64` liveness-word scratch (to replace the
/// per-call `Vec<u64>` snapshot in `query_aabb`/`query_frustum`); a `get_u64`
/// companion is deferred to M2.
pub struct Scratchpad {
    u32_buf: Vec<u32>,
    u64_buf: Vec<u64>,
    peak_this_window: usize,
    peak_u64_this_window: usize,
    frames_in_window: u32,
}

/// Frames of sustained low usage before halving (spec §9.1 default).
pub const DECAY_FRAMES: u32 = 8;

impl Scratchpad {
    #[must_use]
    pub fn new() -> Self {
        Self {
            u32_buf: Vec::new(),
            u64_buf: Vec::new(),
            peak_this_window: 0,
            peak_u64_this_window: 0,
            frames_in_window: 0,
        }
    }

    /// Borrow a u32 buffer of at least `len`, growing if needed. The buffer is
    /// not zeroed (callers overwrite `[0..used]`).
    pub fn get_u32(&mut self, len: usize) -> &mut [u32] {
        if self.u32_buf.len() < len {
            self.u32_buf.resize(len, 0);
        }
        self.peak_this_window = self.peak_this_window.max(len);
        &mut self.u32_buf[..len]
    }

    /// Logical size of the u32 scratch buffer (number of elements it currently
    /// maintains; grows on demand, shrinks on decay).
    #[must_use]
    pub fn buf_len_u32(&self) -> usize {
        self.u32_buf.len()
    }

    /// Borrow a u64 buffer of at least `len` (liveness words / dirty words;
    /// the M1b §8.1 carry-forward). Not zeroed.
    pub fn get_u64(&mut self, len: usize) -> &mut [u64] {
        if self.u64_buf.len() < len {
            self.u64_buf.resize(len, 0);
        }
        self.peak_u64_this_window = self.peak_u64_this_window.max(len);
        &mut self.u64_buf[..len]
    }

    /// Logical size of the u64 scratch buffer (number of elements it currently
    /// maintains; grows on demand, shrinks on decay).
    #[must_use]
    pub fn buf_len_u64(&self) -> usize {
        self.u64_buf.len()
    }

    /// Advance the decay window. After `DECAY_FRAMES` frames whose peak usage
    /// stayed below 50% of the buffer size, truncates the buffer to half and
    /// *requests* that the allocator release the surplus (via `shrink_to_fit`;
    /// not guaranteed to return memory immediately).
    pub fn end_frame(&mut self) {
        self.frames_in_window += 1;
        if self.frames_in_window >= DECAY_FRAMES {
            let cap = self.u32_buf.len();
            if cap > 0 && self.peak_this_window * 2 < cap {
                let new_cap = cap / 2;
                self.u32_buf.truncate(new_cap);
                self.u32_buf.shrink_to_fit();
            }
            let cap_u64 = self.u64_buf.len();
            if cap_u64 > 0 && self.peak_u64_this_window * 2 < cap_u64 {
                let new_cap = cap_u64 / 2;
                self.u64_buf.truncate(new_cap);
                self.u64_buf.shrink_to_fit();
            }
            self.frames_in_window = 0;
            self.peak_this_window = 0;
            self.peak_u64_this_window = 0;
        }
    }
}

impl Default for Scratchpad {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_release_lease_slots() {
        let mask = LeaseMask::new();
        let a = mask.acquire().unwrap();
        let b = mask.acquire().unwrap();
        assert_ne!(a.slot(), b.slot());
        assert!(mask.any_held());
        drop(a);
        drop(b);
        assert!(!mask.any_held(), "all leases released");
    }

    #[test]
    fn pool_exhaustion_returns_none() {
        let mask = LeaseMask::new();
        let mut held = Vec::new();
        for _ in 0..LEASE_SLOTS {
            held.push(mask.acquire().unwrap());
        }
        assert!(mask.acquire().is_none(), "65th acquire fails on a full pool");
        drop(held);
        assert!(mask.acquire().is_some(), "slot frees after release");
    }

    #[test]
    fn scratchpad_grows_then_decays() {
        let mut pad = Scratchpad::new();
        // Burst: request a big buffer.
        {
            let buf = pad.get_u32(1000);
            assert!(buf.len() >= 1000);
        }
        let cap_before = pad.buf_len_u32();
        assert!(cap_before >= 1000);
        // First decay window: the burst's peak (1000) lands in THIS window, so
        // peak*2 >= cap → no decay (the window's peak must drop below 50% first).
        for _ in 0..DECAY_FRAMES {
            let _ = pad.get_u32(10);
            pad.end_frame();
        }
        // Second decay window: sustained low use (peak 10 << 50% of cap) → halve.
        for _ in 0..DECAY_FRAMES {
            let _ = pad.get_u32(10);
            pad.end_frame();
        }
        assert!(pad.buf_len_u32() < cap_before, "capacity decayed after a low-usage window");
    }

    #[test]
    fn scratchpad_u64_grows_and_decays_independently() {
        let mut pad = Scratchpad::new();
        {
            let b = pad.get_u64(500);
            assert!(b.len() >= 500);
        }
        let cap = pad.buf_len_u64();
        assert!(cap >= 500);
        // u32 buffer untouched by u64 usage:
        assert_eq!(pad.buf_len_u32(), 0);
        for _ in 0..(2 * DECAY_FRAMES) {
            let _ = pad.get_u64(8);
            pad.end_frame();
        }
        assert!(pad.buf_len_u64() < cap, "u64 buffer decayed");
    }
}
