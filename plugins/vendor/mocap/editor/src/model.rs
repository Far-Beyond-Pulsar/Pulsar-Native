//! The session model: everything the panels of one mocap editor share.
//!
//! Any number of sources of any kind run at once (cameras, VMC performers,
//! OSC sensors, synthetic figures). Each has its own pose bus and video feed;
//! the running pose and sensor sources are merged by a [`Mixer`] into one rig
//! and one stream for the live view and for recording. Capture threads are
//! services owned by the sources; file work is an editor task whose result
//! lands in a slot this model polls from `pump`.

use editor_task_queue::{TaskDescription, TaskDuration};
use engine_fs::virtual_fs;
use mocap_capture::{
    apply_calibration, calibrate_neutral, coco17_rig, CaptureSource, EstimatorFactory, EstimatorSource, PoseEstimator, FilterSettings, MixInput, Mixer, OscSensorSource, PoseBus,
    PoseFrame, Processor, RecordStats, Recorder, SessionClock, Skeleton, SourceDescriptor, SourceSink, SourceState,
    SyntheticSource, VideoMailbox, VmcSource,
};
use mocap_format::signal::{sample_linear, sample_quat};
use mocap_format::{read_take, Calibration, ChannelKind, ReadReport, RigProfile, SourceInfo, Take, TakeMeta};
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

/// Frame rate new takes are recorded at.
pub const RECORD_FPS: u32 = 60;
/// VMC's default port; further listeners take the next free ones.
pub const DEFAULT_VMC_PORT: u16 = 39539;
/// The first port offered to OSC sensors.
pub const DEFAULT_SENSOR_PORT: u16 = 9100;
/// A hips height the neutral pose scales to (a 1.75 m adult).
const REFERENCE_HIPS_HEIGHT: f32 = 0.95;

type OpenSlot = Arc<Mutex<Option<Result<(Take, ReadReport), String>>>>;

/// A camera the OS lists (filled when the `camera` feature is on).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CameraChoice {
    pub index: String,
    pub name: String,
}

/// One source in the sources list, with its own pose bus and video feed.
pub struct SourceEntry {
    pub descriptor: SourceDescriptor,
    source: Box<dyn CaptureSource>,
    bus: Arc<PoseBus>,
    video: Arc<VideoMailbox>,
}

impl SourceEntry {
    /// True while the source is starting or delivering.
    pub fn running(&self) -> bool {
        matches!(self.source.status().state, SourceState::Starting | SourceState::Live)
    }

    /// The source's status for the list.
    pub fn status(&self) -> mocap_capture::SourceStatus {
        self.source.status()
    }

    /// True for a source whose output is joints or sensor channels.
    pub fn has_poses(&self) -> bool {
        self.descriptor.rig.is_some()
    }
}

/// One camera's tracked keypoints, `(x, y, score)` in image fractions (gaps are NaN).
pub struct KeypointOverlay {
    pub camera_id: String,
    pub points: Vec<[f32; 3]>,
}

/// A joint hierarchy posed for drawing.
pub struct FigureView {
    pub joints: Vec<[f32; 3]>,
    pub bones: Vec<(usize, usize)>,
    pub names: Vec<String>,
}

struct ActiveRecording {
    recorder: Recorder<engine_fs::FsWriter>,
    path: PathBuf,
}

/// The mixer for the pose sources that were running when it was built.
struct MixState {
    mixer: Mixer,
    ids: Vec<String>,
}

/// What the "add source" menu can make.
#[derive(Clone, Copy, Debug)]
pub enum NewSource {
    /// A synthetic figure (a new one each time).
    Synthetic,
    /// A VMC listener on the next free port.
    Vmc,
    /// An IMU-style OSC sensor on the next free port.
    Sensor,
}

