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

/// The camera layout error: positions after the best rigid alignment (the world is defined by
/// the subject's average pose, so it legitimately differs from the truth frame by a rigid move).
fn layout_error(rig: &CameraRig, truth: &[CameraModel]) -> f64 {
    let found: Vec<Vector3<f64>> = rig.cameras.iter().map(|c| Vector3::from_row_slice(&c.position())).collect();
    let real: Vec<Vector3<f64>> = truth.iter().map(|c| Vector3::from_row_slice(&c.position())).collect();
    rigid_aligned_error(&found, &real)
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
    let layout = layout_error(&rig, &truth);
    eprintln!("camera layout error after rigid alignment: {:.1} cm", layout * 100.0);
    assert!(layout < 0.08, "layout {layout} m");
    let dist = |a: &CameraModel, b: &CameraModel| (Vector3::from_row_slice(&a.position()) - Vector3::from_row_slice(&b.position())).norm();
    let ratios: Vec<f64> = (1..4).map(|i| dist(&rig.cameras[0], &rig.cameras[i]) / dist(&truth[0], &truth[i])).collect();
    eprintln!("scale ratios (found / true camera spacing): {ratios:?}");
    assert!(rig.rms_px < 6.0, "rms {}", rig.rms_px);
    assert_located(&rig, &truth, 0.30, 5.0);
}

#[test]
fn a_camera_that_sees_only_the_head_and_arms_is_placed_among_full_body_cameras() {
    // A close camera crops the legs. It shares the head, shoulders, arms and hips with the others.
    // Every camera is placed against the first one, so the cropped camera goes last.
    let truth: Vec<CameraModel> = truth_cameras(4);
    let qualities = vec![Quality::webcam(), Quality::webcam(), Quality::webcam(), Quality::webcam().seeing(up_to_hips)];
    let (result, calibrator) = calibrate_real(&truth, &qualities, 90.0);
    let rig = result.unwrap_or_else(|e| panic!("{e} ({} samples)", calibrator.samples()));
    report("one cropped camera", &rig, &truth);
    let layout = layout_error(&rig, &truth);
    eprintln!("camera layout error after rigid alignment: {:.1} cm", layout * 100.0);
    assert!(layout < 0.10, "layout {layout} m");
}

#[test]
fn a_cropped_reference_camera_is_refused_with_itself_named_not_used_to_place_the_others_badly() {
    // The same cameras with the cropped one first: the others would all be placed against a view
    // that sees only part of the body. The fit check refuses and says which camera to take out.
    let truth = truth_cameras(4);
    let qualities = vec![Quality::webcam().seeing(up_to_hips), Quality::webcam(), Quality::webcam(), Quality::webcam()];
    let (result, _) = calibrate_real(&truth, &qualities, 90.0);
    match result {
        Ok(rig) => assert!(layout_error(&rig, &truth) < 0.10, "if it solves it must be right"),
        Err(e) => assert!(e.to_string().contains("camera:0"), "the cropped camera is named: {e}"),
    }
}

