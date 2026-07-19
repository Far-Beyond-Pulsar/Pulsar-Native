use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Monotonic submission serials + completion watermark (C6). Frame-counter
/// arithmetic is forbidden (spec §20.1): completion is only ever inferred
/// from `Queue::on_submitted_work_done`.
pub struct SubmissionTracker {
    next: AtomicU64,
    completed: Arc<AtomicU64>,
}

impl SubmissionTracker {
    pub fn new() -> Self {
        Self { next: AtomicU64::new(1), completed: Arc::new(AtomicU64::new(0)) }
    }

    /// Reserve the serial for the next submission batch.
    pub fn next_serial(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed)
    }

    /// Register completion for work submitted up to `serial`. The queue
    /// timeline is FIFO: when this callback fires, all work ≤ serial is done.
    ///
    /// Must be called only after the work for `serial` has been submitted —
    /// signaling first completes the watermark early and breaks C6.
    pub fn signal_submitted(&self, queue: &wgpu::Queue, serial: u64) {
        let completed = Arc::clone(&self.completed);
        queue.on_submitted_work_done(move || {
            completed.fetch_max(serial, Ordering::AcqRel);
        });
    }

    /// Highest serial confirmed complete by the GPU.
    pub fn completed(&self) -> u64 {
        self.completed.load(Ordering::Acquire)
    }

    /// Test hook: the controllable completion signal (design §9) standing in
    /// for real GPU timing in retirement-invariant tests.
    #[doc(hidden)]
    pub fn force_complete(&self, serial: u64) {
        self.completed.fetch_max(serial, Ordering::AcqRel);
    }
}

impl Default for SubmissionTracker {
    fn default() -> Self {
        Self::new()
    }
}
