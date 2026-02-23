//! Caller-site allocation tracking.
//!
//! ## Design
//!
//! Hot path (allocator thread, 10k+ allocs/sec):
//!   - `backtrace::trace_unsynchronized` captures 6 raw frame addresses (~200ns)
//!   - Hash the frame slice → u64 key
//!   - DashMap shard lock (sharded, held < 1µs) to fetch/insert CallerStats
//!   - Atomic increments on CallerStats — no blocking
//!
//! Background thread (every 500ms):
//!   - Snapshot all DashMap entries (clone stats atomically)
//!   - Resolve symbols lazily (cached: O(1) after first resolve)
//!   - Sort by total_bytes, cap at 2000 rows
//!   - Swap into CALLER_SNAPSHOT (write lock held < 1ms)
//!
//! UI thread:
//!   - Reads CALLER_SNAPSHOT via a brief read lock per render frame
//!   - Never touches the DashMap directly

use std::hash::{Hash, Hasher};
use std::collections::hash_map::DefaultHasher;
use std::sync::atomic::{AtomicU64, AtomicI64, Ordering};
use std::cell::Cell;
use once_cell::sync::Lazy;
use dashmap::DashMap;
use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use std::sync::Arc;

// ─── Re-entrancy guard ────────────────────────────────────────────────────────
// Prevents DashMap deadlock: if this thread is already inside caller_tracking
// (e.g. refresh_snapshot is iterating the map), silently skip new record calls.
thread_local! {
    static CALLER_BUSY: Cell<bool> = const { Cell::new(false) };
}

// ─── Stack key ───────────────────────────────────────────────────────────────

/// Up to 6 raw return addresses compressed into a single u64 hash.
/// We store the hash directly (collision probability negligible for profiling).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct StackKey(pub u64);

impl StackKey {
    pub fn from_frames(frames: &[usize]) -> Self {
        let mut h = DefaultHasher::new();
        frames.hash(&mut h);
        Self(h.finish())
    }
}

// ─── Per-callsite stats (all atomic — zero locking on hot path) ───────────────

pub struct CallerStats {
    pub total_allocs:   AtomicU64,
    pub total_deallocs: AtomicU64,
    pub total_bytes:    AtomicU64,
    /// Signed: live_bytes can transiently be negative due to dealloc-before-alloc races.
    pub live_bytes:     AtomicI64,
    /// Representative first frame address (for symbol resolution).
    pub first_frame:    AtomicU64,
}

impl Default for CallerStats {
    fn default() -> Self {
        Self {
            total_allocs:   AtomicU64::new(0),
            total_deallocs: AtomicU64::new(0),
            total_bytes:    AtomicU64::new(0),
            live_bytes:     AtomicI64::new(0),
            first_frame:    AtomicU64::new(0),
        }
    }
}

// ─── Global maps ─────────────────────────────────────────────────────────────

/// Maps hashed call-site key → aggregate stats.
pub static CALLER_MAP: Lazy<DashMap<StackKey, CallerStats>> =
    Lazy::new(|| DashMap::with_capacity(4096));

/// Maps allocation pointer → StackKey, so deallocs can be paired with their alloc site.
/// Size is bounded by the number of currently live tracked allocations (naturally shrinks on dealloc).
pub static ALLOC_KEYS: Lazy<DashMap<usize, StackKey>> =
    Lazy::new(|| DashMap::with_capacity(131072));

/// Global live-bytes counter: incremented on every tracked alloc, decremented on every matched dealloc.
/// This is the ground-truth live memory from ALL tracked call sites, regardless of CALLER_MAP cap.
pub static GLOBAL_LIVE_BYTES: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

// ─── Snapshot (UI-readable) ───────────────────────────────────────────────────

/// A resolved row for display in the UI.
#[derive(Clone, Debug)]
pub struct CallerRow {
    pub key:              StackKey,
    pub symbol:           String,
    pub total_allocs:     u64,
    pub total_deallocs:   u64,
    pub total_bytes:      u64,
    pub live_bytes:       i64,
    pub avg_size:         u64,
    /// Estimated live bytes: live_bytes.max(0) — exact when dealloc pairing succeeds.
    pub leaked_estimate:  u64,
}

/// Shared snapshot: background thread writes, UI reads.
/// Write lock is held only while swapping the Vec (< 1ms).
pub static CALLER_SNAPSHOT: Lazy<Arc<RwLock<Vec<CallerRow>>>> =
    Lazy::new(|| Arc::new(RwLock::new(Vec::new())));

/// Symbol cache: first_frame address → resolved string.
/// Locked only during symbol resolution (background thread, infrequent).
static SYMBOL_CACHE: Lazy<Mutex<HashMap<u64, String>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

// ─── Hot-path recording ───────────────────────────────────────────────────────

/// Record an allocation. Called from within the global allocator with TRACKING_ENABLED=false
/// so recursion from DashMap's own allocations is suppressed.
#[inline]
pub fn record_alloc(ptr: usize, frames: &[usize], size: usize) {
    if frames.is_empty() { return; }

    // Global counter updated unconditionally — even if CALLER_BUSY blocks per-site recording.
    GLOBAL_LIVE_BYTES.fetch_add(size as i64, Ordering::Relaxed);

    if CALLER_BUSY.with(|b| b.replace(true)) { return; }

    // Skip allocator-internal frames:
    //   0: backtrace::trace_unsynchronized
    //   1: record_alloc (tracking_allocator)
    //   2: GlobalAlloc::alloc / alloc_zeroed / realloc
    //   3: compiler-generated alloc shim
    let skip = frames.len().min(4);
    let meaningful = &frames[skip..];

    if !meaningful.is_empty() {
        let key   = StackKey::from_frames(meaningful);
        let first = meaningful[0] as u64;

        // Allow up to 8192 unique call sites; existing keys always update.
        if CALLER_MAP.len() < 8192 || CALLER_MAP.contains_key(&key) {
            let entry = CALLER_MAP.entry(key).or_default();
            entry.total_allocs.fetch_add(1, Ordering::Relaxed);
            entry.total_bytes.fetch_add(size as u64, Ordering::Relaxed);
            entry.live_bytes.fetch_add(size as i64, Ordering::Relaxed);
            if entry.first_frame.load(Ordering::Relaxed) == 0 {
                let _ = entry.first_frame.compare_exchange(0, first, Ordering::Relaxed, Ordering::Relaxed);
            }
            // Store ptr → key for dealloc pairing.
            if ptr != 0 {
                ALLOC_KEYS.insert(ptr, key);
            }
        }
    }

    CALLER_BUSY.with(|b| b.set(false));
}

