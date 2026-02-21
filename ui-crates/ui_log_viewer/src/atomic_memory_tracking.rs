//! Lock-free atomic memory tracking - zero overhead, no locks
//!
//! Uses atomic counters for each category to track allocations without any locking.

use std::sync::atomic::{AtomicUsize, Ordering};
use crate::memory_tracking::MemoryCategory;

/// Lock-free atomic memory counters (one per category)
pub struct AtomicMemoryCounters {
    unknown: AtomicUsize,
    engine: AtomicUsize,
    renderer: AtomicUsize,
    ui: AtomicUsize,
    physics: AtomicUsize,
    audio: AtomicUsize,
    assets: AtomicUsize,
    scripts: AtomicUsize,
    network: AtomicUsize,
}

impl AtomicMemoryCounters {
    pub const fn new() -> Self {
        Self {
            unknown: AtomicUsize::new(0),
            engine: AtomicUsize::new(0),
            renderer: AtomicUsize::new(0),
            ui: AtomicUsize::new(0),
            physics: AtomicUsize::new(0),
            audio: AtomicUsize::new(0),
            assets: AtomicUsize::new(0),
            scripts: AtomicUsize::new(0),
            network: AtomicUsize::new(0),
        }
    }

    /// Record allocation (lock-free, atomic)
    #[inline]
    pub fn record_alloc(&self, size: usize, category: MemoryCategory) {
        let counter = self.get_counter(category);
        counter.fetch_add(size, Ordering::Relaxed);
    }

    /// Record deallocation (lock-free, atomic)
    #[inline]
    pub fn record_dealloc(&self, size: usize, category: MemoryCategory) {
        let counter = self.get_counter(category);
        counter.fetch_sub(size, Ordering::Relaxed);
    }

    /// Get the atomic counter for a category
    #[inline]
    fn get_counter(&self, category: MemoryCategory) -> &AtomicUsize {
        match category {
            MemoryCategory::Unknown => &self.unknown,
            MemoryCategory::Engine => &self.engine,
            MemoryCategory::Renderer => &self.renderer,
            MemoryCategory::UI => &self.ui,
            MemoryCategory::Physics => &self.physics,
            MemoryCategory::Audio => &self.audio,
            MemoryCategory::Assets => &self.assets,
            MemoryCategory::Scripts => &self.scripts,
            MemoryCategory::Network => &self.network,
        }
    }

    /// Get current value for a category
    pub fn get(&self, category: MemoryCategory) -> usize {
        self.get_counter(category).load(Ordering::Relaxed)
    }

    /// Get total current usage across all categories
    pub fn total(&self) -> usize {
        self.unknown.load(Ordering::Relaxed)
            + self.engine.load(Ordering::Relaxed)
            + self.renderer.load(Ordering::Relaxed)
            + self.ui.load(Ordering::Relaxed)
            + self.physics.load(Ordering::Relaxed)
            + self.audio.load(Ordering::Relaxed)
            + self.assets.load(Ordering::Relaxed)
            + self.scripts.load(Ordering::Relaxed)
            + self.network.load(Ordering::Relaxed)
    }

    /// Get snapshot of all categories (for UI rendering)
    pub fn snapshot(&self) -> Vec<(MemoryCategory, usize)> {
        let mut result = Vec::with_capacity(9);

        for category in [
            MemoryCategory::Unknown,
            MemoryCategory::Engine,
            MemoryCategory::Renderer,
            MemoryCategory::UI,
            MemoryCategory::Physics,
            MemoryCategory::Audio,
            MemoryCategory::Assets,
            MemoryCategory::Scripts,
            MemoryCategory::Network,
        ] {
            let value = self.get(category);
            if value > 0 {
                result.push((category, value));
            }
        }

        // Sort by size descending
        result.sort_by(|a, b| b.1.cmp(&a.1));
        result
    }
}

/// Global atomic counters instance
pub static ATOMIC_MEMORY_COUNTERS: AtomicMemoryCounters = AtomicMemoryCounters::new();
