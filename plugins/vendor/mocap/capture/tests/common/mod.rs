//! Shared helpers for the calibration tests; each test binary uses a different subset.
#![allow(dead_code)]

use mocap_capture::{CalibrationError, Calibrator, CameraModel, CameraSpec};
use nalgebra::{Matrix3, Vector3};


pub struct Noise(pub u64);

impl Noise {
    pub fn uniform(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 11) as f64) / ((1u64 << 53) as f64)
    }

    pub fn gaussian(&mut self) -> f64 {
        let (a, b) = (self.uniform().max(1e-12), self.uniform());
        (-2.0 * a.ln()).sqrt() * (std::f64::consts::TAU * b).cos()
    }
}

/// The subject's COCO-17 joints at time `t`, in metres (+Y up, facing +Z at yaw 0,
/// left at +X). Body proportions equal the calibration's priors.
pub fn subject(t: f64) -> [Vector3<f64>; 17] {
    let yaw = 0.8 * (0.9 * t).sin();
    let facing = Vector3::new(yaw.sin(), 0.0, yaw.cos());
    let left = Vector3::new(yaw.cos(), 0.0, -yaw.sin());
    let up = Vector3::new(0.0, 1.0, 0.0);
    let pelvis = Vector3::new(0.3 * (0.5 * t).sin(), 0.90, 0.25 * (0.7 * t).cos());
    let neck = pelvis + up * 0.5;
    let (ls, rs) = (neck + left * 0.19, neck - left * 0.19);
    let (lh, rh) = (pelvis + left * 0.10, pelvis - left * 0.10);
    let swing = |phase: f64| 0.6 * (1.7 * t + phase).sin();
    // A limb hanging from `from`, bending at the middle joint, swinging forward and back.
    let limb = |from: Vector3<f64>, a: f64, b: f64, angle: f64, side: f64| {
        let dir = |theta: f64| up * -theta.cos() + facing * theta.sin() + left * side * 0.15;
        let mid = from + dir(angle).normalize() * a;
        let end = mid + dir(angle + 0.5 + 0.3 * angle.sin()).normalize() * b;
        (mid, end)
    };
    let (lel, lwr) = limb(ls, 0.29, 0.26, swing(0.0), 1.0);
    let (rel, rwr) = limb(rs, 0.29, 0.26, swing(3.1), -1.0);
    let (lkn, lan) = limb(lh, 0.42, 0.40, swing(3.1) * 0.7, 1.0);
    let (rkn, ran) = limb(rh, 0.42, 0.40, swing(0.0) * 0.7, -1.0);
    let head = neck + up * 0.22;
    [
        head + facing * 0.1,
        head + facing * 0.08 + left * 0.03 + up * 0.03,
        head + facing * 0.08 - left * 0.03 + up * 0.03,
        head + left * 0.08,
        head - left * 0.08,
        ls,
        rs,
        lel,
        rel,
        lwr,
        rwr,
        lh,
        rh,
        lkn,
        rkn,
        lan,
        ran,
    ]
}

/// A camera at `position` looking at `target` (right-handed, +Z forward, +Y down in the image).
pub fn look_at(id: &str, position: Vector3<f64>, target: Vector3<f64>, width: u32, height: u32, focal: f64) -> CameraModel {
    let z = (target - position).normalize();
    let y = Vector3::new(0.0, -1.0, 0.0);
    let x = y.cross(&z).normalize();
    let y = z.cross(&x);
    let rotation = Matrix3::from_rows(&[x.transpose(), y.transpose(), z.transpose()]);
    CameraModel { id: id.to_owned(), width, height, focal_px: focal, rotation, translation: -(rotation * position) }
}

pub fn truth_cameras(count: usize) -> Vec<CameraModel> {
    (0..count)
        .map(|i| {
            let angle = (i as f64) * std::f64::consts::TAU / count as f64 + 0.4;
            let position = Vector3::new(3.0 * angle.sin(), 1.2 + 0.25 * (i % 3) as f64, 3.0 * angle.cos());
            look_at(&format!("camera:{i}"), position, Vector3::new(0.0, 1.0, 0.0), 1280, 720, 880.0 + 40.0 * i as f64)
        })
        .collect()
}

pub fn observe(cam: &CameraModel, joints: &[Vector3<f64>; 17], noise: &mut Noise, sigma: f64) -> Option<Vec<[f32; 3]>> {
    if noise.uniform() < 0.05 {
        return None;
    }
    let points: Vec<[f32; 3]> = joints
        .iter()
        .map(|&j| match cam.project(j) {
            Some(p) if (0.0..1280.0).contains(&p[0]) && (0.0..720.0).contains(&p[1]) && noise.uniform() > 0.03 => {
                [((p[0] + sigma * noise.gaussian()) / 1280.0) as f32, ((p[1] + sigma * noise.gaussian()) / 720.0) as f32, 0.9]
            }
            _ => [f32::NAN, f32::NAN, 0.0],
        })
        .collect();
    Some(points)
}

pub fn calibrate(cameras: &[CameraModel], sigma: f64, frames: usize) -> (Result<mocap_capture::CameraRig, CalibrationError>, Calibrator) {
    let specs = cameras.iter().map(|c| CameraSpec { id: c.id.clone(), width: c.width, height: c.height }).collect();
    let mut calibrator = Calibrator::new(specs);
    let mut noise = Noise(7);
    for f in 0..frames {
        let joints = subject(f as f64 * 0.2);
        let views: Vec<Option<Vec<[f32; 3]>>> = cameras.iter().map(|c| observe(c, &joints, &mut noise, sigma)).collect();
        let refs: Vec<Option<&[[f32; 3]]>> = views.iter().map(|v| v.as_deref()).collect();
        calibrator.offer(&refs);
    }
    let result = calibrator.solve(1.0);
    (result, calibrator)
}

/// Mean distance between `a` and `b` after the best rotation and translation (Kabsch).
pub fn rigid_aligned_error(a: &[Vector3<f64>], b: &[Vector3<f64>]) -> f64 {
    let n = a.len() as f64;
    let (ca, cb) = (a.iter().sum::<Vector3<f64>>() / n, b.iter().sum::<Vector3<f64>>() / n);
    let h = a.iter().zip(b).fold(Matrix3::zeros(), |h, (p, q)| h + (p - ca) * (q - cb).transpose());
    let svd = h.svd(true, true);
    let (u, v_t) = (svd.u.expect("u"), svd.v_t.expect("v"));
    let mut d = Matrix3::identity();
    d[(2, 2)] = (v_t.transpose() * u.transpose()).determinant().signum();
    let r = v_t.transpose() * d * u.transpose();
    a.iter().zip(b).map(|(p, q)| (r * (p - ca) + cb - q).norm()).sum::<f64>() / n
}

pub fn angle_deg(a: [f64; 3], b: [f64; 3]) -> f64 {
    let (a, b) = (Vector3::from_row_slice(&a), Vector3::from_row_slice(&b));
    a.normalize().dot(&b.normalize()).clamp(-1.0, 1.0).acos().to_degrees()
}

pub mod realism;
