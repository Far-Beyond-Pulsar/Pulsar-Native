use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Number of concurrent read-lease slots per cell (spec §9.2, matches the
/// bitmask width). Not bound to thread identity — acquired from a pool, so
/// dynamic pools / work-stealing / nesting all work.
pub const LEASE_SLOTS: usize = 64;

/// Per-cell atomic lease bitmask. A reader acquires a slot for the duration of
/// a query; the frame-boundary compaction checks `any_held()` is false before
/// swap-and-pop (enforced by Layer 2's phase machine in M2).
///
/// **M3-b / contract #32 addendum:** `any_held()` alone used to be able to
/// stall compaction indefinitely behind a revoked-but-undropped lease — the
/// mask bit only ever cleared via a holder's own `Drop`. `Lease::force_release`
/// (called by `gpu::HarvestPipeline::revoke_overdue` on revocation, §9.2.1)
/// now lets `any_held()` clear immediately at revocation time, independent of
/// when the holder actually drops its guard. See `force_release`'s doc for
/// the ABA hazard that a naive "just clear the bit early" fix would introduce
/// and how the idempotency guard avoids it.
pub struct LeaseMask {
    bits: AtomicU64,
}

/// RAII lease guard — releases its slot on drop (or earlier, via
/// `force_release`).
pub struct Lease<'a> {
    mask: &'a LeaseMask,
    slot: u32,
    /// §9.2.1 / contract #32: set exactly once, by whichever of
    /// `force_release`/`Drop` executes first — see `force_release`'s doc.
    force_released: AtomicBool,
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
                return Some(Lease { mask: self, slot, force_released: AtomicBool::new(false) });
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

    /// §9.2.1 / contract #32: force-release this lease's mask slot NOW,
    /// decoupled from the holder's own RAII drop. This is what lets
    /// `any_held()` clear immediately on revocation instead of waiting
    /// (possibly indefinitely) for the holder to finish and drop its guard —
    /// the gap the perf-val T6 review found: "a revoked-but-not-dropped
    /// lease still blocks an `any_held()`-gated compaction indefinitely."
    ///
    /// Idempotent and safe under a concurrent/later `Drop`: the mask bit is
    /// cleared exactly once, by whichever of {this method, `Drop`} runs
    /// first; the other observes `force_released` already set and becomes a
    /// no-op. This IS the ABA guard: without it, a slot force-released here
    /// could be reissued by `LeaseMask::acquire` (which always hands out the
    /// lowest free bit) to a brand-new, unrelated lease before the ORIGINAL
    /// holder's `Drop` finally runs; that `Drop`'s unconditional bit-clear
    /// would then silently un-hold the NEW lease's slot instead of a
    /// no-longer-existent one — a real correctness hazard that a naive
    /// "just clear the bit in `revoke_overdue`" fix would introduce.
    pub(crate) fn force_release(&self) {
        if self.force_released.swap(true, Ordering::AcqRel) {
            return; // already released, by a prior call or by Drop
        }
        self.mask.release(self.slot);
    }
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        if self.force_released.swap(true, Ordering::AcqRel) {
            return; // already force-released (§9.2.1 revocation) — no-op
        }
        self.mask.release(self.slot);
    }
}

/// Thread-local scratchpad with the 8-frame / 50% decay policy (spec §9.1).
/// Holds reusable query buffers so the harvest path never touches the heap
/// mid-frame after warm-up.
///
/// The `u32` buffer (`get_u32`) backs query token output (M1b). The `u64`
/// buffer (`get_u64`) backs liveness-word scratch and landed in M2a; wiring
/// it into the harvest path (replacing the per-call `Vec<u64>` snapshot in
/// `query_aabb`/`query_frustum`) is M2b scope.
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

    /// Borrow both scratch buffers simultaneously. `get_u32`/`get_u64` are
    /// exclusive `&mut self` borrows and can't be held at once; the harvest
    /// path needs token output space (`u32`) and a liveness-word snapshot
    /// (`u64`) live together (spec §8.1). `u32_buf` and `u64_buf` are
    /// disjoint fields, so borrowing both from `&mut self` simultaneously is
    /// a legal field-level split borrow. Grows both buffers first, then
    /// returns non-overlapping slices; neither is zeroed. Updates both peaks.
    pub fn get_u32_u64(&mut self, len32: usize, len64: usize) -> (&mut [u32], &mut [u64]) {
        if self.u32_buf.len() < len32 {
            self.u32_buf.resize(len32, 0);
        }
        if self.u64_buf.len() < len64 {
            self.u64_buf.resize(len64, 0);
        }
        self.peak_this_window = self.peak_this_window.max(len32);
        self.peak_u64_this_window = self.peak_u64_this_window.max(len64);
        (&mut self.u32_buf[..len32], &mut self.u64_buf[..len64])
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

    /// §9.2.1 / contract #32: `force_release` must clear `any_held()`
    /// immediately, before the holder's own `Drop` ever runs. Mutation-kill
    /// shape: delete the `force_release` call site (or its body) and this
    /// assert fails, since the bit would only clear on `drop(lease)` below.
    #[test]
    fn force_release_clears_any_held_before_drop() {
        let mask = LeaseMask::new();
        let lease = mask.acquire().unwrap();
        assert!(mask.any_held());
        lease.force_release();
        assert!(!mask.any_held(), "force_release must clear any_held() immediately, ahead of Drop");
        drop(lease); // must not panic and must not touch a reissued slot (next test)
        assert!(!mask.any_held());
    }

    /// The ABA hazard `force_release`'s idempotency guard exists to prevent:
    /// force-release a slot, let a DIFFERENT lease claim the same slot index
    /// (the mask always hands out the lowest free bit), then let the
    /// ORIGINAL lease drop late. Without the guard, that late `Drop` would
    /// blindly clear the bit again — silently un-holding the new lease.
    #[test]
    fn force_release_then_late_drop_does_not_reclaim_a_reissued_slot() {
        let mask = LeaseMask::new();
        let a = mask.acquire().unwrap();
        let slot = a.slot();
        a.force_release();
        assert!(!mask.any_held());
        let b = mask.acquire().unwrap();
        assert_eq!(b.slot(), slot, "lowest-free-bit acquire reissues the just-freed slot");
        assert!(mask.any_held(), "b now genuinely holds the slot");
        // `a`'s late drop must be a no-op — must NOT release b's slot.
        drop(a);
        assert!(mask.any_held(), "a's late drop must not clear b's still-live slot (ABA guard)");
        drop(b);
        assert!(!mask.any_held(), "b's own drop releases it normally");
    }

    /// Calling `force_release` twice (e.g. a repeated revoke sweep against
    /// an already-revoked lease) must not panic and must not affect a
    /// meanwhile-reissued slot.
    #[test]
    fn force_release_is_idempotent_under_repeated_calls() {
        let mask = LeaseMask::new();
        let a = mask.acquire().unwrap();
        a.force_release();
        a.force_release(); // second call: no-op, must not panic
        assert!(!mask.any_held());
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
    fn split_borrow_returns_both_buffers() {
        let mut pad = Scratchpad::new();
        let (t, w) = pad.get_u32_u64(100, 4);
        t[0] = 7;
        w[0] = 0xFF;
        assert!(t.len() >= 100 && w.len() >= 4);
        assert!(pad.buf_len_u32() >= 100 && pad.buf_len_u64() >= 4);
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
