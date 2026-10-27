//! Large-allocation registry: every *live* heap allocation at or above a size
//! threshold, each with a deep call stack.
//!
//! [`caller_tracking`](super::caller_tracking) aggregates by call site and keeps
//! only a few frames, which is right for "who allocates the most" but useless for
//! "who is holding this one 64 MB block": the top of such a stack is
//! `RawVec::finish_grow`. This registry keeps one record per large allocation,
//! with up to [`MAX_FRAMES`] frames, so the Memory Explorer can show the whole
//! chain from the owning system down to the `Vec`.
//!
//! It runs only while allocation tracking is on (see
//! [`super::tracking_allocator`]); with tracking off nothing here is reached.
//!
//! ## Re-entrancy
//!
//! Recording happens inside the global allocator, so everything here allocates
//! only through paths the allocator already guards (the caller sets its
//! thread-local tracking flag around the call). [`build_snapshot`] iterates the
//! live map on a background thread, so it sets this module's own `BUSY` flag to
//! make allocations it triggers skip the map instead of deadlocking on the shard
//! it is reading.

use dashmap::DashMap;
use once_cell::sync::Lazy;
use std::cell::Cell;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

/// Frames kept per large allocation.
pub const MAX_FRAMES: usize = 48;
/// Frames at the top of a captured stack that belong to the tracker itself
/// (`trace_unsynchronized`, `record_alloc`, `GlobalAlloc::alloc`, the shim).
const TRACKER_FRAMES: usize = 4;
/// Smallest threshold that can be set, so `record_dealloc` can ignore every
/// smaller free without a map lookup.
pub const MIN_THRESHOLD: usize = 1024 * 1024;
pub const DEFAULT_THRESHOLD: usize = 16 * 1024 * 1024;

static THRESHOLD: AtomicUsize = AtomicUsize::new(DEFAULT_THRESHOLD);

pub fn threshold() -> usize {
    THRESHOLD.load(Ordering::Relaxed)
}

/// Set the size from which allocations are recorded (at least
/// [`MIN_THRESHOLD`]). Allocations already recorded stay recorded.
pub fn set_threshold(bytes: usize) {
    THRESHOLD.store(bytes.max(MIN_THRESHOLD), Ordering::Relaxed);
}

/// Whether an allocation of `size` bytes is recorded here.
#[inline]
pub fn wants(size: usize) -> bool {
    size >= threshold()
}

struct LargeAlloc {
    size: usize,
    frames: Box<[usize]>,
    thread: u64,
    at: Instant,
}

static LIVE: Lazy<DashMap<usize, LargeAlloc>> = Lazy::new(|| DashMap::with_capacity(256));
static LIVE_BYTES: AtomicI64 = AtomicI64::new(0);
static PEAK_BYTES: AtomicI64 = AtomicI64::new(0);
static TOTAL_ALLOCS: AtomicU64 = AtomicU64::new(0);
static TOTAL_FREES: AtomicU64 = AtomicU64::new(0);
static NEXT_THREAD: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static BUSY: Cell<bool> = const { Cell::new(false) };
    static THREAD: Cell<u64> = const { Cell::new(0) };
}

fn thread_number() -> u64 {
    THREAD.with(|t| {
        if t.get() == 0 {
            t.set(NEXT_THREAD.fetch_add(1, Ordering::Relaxed));
        }
        t.get()
    })
}

/// Record a live allocation of `size` bytes at `ptr` made from `frames` (the raw
/// return addresses, innermost first). Called from the allocator for sizes
/// where [`wants`] is true.
pub fn record_alloc(ptr: usize, size: usize, frames: &[usize]) {
    if ptr == 0 || BUSY.with(|b| b.replace(true)) {
        return;
    }
    TOTAL_ALLOCS.fetch_add(1, Ordering::Relaxed);
    let live = LIVE_BYTES.fetch_add(size as i64, Ordering::Relaxed) + size as i64;
    PEAK_BYTES.fetch_max(live, Ordering::Relaxed);
    LIVE.insert(
        ptr,
        LargeAlloc {
            size,
            frames: frames.into(),
            thread: thread_number(),
            at: Instant::now(),
        },
    );
    BUSY.with(|b| b.set(false));
}

