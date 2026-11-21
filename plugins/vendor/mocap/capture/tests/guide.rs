//! The guided calibration: each requested movement is detected from keypoints
//! alone, and nothing else advances it.

use mocap_capture::{Features, Guide, GuideState, Step};
use std::f64::consts::TAU;

/// A person facing the camera in image fractions (square frame), torso 0.3 tall.
struct Person {
    left_wrist: [f32; 2],
    right_wrist: [f32; 2],
    left_knee: [f32; 2],
    right_knee: [f32; 2],
    half_shoulders: f32,
}

impl Default for Person {
    fn default() -> Self {
        Self { left_wrist: [0.40, 0.60], right_wrist: [0.60, 0.60], left_knee: [0.46, 0.80], right_knee: [0.54, 0.80], half_shoulders: 0.05 }
    }
}

impl Person {
    fn keypoints(&self) -> Vec<[f32; 3]> {
        let mut k = vec![[f32::NAN, f32::NAN, 0.0]; 17];
        let mut put = |i: usize, p: [f32; 2]| k[i] = [p[0], p[1], 0.9];
        put(5, [0.5 - self.half_shoulders, 0.35]);
        put(6, [0.5 + self.half_shoulders, 0.35]);
        put(11, [0.46, 0.65]);
        put(12, [0.54, 0.65]);
        put(9, self.left_wrist);
        put(10, self.right_wrist);
        put(13, self.left_knee);
        put(14, self.right_knee);
        k
    }

    fn features(&self) -> Option<Features> {
        Features::measure(&self.keypoints(), 1.0)
    }
}

const FPS: f64 = 30.0;

/// Feeds `seconds` of `scene(t)` to the guide; returns the steps completed.
fn run(guide: &mut Guide, start: f64, seconds: f64, scene: impl Fn(f64) -> Option<Person>) -> Vec<Step> {
    let mut done = Vec::new();
    for i in 0..(seconds * FPS) as usize {
        let t = start + i as f64 / FPS;
        let features = scene(t).and_then(|p| p.features());
        done.extend(guide.update(t, features.as_ref()));
    }
    done
}

fn at_step(step: Step) -> Guide {
    let mut guide = Guide::new();
    while guide.current() != Some(step) {
        guide.skip();
    }
    guide
}

#[test]
fn features_are_measured_in_torso_lengths_and_fall_back_to_whatever_bones_are_seen() {
    let f = Person { left_wrist: [0.40, 0.05], ..Person::default() }.features().expect("features");
    assert!((f.left_up.expect("up") - 1.0).abs() < 0.01, "wrist a torso above the shoulder");
    assert!(f.right_up.expect("up") < -0.5);
    assert!((f.shoulders.expect("width") - 0.333).abs() < 0.01);
    let mut k = Person::default().keypoints();
    k[9][2] = 0.1;
    assert!(Features::measure(&k, 1.0).expect("torso still seen").left_up.is_none(), "an unsure wrist is not guessed");
    k[5][2] = 0.1;
    let side = Features::measure(&k, 1.0).expect("the other side's shoulder-to-hip still gives a scale");
    assert!(side.right_up.is_some() && side.left_up.is_none(), "only what the seen keypoints allow");
    let face_only: Vec<[f32; 3]> = Person::default().keypoints().into_iter().enumerate().map(|(i, p)| if i <= 4 { p } else { [f32::NAN, f32::NAN, 0.0] }).collect();
    assert!(Features::measure(&face_only, 1.0).is_none(), "no bone to scale by, no features");
}

#[test]
fn standing_still_completes_the_first_step_after_a_hold() {
    let mut guide = Guide::new();
    assert!(run(&mut guide, 0.0, 0.6, |_| Some(Person::default())).is_empty(), "not yet");
    let done = run(&mut guide, 0.6, 1.0, |_| Some(Person::default()));
    assert_eq!(done, vec![Step::Neutral]);
    assert_eq!(guide.current(), Some(Step::RaiseLeftHand));
}

