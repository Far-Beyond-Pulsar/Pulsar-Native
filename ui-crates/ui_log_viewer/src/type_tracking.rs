//! Type-level allocation tracking using Layout (size + alignment)
//! Uses channel to offload expensive processing from allocator

use dashmap::DashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::alloc::Layout;
use std::sync::Arc;
use parking_lot::Mutex;
use crossbeam_channel::{Sender, Receiver, unbounded};

/// Allocation site information (identified by Layout)
#[derive(Clone, Debug)]
pub struct AllocationSite {
    pub type_signature: String,
    pub count: usize,
    pub total_bytes: usize,
    pub size: usize,
    pub align: usize,
}

/// Simple allocation record sent through channel (no processing)
#[derive(Clone, Debug)]
struct AllocRecord {
    size: usize,
    align: usize,
    is_alloc: bool, // true = alloc, false = dealloc
}

/// Type tracker using Layout (size + alignment) to identify allocations
pub struct TypeTracker {
    /// Map of (size, align) -> (count, bytes)
    layouts: DashMap<(usize, usize), (AtomicUsize, AtomicUsize)>,
    /// Channel for offloading to background thread
    sender: Sender<AllocRecord>,
    /// Database writer (lazy init)
    pub(crate) db_writer: Mutex<Option<Arc<crate::memory_database::MemoryDatabaseWriter>>>,
}

impl TypeTracker {
    pub fn new() -> Self {
        let (sender, receiver) = unbounded();
        
        // Spawn background processor thread
        std::thread::spawn(move || {
            background_processor(receiver);
        });
        
        Self {
            layouts: DashMap::new(),
            sender,
            db_writer: Mutex::new(None),
        }
    }
    
    /// Initialize database writer (called once at startup)
    pub fn init_db_writer(&self) {
        if self.db_writer.lock().is_some() {
            return; // Already initialized
        }
        
        match crate::memory_database::get_memory_db_path() {
            Ok(db_path) => {
                match crate::memory_database::MemoryDatabaseWriter::new(db_path.clone()) {
                    Ok(writer) => {
                        *self.db_writer.lock() = Some(Arc::new(writer));
                        tracing::info!("[MEMORY] Database writer initialized: {}", db_path.display());
                    }
                    Err(e) => {
                        tracing::error!("[MEMORY] Failed to initialize database writer: {}", e);
                    }
                }
            }
            Err(e) => {
                tracing::error!("[MEMORY] Failed to get database path: {}", e);
            }
        }
    }
    
    /// Record an allocation by its layout (FAST - just send to channel)
    #[inline]
    pub fn record_alloc(&self, layout: Layout) {
        let key = (layout.size(), layout.align());

        // Update in-memory counters (fast)
        self.layouts
            .entry(key)
            .or_insert_with(|| (AtomicUsize::new(0), AtomicUsize::new(0)))
            .value()
            .0
            .fetch_add(1, Ordering::Relaxed);

        if let Some(entry) = self.layouts.get(&key) {
            entry.1.fetch_add(layout.size(), Ordering::Relaxed);
        }
        
        // Send to background thread for DB processing (non-blocking)
        let _ = self.sender.send(AllocRecord {
            size: layout.size(),
            align: layout.align(),
            is_alloc: true,
        });
    }

    /// Record a deallocation by its layout (FAST - just send to channel)
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
        
        // Send to background thread for DB processing (non-blocking)
        let _ = self.sender.send(AllocRecord {
            size: layout.size(),
            align: layout.align(),
            is_alloc: false,
        });
    }

    /// Get the count of unique layouts being tracked (cheap - just DashMap len)
    pub fn layouts_count(&self) -> usize {
        self.layouts.len()
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

    /// Get snapshot for immediate display (still needed for fallback, but not in hot path)
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

/// Background thread that processes allocation records and writes to DB
fn background_processor(receiver: Receiver<AllocRecord>) {
    // Build aggregated snapshot periodically
    let mut last_write = std::time::Instant::now();
    let mut pending_records = Vec::with_capacity(10000);
    
    loop {
        // Drain channel in batches
        while let Ok(record) = receiver.try_recv() {
            pending_records.push(record);
            
            // Process in chunks to avoid unbounded growth
            if pending_records.len() >= 10000 {
                break;
            }
        }
        
        // Write to DB every 5 seconds if we have data
        if !pending_records.is_empty() && last_write.elapsed().as_secs() >= 5 {
            // Get current snapshot from TYPE_TRACKER and write to DB
            let sites = crate::TYPE_TRACKER.get_sites();
            
            if let Some(writer) = crate::TYPE_TRACKER.db_writer.lock().as_ref() {
                writer.write_batch(sites);
            }
            
            pending_records.clear();
            last_write = std::time::Instant::now();
        }
        
        // Sleep to avoid busy loop
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// Global type tracker instance
pub static TYPE_TRACKER: once_cell::sync::Lazy<TypeTracker> =
    once_cell::sync::Lazy::new(|| TypeTracker::new());
