//! Custom global allocator with memory tracking hooks
//!
//! This allocator wraps the system allocator and tracks all allocations/deallocations
//! in real-time, providing detailed memory usage statistics categorized by subsystem.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::cell::Cell;
use crate::memory_tracking::MemoryCategory;
use crate::atomic_memory_tracking::ATOMIC_MEMORY_COUNTERS;
use crate::caller_tracking;

/// Thread-local flag to prevent recursive tracking
thread_local! {
    static TRACKING_ENABLED: Cell<bool> = const { Cell::new(true) };
}

/// Sample 1 in N allocations for callsite tracking.
/// Counters/bytes are still recorded for every alloc via ATOMIC_MEMORY_COUNTERS.
const CALLER_SAMPLE_RATE: usize = 1000;
static SAMPLE_COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Global tracking allocator instance
pub struct TrackingAllocator;

impl TrackingAllocator {
    pub const fn new() -> Self { Self }

    fn categorize_allocation() -> MemoryCategory {
        CURRENT_CATEGORY.with(|cat| cat.load(Ordering::Relaxed).into())
    }

    #[inline]
    fn record_alloc(&self, ptr: *mut u8, layout: Layout) {
        let was_enabled = TRACKING_ENABLED.with(|e| { let v = e.get(); if v { e.set(false); } v });
        if !was_enabled { return; }

        let category = Self::categorize_allocation();
        ATOMIC_MEMORY_COUNTERS.record_alloc(layout.size(), category);

        // Sample 1 in CALLER_SAMPLE_RATE allocs for callsite capture.
        if SAMPLE_COUNTER.fetch_add(1, Ordering::Relaxed) % CALLER_SAMPLE_RATE == 0 {
            let mut frames = [0usize; 10];
            let mut count = 0usize;
            unsafe {
                backtrace::trace_unsynchronized(|frame| {
                    if count < frames.len() { frames[count] = frame.ip() as usize; count += 1; true }
                    else { false }
                });
            }
            if count > 0 {
                caller_tracking::record_alloc(ptr as usize, &frames[..count], layout.size());
            }
        }

        TRACKING_ENABLED.with(|e| e.set(true));
    }

    /// Dealloc: look up the ptr in the alloc-key table to pair with the original alloc site.
    #[inline]
    fn record_dealloc(&self, ptr: *mut u8, layout: Layout) {
        let was_enabled = TRACKING_ENABLED.with(|e| { let v = e.get(); if v { e.set(false); } v });
        if !was_enabled { return; }

        let category = Self::categorize_allocation();
        ATOMIC_MEMORY_COUNTERS.record_dealloc(layout.size(), category);
        caller_tracking::record_dealloc(ptr as usize, layout.size());

        TRACKING_ENABLED.with(|e| e.set(true));
    }
}

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

/// Thread-local current category for allocation tracking
thread_local! {
    static CURRENT_CATEGORY: AtomicUsize = const { AtomicUsize::new(0) };
}

/// Guard that sets the current memory category for the duration of its lifetime
pub struct MemoryCategoryGuard {
    previous: usize,
}

impl MemoryCategoryGuard {
    pub fn new(category: MemoryCategory) -> Self {
        let cat_value = category as usize;
        let previous = CURRENT_CATEGORY.with(|cat| cat.swap(cat_value, Ordering::Relaxed));
        Self { previous }
    }
}

impl Drop for MemoryCategoryGuard {
    fn drop(&mut self) {
        CURRENT_CATEGORY.with(|cat| cat.store(self.previous, Ordering::Relaxed));
    }
}

impl From<usize> for MemoryCategory {
    fn from(value: usize) -> Self {
        match value {
            0 => MemoryCategory::Unknown,
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

/// Macro to mark a block of code with a memory category
///
/// # Example
/// ```
/// use_memory_category!(MemoryCategory::Renderer, {
///     // All allocations in this block will be categorized as Renderer
///     let data = vec![0u8; 1024];
/// });
/// ```
#[macro_export]
macro_rules! use_memory_category {
    ($category:expr, $block:block) => {{
        let _guard = $crate::tracking_allocator::MemoryCategoryGuard::new($category);
        $block
    }};
}
