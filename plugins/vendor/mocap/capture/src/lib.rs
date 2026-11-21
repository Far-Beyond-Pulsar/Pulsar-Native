//! Mocap capture (`pulsar.mocap`): sources, the frame bus, live processing and
//! the recorder. GPUI-free; every capture thread is a registered editor
//! service. Design: `.agents/MOCAP.md`.

mod bus;
#[cfg(feature = "camera")]
mod camera;
mod error;
mod estimator;
pub mod osc;
mod processor;
mod recorder;
mod rigs;
mod skeleton;
mod source;
mod synthetic;
mod vmc;

pub use bus::{BusStats, PoseBus, VideoMailbox};
#[cfg(feature = "camera")]
pub use camera::{enumerate_cameras, CameraInfo, CameraMode, CameraSource};
pub use error::CaptureError;
pub use estimator::PoseEstimator;
pub use frame::{PixelFormat, PoseFrame, VideoFrame};
pub use processor::{apply_calibration, calibrate_neutral, FilterSettings, Processor};
pub use recorder::{RecordStats, Recorder, MAX_HOLD_S};
pub use rigs::{vrm_humanoid, RigIndex, VRM_BLENDSHAPES};
pub use skeleton::{rest_offset, Skeleton};
pub use source::{
    spawn_capture_loop, CaptureSource, SessionClock, SourceDescriptor, SourceKind, SourceShared,
    SourceSink, SourceState, SourceStatus,
};
pub use synthetic::{axis_angle, synthetic_pose, SyntheticSource};
pub use vmc::{VmcDecoder, VmcSource};

mod frame;
