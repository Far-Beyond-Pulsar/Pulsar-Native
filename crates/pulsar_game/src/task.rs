use smol::{Executor, Task};
use std::future::Future;
use std::sync::Arc;

/// Async task pool backed by a smol `Executor`.
///
/// Spawned tasks run on the pool's background threads.  The pool owns its
/// threads; dropping the `TaskPool` sends a shutdown signal and joins them.
pub struct TaskPool {
    executor: Arc<Executor<'static>>,
    // Shutdown flag.
    _threads: Vec<std::thread::JoinHandle<()>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl TaskPool {
    /// Create a pool with `thread_count` background threads.
    pub fn new(thread_count: usize) -> Self {
        let executor = Arc::new(Executor::new());
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));

        let threads = (0..thread_count.max(1))
            .map(|i| {
                let ex = executor.clone();
                let stop = stop.clone();
                std::thread::Builder::new()
                    .name(format!("pulsar-task-{i}"))
                    .spawn(move || {
                        smol::block_on(async {
                            loop {
                                if stop.load(std::sync::atomic::Ordering::Relaxed) {
                                    break;
                                }
                                // Run one tick; yield if nothing is ready.
                                if !ex.try_tick() {
                                    std::thread::yield_now();
                                }
                            }
                        });
                    })
                    .expect("failed to spawn task pool thread")
            })
            .collect();

        Self {
            executor,
            _threads: threads,
            stop,
        }
    }

    /// Spawn a future on the pool.  Returns a `Task<T>` handle that can be
    /// awaited or detached.
    pub fn spawn<T: Send + 'static>(
        &self,
        future: impl Future<Output = T> + Send + 'static,
    ) -> Task<T> {
        self.executor.spawn(future)
    }

    /// Spawn a future and detach it — fire-and-forget.
    pub fn fire<T: Send + 'static>(&self, future: impl Future<Output = T> + Send + 'static) {
        self.executor.spawn(future).detach();
    }
}

impl Drop for TaskPool {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}
