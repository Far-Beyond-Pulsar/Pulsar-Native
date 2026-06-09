use smol::{Executor, Task};
use std::future::Future;
use std::sync::Arc;

pub struct TaskPool {
    executor: Arc<Executor<'static>>,
    _threads: Vec<std::thread::JoinHandle<()>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl TaskPool {
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

    pub fn spawn<T: Send + 'static>(
        &self,
        future: impl Future<Output = T> + Send + 'static,
    ) -> Task<T> {
        self.executor.spawn(future)
    }

    pub fn fire<T: Send + 'static>(&self, future: impl Future<Output = T> + Send + 'static) {
        self.executor.spawn(future).detach();
    }
}

impl Drop for TaskPool {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}
