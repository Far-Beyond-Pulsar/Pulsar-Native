//! Memory tracking and allocation monitoring

use std::collections::HashMap;
use std::sync::Arc;
use parking_lot::RwLock;

/// Category of memory allocation
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
#[repr(usize)]
pub enum MemoryCategory {
    Unknown = 0,
    Engine = 1,
    Renderer = 2,
    UI = 3,
    Physics = 4,
    Audio = 5,
    Assets = 6,
    Scripts = 7,
    Network = 8,
}

impl MemoryCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            MemoryCategory::Unknown => "Unknown",
            MemoryCategory::Engine => "Engine",
            MemoryCategory::Renderer => "Renderer",
            MemoryCategory::UI => "UI",
            MemoryCategory::Physics => "Physics",
            MemoryCategory::Audio => "Audio",
            MemoryCategory::Assets => "Assets",
            MemoryCategory::Scripts => "Scripts",
            MemoryCategory::Network => "Network",
        }
    }
}

/// Detailed memory allocation entry
#[derive(Debug, Clone)]
pub struct MemoryAllocation {
    pub category: MemoryCategory,
    pub size: usize,
    pub count: usize,
    pub description: String,
}

/// Memory tracking statistics
#[derive(Debug, Clone)]
pub struct MemoryStats {
    pub total_allocated: usize,
    pub total_deallocated: usize,
    pub current_usage: usize,
    pub peak_usage: usize,
    pub allocation_count: usize,
    pub deallocation_count: usize,
    pub by_category: HashMap<MemoryCategory, usize>,
}

impl Default for MemoryStats {
    fn default() -> Self {
        Self {
            total_allocated: 0,
            total_deallocated: 0,
            current_usage: 0,
            peak_usage: 0,
            allocation_count: 0,
            deallocation_count: 0,
            by_category: HashMap::new(),
        }
    }
}

impl MemoryStats {
    /// Record an allocation
    pub fn record_allocation(&mut self, size: usize, category: MemoryCategory) {
        self.total_allocated += size;
        self.current_usage += size;
        self.allocation_count += 1;

        if self.current_usage > self.peak_usage {
            self.peak_usage = self.current_usage;
        }

        *self.by_category.entry(category).or_insert(0) += size;
    }

    /// Record a deallocation
    pub fn record_deallocation(&mut self, size: usize, category: MemoryCategory) {
        self.total_deallocated += size;
        if self.current_usage >= size {
            self.current_usage -= size;
        }
        self.deallocation_count += 1;

        if let Some(cat_size) = self.by_category.get_mut(&category) {
            if *cat_size >= size {
                *cat_size -= size;
            }
        }
    }

    /// Get current usage in MB
    pub fn current_mb(&self) -> f64 {
        self.current_usage as f64 / 1024.0 / 1024.0
    }

    /// Get peak usage in MB
    pub fn peak_mb(&self) -> f64 {
        self.peak_usage as f64 / 1024.0 / 1024.0
    }

    /// Get total allocated in MB
    pub fn total_allocated_mb(&self) -> f64 {
        self.total_allocated as f64 / 1024.0 / 1024.0
    }

    /// Get category breakdown sorted by size
    pub fn category_breakdown(&self) -> Vec<(MemoryCategory, usize)> {
        let mut categories: Vec<_> = self.by_category.iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        categories.sort_by(|a, b| b.1.cmp(&a.1));
        categories
    }
}

/// Global memory tracker
pub struct MemoryTracker {
    stats: Arc<RwLock<MemoryStats>>,
}

impl MemoryTracker {
    pub fn new() -> Self {
        Self {
            stats: Arc::new(RwLock::new(MemoryStats::default())),
        }
    }

    pub fn stats(&self) -> Arc<RwLock<MemoryStats>> {
        self.stats.clone()
    }

    /// Record an allocation
    pub fn allocate(&self, size: usize, category: MemoryCategory) {
        self.stats.write().record_allocation(size, category);
    }

    /// Record a deallocation
    pub fn deallocate(&self, size: usize, category: MemoryCategory) {
        self.stats.write().record_deallocation(size, category);
    }

    /// Simulate some allocations for testing
    pub fn simulate_allocations(&self) {
        use rand::Rng;
        let mut rng = rand::thread_rng();

        let categories = vec![
            MemoryCategory::Engine,
            MemoryCategory::Renderer,
            MemoryCategory::UI,
            MemoryCategory::Physics,
            MemoryCategory::Audio,
            MemoryCategory::Assets,
            MemoryCategory::Scripts,
        ];

        for category in categories {
            let size = rng.gen_range(1024 * 1024..100 * 1024 * 1024); // 1MB to 100MB
            self.allocate(size, category);
        }
    }
}

impl Default for MemoryTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared memory tracker
pub type SharedMemoryTracker = Arc<RwLock<MemoryTracker>>;

/// Create a shared memory tracker
pub fn create_memory_tracker() -> SharedMemoryTracker {
    let tracker = MemoryTracker::new();
    // Real allocations will be tracked by the global allocator
    Arc::new(RwLock::new(tracker))
}
