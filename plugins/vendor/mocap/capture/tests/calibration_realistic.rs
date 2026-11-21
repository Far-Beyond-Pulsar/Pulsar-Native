//! Calibration under real conditions: a noisy detector with gross misses and
//! timing jitter, and cameras that each see only part of the body. A handful of
//! keypoints shared between cameras must be enough.

mod common;

use common::realism::*;
use common::{rigid_aligned_error, subject, truth_cameras, Noise};
use mocap_capture::{CalibrationError, CameraModel, CameraRig};
use nalgebra::Vector3;

fn report(name: &str, rig: &CameraRig, truth: &[CameraModel]) {
    let worst = rig.cameras.iter().zip(truth).map(|(f, r)| camera_error(f, r)).fold((0.0f64, 0.0f64), |a, e| (a.0.max(e.0), a.1.max(e.1)));
    let per_camera: Vec<f64> = rig.per_camera_rms.iter().map(|r| (r * 10.0).round() / 10.0).collect();
    eprintln!("{name}: rms {:.2} px, parallax {:.0} deg, worst camera {:.3} m / {:.2} deg, per-camera rms {per_camera:?}", rig.rms_px, rig.parallax_deg, worst.0, worst.1);
}

fn assert_located(rig: &CameraRig, truth: &[CameraModel], metres: f64, degrees: f64) {
    for (found, real) in rig.cameras.iter().zip(truth) {
        let (m, d) = camera_error(found, real);
        assert!(m < metres, "{} is {m:.3} m off (limit {metres})", real.id);
        assert!(d < degrees, "{} looks {d:.2} degrees off (limit {degrees})", real.id);
    }
}

#[test]
fn a_noisy_detector_with_misses_and_timing_jitter_still_calibrates() {
    let truth = truth_cameras(4);
    let qualities = vec![Quality::webcam(); 4];
    let (result, calibrator) = calibrate_real(&truth, &qualities, 90.0);
    let rig = result.unwrap_or_else(|e| panic!("{e} ({} samples)", calibrator.samples()));
    report("noisy webcam", &rig, &truth);
    let dist = |a: &CameraModel, b: &CameraModel| (Vector3::from_row_slice(&a.position()) - Vector3::from_row_slice(&b.position())).norm();
    let ratios: Vec<f64> = (1..4).map(|i| dist(&rig.cameras[0], &rig.cameras[i]) / dist(&truth[0], &truth[i])).collect();
    eprintln!("scale ratios (found / true camera spacing): {ratios:?}");
    assert!(rig.rms_px < 6.0, "rms {}", rig.rms_px);
    assert_located(&rig, &truth, 0.30, 5.0);
}

#[test]
fn cameras_that_see_only_the_upper_body_the_legs_or_everything_are_all_placed() {
    // A close camera crops the legs, a low one crops the head: those two share only the hips.
    // Each links to the full-body cameras through a handful of shared keypoints.
    let truth = truth_cameras(4);
    let qualities = vec![Quality::webcam().seeing(upper_body), Quality::webcam().seeing(legs_and_hips), Quality::webcam(), Quality::webcam()];
    let (result, calibrator) = calibrate_real(&truth, &qualities, 90.0);
    let rig = result.unwrap_or_else(|e| panic!("{e} ({} samples)", calibrator.samples()));
    report("partial views", &rig, &truth);
    assert_located(&rig, &truth, 0.40, 6.0);
}

#[test]
fn two_cameras_sharing_only_the_torso_and_arms_are_enough() {
    let truth = truth_cameras(2);
    // Shared: keypoints 5 to 12 (shoulders, arms, hips): eight in all.
    let qualities = vec![Quality::webcam().seeing(up_to_hips), Quality::webcam().seeing(torso_down)];
    let (result, calibrator) = calibrate_real(&truth, &qualities, 120.0);
    let rig = result.unwrap_or_else(|e| panic!("{e} ({} samples)", calibrator.samples()));
    report("two cameras, eight shared keypoints", &rig, &truth);
    assert_located(&rig, &truth, 0.50, 8.0);
}

#[test]
fn cameras_with_no_keypoint_in_common_are_reported_not_guessed() {
    // One sees only the head and arms, the other only the legs.
    let truth = truth_cameras(2);
    let qualities = vec![Quality::webcam().seeing(upper_body), Quality::webcam().seeing(legs_only)];
    let (result, _) = calibrate_real(&truth, &qualities, 90.0);
    assert!(matches!(result, Err(CalibrationError::TooFewSamples { .. }) | Err(CalibrationError::Unplaced(_))), "{result:?}");
}

#[test]
fn the_triangulated_joints_are_accurate_to_a_few_centimetres_despite_the_noise() {
    let truth = truth_cameras(4);
    let qualities = vec![Quality::webcam(); 4];
    let (result, _) = calibrate_real(&truth, &qualities, 90.0);
    let rig = result.expect("solved");
    let mut noise = Noise(77);
    let (mut found, mut real) = (Vec::new(), Vec::new());
    for f in 0..60 {
        let t = 300.0 + f as f64 * 0.37;
        let joints = subject(t);
        let views: Vec<Option<Vec<[f32; 3]>>> =
            truth.iter().zip(&qualities).map(|(c, q)| observe_real(c, t, &mut noise, &Quality { sync_jitter: 0.0, ..q.clone() })).collect();
        let refs: Vec<Option<&[[f32; 3]]>> = views.iter().map(|v| v.as_deref()).collect();
        for (k, p) in rig.triangulate_frame(&refs).iter().enumerate().filter(|(_, p)| p[0].is_finite()) {
            found.push(Vector3::new(f64::from(p[0]), f64::from(p[1]), f64::from(p[2])));
            real.push(joints[k]);
        }
    }
    let mean = rigid_aligned_error(&found, &real);
    eprintln!("noisy triangulation: {} joints, mean error {:.1} mm", found.len(), mean * 1000.0);
    assert!(mean < 0.06, "mean 3D error {:.1} mm", mean * 1000.0);
}