#[test]
fn a_raised_hand_counts_only_for_the_hand_that_was_asked_for() {
    let mut guide = at_step(Step::RaiseLeftHand);
    let right_up = |_t: f64| Some(Person { right_wrist: [0.60, 0.05], ..Person::default() });
    assert!(run(&mut guide, 0.0, 3.0, right_up).is_empty(), "the wrong hand does not count");
    assert!(matches!(guide.state(), GuideState::Step { step: Step::RaiseLeftHand, detected: false, .. }));
    let done = run(&mut guide, 3.0, 1.5, |_| Some(Person { left_wrist: [0.40, 0.05], ..Person::default() }));
    assert_eq!(done, vec![Step::RaiseLeftHand]);
    let done = run(&mut guide, 5.0, 1.5, right_up);
    assert_eq!(done, vec![Step::RaiseRightHand]);
}

#[test]
fn arms_held_out_to_the_sides_complete_the_t_step() {
    let mut guide = at_step(Step::ArmsOut);
    let down = run(&mut guide, 0.0, 2.0, |_| Some(Person::default()));
    assert!(down.is_empty(), "arms down is not a T");
    let t_pose = |_t: f64| Some(Person { left_wrist: [0.20, 0.35], right_wrist: [0.80, 0.35], ..Person::default() });
    assert_eq!(run(&mut guide, 2.0, 1.5, t_pose), vec![Step::ArmsOut]);
}

#[test]
fn swinging_walking_and_jogging_are_told_apart_from_standing_and_from_each_other() {
    let swing = |t: f64| {
        let a = 0.12 * (TAU * 1.2 * t).sin() as f32;
        Some(Person { left_wrist: [0.40, 0.55 + a], right_wrist: [0.60, 0.55 - a], ..Person::default() })
    };
    let walk = |hz: f64, amp: f32| {
        move |t: f64| {
            let a = amp * (TAU * hz * t).sin() as f32;
            Some(Person { left_knee: [0.46, 0.80 + a], right_knee: [0.54, 0.80 - a], ..Person::default() })
        }
    };
    let mut guide = at_step(Step::SwingArms);
    assert!(run(&mut guide, 0.0, 6.0, |_| Some(Person::default())).is_empty(), "standing is not swinging");
    assert_eq!(run(&mut guide, 6.0, 4.0, swing), vec![Step::SwingArms]);

    // Walking in place (about 1 Hz, modest knee lift) completes walk; a stand does not.
    assert!(run(&mut guide, 10.0, 5.0, |_| Some(Person::default())).is_empty());
    assert_eq!(run(&mut guide, 15.0, 5.0, walk(1.0, 0.08)), vec![Step::WalkInPlace]);
    // Jogging needs bigger and faster knees: the same gentle walk does not complete it.
    assert!(run(&mut guide, 20.0, 5.0, walk(1.0, 0.08)).is_empty(), "walking is not jogging");
    assert_eq!(run(&mut guide, 25.0, 4.0, walk(2.0, 0.13)), vec![Step::JogInPlace]);
}

#[test]
fn a_full_turn_is_two_wide_and_two_narrow_shoulders_and_a_half_turn_is_not_enough() {
    let turning = |turns: f64| {
        move |t: f64| {
            let w = 0.05 * (TAU * turns * t / 6.0).cos().abs() as f32;
            Some(Person { half_shoulders: w.max(0.004), ..Person::default() })
        }
    };
    let mut guide = at_step(Step::TurnAround);
    assert!(run(&mut guide, 0.0, 3.0, turning(0.5)).is_empty(), "half a turn");
    let mut guide = at_step(Step::TurnAround);
    assert_eq!(run(&mut guide, 0.0, 7.0, turning(1.0)), vec![Step::TurnAround]);
    assert_eq!(guide.state(), GuideState::Finished);
}

#[test]
fn losing_the_subject_resets_a_hold_and_a_skipped_step_is_not_completed() {
    let mut guide = at_step(Step::RaiseLeftHand);
    let up = |_t: f64| Some(Person { left_wrist: [0.40, 0.05], ..Person::default() });
    assert!(run(&mut guide, 0.0, 0.5, up).is_empty());
    run(&mut guide, 0.5, 0.6, |_| None);
    assert!(run(&mut guide, 1.1, 0.5, up).is_empty(), "a gap longer than the grace period restarted the hold");
    guide.skip();
    assert_eq!(guide.current(), Some(Step::RaiseRightHand));
    assert!(!guide.completed().contains(&Step::RaiseLeftHand));
}

