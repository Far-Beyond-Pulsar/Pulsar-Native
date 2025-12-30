/// Core audio types and structures for the DAW engine
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

/// Sample rate in Hz
pub const SAMPLE_RATE: f32 = 48000.0;

/// Buffer size for real-time audio processing
pub const BUFFER_SIZE: usize = 512;

/// Maximum number of tracks in a project
pub const MAX_TRACKS: usize = 128;

/// Maximum number of sends per track
pub const MAX_SENDS: usize = 8;

/// Audio sample buffer type
pub type AudioBuffer = Vec<f32>;

/// Stereo audio buffer (L, R)
pub type StereoBuffer = (AudioBuffer, AudioBuffer);

/// Unique identifier for tracks
pub type TrackId = uuid::Uuid;

/// Unique identifier for clips
pub type ClipId = uuid::Uuid;

/// Unique identifier for automation points
pub type AutomationId = uuid::Uuid;

/// Time position in samples
pub type SampleTime = u64;

/// Time position in beats
pub type BeatTime = f64;

/// Decibel value
pub type Decibels = f32;

/// Audio asset reference
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioAssetRef {
    pub path: PathBuf,
    pub sample_rate: f32,
    pub channels: usize,
    pub duration_samples: usize,
}

/// Cached audio data
#[derive(Debug, Clone)]
pub struct AudioAssetData {
    pub asset_ref: AudioAssetRef,
    pub samples: Arc<Vec<f32>>,
}

/// Track type enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrackType {
    Audio,
    Aux,
    Master,
}

/// Automation curve type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CurveType {
    Linear,
    Hold,
    Bezier,
}

/// Automation parameter type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AutomationParameter {
    Volume,
    Pan,
    Send(usize),
    EffectParam { effect_index: usize, param_index: usize },
}

/// Automation point
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomationPoint {
    pub id: AutomationId,
    pub time: SampleTime,
    pub value: f32,
    pub curve_type: CurveType,
    pub bezier_handle_in: Option<(f32, f32)>,
    pub bezier_handle_out: Option<(f32, f32)>,
}

/// Automation lane
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomationLane {
    pub parameter: AutomationParameter,
    pub points: Vec<AutomationPoint>,
    pub enabled: bool,
}

impl AutomationLane {
    pub fn new(parameter: AutomationParameter) -> Self {
        Self {
            parameter,
            points: Vec::new(),
            enabled: true,
        }
    }

    /// Get interpolated value at given time
    pub fn value_at(&self, time: SampleTime) -> Option<f32> {
        if !self.enabled || self.points.is_empty() {
            return None;
        }

        let idx = self.points.binary_search_by(|p| p.time.cmp(&time));

        match idx {
            Ok(i) => Some(self.points[i].value),
            Err(0) => Some(self.points[0].value),
            Err(i) if i >= self.points.len() => Some(self.points.last().unwrap().value),
            Err(i) => {
                let p1 = &self.points[i - 1];
                let p2 = &self.points[i];
                
                match p1.curve_type {
                    CurveType::Hold => Some(p1.value),
                    CurveType::Linear => {
                        let t = (time - p1.time) as f32 / (p2.time - p1.time) as f32;
                        Some(p1.value + (p2.value - p1.value) * t)
                    }
                    CurveType::Bezier => {
                        let t = (time - p1.time) as f32 / (p2.time - p1.time) as f32;
                        Some(Self::bezier_interpolate(p1.value, p2.value, t))
                    }
                }
            }
        }
    }

    fn bezier_interpolate(v1: f32, v2: f32, t: f32) -> f32 {
        let t2 = t * t;
        let t3 = t2 * t;
        let mt = 1.0 - t;
        let mt2 = mt * mt;
        let mt3 = mt2 * mt;
        
        mt3 * v1 + 3.0 * mt2 * t * v1 + 3.0 * mt * t2 * v2 + t3 * v2
    }

    pub fn add_point(&mut self, point: AutomationPoint) {
        let idx = self.points.binary_search_by(|p| p.time.cmp(&point.time));
        match idx {
            Ok(i) => self.points[i] = point,
            Err(i) => self.points.insert(i, point),
        }
    }

    pub fn remove_point(&mut self, id: AutomationId) {
        self.points.retain(|p| p.id != id);
    }
}

