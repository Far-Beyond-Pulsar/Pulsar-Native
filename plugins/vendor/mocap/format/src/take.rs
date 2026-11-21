//! The take: metadata, rig, calibration, markers and the sample columns.

use crate::rig::RigProfile;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What produced the samples (one entry per source used in the take).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceInfo {
    pub id: String,
    pub kind: String,
    pub label: String,
}

/// The take's identity and capture settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TakeMeta {
    pub id: String,
    pub name: String,
    pub fps_num: u32,
    pub fps_den: u32,
    pub start_timecode: Option<String>,
    pub created_unix: u64,
    pub performer: String,
    pub notes: String,
    pub sources: Vec<SourceInfo>,
}

impl TakeMeta {
    /// A take called `name` at `fps_num / fps_den` frames per second.
    pub fn new(name: &str, fps_num: u32, fps_den: u32) -> Self {
        Self {
            id: String::new(),
            name: name.to_owned(),
            fps_num: fps_num.max(1),
            fps_den: fps_den.max(1),
            start_timecode: None,
            created_unix: 0,
            performer: String::new(),
            notes: String::new(),
            sources: Vec::new(),
        }
    }

    /// Frames per second as a float.
    pub fn fps(&self) -> f64 {
        f64::from(self.fps_num) / f64::from(self.fps_den.max(1))
    }
}

/// A named moment in the take (a slate, a clap, a user mark).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marker {
    pub frame: u64,
    pub label: String,
}

/// One calibrated camera: `x_cam = rotation * X_world + translation` (row-major
/// rotation), a pinhole with its principal point at the image centre.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CameraCalibration {
    pub id: String,
    pub width: u32,
    pub height: u32,
    pub focal_px: f64,
    pub rotation: [f64; 9],
    pub translation: [f64; 3],
}

/// What the performer's neutral pose and scale measured: applied on read,
/// never baked into the raw samples.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Calibration {
    /// Performer height over the rig's, applied to root translation.
    pub scale: f32,
    /// Per-channel rotation offsets (x, y, z, w) taken from the T-pose.
    pub offsets: BTreeMap<String, [f32; 4]>,
    /// The cameras the take's keypoints were seen by, in the subject's world
    /// (+Y up, floor at 0). Empty when the take had no camera calibration.
    #[serde(default)]
    pub cameras: Vec<CameraCalibration>,
}

/// A whole take in memory: one `Vec<f32>` per column, one sample per frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Take {
    pub meta: TakeMeta,
    pub rig: RigProfile,
    pub calibration: Option<Calibration>,
    pub markers: Vec<Marker>,
    pub columns: Vec<Vec<f32>>,
}

impl Take {
    /// An empty take for `rig`.
    pub fn new(meta: TakeMeta, rig: RigProfile) -> Self {
        let columns = vec![Vec::new(); rig.columns()];
        Self { meta, rig, calibration: None, markers: Vec::new(), columns }
    }

    /// Number of frames.
    pub fn frame_count(&self) -> usize {
        self.columns.first().map_or(0, Vec::len)
    }

    /// Duration in seconds.
    pub fn duration(&self) -> f64 {
        self.frame_count() as f64 / self.meta.fps()
    }

    /// Appends one frame (one sample per column).
    pub fn push_frame(&mut self, frame: &[f32]) {
        for (column, value) in self.columns.iter_mut().zip(frame) {
            column.push(*value);
        }
    }

    /// The samples of frame `frame`, one per column; `None` past the end.
    pub fn frame(&self, frame: usize) -> Option<Vec<f32>> {
        (frame < self.frame_count()).then(|| self.columns.iter().map(|c| c[frame]).collect())
    }
}
