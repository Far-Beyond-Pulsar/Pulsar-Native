//! Calibration from a known layout: the user says where the cameras stand, so no
//! movement or detection is needed. Metres, Y up, the subject at the origin on the floor.
//! It is exact as far as the measuring is, and the right choice for a fixed studio.

use crate::calibration::{CameraModel, CameraRig};
use nalgebra::{Matrix3, Vector3};

/// A camera, as the user describes where it stands.
#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
    pub id: String,
    pub width: u32,
    pub height: u32,
    /// Where the lens is, metres.
    pub position: [f64; 3],
    /// What the camera is aimed at, metres.
    pub target: [f64; 3],
    /// Horizontal field of view, degrees (a webcam is typically 60 to 90).
    pub hfov_deg: f64,
}

/// A ready-made arrangement of cameras around the subject.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arrangement {
    /// Evenly spaced on a circle: the best coverage of the whole body.
    Ring,
    /// Spread over a half circle in front of the subject: a desk or a stage.
    Arc,
    /// Side by side in a row facing the subject: a wall of cameras.
    Line,
}

impl Arrangement {
    pub const ALL: [Arrangement; 3] = [Arrangement::Ring, Arrangement::Arc, Arrangement::Line];

    pub fn name(self) -> &'static str {
        match self {
            Arrangement::Ring => "Ring",
            Arrangement::Arc => "Arc",
            Arrangement::Line => "Line",
        }
    }
}

/// The camera that stands at `p.position` and looks at `p.target`.
pub fn model_for(p: &Placement) -> CameraModel {
    let position = Vector3::from_row_slice(&p.position);
    let target = Vector3::from_row_slice(&p.target);
    let mut z = target - position;
    if z.norm() < 1e-6 {
        z = Vector3::new(0.0, 0.0, 1.0);
    }
    let z = z.normalize();
    let down = Vector3::new(0.0, -1.0, 0.0);
    // Straight up or down has no horizon: fall back to world -Z as the screen's up.
    let mut x = down.cross(&z);
    if x.norm() < 1e-6 {
        x = Vector3::new(1.0, 0.0, 0.0);
    }
    let x = x.normalize();
    let y = z.cross(&x);
    let rotation = Matrix3::from_rows(&[x.transpose(), y.transpose(), z.transpose()]);
    let focal_px = f64::from(p.width) * 0.5 / (p.hfov_deg.clamp(20.0, 150.0).to_radians() * 0.5).tan();
    CameraModel { id: p.id.clone(), width: p.width, height: p.height, focal_px, rotation, translation: -(rotation * position) }
}

/// Places `count` cameras on `arrangement` at `distance` metres from the subject and `height`
/// metres above the floor, all aimed at the subject's middle (`aim` metres up).
pub fn arrange(arrangement: Arrangement, cameras: &[(String, u32, u32)], distance: f64, height: f64, aim: f64, hfov_deg: f64) -> Vec<Placement> {
    let n = cameras.len().max(1) as f64;
    cameras
        .iter()
        .enumerate()
        .map(|(i, (id, width, height_px))| {
            let k = i as f64;
            let position = match arrangement {
                Arrangement::Ring => {
                    let a = k * std::f64::consts::TAU / n;
                    [distance * a.sin(), height, distance * a.cos()]
                }
                Arrangement::Arc => {
                    // From -60 to +60 degrees about straight ahead (+Z).
                    let a = if cameras.len() == 1 { 0.0 } else { (-60.0 + 120.0 * k / (n - 1.0)).to_radians() };
                    [distance * a.sin(), height, distance * a.cos()]
                }
                Arrangement::Line => {
                    let spacing = distance * 0.6;
                    [(k - (n - 1.0) * 0.5) * spacing, height, distance]
                }
            };
            Placement { id: id.clone(), width: *width, height: *height_px, position, target: [0.0, aim, 0.0], hfov_deg }
        })
        .collect()
}

/// The rig for cameras at known places. Nothing was measured from data, so the errors are
/// zero and the notes say so.
pub fn rig_from_placements(placements: &[Placement]) -> Option<CameraRig> {
    if placements.len() < 2 {
        return None;
    }
    let cameras: Vec<CameraModel> = placements.iter().map(model_for).collect();
    let rig = CameraRig {
        per_camera_rms: vec![0.0; cameras.len()],
        cameras,
        rms_px: 0.0,
        samples: 0,
        parallax_deg: 0.0,
        notes: vec!["Camera positions were entered, not measured: they are only as accurate as the measuring"],
    };
    Some(CameraRig { parallax_deg: median_parallax(&rig.cameras), ..rig })
}

/// The median angle between two cameras' lines of sight to the subject's middle.
fn median_parallax(cameras: &[CameraModel]) -> f64 {
    let middle = Vector3::new(0.0, 1.0, 0.0);
    let rays: Vec<Vector3<f64>> = cameras.iter().map(|c| (middle - Vector3::from_row_slice(&c.position())).normalize()).collect();
    let mut angles = Vec::new();
    for i in 0..rays.len() {
        for j in i + 1..rays.len() {
            angles.push(rays[i].dot(&rays[j]).clamp(-1.0, 1.0).acos().to_degrees());
        }
    }
    angles.sort_by(f64::total_cmp);
    angles.get(angles.len() / 2).copied().unwrap_or(0.0)
}
