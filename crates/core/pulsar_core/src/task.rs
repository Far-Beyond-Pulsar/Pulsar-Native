use smol::{Executor, Task};
use std::future::Future;
use std::sync::Arc;

pub struct TaskPool {
    executor: Arc<Executor<'static>>,
    _threads: Vec<std::thread::JoinHandle<()>>,
    /// Dropping this closes the channel, which ends every worker's `run`.
    _stop: smol::channel::Sender<()>,
}

impl TaskPool {
    pub fn new(thread_count: usize) -> Self {
        let executor = Arc::new(Executor::new());
        // Workers park instead of spinning, and the first park initialises
        // smol's global reactor. Do that here so it is not a lazy allocation
        // in the middle of the game loop.
        smol::block_on(smol::Timer::after(std::time::Duration::from_millis(1)));
        let (stop_tx, stop_rx) = smol::channel::bounded::<()>(1);
        // Thread creation does not mean the executor is initialized. Wait for
        // each worker to start so lazy thread-local/queue allocations cannot
        // appear later in the game loop's steady-state memory measurements.
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(0);

        let threads = (0..thread_count.max(1))
            .map(|i| {
                let ex = executor.clone();
                let stop = stop_rx.clone();
                let ready = ready_tx.clone();
                std::thread::Builder::new()
                    .name(format!("pulsar-task-{i}"))
                    .spawn(move || {
                        // Parks the thread while there is no work (a spinning
                        // worker burns a whole core per thread, per process);
                        // returns once the pool is dropped.
                        smol::block_on(ex.run(async move {
                            let mut closed = Box::pin(stop.recv());
                            // One poll registers the listener, so the worker
                            // has done its lazy allocation before the
                            // constructor is released.
                            let _ = smol::future::poll_once(&mut closed).await;
                            ready
                                .send(())
                                .expect("task pool constructor must wait for workers");
                            drop(ready);
                            let _ = closed.await;
                        }));
                    })
                    .expect("failed to spawn task pool thread")
            })
            .collect();

        drop(ready_tx);
        for _ in 0..thread_count.max(1) {
            ready_rx
                .recv()
                .expect("task pool worker failed during initialization");
        }

        // Let the workers reach their first park (a one-off sleeper
        // registration) before returning, again so it is not a lazy
        // allocation inside the game loop.
        std::thread::sleep(std::time::Duration::from_millis(3));

        Self {
            executor,
            _threads: threads,
            _stop: stop_tx,
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
