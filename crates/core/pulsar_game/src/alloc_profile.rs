//! Sampling heap profiler for leak hunting (feature `alloc-profile`, off by
//! default; build a game with `--features pulsar_game/alloc-profile`).
//!
//! It is the game's `#[global_allocator]`. Roughly one allocation per
//! [`SAMPLE_BYTES`] allocated on a thread gets a call stack and is tracked
//! until it is freed, so what remains in the table is what is *still live*,
//! grouped by call site. [`start_reporter`] prints the sites holding the most
//! live bytes to stderr every few seconds; a site whose total keeps rising is
//! the leak. Set `PULSAR_ALLOC_PROFILE=<seconds>` to turn the reporter on.

use std::alloc::{GlobalAlloc, Layout, System};
use std::backtrace::Backtrace;
use std::cell::Cell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// One sample per this many bytes allocated on a thread.
const SAMPLE_BYTES: usize = 256 * 1024;

struct Site {
    stack: Backtrace,
}

struct Live {
    site: Arc<Site>,
    weight: usize,
}

fn table() -> &'static Mutex<HashMap<usize, Live>> {
    static TABLE: OnceLock<Mutex<HashMap<usize, Live>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

thread_local! {
    static SINCE_SAMPLE: Cell<usize> = const { Cell::new(0) };
    static BUSY: Cell<bool> = const { Cell::new(false) };
}

/// Set once the table has an entry, so frees of unsampled blocks skip the lock.
static ANY_TRACKED: AtomicBool = AtomicBool::new(false);
static TRACKED: AtomicUsize = AtomicUsize::new(0);

pub struct ProfilingAlloc;

impl ProfilingAlloc {
    #[inline]
    fn on_alloc(ptr: *mut u8, size: usize) {
        if ptr.is_null() {
            return;
        }
        let due = SINCE_SAMPLE.with(|since| {
            let total = since.get() + size;
            if total >= SAMPLE_BYTES {
                since.set(total % SAMPLE_BYTES);
                true
            } else {
                since.set(total);
                false
            }
        });
        if !due || BUSY.with(|busy| busy.replace(true)) {
            return;
        }
        // Everything allocated below passes through untracked (`BUSY`).
        let stack = Backtrace::force_capture();
        let live = Live {
            site: Arc::new(Site { stack }),
            weight: size.max(SAMPLE_BYTES),
        };
        if let Ok(mut table) = table().lock() {
            table.insert(ptr as usize, live);
            ANY_TRACKED.store(true, Ordering::Relaxed);
            TRACKED.store(table.len(), Ordering::Relaxed);
        }
        BUSY.with(|busy| busy.set(false));
    }

    #[inline]
    fn on_free(ptr: *mut u8) {
        if !ANY_TRACKED.load(Ordering::Relaxed) || BUSY.with(|busy| busy.replace(true)) {
            return;
        }
        if let Ok(mut table) = table().lock() {
            // Dropping the entry frees its backtrace through the allocator,
            // which re-enters `on_free` with `BUSY` set and returns at once.
            let removed = table.remove(&(ptr as usize));
            TRACKED.store(table.len(), Ordering::Relaxed);
            drop(table);
            drop(removed);
        }
        BUSY.with(|busy| busy.set(false));
    }
}

unsafe impl GlobalAlloc for ProfilingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc(layout);
        Self::on_alloc(ptr, layout.size());
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc_zeroed(layout);
        Self::on_alloc(ptr, layout.size());
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        Self::on_free(ptr);
        System.dealloc(ptr, layout);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        Self::on_free(ptr);
        let moved = System.realloc(ptr, layout, new_size);
        Self::on_alloc(moved, new_size);
        moved
    }
}

#[global_allocator]
static ALLOCATOR: ProfilingAlloc = ProfilingAlloc;

/// Print the live sites every `PULSAR_ALLOC_PROFILE` seconds (no-op if unset).
pub fn start_reporter() {
    let Some(seconds) = std::env::var("PULSAR_ALLOC_PROFILE")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&s| s > 0)
    else {
        return;
    };
    let _ = std::thread::Builder::new()
        .name("alloc-profile".into())
        .spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(seconds));
            report();
        });
}

fn report() {
    // The reporter allocates heavily while formatting; keep it out of the table.
    BUSY.with(|busy| busy.set(true));
    let mut by_site: HashMap<usize, (Arc<Site>, usize, usize)> = HashMap::new();
    let (entries, total) = {
        let Ok(table) = table().lock() else { return };
        for live in table.values() {
            let key = Arc::as_ptr(&live.site) as usize;
            let entry = by_site
                .entry(key)
                .or_insert_with(|| (live.site.clone(), 0, 0));
            entry.1 += live.weight;
            entry.2 += 1;
        }
        (table.len(), table.values().map(|l| l.weight).sum::<usize>())
    };
    // Same call stack text => same site (each sample has its own Arc).
    let mut by_text: HashMap<String, (usize, usize)> = HashMap::new();
    for (site, bytes, count) in by_site.values() {
        let text = condense(&site.stack.to_string());
        let slot = by_text.entry(text).or_default();
        slot.0 += bytes;
        slot.1 += count;
    }
    let mut sites: Vec<_> = by_text.into_iter().collect();
    sites.sort_by_key(|(_, (bytes, _))| std::cmp::Reverse(*bytes));
    eprintln!(
        "[alloc-profile] live sampled: {} entries, ~{} MB",
        entries,
        total >> 20
    );
    for (text, (bytes, count)) in sites.into_iter().take(6) {
        eprintln!(
            "[alloc-profile]  ~{:>6} MB in {:>5} samples at:\n{text}",
            bytes >> 20,
            count
        );
    }
    BUSY.with(|busy| busy.set(false));
}

/// The first dozen frames that are not the allocator, panic or thread plumbing.
fn condense(stack: &str) -> String {
    let mut out = String::new();
    let mut kept = 0;
    for line in stack.lines() {
        let trimmed = line.trim();
        if !trimmed.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            continue; // "at file:line" continuation lines
        }
        let name = trimmed
            .split_once(':')
            .map_or(trimmed, |(_, rest)| rest.trim());
        if name.contains("alloc_profile")
            || name.contains("__rust_alloc")
            || name.contains("__rust_realloc")
            || name.contains("alloc::alloc::")
            || name.contains("Backtrace::")
            || name.contains("RtlUserThreadStart")
            || name.contains("BaseThreadInitThunk")
            || name.contains("lang_start")
            || name.contains("rust_begin_short_backtrace")
        {
            continue;
        }
        out.push_str("        ");
        out.push_str(&name[..name.len().min(150)]);
        out.push('\n');
        kept += 1;
        if kept >= 12 {
            break;
        }
    }
    out
}