/// Forget the allocation at `ptr`, if it was recorded.
pub fn record_dealloc(ptr: usize, size: usize) {
    if size < MIN_THRESHOLD || ptr == 0 || LIVE.is_empty() || BUSY.with(|b| b.replace(true)) {
        return;
    }
    if let Some((_, alloc)) = LIVE.remove(&ptr) {
        TOTAL_FREES.fetch_add(1, Ordering::Relaxed);
        LIVE_BYTES.fetch_sub(alloc.size as i64, Ordering::Relaxed);
    }
    BUSY.with(|b| b.set(false));
}

/// Drop every record and counter (the explorer's Clear button).
pub fn clear() {
    BUSY.with(|b| b.set(true));
    LIVE.clear();
    BUSY.with(|b| b.set(false));
    LIVE_BYTES.store(0, Ordering::Relaxed);
    PEAK_BYTES.store(0, Ordering::Relaxed);
    TOTAL_ALLOCS.store(0, Ordering::Relaxed);
    TOTAL_FREES.store(0, Ordering::Relaxed);
}

// ─── Snapshot ─────────────────────────────────────────────────────────────────

/// Live large allocations that share one call stack.
#[derive(Clone, Debug)]
pub struct LargeGroup {
    pub key: u64,
    /// The first frame that is not allocator or standard-library plumbing:
    /// where the memory is attributed.
    pub label: String,
    /// The whole stack, innermost first, tracker frames removed.
    pub stack: Vec<String>,
    pub count: usize,
    pub live_bytes: u64,
    pub largest: u64,
    pub oldest_secs: f64,
    /// Threads that made these allocations (opaque small numbers).
    pub threads: Vec<u64>,
}

#[derive(Clone, Debug, Default)]
pub struct LargeSnapshot {
    pub groups: Vec<LargeGroup>,
    pub live_count: usize,
    pub live_bytes: u64,
    pub peak_bytes: u64,
    pub total_allocs: u64,
    pub total_frees: u64,
    pub threshold: usize,
}

struct RawGroup {
    frames: Vec<usize>,
    count: usize,
    live_bytes: u64,
    largest: u64,
    oldest: Instant,
    threads: Vec<u64>,
}

fn stack_key(frames: &[usize]) -> u64 {
    let mut hasher = DefaultHasher::new();
    frames.hash(&mut hasher);
    hasher.finish()
}

/// Group the live records by call stack and resolve symbols for display. Does
/// blocking work (symbol lookup); call it off the UI thread.
pub fn build_snapshot() -> LargeSnapshot {
    // 1. Group raw addresses while holding shard locks: no symbol work, and
    //    allocations this thread makes skip the map (BUSY).
    BUSY.with(|b| b.set(true));
    let mut raw: HashMap<u64, RawGroup> = HashMap::new();
    for entry in LIVE.iter() {
        let alloc = entry.value();
        let frames = &alloc.frames[TRACKER_FRAMES.min(alloc.frames.len())..];
        let key = stack_key(frames);
        let group = raw.entry(key).or_insert_with(|| RawGroup {
            frames: frames.to_vec(),
            count: 0,
            live_bytes: 0,
            largest: 0,
            oldest: alloc.at,
            threads: Vec::new(),
        });
        group.count += 1;
        group.live_bytes += alloc.size as u64;
        group.largest = group.largest.max(alloc.size as u64);
        group.oldest = group.oldest.min(alloc.at);
        if !group.threads.contains(&alloc.thread) {
            group.threads.push(alloc.thread);
        }
    }
    BUSY.with(|b| b.set(false));

    // 2. Resolve symbols for display, biggest groups first.
    let mut groups: Vec<LargeGroup> = raw
        .into_iter()
        .map(|(key, group)| {
            let stack: Vec<String> = group
                .frames
                .iter()
                .map(|&addr| super::caller_tracking::resolve_symbol(addr as u64))
                .collect();
            let label = attribution(&stack);
            LargeGroup {
                key,
                label,
                stack,
                count: group.count,
                live_bytes: group.live_bytes,
                largest: group.largest,
                oldest_secs: group.oldest.elapsed().as_secs_f64(),
                threads: group.threads,
            }
        })
        .collect();
    groups.sort_unstable_by(|a, b| b.live_bytes.cmp(&a.live_bytes));

    LargeSnapshot {
        live_count: LIVE.len(),
        live_bytes: LIVE_BYTES.load(Ordering::Relaxed).max(0) as u64,
        peak_bytes: PEAK_BYTES.load(Ordering::Relaxed).max(0) as u64,
        total_allocs: TOTAL_ALLOCS.load(Ordering::Relaxed),
        total_frees: TOTAL_FREES.load(Ordering::Relaxed),
        threshold: threshold(),
        groups,
    }
}

