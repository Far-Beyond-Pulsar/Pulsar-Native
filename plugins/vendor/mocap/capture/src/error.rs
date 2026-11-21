//! Capture errors.

/// Everything starting or running a source can refuse.
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    /// The source is already running.
    #[error("the source is already running")]
    AlreadyRunning,
    /// A socket or device could not be opened.
    #[error("could not open {what}: {reason}")]
    Open { what: String, reason: String },
    /// The capture thread could not start.
    #[error("could not start the capture thread: {0}")]
    Thread(String),
    /// The recording destination refused a frame.
    #[error("recording failed: {0}")]
    Record(#[from] mocap_format::FormatError),
}
