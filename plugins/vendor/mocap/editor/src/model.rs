//! The session model: everything the panels of one mocap editor share.
//!
//! GPUI-free state (it is held in an `Entity` by the editor and observed by the
//! panels). Capture threads are services owned by the sources; file work is an
//! editor task whose result lands in a slot this model polls from `pump`.

use editor_task_queue::{TaskDescription, TaskDuration};
use engine_fs::virtual_fs;
use mocap_capture::{
    apply_calibration, calibrate_neutral, vrm_humanoid, CaptureSource, FilterSettings, PoseBus, PoseFrame,
    Processor, RecordStats, Recorder, RigIndex, SessionClock, Skeleton, SourceDescriptor, SourceSink,
    SourceState, SyntheticSource, VideoMailbox, VmcSource,
};
use mocap_format::signal::{sample_linear, sample_quat};
use mocap_format::{read_take, Calibration, ChannelKind, ReadReport, RigProfile, SourceInfo, Take, TakeMeta};
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

/// Frame rate new takes are recorded at.
pub const RECORD_FPS: u32 = 60;
/// VMC's default port.
pub const DEFAULT_VMC_PORT: u16 = 39539;
/// A hips height the neutral pose scales to (a 1.75 m adult).
const REFERENCE_HIPS_HEIGHT: f32 = 0.95;

type OpenSlot = Arc<Mutex<Option<Result<(Take, ReadReport), String>>>>;

/// A camera the OS lists (filled when the `camera` feature is on).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CameraChoice {
    pub index: String,
    pub name: String,
}

/// One source in the sources list.
pub struct SourceEntry {
    pub descriptor: SourceDescriptor,
    source: Box<dyn CaptureSource>,
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
}

struct ActiveRecording {
    recorder: Recorder<engine_fs::FsWriter>,
    path: PathBuf,
}

/// The state of one editor window.
pub struct SessionModel {
    /// The file this editor was opened on.
    pub path: PathBuf,
    pub video: Arc<VideoMailbox>,
    pub sources: Vec<SourceEntry>,
    pub rig: RigProfile,
    pub skeleton: Skeleton,
    pub filter: FilterSettings,
    /// The newest live pose, filtered and calibrated.
    pub live: Option<Arc<PoseFrame>>,
    pub take: Option<Arc<Take>>,
    pub playhead: f64,
    pub playing: bool,
    pub calibration: Option<Calibration>,
    pub selected_channel: Option<usize>,
    pub mirror: bool,
    pub status: String,
    pub loading: bool,
    pub record_stats: RecordStats,
    pub cameras: Vec<CameraChoice>,
    pub cameras_loading: bool,
    filtered: Option<Arc<PoseFrame>>,
    video_seen: u64,
    bus: Arc<PoseBus>,
    clock: SessionClock,
    processor: Processor,
    recording: Option<ActiveRecording>,
    last_tick: Instant,
    open_slot: OpenSlot,
    camera_slot: Arc<Mutex<Option<Result<Vec<CameraChoice>, String>>>>,
}

/// What the "add source" menu can make.
#[derive(Clone, Copy, Debug)]
pub enum NewSource {
    Synthetic,
    Vmc(u16),
}

