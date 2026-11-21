//! A single-person keypoint model run with `tract` (pure Rust ONNX, no native
//! runtime to ship), behind the `pose` feature.
//!
//! The input layout (NHWC or NCHW), size and element type are read from the
//! model, so MoveNet-style exports and compatible models load without
//! configuration. The output is the first tensor: 17 keypoints of
//! `(y, x, score)` in model-input fractions. Input values are 0..255 by
//! default; models trained on other ranges set `PULSAR_MOCAP_POSE_SCALE` to
//! `unit` (0..1) or `signed` (-1..1).

use crate::estimator::PoseEstimator;
use crate::frame::VideoFrame;
use crate::keypoints::{coco17_rig, decode_single_person, letterbox_rgb, Letterbox};
use mocap_format::RigProfile;
use std::path::Path;
use tract_onnx::prelude::*;
use tract_onnx::tract_hir::internal::Factoid as _;

/// Keypoints under this confidence are recorded as gaps.
const MIN_SCORE: f32 = 0.25;
/// Spatial size assumed when a model leaves its input size open.
const FALLBACK_SIZE: usize = 192;

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
    model: TypedRunnableModel<TypedModel>,
    name: String,
    input: (u32, u32),
    nchw: bool,
    integer: bool,
    scale: Scale,
}

fn fail<E: std::fmt::Display>(what: &str) -> impl FnOnce(E) -> String + '_ {
    move |e| format!("{what}: {e}")
}

impl OnnxPoseEstimator {
    /// Loads the model at `path`.
    pub fn load(path: &Path) -> Result<Self, String> {
        let shown = path.display().to_string();
        let mut graph = tract_onnx::onnx().model_for_path(path).map_err(fail(&format!("could not read {shown}")))?;
        let fact = graph.input_fact(0).map_err(fail("the model has no input"))?.clone();
        let dims: Vec<usize> = fact
            .shape
            .concretize()
            .map(|d| d.iter().map(|d| d.to_i64().map_or(0, |v| v.max(0) as usize)).collect())
            .unwrap_or_default();
        let (nchw, height, width) = match dims.as_slice() {
            [_, h, w, 3] => (false, *h, *w),
            [_, 3, h, w] => (true, *h, *w),
            _ => (false, FALLBACK_SIZE, FALLBACK_SIZE),
        };
        let (height, width) = (if height == 0 { FALLBACK_SIZE } else { height }, if width == 0 { FALLBACK_SIZE } else { width });
        let integer = fact.datum_type.concretize() == Some(i32::datum_type());
        let shape = if nchw { tvec![1, 3, height, width] } else { tvec![1, height, width, 3] };
        let datum = if integer { i32::datum_type() } else { f32::datum_type() };
        graph = graph
            .with_input_fact(0, InferenceFact::dt_shape(datum, shape))
            .map_err(fail("the input size was refused"))?;
        let model = graph
            .into_optimized()
            .map_err(fail("the model could not be optimised"))?
            .into_runnable()
            .map_err(fail("the model could not be prepared"))?;
        let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("pose model").to_owned();
        Ok(Self { model, name, input: (width as u32, height as u32), nchw, integer, scale: Scale::from_env() })
    }

    fn tensor(&self, rgb: &[u8]) -> Result<Tensor, String> {
        let (w, h) = (self.input.0 as usize, self.input.1 as usize);
        let at = |x: usize, y: usize, c: usize| rgb[(y * w + x) * 3 + c];
        let scale = self.scale;
        macro_rules! build {
            ($t:ty, $f:expr) => {{
                let value = $f;
                if self.nchw {
                    tract_ndarray::Array4::<$t>::from_shape_fn((1, 3, h, w), |(_, c, y, x)| value(at(x, y, c))).into_tensor()
                } else {
                    tract_ndarray::Array4::<$t>::from_shape_fn((1, h, w, 3), |(_, y, x, c)| value(at(x, y, c))).into_tensor()
                }
            }};
        }
        Ok(if self.integer { build!(i32, |b: u8| i32::from(b)) } else { build!(f32, move |b: u8| scale.apply(b)) })
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
        let tensor = self.tensor(&letterbox_rgb(frame, &lb)).ok()?;
        let outputs = self.model.run(tvec!(tensor.into())).ok()?;
        let view = outputs.first()?.to_array_view::<f32>().ok()?;
        let flat: Vec<f32> = view.iter().copied().collect();
        decode_single_person(&flat, &lb, MIN_SCORE)
    }
}
