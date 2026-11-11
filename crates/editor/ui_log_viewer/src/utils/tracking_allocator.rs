//! Custom global allocator with memory tracking hooks.
//!
//! Tracks every allocation via raw frame-address capture (~200ns on Windows via
//! RtlCaptureStackBackTrace). No sampling — accurate counts. Symbol resolution
//! happens only in the background snapshot thread for the top 100 call sites.

use crate::utils::atomic_memory_tracking::ATOMIC_MEMORY_COUNTERS;
use crate::utils::caller_tracking;
use crate::utils::large_allocs;
use crate::utils::memory_tracking::MemoryCategory;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Global flag to enable/disable tracking. When false, allocator behaves exactly like System allocator.
static TRACKING_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Enable allocation tracking (captures backtraces and records stats).
///
/// Starts a fresh window: everything allocated while tracking was off was
/// never counted, so the counters and the set of live tracked pointers start
/// from zero rather than being credited for frees they never saw charged.
pub fn enable_tracking() {
    TRACKING_ACTIVE.store(false, Ordering::Relaxed);
    caller_tracking::reset();
    ATOMIC_MEMORY_COUNTERS.reset();
    TRACKING_ACTIVE.store(true, Ordering::Relaxed);
}

/// Disable allocation tracking (zero overhead, behaves like System allocator).
pub fn disable_tracking() {
    TRACKING_ACTIVE.store(false, Ordering::Relaxed);
}

/// Check if tracking is currently active.
pub fn is_tracking_active() -> bool {
    TRACKING_ACTIVE.load(Ordering::Relaxed)
}

thread_local! {
    static TRACKING_ENABLED: Cell<bool> = const { Cell::new(true) };
}

pub struct TrackingAllocator;

impl Default for TrackingAllocator {
    fn default() -> Self {
        Self::new()
    }
}

impl TrackingAllocator {
    pub const fn new() -> Self {
        Self
    }

    fn categorize_allocation() -> MemoryCategory {
        CURRENT_CATEGORY.with(|cat| cat.load(Ordering::Relaxed).into())
    }

    #[inline]
    fn record_alloc(&self, ptr: *mut u8, layout: Layout) {
        // Early exit if tracking is disabled — zero overhead, just like System allocator.
        if !TRACKING_ACTIVE.load(Ordering::Relaxed) {
            return;
        }

        let was_enabled = TRACKING_ENABLED.with(|e| {
            let v = e.get();
            if v {
                e.set(false);
            }
            v
        });
        if !was_enabled {
            // Re-entry: this is the tracker's own internal allocation (DashMap resize, Vec, etc.).
            // We can't do per-site recording (CALLER_BUSY is set, doing DashMap ops would deadlock),
            // but we DO count it in the global live total so "process live" stays honest.
            caller_tracking::GLOBAL_LIVE_BYTES.fetch_add(layout.size() as i64, Ordering::Relaxed);
            TRACKING_ENABLED.with(|e| e.set(true));
            return;
        }

        let category = Self::categorize_allocation();

        // Capture raw return addresses only — no symbol resolution, no heap alloc.
        // Ordinary allocations keep 8 frames; a large one (see `large_allocs`)
        // gets a deep stack so the Memory Explorer can name its owner.
        let large = large_allocs::wants(layout.size());
        let limit = if large { large_allocs::MAX_FRAMES } else { 8 };
        let mut frames = [0usize; large_allocs::MAX_FRAMES];
        let mut count = 0usize;
        // SAFETY: `trace_unsynchronized` skips the backtrace crate's lock (taking
        // it would allocate), so it must not run concurrently with another
        // backtrace call; its documented requirement. Here it relies on the
        // platform unwinder (`RtlVirtualUnwind` on 64-bit Windows, `libunwind` on
        // Unix) being reentrant, and the callback only writes into the local
        // `frames` array, so it neither allocates nor recurses.
        unsafe {
            backtrace::trace_unsynchronized(|frame| {
                if count < limit {
                    frames[count] = frame.ip() as usize;
                    count += 1;
                    true
                } else {
                    false
                }
            });
        }
        if count > 0 {
            // The category counters are charged only for a pointer the caller
            // map now holds, so `record_dealloc` credits exactly what was charged.
            if caller_tracking::record_alloc(
                ptr as usize,
                &frames[..count.min(8)],
                layout.size(),
                category as u8,
            ) {
                ATOMIC_MEMORY_COUNTERS.record_alloc(layout.size(), category);
            }
            if large {
                large_allocs::record_alloc(ptr as usize, layout.size(), &frames[..count]);
            }
        }

        TRACKING_ENABLED.with(|e| e.set(true));
    }

