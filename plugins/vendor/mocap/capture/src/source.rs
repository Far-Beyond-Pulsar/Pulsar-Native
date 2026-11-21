//! The capture source contract and the shared plumbing every source uses.

use crate::bus::{PoseBus, VideoMailbox};
use editor_task_queue::{ServiceDescription, ServiceHandle, TaskContext, ThreadRole};
use mocap_format::RigProfile;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// What a source is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    Camera,
    Vmc,
    Synthetic,
}

impl SourceKind {
    /// The stable name written into a take's source list.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Camera => "camera",
            Self::Vmc => "vmc",
            Self::Synthetic => "synthetic",
        }
    }
}

/// What a source offers, listed by the editor before it starts.
#[derive(Clone, Debug)]
pub struct SourceDescriptor {
    pub id: String,
    pub kind: SourceKind,
    pub label: String,
    /// The rig its pose frames are in; `None` for video only.
    pub rig: Option<RigProfile>,
    pub has_video: bool,
}

/// The lifecycle the sources list shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceState {
    Idle,
    Starting,
    Live,
    Failed(String),
}

/// A snapshot of a source for the UI.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceStatus {
    pub state: SourceState,
    pub frames: u64,
    /// Frames per second over the last second.
    pub rate_hz: f32,
}

/// The seconds clock every frame of a session is stamped with.
#[derive(Clone, Copy, Debug)]
pub struct SessionClock(Instant);

impl SessionClock {
    /// Starts a clock now.
    pub fn start() -> Self {
        Self(Instant::now())
    }

    /// Seconds since it started.
    pub fn now_s(&self) -> f64 {
        self.0.elapsed().as_secs_f64()
    }
}

/// Where a source delivers.
#[derive(Clone)]
pub struct SourceSink {
    pub poses: Arc<PoseBus>,
    pub video: Arc<VideoMailbox>,
    pub clock: SessionClock,
}

/// Counters and state a source's thread updates and its owner reads.
#[derive(Default)]
pub struct SourceShared {
    state: Mutex<Option<SourceState>>,
    frames: AtomicU64,
    rate: Mutex<RateWindow>,
}

#[derive(Default)]
struct RateWindow {
    since: Option<Instant>,
    count: u32,
    hz: f32,
}

impl SourceShared {
    /// Fresh shared state in `Idle`.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Sets the state.
    pub fn set_state(&self, state: SourceState) {
        *self.state.lock() = Some(state);
    }

    /// Counts one delivered frame and refreshes the rate once a second.
    pub fn frame_delivered(&self) {
        self.frames.fetch_add(1, Ordering::Relaxed);
        let mut rate = self.rate.lock();
        let now = Instant::now();
        let since = *rate.since.get_or_insert(now);
        rate.count += 1;
        let elapsed = now.duration_since(since).as_secs_f32();
        if elapsed >= 1.0 {
            rate.hz = rate.count as f32 / elapsed;
            rate.count = 0;
            rate.since = Some(now);
        }
    }

    /// The snapshot the UI reads.
    pub fn status(&self) -> SourceStatus {
        SourceStatus {
            state: self.state.lock().clone().unwrap_or(SourceState::Idle),
            frames: self.frames.load(Ordering::Relaxed),
            rate_hz: self.rate.lock().hz,
        }
    }
}

/// A source of video or poses. It owns its thread; `start` returns once the
/// source is running (or has failed to) and `stop` returns once it is gone.
pub trait CaptureSource: Send {
    /// What the source is and offers.
    fn descriptor(&self) -> SourceDescriptor;
    /// Starts delivering into `sink`.
    fn start(&mut self, sink: SourceSink) -> Result<(), crate::CaptureError>;
    /// Stops delivering and joins the source's thread.
    fn stop(&mut self);
    /// The source's current state.
    fn status(&self) -> SourceStatus;
}

/// Starts a capture loop as a registered service (visible in the Tasks window
/// with a heartbeat). The loop watches `ctx.is_cancelled()`.
pub fn spawn_capture_loop(
    title: &str,
    owner: &str,
    body: impl FnOnce(&TaskContext) + Send + 'static,
) -> Result<ServiceHandle<()>, crate::CaptureError> {
    let description = ServiceDescription::new(title, "mocap.capture", title, ThreadRole::Io)
        .owner(owner)
        .heartbeat(std::time::Duration::from_secs(2));
    editor_task_queue::spawn_service(description, body)
        .map_err(|e| crate::CaptureError::Thread(e.to_string()))
}
