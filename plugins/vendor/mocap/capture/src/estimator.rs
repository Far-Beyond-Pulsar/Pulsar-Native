//! The seam between video and joints.
//!
//! A camera source delivers video; turning video into a pose is a model's job
//! (body, hands, face), and none ships yet. An estimator plugin implements this
//! trait and the session feeds it the mailbox's newest frame.

use crate::frame::{PoseFrame, VideoFrame};
use mocap_format::RigProfile;

/// Estimates a pose from a camera frame.
pub trait PoseEstimator: Send {
    /// The rig its poses are in.
    fn rig(&self) -> RigProfile;
    /// A short name for the sources list.
    fn name(&self) -> &str;
    /// The pose in `frame`, or `None` when nobody is visible.
    fn estimate(&mut self, frame: &VideoFrame) -> Option<PoseFrame>;
}
