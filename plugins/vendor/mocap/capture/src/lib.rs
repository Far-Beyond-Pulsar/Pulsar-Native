//! Mocap capture (`pulsar.mocap`): sources, the frame bus, live processing and
//! the recorder. GPUI-free; every capture thread is a registered editor
//! service. Design: `.agents/MOCAP.md`.

mod bus;
mod calibration;
#[cfg(feature = "camera")]
mod camera;
mod epipolar;
mod error;
mod guide;
mod estimator;
mod keypoints;
#[cfg(feature = "pose")]
mod onnx;
pub mod osc;
mod mix;
mod processor;
mod recorder;
mod rigs;
mod sensor;
mod scene;
mod skeleton;
mod source;
mod synthetic;
mod vmc;

pub use bus::{BusStats, PoseBus, VideoMailbox};
pub use calibration::{CalibrationError, Calibrator, CameraModel, CameraRig, CameraSpec, Check, HoldAverager, PairReport, MAX_RMS_PX, MIN_SAMPLES, SAMPLES_TARGET};
pub use guide::{Features, Guide, GuideState, Step};
#[cfg(feature = "camera")]
pub use camera::{enumerate_cameras, CameraInfo, CameraMode, CameraSource};
pub use error::CaptureError;
pub use estimator::{EstimatorFactory, EstimatorSource, PoseEstimator};
#[cfg(feature = "pose")]
pub use onnx::OnnxPoseEstimator;
pub use keypoints::{coco17_rig, decode_single_person, letterbox_rgb, Letterbox, COCO17, COCO17_EDGES};
pub use frame::{PixelFormat, PoseFrame, VideoFrame};
pub use mix::{merge_rigs, MixInput, Mixer};
pub use processor::{apply_calibration, calibrate_neutral, FilterSettings, Processor};
pub use recorder::{RecordStats, Recorder, MAX_HOLD_S};
pub use rigs::{vrm_humanoid, RigIndex, VRM_BLENDSHAPES};
pub use sensor::{sensor_rig, OscSensorSource, SensorChannel, SensorDecoder};
pub use scene::{build_scene, lift_keypoints, CameraInput, Contribution, FigureSource, Scene, SceneAxes, SceneCamera, SceneFigure, Subject};
pub use skeleton::{rest_offset, Skeleton};
pub use source::{
    spawn_capture_loop, CaptureSource, SessionClock, SourceDescriptor, SourceKind, SourceShared,
    SourceSink, SourceState, SourceStatus,
};
pub use synthetic::{axis_angle, synthetic_pose, SyntheticSource};
pub use vmc::{VmcDecoder, VmcSource};

mod frame;
