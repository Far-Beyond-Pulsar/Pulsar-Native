//! Custom global allocator with memory tracking hooks
//!
//! This allocator wraps the system allocator and tracks all allocations/deallocations
//! in real-time, providing detailed memory usage statistics categorized by subsystem.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::cell::Cell;
use crate::memory_tracking::MemoryCategory;
use crate::atomic_memory_tracking::ATOMIC_MEMORY_COUNTERS;
use crate::type_tracking::TYPE_TRACKER;

/// Thread-local flag to prevent recursive tracking
thread_local! {
    static TRACKING_ENABLED: Cell<bool> = const { Cell::new(true) };
}

/// Global tracking allocator instance
pub struct TrackingAllocator;

impl TrackingAllocator {
    /// Create a new tracking allocator
    pub const fn new() -> Self {
        Self
    }

    /// Categorize allocation based on call stack
    /// This is a heuristic - we check the backtrace to determine which subsystem
    /// is making the allocation
    fn categorize_allocation() -> MemoryCategory {
        // For now, use a simple thread-local categorization
        // In a full implementation, we'd use backtrace analysis
        CURRENT_CATEGORY.with(|cat| cat.load(Ordering::Relaxed).into())
    }

    /// Record an allocation (lock-free, atomic - ultra fast)
    #[inline]
    fn record_alloc(&self, layout: Layout) {
        // Check if tracking is enabled to prevent recursion
        let was_enabled = TRACKING_ENABLED.with(|enabled| {
            let was = enabled.get();
            if was {
                enabled.set(false);
            }
            was
        });

        if !was_enabled {
            return; // Already tracking, prevent recursion
        }

        // Track allocations (no allocations in these calls)
        let category = Self::categorize_allocation();
        ATOMIC_MEMORY_COUNTERS.record_alloc(layout.size(), category);

        // Track by type (layout) - this might allocate via DashMap but tracking is disabled
        TYPE_TRACKER.record_alloc(layout);

        // Re-enable tracking
        TRACKING_ENABLED.with(|enabled| enabled.set(true));
    }

    /// Record a deallocation (lock-free, atomic - ultra fast)
    #[inline]
    fn record_dealloc(&self, layout: Layout) {
        // Check if tracking is enabled to prevent recursion
        let was_enabled = TRACKING_ENABLED.with(|enabled| {
            let was = enabled.get();
            if was {
                enabled.set(false);
            }
            was
        });

        if !was_enabled {
            return; // Already tracking, prevent recursion
        }

        // Track deallocations
        let category = Self::categorize_allocation();
        ATOMIC_MEMORY_COUNTERS.record_dealloc(layout.size(), category);

        // Track by type (layout)
        TYPE_TRACKER.record_dealloc(layout);

        // Re-enable tracking
        TRACKING_ENABLED.with(|enabled| enabled.set(true));
    }
}

unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc(layout);
        if !ptr.is_null() {
            self.record_alloc(layout);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        self.record_dealloc(layout);
        System.dealloc(ptr, layout);
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc_zeroed(layout);
        if !ptr.is_null() {
            self.record_alloc(layout);
        }
        ptr
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_ptr = System.realloc(ptr, layout, new_size);

        if !new_ptr.is_null() {
            // Record as dealloc of old layout and alloc of new layout
            self.record_dealloc(layout);
            let new_layout = Layout::from_size_align_unchecked(new_size, layout.align());
            self.record_alloc(new_layout);
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
