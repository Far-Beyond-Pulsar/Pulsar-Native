//! Multi-camera self-calibration against synthetic ground truth: a moving,
//! turning subject seen by cameras arranged around it, with noisy detections.

mod common;

use common::*;
use mocap_capture::{CalibrationError, Calibrator, CameraSpec};
use nalgebra::Vector3;

#[test]
fn four_cameras_around_a_moving_subject_are_located_in_the_subjects_world() {
    let truth = truth_cameras(4);
    let (result, calibrator) = calibrate(&truth, 0.7, 400);
    assert!(calibrator.samples() >= 50, "{} samples", calibrator.samples());
    let rig = result.expect("the calibration solves");
    assert!(rig.rms_px < 2.5, "reprojection error {} px", rig.rms_px);
    for (found, real) in rig.cameras.iter().zip(&truth) {
        let (p, q) = (found.position(), real.position());
        let error = ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)).sqrt();
        assert!(error < 0.15, "{} is {error:.3} m off: found {p:?}, truth {q:?}", real.id);
        let tilt = angle_deg(found.forward(), real.forward());
        assert!(tilt < 3.0, "{} looks {tilt:.2} degrees off", real.id);
        let focal = (found.focal_px - real.focal_px).abs() / real.focal_px;
        assert!(focal < 0.08, "{} focal {} vs {}", real.id, found.focal_px, real.focal_px);
    }
}

#[test]
fn triangulating_with_the_solved_cameras_recovers_the_joints_to_a_few_centimetres() {
    let truth = truth_cameras(4);
    let (result, _) = calibrate(&truth, 0.7, 400);
    let rig = result.expect("solved");
    let mut noise = Noise(99);
    let (mut found, mut real) = (Vec::new(), Vec::new());
    for f in 0..30 {
        let joints = subject(120.0 + f as f64 * 0.37);
        let views: Vec<Option<Vec<[f32; 3]>>> = truth.iter().map(|c| observe(c, &joints, &mut noise, 0.7)).collect();
        let refs: Vec<Option<&[[f32; 3]]>> = views.iter().map(|v| v.as_deref()).collect();
        for (k, p) in rig.triangulate_frame(&refs).iter().enumerate().filter(|(_, p)| p[0].is_finite()) {
            found.push(Vector3::new(f64::from(p[0]), f64::from(p[1]), f64::from(p[2])));
            real.push(joints[k]);
        }
    }
    assert!(found.len() > 200, "{} triangulated joints", found.len());
    // The world is defined by the subject's average pose, so compare up to the rigid transform.
    let mean = rigid_aligned_error(&found, &real);
    assert!(mean < 0.03, "mean 3D error after rigid alignment {:.1} mm", mean * 1000.0);
    // And the world is the subject's: up is +Y and the floor is near 0.
    let ankles: Vec<f64> = found.iter().zip(&real).filter(|(_, r)| r[1] < 0.2).map(|(f, _)| f[1]).collect();
    let low = ankles.iter().copied().fold(f64::INFINITY, f64::min);
    assert!(low.abs() < 0.12, "the floor is at the ankles: {low}");
}

#[test]
fn two_cameras_are_enough_and_a_third_that_sees_little_is_reported_not_guessed() {
    let truth = truth_cameras(2);
    let (result, _) = calibrate(&truth, 0.7, 400);
    let rig = result.expect("two cameras solve");
    let (a, b) = (rig.cameras[0].position(), rig.cameras[1].position());
    let baseline = ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();
    let real = (Vector3::from_row_slice(&truth[0].position()) - Vector3::from_row_slice(&truth[1].position())).norm();
    assert!((baseline - real).abs() / real < 0.06, "baseline {baseline:.2} m vs {real:.2} m");

    // A camera pointed away from the subject never sees it.
    let mut cameras = truth_cameras(2);
    cameras.push(look_at("camera:blind", Vector3::new(0.0, 1.5, 3.0), Vector3::new(0.0, 1.5, 9.0), 1280, 720, 900.0));
    let (result, _) = calibrate(&cameras, 0.7, 300);
    assert_eq!(result.unwrap_err(), CalibrationError::Unplaced(vec!["camera:blind".to_owned()]));
}

