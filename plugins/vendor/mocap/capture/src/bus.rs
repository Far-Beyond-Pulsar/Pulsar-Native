//! Hand-off between a capture thread and its consumers.
//!
//! Video never queues: a consumer reads the newest frame, so a slow consumer
//! costs frames, not latency. Poses queue (a recording must not lose them
//! silently) in a bounded buffer that drops the oldest and counts it.

use crate::frame::{PoseFrame, VideoFrame};
use parking_lot::{Condvar, Mutex};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Counters a HUD shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BusStats {
    pub pushed: u64,
    /// Pose frames dropped because the queue was full.
    pub dropped: u64,
}

#[derive(Default)]
struct PoseQueue {
    queue: VecDeque<Arc<PoseFrame>>,
    latest: Option<Arc<PoseFrame>>,
}

/// A bounded queue of pose frames with a non-consuming "latest" view.
pub struct PoseBus {
    inner: Mutex<PoseQueue>,
    ready: Condvar,
    capacity: usize,
    pushed: AtomicU64,
    dropped: AtomicU64,
}

impl PoseBus {
    /// A bus holding up to `capacity` unread frames.
    pub fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(PoseQueue::default()),
            ready: Condvar::new(),
            capacity: capacity.max(1),
            pushed: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
        })
    }

    /// Adds a frame; when the queue is full the oldest unread frame is dropped
    /// and counted.
    pub fn push(&self, frame: PoseFrame) {
        let frame = Arc::new(frame);
        let mut inner = self.inner.lock();
        if inner.queue.len() >= self.capacity {
            inner.queue.pop_front();
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
        inner.queue.push_back(Arc::clone(&frame));
        inner.latest = Some(frame);
        self.pushed.fetch_add(1, Ordering::Relaxed);
        drop(inner);
        self.ready.notify_one();
    }

    /// Moves every unread frame into `out`, oldest first.
    pub fn drain(&self, out: &mut Vec<Arc<PoseFrame>>) {
        out.extend(self.inner.lock().queue.drain(..));
    }

    /// Waits up to `timeout` for a frame, then drains. Returns how many
    /// frames were taken (zero on timeout).
    pub fn wait_drain(&self, timeout: Duration, out: &mut Vec<Arc<PoseFrame>>) -> usize {
        let mut inner = self.inner.lock();
        if inner.queue.is_empty() {
            self.ready.wait_for(&mut inner, timeout);
        }
        let taken = inner.queue.len();
        out.extend(inner.queue.drain(..));
        taken
    }

    /// The newest frame, without consuming anything (the preview reads this).
    pub fn latest(&self) -> Option<Arc<PoseFrame>> {
        self.inner.lock().latest.clone()
    }

    /// Frames pushed and dropped so far.
    pub fn stats(&self) -> BusStats {
        BusStats {
            pushed: self.pushed.load(Ordering::Relaxed),
            dropped: self.dropped.load(Ordering::Relaxed),
        }
    }
}

/// The newest camera frame.
#[derive(Default)]
pub struct VideoMailbox {
    latest: Mutex<Option<Arc<VideoFrame>>>,
    published: AtomicU64,
}

impl VideoMailbox {
    /// An empty mailbox.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Replaces the newest frame.
    pub fn put(&self, frame: VideoFrame) {
        *self.latest.lock() = Some(Arc::new(frame));
        self.published.fetch_add(1, Ordering::Release);
    }

    /// The newest frame.
    pub fn latest(&self) -> Option<Arc<VideoFrame>> {
        self.latest.lock().clone()
    }

    /// The newest frame when it is newer than `seen` (a count from
    /// [`published`](Self::published)); the viewport uses this to upload only
    /// new frames.
    pub fn latest_if_newer(&self, seen: u64) -> Option<(u64, Arc<VideoFrame>)> {
        let published = self.published.load(Ordering::Acquire);
        if published <= seen {
            return None;
        }
        self.latest().map(|frame| (published, frame))
    }

    /// How many frames have been put.
    pub fn published(&self) -> u64 {
        self.published.load(Ordering::Acquire)
    }
}
