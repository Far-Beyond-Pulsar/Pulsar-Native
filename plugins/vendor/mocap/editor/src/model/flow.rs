//! The guided multi-camera calibration, as part of the session model: walks the
//! subject through the steps, collects synchronized keypoints from the camera
//! trackers while each step is done, solves in a background task and, on success,
//! turns the trackers' 2D keypoints into recorded, triangulated 3D ones.

use super::{SessionModel, SourceEntry};
use editor_task_queue::{TaskDescription, TaskDuration};
use mocap_capture::{CalibrationError, Calibrator, CameraRig, CameraSpec, Check, EstimatorSource, Features, Guide, HoldAverager, PoseFrame, Step, SAMPLES_TARGET};
use mocap_format::{ChannelDesc, ChannelKind, Quantisation, RigProfile};
use parking_lot::Mutex;
use std::ops::Range;
use std::sync::Arc;

/// Longest gap between the cameras' frames for them to count as one moment.
const SYNC_WINDOW_S: f64 = 0.08;
/// A triangulated point whose cameras disagree by more than this (pixels) is dropped.
const MAX_POINT_ERROR_PX: f32 = 15.0;
/// The body scale the priors assume (a 1.75 m adult).
const DEFAULT_BODY_SCALE: f64 = 1.0;

/// Where the calibration is.
#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    Idle,
    Guiding,
    Solving,
    Done,
    Failed(String),
}

type SolveSlot = Arc<Mutex<Option<Result<(CameraRig, Vec<Check>), String>>>>;

/// The state of a calibration in progress (or its result).
pub struct CalibrationFlow {
    pub phase: Phase,
    pub guide: Guide,
    /// Samples collected for the solver so far.
    pub collected: usize,
    pub checks: Vec<Check>,
    pub rms_px: Option<f64>,
    /// The subject's height over 1.75 m, which sets the metric scale.
    pub body_scale: f64,
    calibrator: Option<Calibrator>,
    /// Averages a held pose over several frames into one clean sample.
    averager: HoldAverager,
    /// `(tracker source id, camera frame aspect)`, in calibrator camera order.
    trackers: Vec<(String, f32)>,
    last_seq: Vec<u64>,
    slot: SolveSlot,
}

impl Default for CalibrationFlow {
    fn default() -> Self {
        Self {
            phase: Phase::Idle,
            guide: Guide::new(),
            collected: 0,
            checks: Vec::new(),
            rms_px: None,
            body_scale: DEFAULT_BODY_SCALE,
            calibrator: None,
            averager: HoldAverager::new(),
            trackers: Vec::new(),
            last_seq: Vec::new(),
            slot: Arc::default(),
        }
    }
}

/// How the triangulated keypoints are taken from a merged frame.
pub struct Derived {
    /// `(index of the camera in the rig, its keypoint columns in the merged frame)`.
    pub cameras: Vec<(usize, Range<usize>)>,
}

impl Derived {
    /// Finds each running tracker's keypoint columns in the merged `rig`; `None`
    /// unless at least two of them are cameras of `calibrated`.
    pub fn for_rig(rig: &RigProfile, tracker_ids: &[String], calibrated: &CameraRig) -> Option<Self> {
        let offsets = rig.column_offsets();
        let cameras: Vec<(usize, Range<usize>)> = tracker_ids
            .iter()
            .filter_map(|id| {
                let camera = EstimatorSource::camera_of(id)?;
                let index = calibrated.cameras.iter().position(|c| c.id == camera)?;
                let prefix = format!("{id}/");
                let first = rig.channels.iter().position(|c| c.name.starts_with(&prefix))?;
                let last = rig.channels.iter().rposition(|c| c.name.starts_with(&prefix))?;
                Some((index, offsets[first]..offsets[last] + rig.channels[last].kind.width()))
            })
            .collect();
        (cameras.len() >= 2).then_some(Self { cameras })
    }

    /// The 17 triangulated keypoints (`x, y, z` each; gaps are `NaN`) of one merged frame.
    pub fn triangulate(&self, rig: &CameraRig, values: &[f32]) -> Vec<f32> {
        let mut views: Vec<Option<Vec<[f32; 3]>>> = vec![None; rig.cameras.len()];
        for (index, range) in &self.cameras {
            if let Some(slice) = values.get(range.clone()) {
                views[*index] = Some(slice.chunks_exact(3).map(|p| [p[0], p[1], p[2]]).collect());
            }
        }
        let refs: Vec<Option<&[[f32; 3]]>> = views.iter().map(|v| v.as_deref()).collect();
        rig.triangulate_frame(&refs)
            .into_iter()
            .flat_map(|p| if p[3] <= MAX_POINT_ERROR_PX { [p[0], p[1], p[2]] } else { [f32::NAN; 3] })
            .collect()
    }

