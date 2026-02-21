//! Lock-free atomic memory tracking - zero overhead, no locks
//!
//! Uses atomic counters for each category to track allocations without any locking.

use std::sync::atomic::{AtomicUsize, Ordering};
use crate::memory_tracking::MemoryCategory;

/// Allocation size bucket for detailed tracking
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SizeBucket {
    Tiny,      // < 64 bytes
    Small,     // 64B - 1KB
    Medium,    // 1KB - 16KB
    Large,     // 16KB - 256KB
    Huge,      // > 256KB
}

impl SizeBucket {
    pub fn from_size(size: usize) -> Self {
        if size < 64 {
            SizeBucket::Tiny
        } else if size < 1024 {
            SizeBucket::Small
        } else if size < 16 * 1024 {
            SizeBucket::Medium
        } else if size < 256 * 1024 {
            SizeBucket::Large
        } else {
            SizeBucket::Huge
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            SizeBucket::Tiny => "Tiny (<64B)",
            SizeBucket::Small => "Small (64B-1KB)",
            SizeBucket::Medium => "Medium (1-16KB)",
            SizeBucket::Large => "Large (16-256KB)",
            SizeBucket::Huge => "Huge (>256KB)",
        }
    }
}

/// Detailed allocation entry
#[derive(Clone)]
pub struct AllocationEntry {
    pub name: String,
    pub size: usize,
    pub category: crate::memory_tracking::MemoryCategory,
    pub bucket: SizeBucket,
}

/// Lock-free atomic memory counters (one per category)
pub struct AtomicMemoryCounters {
    // Per-category counters
    unknown: AtomicUsize,
    engine: AtomicUsize,
    renderer: AtomicUsize,
    ui: AtomicUsize,
    physics: AtomicUsize,
    audio: AtomicUsize,
    assets: AtomicUsize,
    scripts: AtomicUsize,
    network: AtomicUsize,

    // Per-size bucket counters
    tiny_count: AtomicUsize,
    small_count: AtomicUsize,
    medium_count: AtomicUsize,
    large_count: AtomicUsize,
    huge_count: AtomicUsize,