/// The state of one editor window.
pub struct SessionModel {
    /// The file this editor was opened on.
    pub path: PathBuf,
    pub sources: Vec<SourceEntry>,
    /// The merged rig of the running pose and sensor sources.
    pub rig: RigProfile,
    pub skeleton: Skeleton,
    pub filter: FilterSettings,
    /// The newest merged pose, filtered and calibrated.
    pub live: Option<Arc<PoseFrame>>,
    pub take: Option<Arc<Take>>,
    pub playhead: f64,
    pub playing: bool,
    pub calibration: Option<Calibration>,
    pub selected_channel: Option<usize>,
    pub mirror: bool,
    pub show_overlay: bool,
    pub status: String,
    pub loading: bool,
    pub record_stats: RecordStats,
    pub cameras: Vec<CameraChoice>,
    pub cameras_loading: bool,
    pose_model: Option<PathBuf>,
    /// The model path trackers read each time they start, so choosing a model reaches trackers already listed.
    pose_path: Arc<Mutex<PathBuf>>,
    take_skeleton: Option<Skeleton>,
    mix: Option<MixState>,
    filtered: Option<Arc<PoseFrame>>,
    video_seen: u64,
    synthetic_count: u32,
    clock: SessionClock,
    processor: Processor,
    recording: Option<ActiveRecording>,
    last_tick: Instant,
    open_slot: OpenSlot,
    camera_slot: Arc<Mutex<Option<Result<Vec<CameraChoice>, String>>>>,
}

impl SessionModel {
    /// A session for `path`; a file holding a take (the `PMOC` magic) opens for
    /// review, any other starts a session.
    pub fn new(path: PathBuf) -> Self {
        let rig = RigProfile::default();
        let filter = FilterSettings::default();
        let mut model = Self {
            path: path.clone(),
            sources: Vec::new(),
            skeleton: Skeleton::new(&rig),
            processor: Processor::new(&rig, filter),
            rig,
            filter,
            live: None,
            take: None,
            playhead: 0.0,
            playing: false,
            calibration: None,
            selected_channel: None,
            mirror: true,
            show_overlay: true,
            status: String::new(),
            loading: false,
            record_stats: RecordStats::default(),
            cameras: Vec::new(),
            cameras_loading: false,
            pose_model: None,
            pose_path: Arc::new(Mutex::new(PathBuf::new())),
            take_skeleton: None,
            mix: None,
            filtered: None,
            video_seen: 0,
            synthetic_count: 0,
            clock: SessionClock::start(),
            recording: None,
            last_tick: Instant::now(),
            open_slot: Arc::default(),
            camera_slot: Arc::default(),
        };
        if virtual_fs::read_file_prefix(&path, 4).is_ok_and(|head| head == mocap_format::MAGIC) {
            model.begin_open(path);
        }
        model
    }

    /// Frames still need to be driven (a live source, a load, playback, a take being written).
    pub fn needs_frames(&self) -> bool {
        self.loading
            || self.cameras_loading
            || self.playing
            || self.recording.is_some()
            || self.sources.iter().any(SourceEntry::running)
    }

    /// True while any pose or sensor source delivers.
    pub fn live_poses(&self) -> bool {
        self.sources.iter().any(|s| s.has_poses() && s.running())
    }

    /// True while any camera delivers.
    pub fn live_video(&self) -> bool {
        self.sources.iter().any(|s| s.descriptor.has_video && s.running())
    }

    /// The running cameras' feeds, with their source ids, in list order.
    pub fn video_feeds(&self) -> Vec<(String, Arc<VideoMailbox>)> {
        self.sources
            .iter()
            .filter(|s| s.descriptor.has_video && s.running())
            .map(|s| (s.descriptor.id.clone(), Arc::clone(&s.video)))
            .collect()
    }

    /// True while recording.
    pub fn is_recording(&self) -> bool {
        self.recording.is_some()
    }

