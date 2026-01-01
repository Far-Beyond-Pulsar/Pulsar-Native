/// GPU Memory Allocation Tracker
/// Instruments RenderImage creation and destruction to track VRAM leaks
use std::sync::atomic::{AtomicUsize, AtomicU64, Ordering};
use std::sync::Arc;
use parking_lot::Mutex;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct AllocationInfo {
    pub size_bytes: usize,
    pub width: u32,
    pub height: u32,
    pub timestamp: std::time::Instant,
    pub backtrace: String,
}

pub struct GpuMemTracker {
    total_allocated: AtomicU64,
    total_freed: AtomicU64,
    current_allocations: AtomicUsize,
    peak_allocations: AtomicUsize,
    allocations: Arc<Mutex<HashMap<usize, AllocationInfo>>>,
    next_id: AtomicUsize,
}

impl GpuMemTracker {
    pub fn new() -> Self {
        Self {
            total_allocated: AtomicU64::new(0),
            total_freed: AtomicU64::new(0),
            current_allocations: AtomicUsize::new(0),
            peak_allocations: AtomicUsize::new(0),
            allocations: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicUsize::new(1),
        }
    }

    pub fn track_allocation(&self, width: u32, height: u32) -> usize {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let size_bytes = (width * height * 4) as usize; // RGBA8

        self.total_allocated.fetch_add(size_bytes as u64, Ordering::Relaxed);
        let current = self.current_allocations.fetch_add(1, Ordering::Relaxed) + 1;

        // Update peak
        let mut peak = self.peak_allocations.load(Ordering::Relaxed);
        while current > peak {
            match self.peak_allocations.compare_exchange_weak(
                peak,
                current,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(x) => peak = x,
            }
        }

        let info = AllocationInfo {
            size_bytes,
            width,
            height,
            timestamp: std::time::Instant::now(),
            backtrace: format!("{}x{}", width, height), // Could use backtrace crate for full stack
        };

        self.allocations.lock().insert(id, info);

        id
    }

    pub fn track_deallocation(&self, id: usize) {
        if let Some(info) = self.allocations.lock().remove(&id) {
            self.total_freed.fetch_add(info.size_bytes as u64, Ordering::Relaxed);
            let current = self.current_allocations.fetch_sub(1, Ordering::Relaxed) - 1;

        } else {
            tracing::warn!("[GPU-MEM] WARNING: Attempted to free unknown allocation #{}", id);
        }
    }

    pub fn print_stats(&self) {
        let total_alloc = self.total_allocated.load(Ordering::Relaxed);
        let total_free = self.total_freed.load(Ordering::Relaxed);
        let leaked = total_alloc - total_free;
        let current = self.current_allocations.load(Ordering::Relaxed);
        let peak = self.peak_allocations.load(Ordering::Relaxed);

        tracing::info!("\n========== GPU MEMORY STATS ==========");
        tracing::info!("Total Allocated: {:.2} MB", total_alloc as f64 / 1_000_000.0);
        tracing::info!("Total Freed: {:.2} MB", total_free as f64 / 1_000_000.0);
        tracing::info!("Leaked: {:.2} MB", leaked as f64 / 1_000_000.0);
        tracing::info!("Current Allocations: {}", current);
        tracing::info!("Peak Allocations: {}", peak);

        if current > 0 {
            tracing::warn!("\nLEAKED ALLOCATIONS:");
            let allocs = self.allocations.lock();
            for (id, info) in allocs.iter() {
                tracing::warn!(
                    "  #{}: {}x{} = {} bytes (age: {:.2}s)",
                    id,
                    info.width,
                    info.height,
                    info.size_bytes,
                    info.timestamp.elapsed().as_secs_f64()
                );
            }
        }
        tracing::info!("======================================\n");
    }

    pub fn get_leaked_bytes(&self) -> u64 {
        self.total_allocated.load(Ordering::Relaxed) - self.total_freed.load(Ordering::Relaxed)
    }
    
    /// Get current memory stats (leaked_bytes, current_allocations)
    pub fn get_stats(&self) -> (u64, usize) {
        let leaked = self.get_leaked_bytes();
        let current = self.current_allocations.load(Ordering::Relaxed);
        (leaked, current)
    }
}

// Global tracker instance
lazy_static::lazy_static! {
    pub static ref GPU_MEM_TRACKER: GpuMemTracker = GpuMemTracker::new();
}