#[test]
fn two_complementary_partial_cameras_are_refused_with_the_culprit_named_not_solved_badly() {
    // One camera sees only the head and arms, another only the hips and legs: they share the hips
    // and nothing else. Alongside two full cameras the combination does not fit, and the calibration
    // says which camera to take out instead of returning a wrong answer.
    let truth = truth_cameras(4);
    let qualities = vec![Quality::webcam().seeing(upper_body), Quality::webcam().seeing(legs_and_hips), Quality::webcam(), Quality::webcam()];
    let (result, _) = calibrate_real(&truth, &qualities, 90.0);
    match result {
        Ok(rig) => assert!(layout_error(&rig, &truth) < 0.10, "if it solves it must be right"),
        Err(e) => {
            let message = e.to_string();
            eprintln!("refused: {message}");
            assert!(message.contains("does not agree") || message.contains("shared"), "{message}");
        }
    }
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
    assert!(matches!(result, Err(CalibrationError::TooFewSamples { .. }) | Err(CalibrationError::Unplaced { .. })), "{result:?}");
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

/// The cameras' keypoints for 45 held poses, `frames` noisy frames each (the subject holds
/// still, so the cameras see the same pose with no timing error).
fn held_poses(truth: &[CameraModel], frames: usize) -> Vec<Vec<Vec<Option<Vec<[f32; 3]>>>>> {
    let quality = Quality { sync_jitter: 0.0, ..Quality::webcam() };
    let mut noise = Noise(5150);
    (0..45)
        .map(|pose| (0..frames).map(|_| truth.iter().map(|c| observe_real(c, 3.0 + pose as f64 * 0.9, &mut noise, &quality)).collect()).collect())
        .collect()
}

fn calibrator_for(truth: &[CameraModel]) -> mocap_capture::Calibrator {
    mocap_capture::Calibrator::new(truth.iter().map(|c| mocap_capture::CameraSpec { id: c.id.clone(), width: c.width, height: c.height }).collect())
}

#[test]
fn averaging_a_held_pose_over_frames_beats_single_noisy_frames() {
    let truth = truth_cameras(4);
    let poses = held_poses(&truth, 12);
    let layout = |average: bool| {
        let mut calibrator = calibrator_for(&truth);
        let mut averager = mocap_capture::HoldAverager::new();
        for pose in &poses {
            for frame in pose {
                let refs: Vec<Option<&[[f32; 3]]>> = frame.iter().map(|v| v.as_deref()).collect();
                if average {
                    if let Some(avg) = averager.push(&refs) {
                        let a: Vec<Option<&[[f32; 3]]>> = avg.iter().map(|v| v.as_deref()).collect();
                        calibrator.offer(&a);
                    }
                } else if averager.push(&[]).is_none() && calibrator.samples() < pose_index(&poses, frame) + 1 {
                    calibrator.offer(&refs);
                    break;
                }
            }
            averager.clear();
        }
        (calibrator.samples(), calibrator.solve(1.0))
    };
    let (n_avg, averaged) = layout(true);
    let (n_raw, single) = layout(false);
    let averaged = averaged.unwrap_or_else(|e| panic!("averaged: {e} ({n_avg} samples)"));
    let a = layout_error(&averaged, &truth);
    let single_error = single.as_ref().map(|r| layout_error(r, &truth));
    eprintln!("held poses: averaged {n_avg} samples -> {:.1} cm; single frames {n_raw} samples -> {single_error:?}", a * 100.0);
    assert!(a < 0.07, "layout {a} m");
    // With the same poses, single noisy frames either fail to link every camera or fit worse.
    if let Ok(s) = single_error {
        assert!(a <= s + 0.005, "averaging ({a}) should not be worse than single frames ({s})");
    }
}

fn pose_index(poses: &[Vec<Vec<Option<Vec<[f32; 3]>>>>], frame: &Vec<Option<Vec<[f32; 3]>>>) -> usize {
    poses.iter().position(|p| p.iter().any(|f| std::ptr::eq(f, frame))).unwrap_or(0)
}

#[test]
fn a_window_with_a_moving_subject_or_too_few_good_keypoints_gives_no_sample() {
    let truth = truth_cameras(2);
    let quality = Quality { sync_jitter: 0.0, ..Quality::webcam() };
    let mut noise = Noise(9);
    let mut averager = mocap_capture::HoldAverager::new();
    // The subject keeps moving: every frame is a different pose, so no keypoint is steady.
    let mut got = 0;
    for f in 0..40 {
        let views: Vec<Option<Vec<[f32; 3]>>> = truth.iter().map(|c| observe_real(c, f as f64 * 0.4, &mut noise, &quality)).collect();
        let refs: Vec<Option<&[[f32; 3]]>> = views.iter().map(|v| v.as_deref()).collect();
        got += usize::from(averager.push(&refs).is_some());
    }
    assert_eq!(got, 0, "a moving subject is not a held pose");
    // A held pose seen by cameras that share nothing gives nothing.
    averager.clear();
    let blind = vec![Quality::webcam().seeing(upper_body), Quality::webcam().seeing(legs_only)];
    for _ in 0..30 {
        let views: Vec<Option<Vec<[f32; 3]>>> = truth.iter().zip(&blind).map(|(c, q)| observe_real(c, 5.0, &mut noise, &Quality { sync_jitter: 0.0, ..q.clone() })).collect();
        let refs: Vec<Option<&[[f32; 3]]>> = views.iter().map(|v| v.as_deref()).collect();
        assert!(averager.push(&refs).is_none(), "no pair shares enough keypoints");
    }
}

#[test]
fn two_labelled_keypoints_are_enough_when_they_are_the_right_ones() {
    // The cameras share exactly two keypoints: the left elbow and left wrist, a forearm. That is
    // a bone, so the subject's size and the scale can be measured, and an arm sweeps through space.
    let truth = truth_cameras(2);
    let qualities = vec![Quality::webcam().seeing(forearm_and_head_side), Quality::webcam().seeing(forearm_and_legs)];
    let (result, calibrator) = calibrate_real(&truth, &qualities, 150.0);
    let report = calibrator.shared_report();
    assert_eq!(report[0].labels.iter().map(|(name, _)| *name).collect::<std::collections::BTreeSet<_>>(), ["left_elbow", "left_wrist"].into_iter().collect(), "{report:?}");
    let rig = result.unwrap_or_else(|e| panic!("{e} ({} samples, {report:?})", calibrator.samples()));
    let baseline = |a: &CameraModel, b: &CameraModel| (Vector3::from_row_slice(&a.position()) - Vector3::from_row_slice(&b.position())).norm();
    let ratio = baseline(&rig.cameras[0], &rig.cameras[1]) / baseline(&truth[0], &truth[1]);
    eprintln!("two shared keypoints: rms {:.2} px, baseline ratio {ratio:.3}", rig.rms_px);
    assert!((ratio - 1.0).abs() < 0.12, "found baseline is {ratio:.3} of the true one");
}

#[test]
fn two_keypoints_that_are_not_a_bone_cannot_give_the_scale_and_the_reason_is_clear() {
    // Left shoulder and right hip are reliable and both seen, but they are not connected, so
    // there is no bone to measure the subject's size by.
    let truth = truth_cameras(2);
    let qualities = vec![Quality::webcam().seeing(head_and_left_shoulder_and_right_hip), Quality::webcam().seeing(left_shoulder_right_hip_and_legs)];
    let (result, _) = calibrate_real(&truth, &qualities, 150.0);
    assert!(matches!(result, Err(CalibrationError::NoSharedBone { .. })), "{result:?}");
    let message = result.unwrap_err().to_string();
    assert!(message.contains("share no connected pair of keypoints"), "{message}");
}

#[test]
fn two_wrists_alone_are_the_wrong_keypoints_and_are_not_taken_as_a_link() {
    // Wrists are the least reliable keypoints; two of them do not link cameras on their own.
    let truth = truth_cameras(2);
    let qualities = vec![Quality::webcam().seeing(both_wrists_and_head), Quality::webcam().seeing(both_wrists_and_legs)];
    let (result, calibrator) = calibrate_real(&truth, &qualities, 90.0);
    assert!(matches!(result, Err(CalibrationError::TooFewSamples { .. })), "{result:?}");
    assert_eq!(calibrator.samples(), 0, "no sample was accepted from a link made of two wrists");
}

#[test]
fn the_shared_report_names_the_labelled_keypoints_each_pair_has_in_common() {
    let truth = truth_cameras(3);
    let qualities = vec![Quality::webcam(), Quality::webcam().seeing(upper_body), Quality::webcam().seeing(legs_and_hips)];
    let (_, calibrator) = calibrate_real(&truth, &qualities, 60.0);
    let report = calibrator.shared_report();
    assert_eq!(report.len(), 3, "one entry per pair of cameras");
    let names = |r: &mocap_capture::PairReport| r.labels.iter().map(|(n, _)| *n).collect::<Vec<_>>();
    assert!(names(&report[0]).contains(&"nose") && !names(&report[0]).contains(&"left_knee"), "{:?}", report[0]);
    assert!(names(&report[1]).contains(&"left_knee") && !names(&report[1]).contains(&"nose"), "{:?}", report[1]);
    assert!(names(&report[2]).is_empty(), "the head-and-arms camera and the legs camera share nothing: {:?}", report[2]);
    assert!(report[0].correspondences > 0 && report[2].correspondences == 0);
}

#[test]
fn a_failure_says_what_the_cameras_shared_and_why_it_was_not_enough() {
    // A camera that sees the subject, but only keypoints the others never see together with it.
    let truth = truth_cameras(3);
    let qualities = vec![Quality::webcam(), Quality::webcam(), Quality::webcam().seeing(|k| k == 0 || k == 15)];
    let (result, _) = calibrate_real(&truth, &qualities, 90.0);
    let message = result.expect_err("a camera that shares almost nothing cannot be placed").to_string();
    eprintln!("failure message: {message}");
    assert!(message.contains("camera:2"), "{message}");
    assert!(message.contains("shared") || message.contains("never saw"), "the message names what was shared: {message}");
}

#[test]
fn a_poor_fit_names_the_camera_that_fits_worst() {
    // One camera's detections are noise: the others agree with each other, it does not.
    let truth = truth_cameras(4);
    let mut qualities = vec![Quality::webcam(); 4];
    qualities[2] = Quality { sigma: 40.0, outliers: 0.5, ..Quality::webcam() };
    let (result, _) = calibrate_real(&truth, &qualities, 90.0);
    match result {
        Err(CalibrationError::PoorFit { worst, .. }) => assert!(worst.contains("camera:2"), "{worst}"),
        Ok(rig) => assert!(rig.rms_px < 8.0, "if it solves it must fit: {}", rig.rms_px),
        Err(other) => panic!("{other}"),
    }
}

#[test]
fn a_nearly_still_subject_seen_by_the_torso_only_gets_an_actionable_message() {
    // The situation of a real failed calibration: two cameras that share only head and torso
    // keypoints (nose, ears, shoulders, hips) and a subject who barely moves between samples.
    let truth = truth_cameras(2);
    let specs = truth.iter().map(|c| mocap_capture::CameraSpec { id: c.id.clone(), width: c.width, height: c.height }).collect();
    let mut calibrator = mocap_capture::Calibrator::new(specs);
    let quality = Quality { sync_jitter: 0.0, ..Quality::webcam().seeing(|k| matches!(k, 0 | 3 | 4 | 5 | 6 | 11 | 12)) };
    let mut noise = Noise(31);
    for f in 0..6000 {
        // Two almost identical poses, alternating: enough change to be kept, far too little to solve from.
        let t = 3.0 + if f % 2 == 0 { 0.0 } else { 0.05 };
        let views: Vec<Option<Vec<[f32; 3]>>> = truth.iter().map(|c| observe_real(c, t, &mut noise, &quality)).collect();
        let refs: Vec<Option<&[[f32; 3]]>> = views.iter().map(|v| v.as_deref()).collect();
        calibrator.offer(&refs);
    }
    let report = calibrator.shared_report();
    eprintln!("still subject: {} samples, motion {:.0} px", calibrator.samples(), report[0].motion_px);
    assert!(report[0].motion_px < 40.0, "{report:?}");
    if calibrator.samples() >= mocap_capture::MIN_SAMPLES {
        let message = calibrator.solve(1.0).expect_err("a still torso cannot be solved from").to_string();
        eprintln!("{message}");
        assert!(message.contains("barely moved"), "{message}");
        assert!(message.contains("head and torso"), "{message}");
    } else {
        assert!(matches!(calibrator.solve(1.0), Err(CalibrationError::TooFewSamples { .. })));
    }
}
