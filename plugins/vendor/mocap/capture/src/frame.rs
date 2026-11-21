//! What sources produce: video frames and pose frames.

use std::sync::Arc;

/// The pixel layouts a camera frame arrives in after decoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    Rgba8,
    Bgra8,
}

/// One decoded camera frame. The bytes are shared: the viewport, an estimator
/// and a video recorder all read the same allocation.
#[derive(Clone, Debug)]
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    /// `width * height * 4` bytes, tightly packed rows.
    pub data: Arc<[u8]>,
    /// Counts up per frame from the source; gaps mean dropped frames.
    pub sequence: u64,
    /// Seconds since the session clock started, when the frame was captured.
    pub time_s: f64,
}

impl VideoFrame {
    /// True when `data` is exactly the size `width`, `height` and `format` say.
    pub fn is_well_formed(&self) -> bool {
        self.data.len() == self.width as usize * self.height as usize * 4
    }
}

/// One pose sample: a value for every column of the source's rig, `NaN` where
/// the source has no measurement.
#[derive(Clone, Debug, PartialEq)]
pub struct PoseFrame {
    pub sequence: u64,
    /// Seconds since the session clock started.
    pub time_s: f64,
    /// One value per rig column, in the rig's column order.
    pub values: Vec<f32>,
}
