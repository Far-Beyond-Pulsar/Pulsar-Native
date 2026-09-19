//! Global profiler state management

use crossbeam_queue::SegQueue;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use crate::events::ProfileEvent;

/// Global profiler state
pub struct Profiler {
    enabled: AtomicBool,
    /// Lock-free producer queue. Profile scopes publish directly here; there
    /// is no channel mutex, rendezvous, or producer-side capacity wait.
    pending: SegQueue<ProfileEvent>,
    events: SegQueue<ProfileEvent>,
    retained_count: AtomicUsize,
    max_events: usize,
    dropped_events: AtomicU64,
    process_id: u32,
}

/// Kept for source compatibility with embedders that configure the old queue.
pub const DEFAULT_EVENT_QUEUE_CAPACITY: usize = 131_072;

/// Retain enough data for a useful session while keeping `get_all_events()` a
/// bounded operation. The SQLite exporter can still persist each delta as it
/// is collected, so this is only the in-memory safety net.
pub const DEFAULT_RETAINED_EVENT_CAPACITY: usize = 1_000_000;

impl Profiler {
    pub fn new() -> Self {
        Self::with_capacity(
            DEFAULT_EVENT_QUEUE_CAPACITY,
            DEFAULT_RETAINED_EVENT_CAPACITY,
        )
    }

    /// Construct a profiler with explicit queue and retention limits.
    ///
    /// This is primarily useful for embedding and tests. Production callers
    /// should normally use [`Profiler::new`].
    pub fn with_capacity(queue_capacity: usize, max_events: usize) -> Self {
        let _ = queue_capacity;
        Self {
            enabled: AtomicBool::new(false),
            pending: SegQueue::new(),
            events: SegQueue::new(),
            retained_count: AtomicUsize::new(0),
            max_events: max_events.max(1),
            dropped_events: AtomicU64::new(0),
            process_id: std::process::id(),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn enable(&self) {
        self.enabled.store(true, Ordering::Release);
    }

    pub fn disable(&self) {
        self.enabled.store(false, Ordering::Release);
    }

    pub fn submit_event(&self, event: ProfileEvent) {
        self.pending.push(event);
    }

    pub fn collect_events(&self) -> Vec<ProfileEvent> {
        let mut collected = Vec::new();
        while let Some(event) = self.pending.pop() {
            collected.push(event);
        }

        for event in &collected {
            self.events.push(event.clone());
            self.retained_count.fetch_add(1, Ordering::Relaxed);
        }
        while self.retained_count.load(Ordering::Relaxed) > self.max_events {
            if self.events.pop().is_some() {
                self.retained_count.fetch_sub(1, Ordering::Relaxed);
            } else {
                break;
            }
        }
        collected
    }

    pub fn get_all_events(&self) -> Vec<ProfileEvent> {
        // This is called after collection stops when exporting a session. A
        // lock-free queue has no snapshot operation, so take and restore the
        // retained values without ever blocking a producer.
        let mut events = Vec::with_capacity(self.retained_count.load(Ordering::Relaxed));
        while let Some(event) = self.events.pop() {
            self.retained_count.fetch_sub(1, Ordering::Relaxed);
            events.push(event);
        }
        for event in &events {
            self.events.push(event.clone());
            self.retained_count.fetch_add(1, Ordering::Relaxed);
        }
        events
    }

    pub fn clear(&self) {
        while self.events.pop().is_some() {
            self.retained_count.fetch_sub(1, Ordering::Relaxed);
        }
        while self.pending.pop().is_some() {}
        self.dropped_events.store(0, Ordering::Relaxed);
    }

    /// Number of events discarded because the producer queue was full.
    pub fn dropped_event_count(&self) -> u64 {
        self.dropped_events.load(Ordering::Relaxed)
    }

    /// Number of events waiting to be collected by the consumer.
    pub fn pending_event_count(&self) -> usize {
        self.pending.len()
    }

    /// Number of events currently retained in memory.
    pub fn retained_event_count(&self) -> usize {
        self.retained_count.load(Ordering::Relaxed)
    }

    /// Maximum number of events retained in memory.
    pub fn max_event_count(&self) -> usize {
        self.max_events
    }

    pub fn get_process_id(&self) -> u32 {
        self.process_id
    }
}

#[cfg(test)]
mod tests {
    use super::Profiler;
    use crate::events::ProfileEvent;

    fn event(index: usize) -> ProfileEvent {
        ProfileEvent {
            scope_id: index as u64 + 1,
            parent_scope_id: None,
            name: format!("event-{index}"),
            thread_id: 1,
            thread_name: None,
            process_id: 1,
            parent_name: None,
            start_ns: index as u64,
            duration_ns: 1,
            depth: 0,
            location: None,
            metadata: None,
            track_name: None,
        }
    }

    #[test]
    fn retention_is_bounded_and_keeps_the_newest_events() {
        let profiler = Profiler::with_capacity(32, 3);
        profiler.enable();
        for index in 0..8 {
            profiler.submit_event(event(index));
        }

        let collected = profiler.collect_events();
        assert_eq!(collected.len(), 8);
        assert_eq!(profiler.retained_event_count(), 3);
        assert_eq!(
            profiler
                .get_all_events()
                .iter()
                .map(|event| event.start_ns)
                .collect::<Vec<_>>(),
            vec![5, 6, 7]
        );
    }

    #[test]
    fn producer_queue_never_blocks_or_drops_events() {
        let profiler = Profiler::with_capacity(1, 16);
        profiler.enable();
        for index in 0..64 {
            profiler.submit_event(event(index));
        }

        assert_eq!(profiler.dropped_event_count(), 0);
        assert_eq!(profiler.pending_event_count(), 64);
        assert_eq!(profiler.collect_events().len(), 64);
        profiler.clear();
        assert_eq!(profiler.dropped_event_count(), 0);
    }
}