#[test]
fn the_whole_sequence_runs_through_in_order() {
    let mut guide = Guide::new();
    let mut t = 0.0;
    let mut all = Vec::new();
    let mut stage = |guide: &mut Guide, seconds: f64, scene: &dyn Fn(f64) -> Option<Person>| {
        all.extend(run(guide, t, seconds, scene));
        t += seconds;
    };
    stage(&mut guide, 1.5, &|_| Some(Person::default()));
    stage(&mut guide, 1.5, &|_| Some(Person { left_wrist: [0.40, 0.05], ..Person::default() }));
    stage(&mut guide, 1.5, &|_| Some(Person { right_wrist: [0.60, 0.05], ..Person::default() }));
    stage(&mut guide, 1.5, &|_| Some(Person { left_wrist: [0.20, 0.35], right_wrist: [0.80, 0.35], ..Person::default() }));
    stage(&mut guide, 4.0, &|t| {
        let a = 0.12 * (TAU * 1.2 * t).sin() as f32;
        Some(Person { left_wrist: [0.40, 0.55 + a], right_wrist: [0.60, 0.55 - a], ..Person::default() })
    });
    stage(&mut guide, 5.0, &|t| {
        let a = 0.08 * (TAU * t).sin() as f32;
        Some(Person { left_knee: [0.46, 0.80 + a], right_knee: [0.54, 0.80 - a], ..Person::default() })
    });
    stage(&mut guide, 4.0, &|t| {
        let a = 0.13 * (TAU * 2.0 * t).sin() as f32;
        Some(Person { left_knee: [0.46, 0.80 + a], right_knee: [0.54, 0.80 - a], ..Person::default() })
    });
    stage(&mut guide, 7.0, &|t| Some(Person { half_shoulders: (0.05 * (TAU * t / 6.0).cos().abs() as f32).max(0.004), ..Person::default() }));
    assert_eq!(all, Step::ALL.to_vec());
    assert_eq!(guide.state(), GuideState::Finished);
    assert_eq!(guide.completed().len(), 8);
}

/// A realistic build: shoulders 0.21 apart for a 0.3 torso.
fn broad() -> Person {
    Person { half_shoulders: 0.107, ..Person::default() }
}

/// The keypoints a camera that only sees some of the body would report.
fn seeing(person: &Person, keep: impl Fn(usize) -> bool) -> Vec<[f32; 3]> {
    person.keypoints().into_iter().enumerate().map(|(k, p)| if keep(k) { p } else { [f32::NAN, f32::NAN, 0.0] }).collect()
}

/// Feeds a guide from several cameras' partial views.
fn run_views(guide: &mut Guide, start: f64, seconds: f64, scene: impl Fn(f64) -> Vec<Vec<[f32; 3]>>) -> Vec<Step> {
    let mut done = Vec::new();
    for i in 0..(seconds * FPS) as usize {
        let t = start + i as f64 / FPS;
        let views = scene(t);
        let refs: Vec<(&[[f32; 3]], f32)> = views.iter().map(|v| (v.as_slice(), 1.0)).collect();
        done.extend(guide.update(t, Features::measure_all(&refs).as_ref()));
    }
    done
}

#[test]
fn a_camera_that_sees_only_the_upper_body_can_judge_a_raised_hand_and_arm_swing() {
    let upper = |p: &Person| seeing(p, |k| k <= 10);
    let mut guide = at_step(Step::RaiseLeftHand);
    let done = run_views(&mut guide, 0.0, 1.5, |_| vec![upper(&Person { left_wrist: [0.40, 0.05], ..broad() })]);
    assert_eq!(done, vec![Step::RaiseLeftHand], "no hips or legs in view, and the step still works");

    let mut guide = at_step(Step::SwingArms);
    let swing = |t: f64| {
        let a = 0.12 * (TAU * 1.2 * t).sin() as f32;
        vec![upper(&Person { left_wrist: [0.40, 0.55 + a], right_wrist: [0.60, 0.55 - a], ..broad() })]
    };
    assert_eq!(run_views(&mut guide, 0.0, 4.0, swing), vec![Step::SwingArms]);
}

#[test]
fn two_cameras_that_each_see_part_of_the_body_together_judge_the_movement() {
    // One camera sees only the arms and shoulders, the other only the hips and legs.
    let arms = |p: &Person| seeing(p, |k| (5..=10).contains(&k));
    let legs = |p: &Person| seeing(p, |k| k >= 11);
    let mut guide = at_step(Step::WalkInPlace);
    let walk = |t: f64| {
        let a = 0.08 * (TAU * t).sin() as f32;
        let p = Person { left_knee: [0.46, 0.80 + a], right_knee: [0.54, 0.80 - a], ..broad() };
        vec![arms(&p), legs(&p)]
    };
    assert_eq!(run_views(&mut guide, 0.0, 5.0, walk), vec![Step::WalkInPlace], "the legs camera supplies the knees");
}

