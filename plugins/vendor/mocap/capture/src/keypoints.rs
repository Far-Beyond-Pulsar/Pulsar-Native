//! Body keypoints from a camera: the COCO-17 rig, image preprocessing and the
//! decoding of single-person keypoint models (MoveNet's `[1, 1, 17, 3]`
//! output of `(y, x, score)`).
//!
//! Keypoints are image-space: a channel's `(x, y)` are fractions of the camera
//! frame (0..1, origin top-left) and `z` is the model's confidence. They record
//! and plot like any channel; turning them into joint rotations is a later
//! retargeting step, and several calibrated cameras can triangulate them.

use crate::frame::{PixelFormat, VideoFrame};
use mocap_format::{ChannelDesc, ChannelKind, Quantisation, RigProfile};

/// The COCO keypoints, in the order single-person models output them.
pub const COCO17: [&str; 17] = [
    "nose",
    "left_eye",
    "right_eye",
    "left_ear",
    "right_ear",
    "left_shoulder",
    "right_shoulder",
    "left_elbow",
    "right_elbow",
    "left_wrist",
    "right_wrist",
    "left_hip",
    "right_hip",
    "left_knee",
    "right_knee",
    "left_ankle",
    "right_ankle",
];

/// The bones drawn between COCO keypoints.
pub const COCO17_EDGES: [(usize, usize); 16] = [
    (0, 1),
    (0, 2),
    (1, 3),
    (2, 4),
    (5, 6),
    (5, 7),
    (7, 9),
    (6, 8),
    (8, 10),
    (5, 11),
    (6, 12),
    (11, 12),
    (11, 13),
    (13, 15),
    (12, 14),
    (14, 16),
];

/// The rig of a body tracker: one image-space channel per keypoint.
pub fn coco17_rig() -> RigProfile {
    RigProfile {
        name: "coco17-image".to_owned(),
        joints: Vec::new(),
        channels: COCO17
            .iter()
            .map(|name| ChannelDesc {
                name: format!("kp.{name}"),
                kind: ChannelKind::Position3,
                joint: None,
                quantisation: Quantisation::Lossless,
            })
            .collect(),
    }
}

/// A source frame fitted inside a model's square-ish input with padding, and
/// the way back to source coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Letterbox {
    /// The model input size `(width, height)`.
    pub input: (u32, u32),
    /// The source frame size.
    pub source: (u32, u32),
    scale: f32,
    pad: (f32, f32),
}

impl Letterbox {
    /// Fits `source` inside `input`, centred, keeping its aspect ratio.
    pub fn new(source: (u32, u32), input: (u32, u32)) -> Self {
        let scale = (input.0 as f32 / source.0.max(1) as f32).min(input.1 as f32 / source.1.max(1) as f32);
        let pad = (
            (input.0 as f32 - source.0 as f32 * scale) * 0.5,
            (input.1 as f32 - source.1 as f32 * scale) * 0.5,
        );
        Self { input, source, scale, pad }
    }

    /// A point in the model input as fractions `(0..1)` of the model input, to
    /// fractions of the source frame (outside 0..1 when it lies in the padding).
    pub fn to_source(&self, x: f32, y: f32) -> (f32, f32) {
        (
            (x * self.input.0 as f32 - self.pad.0) / (self.source.0 as f32 * self.scale),
            (y * self.input.1 as f32 - self.pad.1) / (self.source.1 as f32 * self.scale),
        )
    }
}

/// The frame resampled into the letterbox as tightly packed RGB bytes
/// (bilinear; the padding is black).
pub fn letterbox_rgb(frame: &VideoFrame, lb: &Letterbox) -> Vec<u8> {
    let (iw, ih) = (lb.input.0 as usize, lb.input.1 as usize);
    let (sw, sh) = (frame.width as usize, frame.height as usize);
    let mut out = vec![0u8; iw * ih * 3];
    if sw == 0 || sh == 0 || !frame.is_well_formed() {
        return out;
    }
    let (r, b) = match frame.format {
        PixelFormat::Rgba8 => (0, 2),
        PixelFormat::Bgra8 => (2, 0),
    };
    let at = |x: usize, y: usize, c: usize| f32::from(frame.data[(y * sw + x) * 4 + c]);
    for y in 0..ih {
        let sy = (y as f32 + 0.5 - lb.pad.1) / lb.scale - 0.5;
        if sy < -0.5 || sy > sh as f32 - 0.5 {
            continue;
        }
        let (y0, fy) = (sy.floor().max(0.0) as usize, sy.fract().max(0.0));
        let y1 = (y0 + 1).min(sh - 1);
        for x in 0..iw {
            let sx = (x as f32 + 0.5 - lb.pad.0) / lb.scale - 0.5;
            if sx < -0.5 || sx > sw as f32 - 0.5 {
                continue;
            }
            let (x0, fx) = (sx.floor().max(0.0) as usize, sx.fract().max(0.0));
            let x1 = (x0 + 1).min(sw - 1);
            for (k, channel) in [r, 1, b].into_iter().enumerate() {
                let top = at(x0, y0, channel) * (1.0 - fx) + at(x1, y0, channel) * fx;
                let bottom = at(x0, y1, channel) * (1.0 - fx) + at(x1, y1, channel) * fx;
                out[(y * iw + x) * 3 + k] = (top * (1.0 - fy) + bottom * fy).round() as u8;
            }
        }
    }
    out
}

/// Decodes a single-person `(y, x, score)` x 17 output into the rig's columns:
/// `(x, y, score)` per keypoint in source-image fractions. A keypoint under
/// `min_score` is a gap (`NaN`), never a guess.
pub fn decode_single_person(output: &[f32], lb: &Letterbox, min_score: f32) -> Option<Vec<f32>> {
    if output.len() < COCO17.len() * 3 {
        return None;
    }
    let mut values = Vec::with_capacity(COCO17.len() * 3);
    for kp in output.chunks_exact(3).take(COCO17.len()) {
        let (y, x, score) = (kp[0], kp[1], kp[2]);
        if score >= min_score && x.is_finite() && y.is_finite() {
            let (sx, sy) = lb.to_source(x, y);
            values.extend([sx, sy, score]);
        } else {
            values.extend([f32::NAN; 3]);
        }
    }
    Some(values)
}