/// Audio clip on timeline
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioClip {
    pub id: ClipId,
    pub name: String,
    pub asset_path: PathBuf,
    pub start_time: SampleTime,
    pub duration: SampleTime,
    pub offset: SampleTime,
    pub fade_in: SampleTime,
    pub fade_out: SampleTime,
    pub gain: f32,
    pub muted: bool,
}

impl AudioClip {
    pub fn new(asset_path: PathBuf, start_time: SampleTime, duration: SampleTime) -> Self {
        Self {
            id: uuid::Uuid::new_v4(),
            name: asset_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Clip")
                .to_string(),
            asset_path,
            start_time,
            duration,
            offset: 0,
            fade_in: 0,
            fade_out: 0,
            gain: 1.0,
            muted: false,
        }
    }

    pub fn end_time(&self) -> SampleTime {
        self.start_time + self.duration
    }
    
    /// Convert start time from samples to beats
    pub fn start_beat(&self, tempo: f32) -> BeatTime {
        Self::samples_to_beats(self.start_time, tempo)
    }

    /// Set start time from beats
    pub fn set_start_beat(&mut self, beats: BeatTime, tempo: f32) {
        self.start_time = Self::beats_to_samples(beats, tempo);
    }
    
    /// Convert duration from samples to beats
    pub fn duration_beats(&self, tempo: f32) -> BeatTime {
        Self::samples_to_beats(self.duration, tempo)
    }
    
    /// Convert samples to beats given tempo
    fn samples_to_beats(samples: SampleTime, tempo: f32) -> BeatTime {
        let seconds = samples as f64 / SAMPLE_RATE as f64;
        let beats_per_second = tempo as f64 / 60.0;
        seconds * beats_per_second
    }

    /// Convert beats to samples given tempo
    fn beats_to_samples(beats: BeatTime, tempo: f32) -> SampleTime {
        let beats_per_second = tempo as f64 / 60.0;
        let seconds = beats / beats_per_second;
        (seconds * SAMPLE_RATE as f64) as SampleTime
    }

    /// Check if clip is active at given time
    pub fn is_active_at(&self, time: SampleTime) -> bool {
        !self.muted && time >= self.start_time && time < self.end_time()
    }

    /// Get fade coefficient at given time
    pub fn fade_at(&self, time: SampleTime) -> f32 {
        if !self.is_active_at(time) {
            return 0.0;
        }

        let relative_time = time - self.start_time;
        
        let fade_in_mult = if self.fade_in > 0 && relative_time < self.fade_in {
            relative_time as f32 / self.fade_in as f32
        } else {
            1.0
        };

        let time_from_end = self.duration.saturating_sub(relative_time);
        let fade_out_mult = if self.fade_out > 0 && time_from_end < self.fade_out {
            time_from_end as f32 / self.fade_out as f32
        } else {
            1.0
        };

        fade_in_mult * fade_out_mult * self.gain
    }
}

/// Send configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Send {
    pub target_track: Option<TrackId>,
    pub amount: f32,
    pub pre_fader: bool,
    pub enabled: bool,
}

impl Default for Send {
    fn default() -> Self {
        Self {
            target_track: None,
            amount: 0.0,
            pre_fader: false,
            enabled: false,
        }
    }
}

/// Track state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub id: TrackId,
    pub name: String,
    pub track_type: TrackType,
    pub clips: Vec<AudioClip>,
    pub volume: f32,
    pub pan: f32,
    pub muted: bool,
    pub solo: bool,
    pub record_armed: bool,
    pub sends: Vec<Send>,
    pub automation: Vec<AutomationLane>,
    pub color: [f32; 3],
}

impl Track {
    pub fn new(name: impl Into<String>, track_type: TrackType) -> Self {
        let mut sends = Vec::new();
        for _ in 0..MAX_SENDS {
            sends.push(Send::default());
        }

        Self {
            id: uuid::Uuid::new_v4(),
            name: name.into(),
            track_type,
            clips: Vec::new(),
            volume: 1.0,
            pan: 0.0,
            muted: false,
            solo: false,
            record_armed: false,
            sends,
            automation: Vec::new(),
            color: [0.5, 0.5, 0.5],
        }
    }

    pub fn volume_db(&self) -> Decibels {
        if self.volume <= 0.0 {
            -100.0
        } else {
            20.0 * self.volume.log10()
        }
    }