#[test]
fn a_subject_seen_by_no_camera_well_enough_does_not_advance_anything() {
    let mut guide = at_step(Step::RaiseLeftHand);
    // Only a face: nothing to scale a movement by.
    let face = |_t: f64| vec![seeing(&broad(), |k| k <= 4)];
    assert!(run_views(&mut guide, 0.0, 3.0, face).is_empty());
}

/// Frames of a person holding the left hand up, where `drop` returns true the wrist is not detected at all.
fn hand_up_with(drop: impl Fn(f64) -> bool, wrist_score: f32, wrist_y: f32) -> impl Fn(f64) -> Vec<Vec<[f32; 3]>> {
    move |t| {
        let mut k = Person { left_wrist: [0.40, wrist_y], ..broad() }.keypoints();
        k[9][2] = if drop(t) { 0.0 } else { wrist_score };
        vec![k]
    }
}

#[test]
fn a_raised_hand_still_counts_when_the_wrist_flickers_in_and_out_of_detection() {
    // The wrist drops out for 0.2 s every second, as a webcam detector's does.
    let flicker = |t: f64| (t % 1.0) > 0.8;
    let mut guide = at_step(Step::RaiseLeftHand);
    let done = run_views(&mut guide, 0.0, 4.0, hand_up_with(flicker, 0.9, 0.05));
    assert_eq!(done, vec![Step::RaiseLeftHand], "gaps shorter than the grace period do not restart the hold");
}

#[test]
fn a_raised_hand_counts_with_the_low_confidence_a_blurred_wrist_gets() {
    let mut guide = at_step(Step::RaiseLeftHand);
    let done = run_views(&mut guide, 0.0, 2.5, hand_up_with(|_| false, 0.3, 0.05));
    assert_eq!(done, vec![Step::RaiseLeftHand], "a wrist scored 0.3 is enough to see the hand is up");
}

#[test]
fn an_ear_level_hand_is_enough_it_does_not_have_to_be_straight_up() {
    // Wrist 0.65 torso lengths above the shoulder (torso 0.3, shoulders at 0.35): y = 0.155.
    let mut guide = at_step(Step::RaiseLeftHand);
    let done = run_views(&mut guide, 0.0, 2.5, hand_up_with(|_| false, 0.9, 0.155));
    assert_eq!(done, vec![Step::RaiseLeftHand]);
}

#[test]
fn a_long_loss_of_the_subject_does_restart_the_hold() {
    let mut guide = at_step(Step::RaiseLeftHand);
    let lost = |t: f64| (0.5..1.5).contains(&t);
    assert!(run_views(&mut guide, 0.0, 1.9, hand_up_with(lost, 0.9, 0.05)).is_empty(), "a full second away restarts it");
    assert_eq!(run_views(&mut guide, 1.9, 1.2, hand_up_with(|_| false, 0.9, 0.05)), vec![Step::RaiseLeftHand]);
}

#[test]
fn the_hint_says_what_to_change() {
    let mut guide = at_step(Step::RaiseLeftHand);
    let step = |guide: &mut Guide, person: Person, wrist_score: f32| {
        let mut k = person.keypoints();
        k[9][2] = wrist_score;
        let refs = [(k.as_slice(), 1.0)];
        guide.update(0.0, Features::measure_all(&refs).as_ref());
    };
    step(&mut guide, broad(), 0.0);
    assert!(guide.hint().contains("can't see your left wrist"), "{}", guide.hint());
    step(&mut guide, Person { left_wrist: [0.40, 0.30], ..broad() }, 0.9);
    assert!(guide.hint().contains("higher"), "{}", guide.hint());
    step(&mut guide, Person { right_wrist: [0.60, 0.05], ..broad() }, 0.0);
    assert!(guide.hint().contains("other hand"), "{}", guide.hint());
    step(&mut guide, Person { left_wrist: [0.40, 0.05], ..broad() }, 0.9);
    assert!(guide.hint().contains("hold it"), "{}", guide.hint());
}