    /// Appends the triangulated group's channels to `rig`.
    pub fn extend_rig(rig: &mut RigProfile) {
        rig.channels.extend(mocap_capture::COCO17.iter().map(|name| ChannelDesc {
            name: format!("subject3d/kp3d.{name}"),
            kind: ChannelKind::Position3,
            joint: None,
            quantisation: Quantisation::Lossless,
        }));
    }
}

fn keypoints_of(frame: &PoseFrame) -> Vec<[f32; 3]> {
    frame.values.chunks_exact(3).map(|p| [p[0], p[1], p[2]]).collect()
}

impl SessionModel {
    fn running_trackers(&self) -> Vec<&SourceEntry> {
        self.sources.iter().filter(|s| s.descriptor.id.starts_with("pose:") && s.running()).collect()
    }

    /// Starts the guided calibration of the cameras that are tracking the body.
    pub fn start_camera_calibration(&mut self) {
        let trackers = self.running_trackers();
        if trackers.len() < 2 {
            self.status = "Calibration needs at least two cameras with Track body running".to_owned();
            return;
        }
        let mut specs = Vec::new();
        let mut info = Vec::new();
        for tracker in &trackers {
            let camera_id = EstimatorSource::camera_of(&tracker.descriptor.id).unwrap_or_default();
            let size = self
                .sources
                .iter()
                .find(|s| s.descriptor.id == camera_id)
                .and_then(|s| s.video.latest())
                .map(|f| (f.width, f.height));
            let Some((width, height)) = size else {
                self.status = format!("Camera {camera_id} has not delivered a frame yet");
                return;
            };
            specs.push(CameraSpec { id: camera_id.to_owned(), width, height });
            info.push((tracker.descriptor.id.clone(), width as f32 / height.max(1) as f32));
        }
        let flow = &mut self.calib;
        flow.calibrator = Some(Calibrator::new(specs));
        flow.averager.clear();
        flow.last_seq = vec![0; info.len()];
        flow.trackers = info;
        flow.guide = Guide::new();
        flow.collected = 0;
        flow.checks.clear();
        flow.rms_px = None;
        flow.phase = Phase::Guiding;
        self.status = "Calibrating: follow the instructions, all cameras should see you".to_owned();
    }

    /// Gives up on the current step and moves to the next.
    pub fn skip_calibration_step(&mut self) {
        self.calib.guide.skip();
    }

    /// Stops a calibration without a result.
    pub fn cancel_camera_calibration(&mut self) {
        self.calib.phase = Phase::Idle;
        self.calib.calibrator = None;
    }

    /// Forgets the solved cameras: keypoints are 2D again.
    pub fn clear_camera_calibration(&mut self) {
        if self.recording.is_none() {
            self.camera_rig = None;
            self.rig_epoch += 1;
            self.calib.phase = Phase::Idle;
            self.calib.checks.clear();
        }
    }

    /// Solves with what was collected (as an editor task).
    pub fn solve_camera_calibration(&mut self) {
        let Some(calibrator) = self.calib.calibrator.clone() else { return };
        if self.calib.phase != Phase::Guiding {
            return;
        }
        self.calib.phase = Phase::Solving;
        let (slot, scale) = (Arc::clone(&self.calib.slot), self.calib.body_scale);
        let description = TaskDescription::new("Solve camera calibration", "mocap.calibration", TaskDuration::Long);
        editor_task_queue::global().submit(description, move |ctx| {
            ctx.set_phase("Solving the camera poses");
            let result = calibrator.solve(scale).map(|rig| {
                let checks = calibrator.validate(&rig, scale);
                (rig, checks)
            });
            let failed = result.as_ref().err().map(CalibrationError::to_string);
            *slot.lock() = Some(result.map_err(|e| e.to_string()));
            failed.map_or(Ok(()), Err)
        });
    }

    /// One tick of the calibration: its solve result, then (while guiding) the
    /// newest synchronized keypoints through the guide into the calibrator.
    pub(super) fn calibration_tick(&mut self) -> bool {
        match self.calib.phase {
            Phase::Solving => self.finish_solve(),
            Phase::Guiding => self.collect_sample(),
            _ => false,
        }
    }