    pub fn set_volume_db(&mut self, db: Decibels) {
        self.volume = if db <= -100.0 {
            0.0
        } else {
            10.0_f32.powf(db / 20.0)
        };
    }

    pub fn get_automation_lane(&self, param: AutomationParameter) -> Option<&AutomationLane> {
        self.automation.iter().find(|lane| lane.parameter == param)
    }

    pub fn get_automation_lane_mut(&mut self, param: AutomationParameter) -> &mut AutomationLane {
        if let Some(idx) = self.automation.iter().position(|lane| lane.parameter == param) {
            &mut self.automation[idx]
        } else {
            self.automation.push(AutomationLane::new(param));
            self.automation.last_mut().unwrap()
        }
    }
}

/// Transport state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransportState {
    Stopped,
    Playing,
    Paused,
    Recording,
}

/// Transport configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transport {
    pub state: TransportState,
    pub position: SampleTime,
    pub loop_enabled: bool,
    pub loop_start: SampleTime,
    pub loop_end: SampleTime,
    pub tempo: f32,
    pub time_signature_numerator: u32,
    pub time_signature_denominator: u32,
    pub metronome_enabled: bool,
}

impl Default for Transport {
    fn default() -> Self {
        Self {
            state: TransportState::Stopped,
            position: 0,
            loop_enabled: false,
            loop_start: 0,
            loop_end: SAMPLE_RATE as u64 * 60,
            tempo: 120.0,
            time_signature_numerator: 4,
            time_signature_denominator: 4,
            metronome_enabled: false,
        }
    }
}

impl Transport {
    pub fn play(&mut self) {
        self.state = TransportState::Playing;
    }

    pub fn pause(&mut self) {
        if self.state == TransportState::Playing {
            self.state = TransportState::Paused;
        }
    }

    pub fn stop(&mut self) {
        self.state = TransportState::Stopped;
        self.position = 0;
    }

    pub fn seek(&mut self, position: SampleTime) {
        self.position = position;
        if self.loop_enabled {
            if self.position >= self.loop_end {
                self.position = self.loop_start;
            }
        }
    }

    pub fn advance(&mut self, samples: usize) {
        self.position += samples as u64;
        if self.loop_enabled && self.position >= self.loop_end {
            self.position = self.loop_start + (self.position - self.loop_end);
        }
    }

    pub fn samples_to_beats(&self, samples: SampleTime) -> BeatTime {
        let seconds = samples as f64 / SAMPLE_RATE as f64;
        let beats_per_second = self.tempo as f64 / 60.0;
        seconds * beats_per_second
    }

    pub fn beats_to_samples(&self, beats: BeatTime) -> SampleTime {
        let beats_per_second = self.tempo as f64 / 60.0;
        let seconds = beats / beats_per_second;
        (seconds * SAMPLE_RATE as f64) as SampleTime
    }
}

/// Metering data for visualizations
#[derive(Debug, Clone, Copy, Default)]
pub struct MeterData {
    pub peak_left: f32,
    pub peak_right: f32,
    pub rms_left: f32,
    pub rms_right: f32,
}

impl MeterData {
    pub fn from_buffer(left: &[f32], right: &[f32]) -> Self {
        let peak_left = left.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        let peak_right = right.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        
        let rms_left = (left.iter().map(|s| s * s).sum::<f32>() / left.len() as f32).sqrt();
        let rms_right = (right.iter().map(|s| s * s).sum::<f32>() / right.len() as f32).sqrt();

        Self {
            peak_left,
            peak_right,
            rms_left,
            rms_right,
        }
    }

    pub fn to_db(&self) -> (f32, f32) {
        let left_db = if self.peak_left > 0.0 {
            20.0 * self.peak_left.log10()
        } else {
            -100.0
        };
        
        let right_db = if self.peak_right > 0.0 {
            20.0 * self.peak_right.log10()
        } else {
            -100.0
        };

        (left_db, right_db)
    }
}

/// Audio processing performance metrics
#[derive(Debug, Clone, Copy, Default)]
pub struct PerformanceMetrics {
    pub callback_duration_us: u64,
    pub buffer_fill_time_us: u64,
    pub cpu_usage: f32,
    pub buffer_underruns: u64,
}
