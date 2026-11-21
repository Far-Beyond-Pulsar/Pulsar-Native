//! A webcam keypoint detector as it really behaves: noisy, with gross misses,
//! dropouts, timing jitter, and cameras that see only part of the body.

use super::{angle_deg, subject, Noise};
use mocap_capture::{CalibrationError, Calibrator, CameraModel, CameraRig, CameraSpec};
use nalgebra::Vector3;

/// How badly a detector behaves.
#[derive(Clone)]
pub struct Quality {
    /// Gaussian noise on every keypoint, pixels (wrists and ankles get 1.7x).
    pub sigma: f64,
    /// Chance a keypoint is a gross miss (40 to 120 px away).
    pub outliers: f64,
    /// Chance a keypoint is not detected.
    pub dropout: f64,
    /// Each camera's frame is taken up to this long before or after the others (seconds).
    pub sync_jitter: f64,
    /// The keypoints this camera can see at all (a close or tilted camera crops the body).
    pub sees: fn(usize) -> bool,
}

pub fn everything(_: usize) -> bool {
    true
}

pub fn upper_body(k: usize) -> bool {
    k <= 10
}

pub fn up_to_hips(k: usize) -> bool {
    k <= 12
}

pub fn torso_down(k: usize) -> bool {
    k >= 5
}

pub fn legs_and_hips(k: usize) -> bool {
    k >= 11
}

pub fn legs_only(k: usize) -> bool {
    k >= 13
}

impl Quality {
    /// A plain, noisy detector that sees the whole body.
    pub fn webcam() -> Self {
        Self { sigma: 3.0, outliers: 0.03, dropout: 0.08, sync_jitter: 0.016, sees: everything }
    }

    pub fn seeing(self, sees: fn(usize) -> bool) -> Self {
        Self { sees, ..self }
    }
}

/// One camera's detections of the subject at time `t`, in `quality`.
pub fn observe_real(cam: &CameraModel, t: f64, noise: &mut Noise, quality: &Quality) -> Option<Vec<[f32; 3]>> {
    let joints = subject(t + quality.sync_jitter * (2.0 * noise.uniform() - 1.0));
    let points: Vec<[f32; 3]> = joints
        .iter()
        .enumerate()
        .map(|(k, &j)| {
            let Some(p) = cam.project(j).filter(|p| (0.0..f64::from(cam.width)).contains(&p[0]) && (0.0..f64::from(cam.height)).contains(&p[1])) else {
                return [f32::NAN, f32::NAN, 0.0];
            };
            if !(quality.sees)(k) || noise.uniform() < quality.dropout {
                return [f32::NAN, f32::NAN, 0.0];
            }
            let wobble = if matches!(k, 9 | 10 | 15 | 16) { 1.7 } else { 1.0 };
            let (mut dx, mut dy) = (quality.sigma * wobble * noise.gaussian(), quality.sigma * wobble * noise.gaussian());
            if noise.uniform() < quality.outliers {
                let miss = 40.0 + 80.0 * noise.uniform();
                let angle = std::f64::consts::TAU * noise.uniform();
                (dx, dy) = (miss * angle.cos(), miss * angle.sin());
            }
            let score = 0.55 + 0.4 * noise.uniform();
            [((p[0] + dx) / f64::from(cam.width)) as f32, ((p[1] + dy) / f64::from(cam.height)) as f32, score as f32]
        })
        .collect();
    Some(points)
}

/// Runs `seconds` of capture at 30 fps (the subject moving 2.5x real time) through
/// the calibrator and solves.
pub fn calibrate_real(cameras: &[CameraModel], qualities: &[Quality], seconds: f64) -> (Result<CameraRig, CalibrationError>, Calibrator) {
    let specs = cameras.iter().map(|c| CameraSpec { id: c.id.clone(), width: c.width, height: c.height }).collect();
    let mut calibrator = Calibrator::new(specs);
    let mut noise = Noise(2024);
    for f in 0..(seconds * 30.0) as usize {
        let t = f as f64 / 30.0 * 2.5;
        let views: Vec<Option<Vec<[f32; 3]>>> = cameras.iter().zip(qualities).map(|(c, q)| observe_real(c, t, &mut noise, q)).collect();
        let refs: Vec<Option<&[[f32; 3]]>> = views.iter().map(|v| v.as_deref()).collect();
        calibrator.offer(&refs);
    }
    let result = calibrator.solve(1.0);
    (result, calibrator)
}

/// How far a found camera is from the truth: (metres, degrees of view direction).
pub fn camera_error(found: &CameraModel, real: &CameraModel) -> (f64, f64) {
    let (p, q) = (Vector3::from_row_slice(&found.position()), Vector3::from_row_slice(&real.position()));
    ((p - q).norm(), angle_deg(found.forward(), real.forward()))
}

pub fn forearm_and_head_side(k: usize) -> bool {
    matches!(k, 0..=4 | 7 | 9)
}

pub fn forearm_and_legs(k: usize) -> bool {
    matches!(k, 7 | 9 | 11..=16)
}

pub fn head_and_left_shoulder_and_right_hip(k: usize) -> bool {
    matches!(k, 0..=5 | 12)
}

pub fn left_shoulder_right_hip_and_legs(k: usize) -> bool {
    matches!(k, 5 | 12 | 13..=16)
}

pub fn both_wrists_and_head(k: usize) -> bool {
    matches!(k, 0..=4 | 9 | 10)
}

pub fn both_wrists_and_legs(k: usize) -> bool {
    matches!(k, 9..=16)
}
