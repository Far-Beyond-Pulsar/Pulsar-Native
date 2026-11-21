//! The seam between video and joints: pose estimators and the source that runs
//! one on a camera's frames.
//!
//! An estimator turns a camera frame into a pose. [`EstimatorSource`] runs one
//! on the newest frame of a camera's mailbox, stamped with the frame's own
//! capture time (so it lines up with every other source), and delivers poses
//! like any other source: they merge, record and display with the rest.

use crate::error::CaptureError;
use crate::frame::{PoseFrame, VideoFrame};
use crate::bus::VideoMailbox;
use crate::source::{
    spawn_capture_loop, CaptureSource, SourceDescriptor, SourceKind, SourceShared, SourceSink, SourceState,
    SourceStatus,
};
use editor_task_queue::ServiceHandle;
use mocap_format::RigProfile;
use std::sync::Arc;
use std::time::Duration;

/// Estimates a pose from a camera frame.
pub trait PoseEstimator: Send {
    /// The rig its poses are in.
    fn rig(&self) -> RigProfile;
    /// A short name for the sources list.
    fn name(&self) -> &str;
    /// The pose in `frame` (one value per rig column), or `None` when nobody is visible.
    fn estimate(&mut self, frame: &VideoFrame) -> Option<Vec<f32>>;
}

/// Loads an estimator on the source's own thread (a model can take seconds).
pub type EstimatorFactory = Arc<dyn Fn() -> Result<Box<dyn PoseEstimator>, String> + Send + Sync>;

/// Runs an estimator on a camera's frames.
pub struct EstimatorSource {
    id: String,
    label: String,
    rig: RigProfile,
    camera: Arc<VideoMailbox>,
    factory: EstimatorFactory,
    shared: Arc<SourceShared>,
    handle: Option<ServiceHandle<()>>,
}

impl EstimatorSource {
    /// A tracker for `camera` (its source id is `camera_id`), delivering in `rig`.
    pub fn new(
        camera_id: &str,
        camera_label: &str,
        camera: Arc<VideoMailbox>,
        rig: RigProfile,
        factory: EstimatorFactory,
    ) -> Self {
        Self {
            id: format!("pose:{camera_id}"),
            label: format!("Body tracking: {camera_label}"),
            rig,
            camera,
            factory,
            shared: SourceShared::new(),
            handle: None,
        }
    }

    /// The id of the camera source a tracker id (`pose:<camera id>`) reads.
    pub fn camera_of(tracker_id: &str) -> Option<&str> {
        tracker_id.strip_prefix("pose:")
    }
}

impl CaptureSource for EstimatorSource {
    fn descriptor(&self) -> SourceDescriptor {
        SourceDescriptor {
            id: self.id.clone(),
            kind: SourceKind::Estimator,
            label: self.label.clone(),
            rig: Some(self.rig.clone()),
            has_video: false,
        }
    }

    fn start(&mut self, sink: SourceSink) -> Result<(), CaptureError> {
        if self.handle.is_some() {
            return Err(CaptureError::AlreadyRunning);
        }
        let factory = Arc::clone(&self.factory);
        self.shared.set_state(SourceState::Starting);
        let (camera, shared) = (Arc::clone(&self.camera), Arc::clone(&self.shared));
        self.handle = Some(spawn_capture_loop("Body tracker", &self.label, move |ctx| {
            ctx.set_phase("Loading the pose model");
            let mut estimator = match factory() {
                Ok(estimator) => estimator,
                Err(reason) => {
                    tracing::warn!("pose model did not load: {reason}");
                    shared.set_state(SourceState::Failed(reason));
                    return;
                }
            };
            ctx.set_phase("Tracking");
            shared.set_state(SourceState::Live);
            let (mut seen, mut sequence) = (0u64, 0u64);
            while !ctx.is_cancelled() {
                ctx.heartbeat();
                let Some((count, frame)) = camera.latest_if_newer(seen) else {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                };
                seen = count;
                if let Some(values) = estimator.estimate(&frame) {
                    sequence += 1;
                    sink.poses.push(PoseFrame { sequence, time_s: frame.time_s, values });
                    shared.frame_delivered();
                }
            }
            shared.set_state(SourceState::Idle);
        })?);
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.request_stop();
            let _ = handle.join();
        }
        self.shared.set_state(SourceState::Idle);
    }

    fn status(&self) -> SourceStatus {
        self.shared.status()
    }
}

impl Drop for EstimatorSource {
    fn drop(&mut self) {
        self.stop();
    }
}