/// Record a deallocation by matching the pointer to its original alloc site.
#[inline]
pub fn record_dealloc(ptr: usize, size: usize) {
    if ptr == 0 { return; }

    // Always subtract from global counter.
    GLOBAL_LIVE_BYTES.fetch_sub(size as i64, Ordering::Relaxed);

    if CALLER_BUSY.with(|b| b.replace(true)) { return; }

    if let Some((_, key)) = ALLOC_KEYS.remove(&ptr) {
        if let Some(entry) = CALLER_MAP.get(&key) {
            entry.total_deallocs.fetch_add(1, Ordering::Relaxed);
            entry.live_bytes.fetch_sub(size as i64, Ordering::Relaxed);
        }
    }
    CALLER_BUSY.with(|b| b.set(false));
}

// ─── Background snapshot builder ─────────────────────────────────────────────

/// Build and publish a new snapshot. Called by background task every 500ms.
pub fn refresh_snapshot(filter: &str) {
    CALLER_BUSY.with(|b| b.set(true));

    // 1. Snapshot raw stats — no symbol work yet.
    struct RawRow { first_frame: u64, total_allocs: u64, total_deallocs: u64, total_bytes: u64, live_bytes: i64 }
    let mut raw: Vec<RawRow> = CALLER_MAP.iter().map(|e| RawRow {
        first_frame:    e.value().first_frame.load(Ordering::Relaxed),
        total_allocs:   e.value().total_allocs.load(Ordering::Relaxed),
        total_deallocs: e.value().total_deallocs.load(Ordering::Relaxed),
        total_bytes:    e.value().total_bytes.load(Ordering::Relaxed),
        live_bytes:     e.value().live_bytes.load(Ordering::Relaxed),
    }).collect();

    CALLER_BUSY.with(|b| b.set(false));

    // 2. Sort by live_bytes descending (worst current leakers first), keep top 100 BEFORE resolving symbols.
    //    Sorting by alloc count misses infrequent-but-large leakers; live_bytes is the accurate signal.
    raw.sort_unstable_by(|a, b| b.live_bytes.max(0).cmp(&a.live_bytes.max(0)));
    raw.truncate(100);

    // 3. Resolve symbols + build rows for the top 100 only.
    let f_lower = if filter.is_empty() { None } else { Some(filter.to_lowercase()) };

    let mut rows: Vec<CallerRow> = raw.into_iter().filter_map(|r| {
        let symbol = resolve_symbol(r.first_frame);
        if let Some(ref f) = f_lower {
            if !symbol.to_lowercase().contains(f.as_str()) { return None; }
        }
        let avg_size        = if r.total_allocs > 0 { r.total_bytes / r.total_allocs } else { 0 };
        // Use live_bytes directly — it's maintained atomically (add on alloc, sub on dealloc).
        // Much more accurate than (allocs - deallocs) * avg_size which breaks if sizes vary.
        let leaked_estimate = r.live_bytes.max(0) as u64;
        let key = StackKey::from_frames(&[r.first_frame as usize]);
        Some(CallerRow { key, symbol,
            total_allocs: r.total_allocs, total_deallocs: r.total_deallocs,
            total_bytes: r.total_bytes, live_bytes: r.live_bytes,
            avg_size, leaked_estimate })
    }).collect();

    // Default sort: highest leak estimate first, break ties by alloc count.
    rows.sort_unstable_by(|a, b| b.leaked_estimate.cmp(&a.leaked_estimate)
        .then(b.total_allocs.cmp(&a.total_allocs)));

    *CALLER_SNAPSHOT.write() = rows;
    CALLER_BUSY.with(|b| b.set(false));
}

// ─── Symbol resolution ────────────────────────────────────────────────────────

fn resolve_symbol(addr: u64) -> String {
    if addr == 0 { return "<unknown>".to_string(); }

    // Fast path: already cached
    {
        let cache = SYMBOL_CACHE.lock();
        if let Some(s) = cache.get(&addr) {
            return s.clone();
        }
    }

    // Slow path: resolve (may take a few ms the first time per address)
    let resolved = do_resolve(addr as usize);
    SYMBOL_CACHE.lock().insert(addr, resolved.clone());
    resolved
}

fn do_resolve(addr: usize) -> String {
    let mut name = format!("0x{:016x}", addr);
    backtrace::resolve(addr as *mut _, |sym| {
        if let Some(n) = sym.name() {
            let full = format!("{:#}", n);
            // Strip long generic noise — keep first 120 chars
            name = if full.len() > 120 { format!("{}…", &full[..120]) } else { full };
        }
        if let (Some(file), Some(line)) = (sym.filename(), sym.lineno()) {
            let filename = file.file_name()
                .and_then(|f| f.to_str())
                .unwrap_or("?");
            name = format!("{} ({}:{})", name, filename, line);
        }
    });
    name
}