    fn finish_solve(&mut self) -> bool {
        let Some(result) = self.calib.slot.lock().take() else { return false };
        match result {
            Ok((rig, checks)) => {
                self.status = format!("Cameras calibrated: {:.1} px reprojection error", rig.rms_px);
                self.calib.rms_px = Some(rig.rms_px);
                self.calib.checks = checks;
                self.camera_rig = Some(rig);
                self.rig_epoch += 1;
                self.calib.phase = Phase::Done;
            }
            Err(reason) => {
                self.status = format!("Calibration failed: {reason}");
                self.calib.phase = Phase::Failed(reason);
            }
        }
        true
    }

    fn collect_sample(&mut self) -> bool {
        let Self { sources, calib, .. } = self;
        let mut frames = Vec::new();
        for (id, _) in &calib.trackers {
            let Some(entry) = sources.iter().find(|s| s.descriptor.id == *id && s.running()) else {
                calib.phase = Phase::Failed(format!("{id} stopped while calibrating"));
                return true;
            };
            frames.push(entry.bus.latest());
        }
        let Some(all): Option<Vec<Arc<PoseFrame>>> = frames.into_iter().collect() else { return false };
        let times: Vec<f64> = all.iter().map(|f| f.time_s).collect();
        let (lo, hi) = times.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), t| (lo.min(*t), hi.max(*t)));
        let fresh = all.iter().zip(&calib.last_seq).any(|(f, s)| f.sequence > *s);
        if hi - lo > SYNC_WINDOW_S || !fresh {
            return false;
        }
        calib.last_seq = all.iter().map(|f| f.sequence).collect();
        let views: Vec<Vec<[f32; 3]>> = all.iter().map(|f| keypoints_of(f)).collect();
        // Every camera contributes what it sees: one may see only the arms, another the legs.
        let seen: Vec<(&[[f32; 3]], f32)> = views.iter().zip(&calib.trackers).map(|(v, (_, aspect))| (v.as_slice(), *aspect)).collect();
        let features = Features::measure_all(&seen);
        let step = calib.guide.current();
        calib.guide.update(hi, features.as_ref());
        let mut solve = false;
        let finished = calib.guide.current().is_none();
        let refs: Vec<Option<&[[f32; 3]]>> = views.iter().map(|v| Some(v.as_slice())).collect();
        if let (true, Some(calibrator)) = (finished, calib.calibrator.as_mut()) {
            // The steps are done: keep taking whatever movement there is until there is enough variety
            // to solve from. A capture with too little of it is the commonest cause of a bad fit.
            calibrator.offer_tagged(&refs, u32::MAX);
            calib.collected = calibrator.samples();
            solve = calibrator.samples() >= SAMPLES_TARGET || calibrator.full();
        } else if let (Some(step), Some(calibrator)) = (step, calib.calibrator.as_mut()) {
            if !calib.guide.collecting() {
                calib.averager.clear();
            } else if matches!(step, Step::Neutral | Step::RaiseLeftHand | Step::RaiseRightHand | Step::ArmsOut) {
                // A held pose: the median of a window of frames, with no timing error between cameras.
                if let Some(average) = calib.averager.push(&refs) {
                    let averaged: Vec<Option<&[[f32; 3]]>> = average.iter().map(|v| v.as_deref()).collect();
                    calibrator.offer_tagged(&averaged, step.tag());
                }
            } else {
                calibrator.offer_tagged(&refs, step.tag());
            }
            calib.collected = calibrator.samples();
            solve = calibrator.full();
        }
        if solve {
            self.solve_camera_calibration();
        }
        true
    }

    /// A short line for the HUD describing the calibration.
    pub fn calibration_summary(&self) -> Option<String> {
        match &self.calib.phase {
            Phase::Idle | Phase::Done => self.camera_rig.as_ref().map(|r| format!("Cameras calibrated ({:.1} px)", r.rms_px)),
            Phase::Guiding => Some(format!("Calibrating: {} samples", self.calib.collected)),
            Phase::Solving => Some("Calibrating: solving...".to_owned()),
            Phase::Failed(reason) => Some(format!("Calibration failed: {reason}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SessionModel;
    use mocap_capture::{coco17_rig, CameraModel, CaptureError, CaptureSource, SourceDescriptor, SourceKind, SourceShared, SourceSink, SourceState, SourceStatus};
    use mocap_format::CameraCalibration;

    /// A tracker that is "running" without a thread: the test feeds its bus directly.
    struct FakeTracker {
        id: String,
        shared: Arc<mocap_capture::SourceShared>,
    }

    impl CaptureSource for FakeTracker {
        fn descriptor(&self) -> SourceDescriptor {
            SourceDescriptor { id: self.id.clone(), kind: SourceKind::Estimator, label: self.id.clone(), rig: Some(coco17_rig()), has_video: false }
        }

        fn start(&mut self, _sink: SourceSink) -> Result<(), CaptureError> {
            self.shared.set_state(SourceState::Live);
            Ok(())
        }

        fn stop(&mut self) {
            self.shared.set_state(SourceState::Idle);
        }

        fn status(&self) -> SourceStatus {
            self.shared.status()
        }
    }

    /// A camera 3 m from the origin at `angle`, looking at (0, 1, 0): row-major R and t.
    fn camera(id: &str, angle: f64) -> CameraCalibration {
        let position = [3.0 * angle.sin(), 1.4, 3.0 * angle.cos()];
        let target = [0.0, 1.0, 0.0];
        let norm = |v: [f64; 3]| {
            let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            [v[0] / l, v[1] / l, v[2] / l]
        };
        let cross = |a: [f64; 3], b: [f64; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
        let z = norm([target[0] - position[0], target[1] - position[1], target[2] - position[2]]);
        let x = norm(cross([0.0, -1.0, 0.0], z));
        let y = cross(z, x);
        let t = [-(x[0] * position[0] + x[1] * position[1] + x[2] * position[2]), -(y[0] * position[0] + y[1] * position[1] + y[2] * position[2]), -(z[0] * position[0] + z[1] * position[1] + z[2] * position[2])];
        CameraCalibration { id: id.to_owned(), width: 1280, height: 720, focal_px: 900.0, rotation: [x[0], x[1], x[2], y[0], y[1], y[2], z[0], z[1], z[2]], translation: t }
    }

    fn project(c: &CameraCalibration, p: [f64; 3]) -> [f32; 3] {
        let r = &c.rotation;
        let x = [0, 1, 2].map(|i| r[i * 3] * p[0] + r[i * 3 + 1] * p[1] + r[i * 3 + 2] * p[2] + c.translation[i]);
        [((640.0 + c.focal_px * x[0] / x[2]) / 1280.0) as f32, ((360.0 + c.focal_px * x[1] / x[2]) / 720.0) as f32, 0.9]
    }

    /// A standing figure's 17 joints (metres), enough to triangulate.
    fn joints() -> [[f64; 3]; 17] {
        let mut j = [[0.0; 3]; 17];
        let at = [
            (0, [0.0, 1.68, 0.05]),
            (5, [0.19, 1.45, 0.0]),
            (6, [-0.19, 1.45, 0.0]),
            (7, [0.25, 1.2, 0.05]),
            (8, [-0.25, 1.2, 0.05]),
            (9, [0.28, 0.96, 0.1]),
            (10, [-0.28, 0.96, 0.1]),
            (11, [0.1, 0.95, 0.0]),
            (12, [-0.1, 0.95, 0.0]),
            (13, [0.1, 0.53, 0.05]),
            (14, [-0.1, 0.53, 0.05]),
            (15, [0.1, 0.1, 0.0]),
            (16, [-0.1, 0.1, 0.0]),
        ];
        for (i, p) in at {
            j[i] = p;
        }
        j
    }

    fn model_with_trackers() -> (SessionModel, CameraRig, [[f64; 3]; 17]) {
        let mut model = SessionModel::new(std::path::PathBuf::from("does-not-exist.mocap"));
        let cameras = [camera("camera:0", 0.4), camera("camera:1", 2.0)];
        let rig = CameraRig { rms_px: 0.4, samples: 80, ..CameraRig::from_cameras(cameras.iter().map(CameraModel::from_calibration).collect()) };
        for c in &cameras {
            let id = format!("pose:{}", c.id);
            model.push_source(Box::new(FakeTracker { id, shared: SourceShared::new() }));
        }
        for i in 0..2 {
            model.toggle_source(i);
        }
        let truth = joints();
        for (entry, c) in model.sources.iter().zip(&cameras) {
            let mut values = Vec::new();
            for (k, p) in truth.iter().enumerate() {
                // Joints the figure lacks (the face) are simply not detected.
                values.extend(if p == &[0.0; 3] && k != 0 { [f32::NAN, f32::NAN, 0.0] } else { project(c, *p) });
            }
            entry.bus.push(PoseFrame { sequence: 1, time_s: 1.0, values });
        }
        (model, rig, truth)
    }

    #[test]
    fn calibrated_trackers_add_triangulated_world_keypoints_to_the_live_frame() {
        let (mut model, rig, truth) = model_with_trackers();
        model.camera_rig = Some(rig);
        model.rig_epoch += 1;
        assert!(model.pump(), "the mix is built and a frame arrives");
        let live = model.live.clone().expect("a live frame");
        // Two 17-keypoint camera groups, then the 17 triangulated ones, all in the rig.
        assert_eq!(model.rig.columns(), 51 * 2 + 51);
        assert_eq!(live.values.len(), model.rig.columns(), "frames are as wide as the rig they are recorded in");
        let nose = model.rig.channel_index("subject3d/kp3d.nose").expect("the triangulated channel exists");
        let at = model.rig.column_offsets()[nose];
        let p = &live.values[at..at + 3];
        let error = ((f64::from(p[0]) - truth[0][0]).powi(2) + (f64::from(p[1]) - truth[0][1]).powi(2) + (f64::from(p[2]) - truth[0][2]).powi(2)).sqrt();
        assert!(error < 0.03, "nose at {p:?}, truth {:?}: {error} m", truth[0]);
        let eye = model.rig.channel_index("subject3d/kp3d.left_eye").expect("channel");
        assert!(live.values[model.rig.column_offsets()[eye]].is_nan(), "a keypoint nobody saw stays a gap");
    }

    #[test]
    fn the_scene_uses_the_triangulated_character_and_places_calibrated_cameras() {
        let (mut model, rig, _) = model_with_trackers();
        model.camera_rig = Some(rig);
        model.rig_epoch += 1;
        model.pump();
        let scene = model.current_scene();
        let subject = scene.subject.expect("a subject");
        assert_eq!(subject.figure.source, mocap_capture::FigureSource::Triangulated);
        let hips = subject.figure.joints[11];
        assert!((hips[1] - 0.95).abs() < 0.05, "a real 3D hip height: {hips:?}");
    }

    #[test]
    fn a_calibrated_recording_stores_the_triangulated_channels_and_the_cameras() {
        let dir = tempfile::tempdir().expect("temp dir");
        let (mut model, rig, truth) = model_with_trackers();
        model.path = dir.path().join("take.mocap");
        model.camera_rig = Some(rig);
        model.rig_epoch += 1;
        model.pump();
        model.start_recording();
        assert!(model.is_recording(), "{}", model.status);
        let cameras = [camera("camera:0", 0.4), camera("camera:1", 2.0)];
        for n in 1..=30u64 {
            for (entry, c) in model.sources.iter().zip(&cameras) {
                let values: Vec<f32> = truth.iter().enumerate().flat_map(|(k, p)| if p == &[0.0; 3] && k != 0 { [f32::NAN, f32::NAN, 0.0] } else { project(c, *p) }).collect();
                entry.bus.push(PoseFrame { sequence: n + 1, time_s: 1.0 + n as f64 / 60.0, values });
            }
            model.pump();
        }
        model.stop_recording();
        let bytes = engine_fs::virtual_fs::read_file(&model.path).expect("the take was written");
        let (take, report) = mocap_format::read_take(&bytes).expect("a readable take");
        assert!(report.is_clean());
        assert!(take.frame_count() >= 20, "{} frames", take.frame_count());
        let cameras = &take.calibration.as_ref().expect("a calibration").cameras;
        assert_eq!(cameras.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), vec!["camera:0", "camera:1"]);
        assert!((cameras[0].focal_px - 900.0).abs() < 1e-6);
        let nose = take.rig.channel_index("subject3d/kp3d.nose").expect("the triangulated channel was recorded");
        let at = take.rig.column_offsets()[nose];
        let last = take.frame_count() - 1;
        assert!((f64::from(take.columns[at + 1][last]) - truth[0][1]).abs() < 0.03, "recorded nose height {}", take.columns[at + 1][last]);
    }

    #[test]
    fn without_a_calibration_the_keypoints_stay_two_dimensional() {
        let (mut model, _, _) = model_with_trackers();
        model.pump();
        assert_eq!(model.rig.columns(), 51 * 2, "no triangulated group before cameras are calibrated");
        let scene = model.current_scene();
        assert_eq!(scene.subject.expect("subject").figure.source, mocap_capture::FigureSource::Keypoints);
    }

    #[test]
    fn calibration_needs_two_tracking_cameras_and_says_so() {
        let mut model = SessionModel::new(std::path::PathBuf::from("does-not-exist.mocap"));
        model.start_camera_calibration();
        assert_eq!(model.calib.phase, Phase::Idle);
        assert!(model.status.contains("at least two cameras"), "{}", model.status);
    }
}