    /// Why Record would do nothing right now, for the button's tooltip and the HUD.
    pub fn record_blocker(&self) -> Option<&'static str> {
        (self.recording.is_none() && !self.live_poses()).then_some(
            "Nothing to record: press Track body on a camera, or start a pose or sensor source (Synthetic, VMC, Sensor).",
        )
    }

    /// The file being written, while recording.
    pub fn recording_path(&self) -> Option<&Path> {
        self.recording.as_ref().map(|r| r.path.as_path())
    }

    /// Adds a source (stopped).
    pub fn add_source(&mut self, new: NewSource) {
        let source: Box<dyn CaptureSource> = match new {
            NewSource::Synthetic => {
                self.synthetic_count += 1;
                Box::new(SyntheticSource::numbered(120.0, self.synthetic_count))
            }
            NewSource::Vmc => Box::new(VmcSource::new(self.free_port(DEFAULT_VMC_PORT, "vmc"))),
            NewSource::Sensor => Box::new(OscSensorSource::imu(self.free_port(DEFAULT_SENSOR_PORT, "sensor"))),
        };
        self.push_source(source);
    }

    fn free_port(&self, first: u16, kind: &str) -> u16 {
        (first..first.saturating_add(100))
            .find(|port| !self.sources.iter().any(|s| s.descriptor.id == format!("{kind}:{port}")))
            .unwrap_or(first)
    }

    fn push_source(&mut self, source: Box<dyn CaptureSource>) {
        let descriptor = source.descriptor();
        if self.sources.iter().any(|s| s.descriptor.id == descriptor.id) {
            self.status = format!("{} is already in the list", descriptor.label);
            return;
        }
        self.sources.push(SourceEntry { descriptor, source, bus: PoseBus::new(1024), video: VideoMailbox::new() });
    }

    /// Where the pose model is looked for: the one chosen, else
    /// `PULSAR_MOCAP_POSE_MODEL`, else `.pulsar/models/pose.onnx` in the project
    /// (the nearest folder above the session file with a `Pulsar.toml`).
    pub fn pose_model_path(&self) -> PathBuf {
        if let Some(path) = &self.pose_model {
            return path.clone();
        }
        if let Ok(path) = std::env::var("PULSAR_MOCAP_POSE_MODEL") {
            return PathBuf::from(path);
        }
        let root = self
            .path
            .ancestors()
            .skip(1)
            .find(|dir| virtual_fs::is_file(&dir.join("Pulsar.toml")))
            .or_else(|| self.path.parent())
            .unwrap_or(Path::new("."));
        root.join(".pulsar").join("models").join("pose.onnx")
    }

    /// The cell trackers read their model path from, kept current with `pose_model_path`.
    fn shared_pose_path(&self) -> Arc<Mutex<PathBuf>> {
        *self.pose_path.lock() = self.pose_model_path();
        Arc::clone(&self.pose_path)
    }

    /// Uses the ONNX keypoint model at `path` for body tracking.
    pub fn set_pose_model(&mut self, path: PathBuf) {
        self.status = format!("Pose model: {}", path.display());
        *self.pose_path.lock() = path.clone();
        self.pose_model = Some(path);
    }

    /// Adds body tracking for camera `index` (a source that runs the pose model
    /// on that camera's frames) and starts it.
    pub fn add_body_tracking(&mut self, index: usize) {
        let Some(camera) = self.sources.get(index).filter(|s| s.descriptor.has_video) else { return };
        let id = format!("pose:{}", camera.descriptor.id);
        if let Some(existing) = self.sources.iter().position(|s| s.descriptor.id == id) {
            if !self.sources[existing].running() {
                self.toggle_source(existing);
            }
            return;
        }
        let source = EstimatorSource::new(
            &camera.descriptor.id,
            &camera.descriptor.label,
            Arc::clone(&camera.video),
            coco17_rig(),
            pose_factory(self.shared_pose_path()),
        );
        self.push_source(Box::new(source));
        let added = self.sources.len() - 1;
        self.toggle_source(added);
    }

    /// The running trackers' keypoints, one entry per camera.
    pub fn keypoint_overlays(&self) -> Vec<KeypointOverlay> {
        self.sources
            .iter()
            .filter(|s| s.running())
            .filter_map(|s| {
                let camera_id = EstimatorSource::camera_of(&s.descriptor.id)?;
                let pose = s.bus.latest()?;
                Some(KeypointOverlay {
                    camera_id: camera_id.to_owned(),
                    points: pose.values.chunks_exact(3).map(|p| [p[0], p[1], p[2]]).collect(),
                })
            })
            .collect()
    }

    /// Removes source `index`, stopping it first.
    pub fn remove_source(&mut self, index: usize) {
        if index >= self.sources.len() {
            return;
        }
        if self.recording.is_some() && self.sources[index].has_poses() && self.sources[index].running() {
            self.status = "Stop recording before removing a source that is recording".to_owned();
            return;
        }
        self.sources.remove(index);
    }

    /// Starts or stops source `index`. Sources are independent: any number and
    /// mix can run, and pose and sensor sources are merged into one take.
    pub fn toggle_source(&mut self, index: usize) {
        let Some(entry) = self.sources.get(index) else { return };
        if self.recording.is_some() && entry.has_poses() {
            self.status = "The recording's sources are fixed: stop recording to change them".to_owned();
            return;
        }
        if entry.running() {
            self.sources[index].source.stop();
            return;
        }
        let entry = &mut self.sources[index];
        let sink = SourceSink { poses: Arc::clone(&entry.bus), video: Arc::clone(&entry.video), clock: self.clock };
        if let Err(error) = entry.source.start(sink) {
            self.status = error.to_string();
        }
    }

    /// Rebuilds the mixer when the set of running pose and sensor sources
    /// changed (never while recording: a take's rig is fixed).
    fn sync_mix(&mut self) -> bool {
        if self.recording.is_some() {
            return false;
        }
        let running: Vec<&SourceEntry> = self.sources.iter().filter(|s| s.has_poses() && s.running()).collect();
        let ids: Vec<String> = running.iter().map(|s| s.descriptor.id.clone()).collect();
        if self.mix.as_ref().map(|m| &m.ids) == Some(&ids) || (self.mix.is_none() && ids.is_empty()) {
            return false;
        }
        let inputs: Vec<MixInput> = running
            .iter()
            .filter_map(|s| {
                Some(MixInput { id: s.descriptor.id.clone(), rig: s.descriptor.rig.clone()?, bus: Arc::clone(&s.bus) })
            })
            .collect();
        if inputs.is_empty() {
            self.mix = None;
            self.rig = RigProfile::default();
        } else {
            let (mixer, rig) = Mixer::new(inputs);
            self.mix = Some(MixState { mixer, ids });
            self.rig = rig;
        }
        self.skeleton = Skeleton::new(&self.rig);
        self.processor = Processor::new(&self.rig, self.filter);
        self.live = None;
        self.filtered = None;
        true
    }

    /// Asks the OS for its cameras as an editor task.
    #[cfg(feature = "camera")]
    pub fn refresh_cameras(&mut self) {
        if self.cameras_loading {
            return;
        }
        self.cameras_loading = true;
        let slot = Arc::clone(&self.camera_slot);
        let description = TaskDescription::new("List cameras", "mocap.capture", TaskDuration::Short);
        editor_task_queue::global().submit(description, move |ctx| {
            ctx.set_phase("Enumerating cameras");
            let result = mocap_capture::enumerate_cameras()
                .map(|list| list.into_iter().map(|c| CameraChoice { index: c.index, name: c.name }).collect())
                .map_err(|e| e.to_string());
            *slot.lock() = Some(result);
            Ok(())
        });
    }

    /// Without the `camera` feature there is nothing to list.
    #[cfg(not(feature = "camera"))]
    pub fn refresh_cameras(&mut self) {
        self.status = "This build has no camera support (the `camera` feature)".to_owned();
    }

    /// Adds the camera `choice` as a source.
    #[cfg(feature = "camera")]
    pub fn add_camera(&mut self, choice: &CameraChoice) {
        let info = mocap_capture::CameraInfo { index: choice.index.clone(), name: choice.name.clone() };
        self.push_source(Box::new(mocap_capture::CameraSource::new(info, mocap_capture::CameraMode::default())));
    }

    /// Without the `camera` feature a camera cannot be added.
    #[cfg(not(feature = "camera"))]
    pub fn add_camera(&mut self, _choice: &CameraChoice) {}

    /// Opens `path` as the take under review (an editor task; the result lands in `pump`).
    pub fn open_take(&mut self, path: PathBuf) {
        self.playing = false;
        self.begin_open(path);
    }

    fn begin_open(&mut self, path: PathBuf) {
        self.loading = true;
        let slot = Arc::clone(&self.open_slot);
        let description = TaskDescription::new(
            format!("Open {}", path.file_name().and_then(|n| n.to_str()).unwrap_or("take")),
            "mocap.file",
            TaskDuration::Short,
        );
        editor_task_queue::global().submit(description, move |ctx| {
            ctx.set_phase("Reading the take");
            let result = virtual_fs::read_file(&path)
                .map_err(|e| e.to_string())
                .and_then(|bytes| read_take(&bytes).map_err(|e| e.to_string()));
            let failed = result.as_ref().err().cloned();
            *slot.lock() = Some(result);
            failed.map_or(Ok(()), Err)
        });
    }

    /// One frame of work: results of tasks, the mixer, playback. True when
    /// something the panels show changed.
    pub fn pump(&mut self) -> bool {
        let now = Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f64();
        self.last_tick = now;
        let mut changed = self.take_task_results();
        changed |= self.sync_mix();
        changed |= self.drain_mix();
        changed |= self.advance_playback(dt);
        let published: u64 = self.sources.iter().map(|s| s.video.published()).sum();
        changed |= published != self.video_seen;
        self.video_seen = published;
        changed
    }

    fn take_task_results(&mut self) -> bool {
        let mut changed = false;
        if let Some(result) = self.open_slot.lock().take() {
            self.loading = false;
            changed = true;
            match result {
                Ok((take, report)) => {
                    self.status = if report.is_clean() {
                        format!("{} frames", take.frame_count())
                    } else {
                        format!("Opened with damage: {} bad block(s), truncated: {}", report.bad_ranges.len(), report.truncated)
                    };
                    self.calibration = take.calibration.clone();
                    self.playhead = 0.0;
                    self.take_skeleton = Some(Skeleton::new(&take.rig));
                    self.selected_channel = None;
                    self.take = Some(Arc::new(take));
                }
                Err(message) => self.status = message,
            }
        }
        if let Some(result) = self.camera_slot.lock().take() {
            self.cameras_loading = false;
            changed = true;
            match result {
                Ok(list) => self.cameras = list,
                Err(message) => self.status = message,
            }
        }
        changed
    }

    fn drain_mix(&mut self) -> bool {
        let mut frames = Vec::new();
        if let Some(mix) = &mut self.mix {
            mix.mixer.pump(&mut frames);
        }
        for raw in &frames {
            self.record(raw);
            let mut view = raw.clone();
            self.processor.process(&mut view);
            self.filtered = Some(Arc::new(view.clone()));
            if let Some(calibration) = &self.calibration {
                apply_calibration(&self.rig, calibration, &mut view);
            }
            self.live = Some(Arc::new(view));
        }
        if let Some(recording) = &self.recording {
            self.record_stats = recording.recorder.stats();
        }
        !frames.is_empty()
    }

    fn record(&mut self, raw: &PoseFrame) {
        let Some(recording) = &mut self.recording else { return };
        if let Err(error) = recording.recorder.push(raw) {
            self.status = format!("Recording stopped: {error}");
            self.recording = None;
        }
    }

    fn advance_playback(&mut self, dt: f64) -> bool {
        let Some(take) = &self.take else { return false };
        if !self.playing {
            return false;
        }
        let frames = take.frame_count().max(1) as f64;
        self.playhead = (self.playhead + dt * take.meta.fps()) % frames;
        true
    }

    /// Moves the playhead to `frame` (clamped to the take).
    pub fn seek(&mut self, frame: f64) {
        let last = self.take.as_ref().map_or(0.0, |t| (t.frame_count().saturating_sub(1)) as f64);
        self.playhead = frame.clamp(0.0, last);
    }

    /// Play or pause the take.
    pub fn toggle_play(&mut self) {
        self.playing = self.take.is_some() && !self.playing;
        self.last_tick = Instant::now();
    }

    /// The file a new recording goes to: this editor's file when it does not
    /// hold a take yet, else the first free `<name>_takeN.mocap` beside it.
    fn record_destination(&self) -> PathBuf {
        if self.take.is_none() {
            return self.path.clone();
        }
        let stem = self.path.file_stem().and_then(|s| s.to_str()).unwrap_or("take");
        (2..)
            .map(|n| self.path.with_file_name(format!("{stem}_take{n}.mocap")))
            .find(|p| !virtual_fs::is_file(p))
            .unwrap_or_else(|| self.path.clone())
    }

    /// Starts recording the merged stream of every running pose and sensor source.
    pub fn start_recording(&mut self) {
        if self.recording.is_some() {
            return;
        }
        self.sync_mix();
        if self.mix.is_none() {
            self.status = self.record_blocker().unwrap_or("Nothing to record").to_owned();
            return;
        }
        let path = self.record_destination();
        let writer = match virtual_fs::open_write(&path) {
            Ok(writer) => writer,
            Err(error) => {
                self.status = format!("Could not create {}: {error}", path.display());
                return;
            }
        };
        let meta = self.new_take_meta(&path);
        match Recorder::new(writer, &meta, &self.rig, mocap_format::DEFAULT_BLOCK_FRAMES) {
            Ok(mut recorder) => {
                if let Some(calibration) = &self.calibration {
                    if let Err(error) = recorder.set_calibration(calibration) {
                        self.status = error.to_string();
                    }
                }
                self.recording = Some(ActiveRecording { recorder, path });
                self.record_stats = RecordStats::default();
                self.status = format!("Recording {} source(s)", self.mix.as_ref().map_or(0, |m| m.ids.len()));
            }
            Err(error) => self.status = error.to_string(),
        }
    }

    fn new_take_meta(&self, path: &Path) -> TakeMeta {
        let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("take");
        let mut meta = TakeMeta::new(name, RECORD_FPS, 1);
        let created = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        meta.created_unix = created;
        meta.id = format!("{created:x}-{:x}", self.clock.now_s().to_bits() as u32);
        meta.sources = self
            .sources
            .iter()
            .filter(|s| s.running())
            .map(|s| SourceInfo {
                id: s.descriptor.id.clone(),
                kind: s.descriptor.kind.as_str().to_owned(),
                label: s.descriptor.label.clone(),
            })
            .collect();
        meta
    }

    /// Stops recording, finalises the file and opens it for review.
    pub fn stop_recording(&mut self) {
        let Some(recording) = self.recording.take() else { return };
        let path = recording.path;
        let result = recording
            .recorder
            .finish()
            .map_err(|e| e.to_string())
            .and_then(|writer| writer.finish().map_err(|e| e.to_string()));
        match result {
            Ok(()) => self.begin_open(path),
            Err(error) => self.status = format!("Could not finish the take: {error}"),
        }
    }

    /// Marks the frame being recorded.
    pub fn mark(&mut self) {
        if let Some(recording) = &mut self.recording {
            let label = format!("Marker {}", recording.recorder.stats().frames);
            if let Err(error) = recording.recorder.mark(&label) {
                self.status = error.to_string();
            }
        }
    }

    /// Captures the performers' current pose as the neutral pose.
    pub fn calibrate(&mut self) {
        // From the filtered pose before any calibration, so calibrating twice
        // does not compound.
        let Some(pose) = self.filtered.clone() else {
            self.status = "No live pose to calibrate from".to_owned();
            return;
        };
        self.calibration = Some(calibrate_neutral(&self.rig, &pose, REFERENCE_HIPS_HEIGHT));
        self.status = "Calibrated to the current pose".to_owned();
    }

    /// Clears the calibration.
    pub fn clear_calibration(&mut self) {
        self.calibration = None;
    }

    /// Changes the live filter.
    pub fn set_filter(&mut self, filter: FilterSettings) {
        self.filter = filter;
        self.processor.set_settings(&self.rig, filter);
    }

    /// The figure(s) to draw now: the live merged pose while any pose source
    /// runs, else the take at the playhead.
    pub fn current_figure(&self) -> Option<FigureView> {
        let (values, skeleton, rig) = if self.live_poses() {
            (self.live.as_ref()?.values.clone(), &self.skeleton, &self.rig)
        } else {
            let take = self.take.as_ref()?;
            (sample_take(take, self.playhead), self.take_skeleton.as_ref()?, &take.rig)
        };
        if skeleton.is_empty() {
            return None;
        }
        Some(FigureView {
            joints: skeleton.joints(&values),
            bones: skeleton.bones(),
            names: rig.joints.iter().map(|j| j.name.clone()).collect(),
        })
    }

    /// Pose frames dropped across every source's bus.
    pub fn bus_dropped(&self) -> u64 {
        self.sources.iter().map(|s| s.bus.stats().dropped).sum()
    }
}