    #[inline]
    fn record_dealloc(&self, ptr: *mut u8, layout: Layout) {
        // Early exit if tracking is disabled — zero overhead, just like System allocator.
        if !TRACKING_ACTIVE.load(Ordering::Relaxed) {
            return;
        }

        let was_enabled = TRACKING_ENABLED.with(|e| {
            let v = e.get();
            if v {
                e.set(false);
            }
            v
        });
        if !was_enabled {
            // Re-entry: tracker's own dealloc — keep global total accurate.
            caller_tracking::GLOBAL_LIVE_BYTES.fetch_sub(layout.size() as i64, Ordering::Relaxed);
            TRACKING_ENABLED.with(|e| e.set(true));
            return;
        }

        // Only a block charged by `record_alloc` is credited back, to the
        // category it was charged to: one allocated before tracking began (or
        // freed under another category guard) would otherwise wrap the
        // counters to ~2^64.
        if let Some(category) = caller_tracking::record_dealloc(ptr as usize, layout.size()) {
            ATOMIC_MEMORY_COUNTERS
                .record_dealloc(layout.size(), MemoryCategory::from(category as usize));
        }
        large_allocs::record_dealloc(ptr as usize, layout.size());

        TRACKING_ENABLED.with(|e| e.set(true));
    }
}

// SAFETY: every method forwards the caller's layout and pointer unchanged to
// `System`, which upholds the `GlobalAlloc` contract. The tracking around the
// calls runs behind a thread-local re-entrancy guard and never changes the
// returned pointer; `realloc` rebuilds a layout from the original alignment
// (already valid) and `new_size` that the caller must have made valid.
unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc(layout);
        if !ptr.is_null() {
            self.record_alloc(ptr, layout);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        self.record_dealloc(ptr, layout);
        System.dealloc(ptr, layout);
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc_zeroed(layout);
        if !ptr.is_null() {
            self.record_alloc(ptr, layout);
        }
        ptr
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_ptr = System.realloc(ptr, layout, new_size);
        if !new_ptr.is_null() {
            self.record_dealloc(ptr, layout);
            let new_layout = Layout::from_size_align_unchecked(new_size, layout.align());
            self.record_alloc(new_ptr, new_layout);
        }
        new_ptr
    }
}

thread_local! {
    static CURRENT_CATEGORY: AtomicUsize = const { AtomicUsize::new(0) };
}

pub struct MemoryCategoryGuard {
    previous: usize,
}

impl MemoryCategoryGuard {
    pub fn new(category: MemoryCategory) -> Self {
        let previous = CURRENT_CATEGORY.with(|c| c.swap(category as usize, Ordering::Relaxed));
        Self { previous }
    }
}

impl Drop for MemoryCategoryGuard {
    fn drop(&mut self) {
        CURRENT_CATEGORY.with(|c| c.store(self.previous, Ordering::Relaxed));
    }
}

impl From<usize> for MemoryCategory {
    fn from(v: usize) -> Self {
        match v {
            1 => MemoryCategory::Engine,
            2 => MemoryCategory::Renderer,
            3 => MemoryCategory::UI,
            4 => MemoryCategory::Physics,
            5 => MemoryCategory::Audio,
            6 => MemoryCategory::Assets,
            7 => MemoryCategory::Scripts,
            8 => MemoryCategory::Network,
            _ => MemoryCategory::Unknown,
        }
    }
}

#[macro_export]
macro_rules! use_memory_category {
    ($category:expr, $block:block) => {{
        let _guard = $crate::utils::tracking_allocator::MemoryCategoryGuard::new($category);
        $block
    }};
}
