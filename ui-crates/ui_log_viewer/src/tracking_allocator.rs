//! Custom global allocator with memory tracking hooks
//!
//! This allocator wraps the system allocator and tracks all allocations/deallocations
//! in real-time, providing detailed memory usage statistics categorized by subsystem.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::cell::Cell;
use crate::memory_tracking::{MemoryCategory, SharedMemoryTracker};

/// Thread-local flag to prevent recursive tracking
thread_local! {
    static TRACKING_ENABLED: Cell<bool> = const { Cell::new(true) };
}

/// Global tracking allocator instance
pub struct TrackingAllocator;

// Static instance for accessing the global allocator tracker
static GLOBAL_TRACKER: parking_lot::RwLock<Option<SharedMemoryTracker>> = parking_lot::RwLock::new(None);

/// Set the global memory tracker (must be called after initialization)
pub fn set_global_memory_tracker(tracker: SharedMemoryTracker) {
    *GLOBAL_TRACKER.write() = Some(tracker);
}

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

    /// Record an allocation
    fn record_alloc(&self, size: usize) {
        // Check if tracking is enabled, and if so, disable it to prevent recursion
        let was_enabled = TRACKING_ENABLED.with(|enabled| {
            let was = enabled.get();
            if was {
                enabled.set(false);
            }
            was
        });

        if !was_enabled {
            return;
        }

        if let Some(tracker) = GLOBAL_TRACKER.read().as_ref() {
            let category = Self::categorize_allocation();
            tracker.read().allocate(size, category);
        }

        TRACKING_ENABLED.with(|enabled| enabled.set(true));
    }

    /// Record a deallocation
    fn record_dealloc(&self, size: usize) {
        // Check if tracking is enabled, and if so, disable it to prevent recursion
        let was_enabled = TRACKING_ENABLED.with(|enabled| {
            let was = enabled.get();
            if was {
                enabled.set(false);
            }
            was
        });

        if !was_enabled {
            return;
        }

        if let Some(tracker) = GLOBAL_TRACKER.read().as_ref() {
            let category = Self::categorize_allocation();
            tracker.read().deallocate(size, category);
        }

        TRACKING_ENABLED.with(|enabled| enabled.set(true));
    }
}

unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc(layout);
        if !ptr.is_null() {
            self.record_alloc(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        self.record_dealloc(layout.size());
        System.dealloc(ptr, layout);
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc_zeroed(layout);
        if !ptr.is_null() {
            self.record_alloc(layout.size());
        }
        ptr
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let old_size = layout.size();
        let new_ptr = System.realloc(ptr, layout, new_size);

        if !new_ptr.is_null() {
            // Record as dealloc of old size and alloc of new size
            self.record_dealloc(old_size);
            self.record_alloc(new_size);
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