#[test]
fn too_little_motion_or_too_few_cameras_is_refused_with_a_reason() {
    let one = truth_cameras(1);
    let (result, _) = calibrate(&one, 0.7, 200);
    assert_eq!(result.unwrap_err(), CalibrationError::TooFewCameras);
    // A subject who never moves gives no usable samples.
    let truth = truth_cameras(3);
    let specs = truth.iter().map(|c| CameraSpec { id: c.id.clone(), width: c.width, height: c.height }).collect();
    let mut calibrator = Calibrator::new(specs);
    let mut noise = Noise(3);
    let joints = subject(1.0);
    for _ in 0..300 {
        let views: Vec<Option<Vec<[f32; 3]>>> = truth.iter().map(|c| observe(c, &joints, &mut noise, 0.7)).collect();
        let refs: Vec<Option<&[[f32; 3]]>> = views.iter().map(|v| v.as_deref()).collect();
        calibrator.offer(&refs);
    }
    assert!(calibrator.samples() < 10, "a still subject is not sampled again and again: {}", calibrator.samples());
    assert!(matches!(calibrator.solve(1.0), Err(CalibrationError::TooFewSamples { .. })));
}

#[test]
fn a_taller_subject_scales_the_world_with_the_given_body_scale() {
    let truth = truth_cameras(3);
    let (result, _) = calibrate(&truth, 0.5, 400);
    let base = result.expect("solved");
    let specs = truth.iter().map(|c| CameraSpec { id: c.id.clone(), width: c.width, height: c.height }).collect();
    let mut calibrator = Calibrator::new(specs);
    let mut noise = Noise(7);
    for f in 0..400 {
        let joints = subject(f as f64 * 0.2);
        let views: Vec<Option<Vec<[f32; 3]>>> = truth.iter().map(|c| observe(c, &joints, &mut noise, 0.5)).collect();
        let refs: Vec<Option<&[[f32; 3]]>> = views.iter().map(|v| v.as_deref()).collect();
        calibrator.offer(&refs);
    }
    let tall = calibrator.solve(1.1).expect("solved");
    let (p, q) = (base.cameras[1].position(), tall.cameras[1].position());
    let ratio = (q[0].powi(2) + q[2].powi(2)).sqrt() / (p[0].powi(2) + p[2].powi(2)).sqrt();
    assert!((ratio - 1.1).abs() < 0.03, "distances scale with the subject: {ratio}");
}

/// The subject doing a guided step: hands raised, or arms held out.
fn posed(joints: &mut [Vector3<f64>; 17], step: mocap_capture::Step) {
    let (ls, rs) = (joints[5], joints[6]);
    let left = (ls - rs).normalize();
    match step {
        mocap_capture::Step::RaiseLeftHand => joints[9] = ls + Vector3::new(0.0, 0.55, 0.0),
        mocap_capture::Step::RaiseRightHand => joints[10] = rs + Vector3::new(0.0, 0.55, 0.0),
        mocap_capture::Step::ArmsOut => {
            joints[9] = ls + left * 0.55;
            joints[10] = rs - left * 0.55;
        }
        _ => {}
    }
}

#[test]
fn the_guided_steps_validate_the_solved_rig_and_catch_a_wrong_one() {
    use mocap_capture::Step;
    let truth = truth_cameras(4);
    let specs = truth.iter().map(|c| CameraSpec { id: c.id.clone(), width: c.width, height: c.height }).collect();
    let mut calibrator = Calibrator::new(specs);
    let mut noise = Noise(11);
    for f in 0..900usize {
        let step = match f / 150 {
            0 => Step::RaiseLeftHand,
            1 => Step::RaiseRightHand,
            2 => Step::ArmsOut,
            _ => Step::WalkInPlace,
        };
        let mut joints = subject(f as f64 * 0.2);
        posed(&mut joints, step);
        let views: Vec<Option<Vec<[f32; 3]>>> = truth.iter().map(|c| observe(c, &joints, &mut noise, 0.6)).collect();
        let refs: Vec<Option<&[[f32; 3]]>> = views.iter().map(|v| v.as_deref()).collect();
        calibrator.offer_tagged(&refs, step.tag());
    }
    let rig = calibrator.solve(1.0).expect("solved");
    let checks = calibrator.validate(&rig, 1.0);
    let names: Vec<&str> = checks.iter().map(|c| c.name).collect();
    assert_eq!(names, vec!["Left hand reads up", "Right hand reads up", "Arm span matches the body"], "{checks:?}");
    assert!(checks.iter().all(|c| c.passed), "{checks:?}");
    // The same rig judged for a much taller subject no longer matches the arm span.
    let tall = calibrator.validate(&rig, 1.5);
    assert!(!tall.iter().find(|c| c.name == "Arm span matches the body").expect("span").passed);
}
