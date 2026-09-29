//! Isolated allocator check: worker startup must finish before construction
//! returns. No other tests share this process and no tasks run during sampling.
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::{Duration, Instant};

static BYTES: AtomicIsize = AtomicIsize::new(0);
static BLOCKS: AtomicIsize = AtomicIsize::new(0);
struct Counting;
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc(layout);
        if !ptr.is_null() {
            BYTES.fetch_add(layout.size() as isize, Ordering::Relaxed);
            BLOCKS.fetch_add(1, Ordering::Relaxed);
        }
        ptr
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout);
        BYTES.fetch_sub(layout.size() as isize, Ordering::Relaxed);
        BLOCKS.fetch_sub(1, Ordering::Relaxed);
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let result = System.realloc(ptr, layout, size);
        if !result.is_null() {
            BYTES.fetch_add(size as isize - layout.size() as isize, Ordering::Relaxed);
        }
        result
    }
}
#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn heap() -> (isize, isize) {
    (BYTES.load(Ordering::SeqCst), BLOCKS.load(Ordering::SeqCst))
}

#[test]
fn idle_workers_do_not_allocate_after_constructor_returns() {
    for threads in [0, 1, 4] {
        for _ in 0..4 {
            let pool = pulsar_core::TaskPool::new(threads);
            let before = heap();
            let start = Instant::now();
            while start.elapsed() < Duration::from_millis(30) {
                std::hint::spin_loop();
            }
            let after = heap();
            // Ready-channel and prior worker teardown may still free memory.
            assert!(
                after.0 <= before.0 && after.1 <= before.1,
                "{threads} workers allocated after construction: {before:?} -> {after:?}"
            );
            drop(pool);
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
