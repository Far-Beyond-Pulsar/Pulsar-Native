//! Type-level allocation tracking using Layout (size + alignment)
//!
//! Tracks allocations by their Layout signature to identify types.

use dashmap::DashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::alloc::Layout;

/// Allocation site information (identified by Layout)
#[derive(Clone, Debug)]
pub struct AllocationSite {
    pub type_signature: String,
    pub count: usize,
    pub total_bytes: usize,
    pub size: usize,
    pub align: usize,
}

/// Type tracker using Layout (size + alignment) to identify allocations
pub struct TypeTracker {
    /// Map of (size, align) -> (count, bytes)
    layouts: DashMap<(usize, usize), (AtomicUsize, AtomicUsize)>,
}

impl TypeTracker {
    pub fn new() -> Self {
        Self {
            layouts: DashMap::new(),
        }
    }

    /// Record an allocation by its layout
    #[inline]
    pub fn record_alloc(&self, layout: Layout) {
        let key = (layout.size(), layout.align());

        self.layouts
            .entry(key)
            .or_insert_with(|| (AtomicUsize::new(0), AtomicUsize::new(0)))
            .value()
            .0
            .fetch_add(1, Ordering::Relaxed);

        if let Some(entry) = self.layouts.get(&key) {
            entry.1.fetch_add(layout.size(), Ordering::Relaxed);
        }
    }

    /// Record a deallocation by its layout
    #[inline]
    pub fn record_dealloc(&self, layout: Layout) {
        let key = (layout.size(), layout.align());

        if let Some(entry) = self.layouts.get(&key) {
            entry.0.fetch_sub(1, Ordering::Relaxed);
            entry.1.fetch_sub(
                layout.size().min(entry.1.load(Ordering::Relaxed)),
                Ordering::Relaxed,
            );
        }
    }

    /// Identify common Rust types by their layout
    fn identify_type(size: usize, align: usize) -> String {
        match (size, align) {
            // Common standard types
            (24, 8) => "String/Vec<T>/Box<[T]>".to_string(),
            (16, 8) => "&str/&[T]/Option<Box<T>>".to_string(),
            (8, 8) => "usize/isize/&T/*const T".to_string(),
            (4, 4) => "u32/i32/f32/char".to_string(),
            (2, 2) => "u16/i16".to_string(),
            (1, 1) => "u8/i8/bool".to_string(),
            (0, 1) => "ZST (Zero-Sized Type)".to_string(),

            // HashMap/BTreeMap internal nodes
            (32, 8) => "HashMap Node".to_string(),
            (48, 8) => "Large Struct (48B)".to_string(),
            (64, 8) => "Cache Line Struct (64B)".to_string(),

            // GPUI types (common sizes)
            (56, 8) => "GPUI Element".to_string(),
            (40, 8) => "Medium Struct (40B)".to_string(),

            // Generic patterns
            _ if size >= 1024 * 1024 => format!("Large Buffer ({}MB, align {})", size / 1024 / 1024, align),
            _ if size >= 1024 => format!("Buffer ({}KB, align {})", size / 1024, align),
            _ if size > 64 => format!("Struct ({}B, align {})", size, align),
            _ => format!("Type ({}B, align {})", size, align),
        }
    }

    /// Get all allocation sites sorted by total bytes
    pub fn get_sites(&self) -> Vec<AllocationSite> {
        let mut sites: Vec<AllocationSite> = self
            .layouts
            .iter()
            .map(|entry| {
                let (size, align) = *entry.key();
                let count = entry.value().0.load(Ordering::Relaxed);
                let total_bytes = entry.value().1.load(Ordering::Relaxed);

                AllocationSite {
                    type_signature: Self::identify_type(size, align),
                    count,
                    total_bytes,
                    size,
                    align,
                }
            })
            .filter(|site| site.total_bytes > 0 && site.count > 0)
            .collect();

        sites.sort_by(|a, b| b.total_bytes.cmp(&a.total_bytes));
        sites.truncate(100); // Keep top 100
        sites
    }

    /// Clear all tracked data
    pub fn clear(&self) {
        self.layouts.clear();
    }
}

/// Global type tracker instance
pub static TYPE_TRACKER: once_cell::sync::Lazy<TypeTracker> =
    once_cell::sync::Lazy::new(|| TypeTracker::new());
