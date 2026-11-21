//! A single-person keypoint model run with ONNX Runtime, behind the `pose`
//! feature.
//!
//! The input layout (NHWC or NCHW), size and element type (`f32`, `i32` or
//! `u8`) are read from the model, so MoveNet-style exports and compatible
//! models load without configuration. The output is the first tensor: 17
//! keypoints of `(y, x, score)` in model-input fractions. Float inputs are
//! 0..255 by default; models trained on other ranges set
//! `PULSAR_MOCAP_POSE_SCALE` to `unit` (0..1) or `signed` (-1..1).

use crate::estimator::PoseEstimator;
use crate::frame::VideoFrame;
use crate::keypoints::{coco17_rig, decode_single_person, letterbox_rgb, Letterbox};
use mocap_format::RigProfile;
use ort::environment::Environment;
use ort::session::Session;
use ort::value::{Tensor, TensorElementType};
use std::sync::OnceLock;

/// Keypoints under this confidence are recorded as gaps.
const MIN_SCORE: f32 = 0.25;
/// Spatial size assumed when a model leaves its input size open.
const FALLBACK_SIZE: usize = 192;
/// The model embedded by `build.rs` (empty when it could not be fetched).
const BUNDLED_MODEL: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/pose_model.onnx"));

#[derive(Clone, Copy)]
enum Scale {
    Byte,
    Unit,
    Signed,
}

impl Scale {
    fn from_env() -> Self {
        match std::env::var("PULSAR_MOCAP_POSE_SCALE").as_deref() {
            Ok("unit") => Self::Unit,
            Ok("signed") => Self::Signed,
            _ => Self::Byte,
        }
    }

    fn apply(self, byte: u8) -> f32 {
        let v = f32::from(byte);
        match self {
            Self::Byte => v,
            Self::Unit => v / 255.0,
            Self::Signed => v / 127.5 - 1.0,
        }
    }
}

/// A loaded keypoint model.
pub struct OnnxPoseEstimator {
    session: Session,
    name: String,
    input_name: String,
    input: (u32, u32),
    nchw: bool,
    element: TensorElementType,
    scale: Scale,
}

/// The process's one ONNX Runtime environment (telemetry off), built on first use.
fn environment() -> Result<&'static Environment, String> {
    static ENV: OnceLock<Result<Environment, String>> = OnceLock::new();
    ENV.get_or_init(|| ort::init().with_name("pulsar-mocap").with_telemetry(false).build().map_err(|e| format!("ONNX Runtime did not start: {e}")))
        .as_ref()
        .map_err(Clone::clone)
}

fn fail<E: std::fmt::Display>(what: &'static str) -> impl FnOnce(E) -> String {
    move |e| format!("{what}: {e}")
}

impl OnnxPoseEstimator {
    /// The model built into this binary (MoveNet Lightning), `None` when the
    /// build could not fetch it (see `build.rs`).
    pub fn bundled() -> Option<Result<Self, String>> {
        (!BUNDLED_MODEL.is_empty()).then(|| Self::from_bytes(BUNDLED_MODEL, "MoveNet Lightning (bundled)"))
    }

    /// Loads a model from ONNX bytes.
    pub fn from_bytes(bytes: &[u8], name: &str) -> Result<Self, String> {
        let session = Session::builder(environment()?)
            .map_err(fail("ONNX Runtime did not start"))?
            .commit_from_memory(bytes)
            .map_err(fail("the model could not be loaded"))?;
        let input = session.inputs().first().ok_or("the model has no input")?;
        let element = input.dtype().tensor_type().ok_or("the model's input is not a tensor")?;
        if !matches!(element, TensorElementType::Float32 | TensorElementType::Int32 | TensorElementType::Uint8) {
            return Err(format!("the model's input type {element:?} is not supported (f32, i32 or u8)"));
        }
        let dims: Vec<i64> = input.dtype().tensor_shape().map(|s| s.to_vec()).unwrap_or_default();
        let open = |d: i64| if d > 0 { d as usize } else { FALLBACK_SIZE };
        let (nchw, height, width) = match dims.as_slice() {
            [_, h, w, 3] => (false, open(*h), open(*w)),
            [_, 3, h, w] => (true, open(*h), open(*w)),
            _ => (false, FALLBACK_SIZE, FALLBACK_SIZE),
        };
        let input_name = input.name().to_owned();
        Ok(Self {
            session,
            name: name.to_owned(),
            input_name,
            input: (width as u32, height as u32),
            nchw,
            element,
            scale: Scale::from_env(),
        })
    }

    fn tensor_value(&self, rgb: &[u8]) -> Result<ort::session::SessionInputValue<'static>, String> {
        let (w, h) = (self.input.0 as usize, self.input.1 as usize);
        // The model wants NHWC or NCHW; `rgb` is HWC.
        let index = |i: usize| {
            if self.nchw {
                let (c, rest) = (i / (h * w), i % (h * w));
                rest * 3 + c
            } else {
                i
            }
        };
        let shape = if self.nchw { [1, 3, h, w] } else { [1, h, w, 3] };
        let count = w * h * 3;
        let make = |e: ort::Error| format!("the input tensor could not be built: {e}");
        Ok(match self.element {
            TensorElementType::Float32 => {
                let data: Vec<f32> = (0..count).map(|i| self.scale.apply(rgb[index(i)])).collect();
                Tensor::from_array((shape, data)).map_err(make)?.into()
            }
            TensorElementType::Int32 => {
                let data: Vec<i32> = (0..count).map(|i| i32::from(rgb[index(i)])).collect();
                Tensor::from_array((shape, data)).map_err(make)?.into()
            }
            _ => {
                let data: Vec<u8> = (0..count).map(|i| rgb[index(i)]).collect();
                Tensor::from_array((shape, data)).map_err(make)?.into()
            }
        })
    }
}

impl PoseEstimator for OnnxPoseEstimator {
    fn rig(&self) -> RigProfile {
        coco17_rig()
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn estimate(&mut self, frame: &VideoFrame) -> Option<Vec<f32>> {
        let lb = Letterbox::new((frame.width, frame.height), self.input);
        let value = self.tensor_value(&letterbox_rgb(frame, &lb)).ok()?;
        let outputs = self.session.run(vec![(self.input_name.clone(), value)]).ok()?;
        let (_, data) = outputs[0].try_extract_tensor::<f32>().ok()?;
        decode_single_person(data, &lb, MIN_SCORE)
    }
}