/// The take's pose at fractional frame `frame`.
pub fn sample_take(take: &Take, frame: f64) -> Vec<f32> {
    let mut out = vec![f32::NAN; take.columns.len()];
    for (channel, at) in take.rig.channels.iter().zip(take.rig.column_offsets()) {
        if channel.kind == ChannelKind::Rotation4 {
            let q = sample_quat(
                [&take.columns[at], &take.columns[at + 1], &take.columns[at + 2], &take.columns[at + 3]],
                frame,
            );
            out[at..at + 4].copy_from_slice(&q);
        } else {
            for c in at..at + channel.kind.width() {
                out[c] = sample_linear(&take.columns[c], frame);
            }
        }
    }
    out
}

/// How a tracker loads its model: on its own thread, so a slow load never
/// blocks the UI, and with a message that says what to do when it cannot.
#[cfg(feature = "pose")]
fn pose_factory(path: Arc<Mutex<PathBuf>>) -> EstimatorFactory {
    Arc::new(move || {
        let path = path.lock().clone();
        // A model on disk (chosen, or in the project) wins over the built-in one.
        let estimator = if virtual_fs::is_file(&path) {
            mocap_capture::OnnxPoseEstimator::load(&path)
        } else {
            mocap_capture::OnnxPoseEstimator::bundled().unwrap_or_else(|| {
                Err(format!(
                    "No pose model: none is built in and none is at {}. Use Pose model to choose a single-person keypoint ONNX (MoveNet-style, 17 keypoints).",
                    path.display()
                ))
            })
        };
        estimator.map(|e| Box::new(e) as Box<dyn PoseEstimator>)
    })
}

/// Without the `pose` feature a tracker only reports that.
#[cfg(not(feature = "pose"))]
fn pose_factory(_path: Arc<Mutex<PathBuf>>) -> EstimatorFactory {
    Arc::new(|| Err("This build has no body tracking (the `pose` feature).".to_owned()))
}