/// Symbol prefixes that are plumbing: the first frame without one of them is
/// where the memory is attributed.
const PLUMBING: &[&str] = &[
    "alloc::",
    "core::",
    "std::",
    "<alloc::",
    "<core::",
    "<std::",
    "hashbrown::",
    "<hashbrown::",
    "backtrace::",
    "ui_log_viewer::utils::",
    "<ui_log_viewer::utils::",
    "__rust",
    "_rust",
    "<unknown>",
    "0x",
];

/// The frame to attribute a stack to.
pub fn attribution(stack: &[String]) -> String {
    stack
        .iter()
        .find(|symbol| !PLUMBING.iter().any(|prefix| symbol.starts_with(prefix)))
        .or_else(|| stack.first())
        .cloned()
        .unwrap_or_else(|| "<no stack>".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The registry is process-global; tests serialise on this.
    static LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

    fn frames(tag: usize) -> Vec<usize> {
        // Four tracker frames, then the "real" stack.
        vec![1, 2, 3, 4, tag, tag + 1]
    }

    #[test]
    fn groups_by_stack_and_forgets_freed_allocations() {
        let _guard = LOCK.lock();
        clear();
        let mb = 1024 * 1024;
        record_alloc(0x1000, 64 * mb, &frames(10));
        record_alloc(0x2000, 64 * mb, &frames(10));
        record_alloc(0x3000, 8 * mb, &frames(20));

        let snapshot = build_snapshot();
        assert_eq!(snapshot.live_count, 3);
        assert_eq!(snapshot.live_bytes, 136 * mb as u64);
        assert_eq!(snapshot.groups.len(), 2, "two distinct stacks");
        assert_eq!(snapshot.groups[0].count, 2);
        assert_eq!(snapshot.groups[0].live_bytes, 128 * mb as u64);
        assert_eq!(snapshot.groups[0].largest, 64 * mb as u64);

        record_dealloc(0x1000, 64 * mb);
        let snapshot = build_snapshot();
        assert_eq!(snapshot.live_count, 2);
        assert_eq!(snapshot.total_frees, 1);
        assert_eq!(snapshot.peak_bytes, 136 * mb as u64, "peak survives frees");
        clear();
    }

    #[test]
    fn small_frees_never_touch_the_map() {
        let _guard = LOCK.lock();
        clear();
        record_alloc(0x9000, 32 * 1024 * 1024, &frames(1));
        // A free whose size is below the floor cannot be a recorded block.
        record_dealloc(0x9000, 1024);
        assert_eq!(build_snapshot().live_count, 1);
        record_dealloc(0x9000, 32 * 1024 * 1024);
        assert_eq!(build_snapshot().live_count, 0);
        clear();
    }

    #[test]
    fn threshold_has_a_floor() {
        let _guard = LOCK.lock();
        set_threshold(1);
        assert_eq!(threshold(), MIN_THRESHOLD);
        set_threshold(DEFAULT_THRESHOLD);
        assert!(wants(DEFAULT_THRESHOLD) && !wants(DEFAULT_THRESHOLD - 1));
    }

    #[test]
    fn attribution_skips_plumbing_frames() {
        let stack = vec![
            "alloc::raw_vec::RawVec<T>::finish_grow".to_string(),
            "alloc::vec::Vec<T>::resize".to_string(),
            "helio_component::texture_assets::load_texture_asset (texture_assets.rs:210)"
                .to_string(),
            "pulsar_scenedb::assets::pump".to_string(),
        ];
        assert!(attribution(&stack).starts_with("helio_component::texture_assets"));
        assert_eq!(attribution(&[]), "<no stack>");
    }
}