impl SessionModel {
    /// A session for `path`; a file holding a take (the `PMOC` magic) opens for review, any other starts a session.
    pub fn new(path: PathBuf) -> Self {
        let rig = vrm_humanoid();
        let filter = FilterSettings::default();
        let mut model = Self {
            path: path.clone(),
            video: VideoMailbox::new(),
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
            status: String::new(),
            loading: false,
            record_stats: RecordStats::default(),
            cameras: Vec::new(),
            cameras_loading: false,
            filtered: None,
            video_seen: 0,
            bus: PoseBus::new(1024),
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

    /// True while a pose source delivers.
    pub fn live_poses(&self) -> bool {
        self.sources.iter().any(|s| s.descriptor.rig.is_some() && s.running())
    }

    /// True while a camera delivers.
    pub fn live_video(&self) -> bool {
        self.sources.iter().any(|s| s.descriptor.has_video && s.running())
    }

    /// True while recording.
    pub fn is_recording(&self) -> bool {
        self.recording.is_some()
    }

    /// The file being written, while recording.
    pub fn recording_path(&self) -> Option<&Path> {
        self.recording.as_ref().map(|r| r.path.as_path())
    }

    /// Adds a source (stopped).
    pub fn add_source(&mut self, new: NewSource) {
        let source: Box<dyn CaptureSource> = match new {
            NewSource::Synthetic => Box::new(SyntheticSource::new(120.0)),
            NewSource::Vmc(port) => Box::new(VmcSource::new(port)),
        };
        self.push_source(source);
    }

    fn push_source(&mut self, source: Box<dyn CaptureSource>) {
        let descriptor = source.descriptor();
        if self.sources.iter().any(|s| s.descriptor.id == descriptor.id) {
            self.status = format!("{} is already in the list", descriptor.label);
            return;
        }
        self.sources.push(SourceEntry { descriptor, source });
    }

    /// Removes source `index`, stopping it first.
    pub fn remove_source(&mut self, index: usize) {
        if index < self.sources.len() {
            self.sources.remove(index);
        }
    }

    /// Starts or stops source `index`. Starting one stops the others of its
    /// kind (one pose source and one camera at a time).
    pub fn toggle_source(&mut self, index: usize) {
        let Some(entry) = self.sources.get(index) else { return };
        if entry.running() {
            self.sources[index].source.stop();
            return;
        }
        let (poses, video) = (entry.descriptor.rig.is_some(), entry.descriptor.has_video);
        for (i, other) in self.sources.iter_mut().enumerate() {
            let same_kind = (poses && other.descriptor.rig.is_some()) || (video && other.descriptor.has_video);
            if i != index && same_kind && other.running() {
                other.source.stop();
            }
        }
        let sink = SourceSink { poses: Arc::clone(&self.bus), video: Arc::clone(&self.video), clock: self.clock };
        if let Err(error) = self.sources[index].source.start(sink) {
            self.status = error.to_string();
        }
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

    /// One frame of work: results of tasks, the bus, playback. True when
    /// something the panels show changed.
    pub fn pump(&mut self) -> bool {
        let now = Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f64();
        self.last_tick = now;
        let mut changed = self.take_task_results();
        changed |= self.drain_bus();
        changed |= self.advance_playback(dt);
        let published = self.video.published();
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

    fn drain_bus(&mut self) -> bool {
        let mut frames = Vec::new();
        self.bus.drain(&mut frames);
        for raw in &frames {
            self.record(raw);
            let mut view = (**raw).clone();
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

    /// The file a new recording goes to: this editor's file when it is empty
    /// or new, else the first free `<name>_takeN.mocap` beside it.
    fn record_destination(&self) -> PathBuf {
        if !virtual_fs::is_file(&self.path) || self.take.is_none() {
            return self.path.clone();
        }
        let stem = self.path.file_stem().and_then(|s| s.to_str()).unwrap_or("take");
        (2..)
            .map(|n| self.path.with_file_name(format!("{stem}_take{n}.mocap")))
            .find(|p| !virtual_fs::is_file(p))
            .unwrap_or_else(|| self.path.clone())
    }

    /// Starts recording the live pose stream.
    pub fn start_recording(&mut self) {
        if self.recording.is_some() {
            return;
        }
        if !self.live_poses() {
            self.status = "Start a pose source (VMC or synthetic) to record".to_owned();
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
                self.status = "Recording".to_owned();
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

    /// Captures the performer's current pose as the neutral pose.
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

    /// The pose to show now: the live one while a pose source runs, else the
    /// take at the playhead.
    pub fn current_pose(&self) -> Option<Vec<f32>> {
        if self.live_poses() {
            return self.live.as_ref().map(|f| f.values.clone());
        }
        self.take.as_ref().map(|t| sample_take(t, self.playhead))
    }

    /// The index for column lookup by name.
    pub fn index(&self) -> RigIndex {
        RigIndex::new(&self.rig)
    }

    /// Pose frames dropped by the bus and the capture rate, for the HUD.
    pub fn bus_dropped(&self) -> u64 {
        self.bus.stats().dropped
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