    tiny_bytes: AtomicUsize,
    small_bytes: AtomicUsize,
    medium_bytes: AtomicUsize,
    large_bytes: AtomicUsize,
    huge_bytes: AtomicUsize,
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
            tiny_count: AtomicUsize::new(0),
            small_count: AtomicUsize::new(0),
            medium_count: AtomicUsize::new(0),
            large_count: AtomicUsize::new(0),
            huge_count: AtomicUsize::new(0),
            tiny_bytes: AtomicUsize::new(0),
            small_bytes: AtomicUsize::new(0),
            medium_bytes: AtomicUsize::new(0),
            large_bytes: AtomicUsize::new(0),
            huge_bytes: AtomicUsize::new(0),
        }
    }

    /// Record allocation (lock-free, atomic)
    #[inline]
    pub fn record_alloc(&self, size: usize, category: MemoryCategory) {
        // Update category counter
        let counter = self.get_counter(category);
        counter.fetch_add(size, Ordering::Relaxed);

        // Update size bucket counters
        let bucket = SizeBucket::from_size(size);
        self.update_bucket_alloc(bucket, size);
    }

    /// Record deallocation (lock-free, atomic)
    #[inline]
    pub fn record_dealloc(&self, size: usize, category: MemoryCategory) {
        // Update category counter
        let counter = self.get_counter(category);
        counter.fetch_sub(size, Ordering::Relaxed);

        // Update size bucket counters
        let bucket = SizeBucket::from_size(size);
        self.update_bucket_dealloc(bucket, size);
    }

    /// Update size bucket on allocation
    #[inline]
    fn update_bucket_alloc(&self, bucket: SizeBucket, size: usize) {
        match bucket {
            SizeBucket::Tiny => {
                self.tiny_count.fetch_add(1, Ordering::Relaxed);
                self.tiny_bytes.fetch_add(size, Ordering::Relaxed);
            }
            SizeBucket::Small => {
                self.small_count.fetch_add(1, Ordering::Relaxed);
                self.small_bytes.fetch_add(size, Ordering::Relaxed);
            }
            SizeBucket::Medium => {
                self.medium_count.fetch_add(1, Ordering::Relaxed);
                self.medium_bytes.fetch_add(size, Ordering::Relaxed);
            }
            SizeBucket::Large => {
                self.large_count.fetch_add(1, Ordering::Relaxed);
                self.large_bytes.fetch_add(size, Ordering::Relaxed);
            }
            SizeBucket::Huge => {
                self.huge_count.fetch_add(1, Ordering::Relaxed);
                self.huge_bytes.fetch_add(size, Ordering::Relaxed);
            }
        }
    }

    /// Update size bucket on deallocation
    #[inline]
    fn update_bucket_dealloc(&self, bucket: SizeBucket, size: usize) {
        match bucket {
            SizeBucket::Tiny => {
                self.tiny_count.fetch_sub(1, Ordering::Relaxed);
                self.tiny_bytes.fetch_sub(size, Ordering::Relaxed);
            }
            SizeBucket::Small => {
                self.small_count.fetch_sub(1, Ordering::Relaxed);
                self.small_bytes.fetch_sub(size, Ordering::Relaxed);
            }
            SizeBucket::Medium => {
                self.medium_count.fetch_sub(1, Ordering::Relaxed);
                self.medium_bytes.fetch_sub(size, Ordering::Relaxed);
            }
            SizeBucket::Large => {
                self.large_count.fetch_sub(1, Ordering::Relaxed);
                self.large_bytes.fetch_sub(size, Ordering::Relaxed);
            }
            SizeBucket::Huge => {
                self.huge_count.fetch_sub(1, Ordering::Relaxed);
                self.huge_bytes.fetch_sub(size, Ordering::Relaxed);
            }
        }
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

    /// Get detailed allocation entries for virtual list display
    pub fn get_all_entries(&self) -> Vec<AllocationEntry> {
        let mut entries = Vec::with_capacity(14); // 9 categories + 5 size buckets

        // Add category entries
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
            let size = self.get(category);
            if size > 0 {
                entries.push(AllocationEntry {
                    name: format!("{}", category.as_str()),
                    size,
                    category,
                    bucket: SizeBucket::Tiny, // Not applicable for category entries
                });
            }
        }

        // Add size bucket entries
        let tiny_bytes = self.tiny_bytes.load(Ordering::Relaxed);
        let tiny_count = self.tiny_count.load(Ordering::Relaxed);
        if tiny_bytes > 0 {
            entries.push(AllocationEntry {
                name: format!("{} ({} allocs)", SizeBucket::Tiny.name(), tiny_count),
                size: tiny_bytes,
                category: MemoryCategory::Unknown,
                bucket: SizeBucket::Tiny,
            });
        }

        let small_bytes = self.small_bytes.load(Ordering::Relaxed);
        let small_count = self.small_count.load(Ordering::Relaxed);
        if small_bytes > 0 {
            entries.push(AllocationEntry {
                name: format!("{} ({} allocs)", SizeBucket::Small.name(), small_count),
                size: small_bytes,
                category: MemoryCategory::Unknown,
                bucket: SizeBucket::Small,
            });
        }

        let medium_bytes = self.medium_bytes.load(Ordering::Relaxed);
        let medium_count = self.medium_count.load(Ordering::Relaxed);
        if medium_bytes > 0 {
            entries.push(AllocationEntry {
                name: format!("{} ({} allocs)", SizeBucket::Medium.name(), medium_count),
                size: medium_bytes,
                category: MemoryCategory::Unknown,
                bucket: SizeBucket::Medium,
            });
        }

        let large_bytes = self.large_bytes.load(Ordering::Relaxed);
        let large_count = self.large_count.load(Ordering::Relaxed);
        if large_bytes > 0 {
            entries.push(AllocationEntry {
                name: format!("{} ({} allocs)", SizeBucket::Large.name(), large_count),
                size: large_bytes,
                category: MemoryCategory::Unknown,
                bucket: SizeBucket::Large,
            });
        }

        let huge_bytes = self.huge_bytes.load(Ordering::Relaxed);
        let huge_count = self.huge_count.load(Ordering::Relaxed);
        if huge_bytes > 0 {
            entries.push(AllocationEntry {
                name: format!("{} ({} allocs)", SizeBucket::Huge.name(), huge_count),
                size: huge_bytes,
                category: MemoryCategory::Unknown,
                bucket: SizeBucket::Huge,
            });
        }

        // Sort by size descending
        entries.sort_by(|a, b| b.size.cmp(&a.size));
        entries
    }
}

/// Global atomic counters instance
pub static ATOMIC_MEMORY_COUNTERS: AtomicMemoryCounters = AtomicMemoryCounters::new();
