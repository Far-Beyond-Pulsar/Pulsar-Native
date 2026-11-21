//! A deterministic animated figure: a demo and test source that needs no
//! hardware, so the editor, the recorder and CI have something to capture.

use crate::error::CaptureError;
use crate::frame::PoseFrame;
use crate::rigs::{vrm_humanoid, RigIndex};
use crate::source::{
    spawn_capture_loop, CaptureSource, SourceDescriptor, SourceKind, SourceShared, SourceSink,
    SourceState, SourceStatus,
};
use editor_task_queue::ServiceHandle;
use std::f32::consts::TAU;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A rotation of `angle` radians about the unit `axis`, as (x, y, z, w).
pub fn axis_angle(axis: [f32; 3], angle: f32) -> [f32; 4] {
    let (s, c) = (angle * 0.5).sin_cos();
    [axis[0] * s, axis[1] * s, axis[2] * s, c]
}

/// The figure's pose at `frame` of a `fps` stream: a hip sway, swinging arms
/// and legs, a nodding head and a blink every two seconds. Pure, so a test can
/// predict any frame.
pub fn synthetic_pose(index: &RigIndex, frame: u64, fps: f32) -> Vec<f32> {
    let t = frame as f32 / fps;
    let mut values = vec![0.0; index.width()];
    for &at in index.rotation_columns() {
        values[at + 3] = 1.0;
    }
    let mut set = |name: &str, v: &[f32]| {
        if let Some(at) = index.column(name) {
            values[at..at + v.len()].copy_from_slice(v);
        }
    };
    set("Root.position", &[0.0, 0.0, 0.0]);
    set("Hips.position", &[(t * TAU * 0.5).sin() * 0.03, 0.95 + (t * TAU).sin().abs() * 0.02, 0.0]);
    set("Hips.rotation", &axis_angle([0.0, 1.0, 0.0], (t * TAU * 0.5).sin() * 0.15));
    let swing = (t * TAU).sin();
    set("LeftUpperArm.rotation", &axis_angle([1.0, 0.0, 0.0], swing * 0.6));
    set("RightUpperArm.rotation", &axis_angle([1.0, 0.0, 0.0], -swing * 0.6));
    set("LeftUpperLeg.rotation", &axis_angle([1.0, 0.0, 0.0], -swing * 0.5));
    set("RightUpperLeg.rotation", &axis_angle([1.0, 0.0, 0.0], swing * 0.5));
    set("Head.rotation", &axis_angle([1.0, 0.0, 0.0], (t * TAU * 0.5).sin() * 0.2));
    let blink = if (t % 2.0) < 0.12 { 1.0 } else { 0.0 };
    set("blend.Blink", &[blink]);
    values
}

/// A source of [`synthetic_pose`] at a fixed rate.
pub struct SyntheticSource {
    fps: f32,
    index: u32,
    shared: Arc<SourceShared>,
    handle: Option<ServiceHandle<()>>,
}

impl SyntheticSource {
    /// Figure number `index` (1-based): several can run side by side.
    pub fn numbered(fps: f32, index: u32) -> Self {
        Self { fps: fps.clamp(1.0, 1000.0), index, shared: SourceShared::new(), handle: None }
    }

    /// A synthetic figure streaming at `fps`.
    pub fn new(fps: f32) -> Self {
        Self::numbered(fps, 1)
    }
}

impl CaptureSource for SyntheticSource {
    fn descriptor(&self) -> SourceDescriptor {
        SourceDescriptor {
            id: if self.index <= 1 { "synthetic".to_owned() } else { format!("synthetic:{}", self.index) },
            kind: SourceKind::Synthetic,
            label: format!("Synthetic figure {} ({} Hz)", self.index, self.fps),
            rig: Some(vrm_humanoid()),
            has_video: false,
        }
    }

    fn start(&mut self, sink: SourceSink) -> Result<(), CaptureError> {
        if self.handle.is_some() {
            return Err(CaptureError::AlreadyRunning);
        }
        let (fps, shared) = (self.fps, Arc::clone(&self.shared));
        self.shared.set_state(SourceState::Starting);
        self.handle = Some(spawn_capture_loop("Synthetic source", "synthetic", move |ctx| {
            let index = RigIndex::new(&vrm_humanoid());
            let begun = Instant::now();
            let mut frame = 0u64;
            shared.set_state(SourceState::Live);
            while !ctx.is_cancelled() {
                ctx.heartbeat();
                let due = begun + Duration::from_secs_f64(frame as f64 / f64::from(fps));
                let now = Instant::now();
                if now < due {
                    std::thread::sleep((due - now).min(Duration::from_millis(5)));
                    continue;
                }
                sink.poses.push(PoseFrame {
                    sequence: frame,
                    time_s: sink.clock.now_s(),
                    values: synthetic_pose(&index, frame, fps),
                });
                shared.frame_delivered();
                frame += 1;
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

impl Drop for SyntheticSource {
    fn drop(&mut self) {
        self.stop();
    }
}
