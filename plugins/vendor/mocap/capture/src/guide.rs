//! The guided calibration: a sequence of known movements, each detected from the
//! subject's own keypoints, so the calibrator gets the diversity it needs and the
//! result can be checked against what the subject was asked to do.
//!
//! Features are measured in torso lengths (mid-shoulder to mid-hip), so they do not
//! depend on how far the subject stands from a camera. They are computed from
//! the one camera that sees the subject best; a keypoint under the confidence
//! threshold makes its features unavailable rather than wrong.

/// A keypoint under this confidence is not used to judge a movement. Lower than the
/// calibrator's own cutoff: a raised, blurred wrist often scores 0.2 to 0.4, and the guide
/// only needs to see that the movement is happening, not measure it precisely.
const GUIDE_MIN_SCORE: f32 = 0.25;

/// One movement the subject is asked to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Neutral,
    RaiseLeftHand,
    RaiseRightHand,
    ArmsOut,
    SwingArms,
    WalkInPlace,
    JogInPlace,
    TurnAround,
}

impl Step {
    /// Every step, in order.
    pub const ALL: [Step; 8] = [
        Step::Neutral,
        Step::RaiseLeftHand,
        Step::RaiseRightHand,
        Step::ArmsOut,
        Step::SwingArms,
        Step::WalkInPlace,
        Step::JogInPlace,
        Step::TurnAround,
    ];

    /// What to tell the subject.
    pub fn instruction(self) -> &'static str {
        match self {
            Step::Neutral => "Stand in the middle of the capture area, arms relaxed at your sides",
            Step::RaiseLeftHand => "Raise your LEFT hand (the one on your left side) up high, above your head if you can, and hold it",
            Step::RaiseRightHand => "Raise your RIGHT hand (the one on your right side) up high, above your head if you can, and hold it",
            Step::ArmsOut => "Hold both arms straight out to the sides (a T)",
            Step::SwingArms => "Swing both arms forward and back at your waist",
            Step::WalkInPlace => "Walk in place",
            Step::JogInPlace => "Jog in place",
            Step::TurnAround => "Slowly turn all the way around",
        }
    }

    /// A short name for the checklist.
    pub fn name(self) -> &'static str {
        match self {
            Step::Neutral => "Stand still",
            Step::RaiseLeftHand => "Left hand up",
            Step::RaiseRightHand => "Right hand up",
            Step::ArmsOut => "Arms out",
            Step::SwingArms => "Swing arms",
            Step::WalkInPlace => "Walk in place",
            Step::JogInPlace => "Jog in place",
            Step::TurnAround => "Turn around",
        }
    }

    /// A stable number to tag samples with.
    pub fn tag(self) -> u32 {
        Step::ALL.iter().position(|s| *s == self).unwrap_or(0) as u32
    }
}

/// What the subject's pose looks like, in torso lengths.
#[derive(Clone, Copy, Debug, Default)]
pub struct Features {
    /// Wrist above shoulder (positive is up), per hand.
    pub left_up: Option<f32>,
    pub right_up: Option<f32>,
    /// Wrist above the nose (positive is above), per hand: a hand held overhead.
    pub left_over_head: Option<f32>,
    pub right_over_head: Option<f32>,
    /// Wrist's horizontal distance from its shoulder.
    pub left_out: Option<f32>,
    pub right_out: Option<f32>,
    /// Distance between the wrists.
    pub wrists_apart: Option<f32>,
    /// Vertical position of each wrist and knee in the image, in torso lengths. The subject
    /// stays in place, so only the swing matters.
    pub left_wrist: Option<f32>,
    pub right_wrist: Option<f32>,
    pub left_knee: Option<f32>,
    pub right_knee: Option<f32>,
    /// Shoulder width: large facing the camera, small side-on.
    pub shoulders: Option<f32>,
}

impl Features {
    /// Measures `points` (COCO-17 `(x, y, score)` image fractions) of a frame of
    /// width over height `aspect`. Only the keypoints a feature needs have to be seen:
    /// the scale is the torso when visible, else any bone that is (a shoulder width, an
    /// arm, a thigh), and a feature whose keypoints are missing is `None`. `None`
    /// overall only when no bone at all gives a scale.
    pub fn measure(points: &[[f32; 3]], aspect: f32) -> Option<Self> {
        let at = |i: usize| points.get(i).filter(|p| p[0].is_finite() && p[1].is_finite() && p[2] >= GUIDE_MIN_SCORE).map(|p| [p[0] * aspect, p[1]]);
        let dist = |a: usize, b: usize| Some(((at(a)?[0] - at(b)?[0]).powi(2) + (at(a)?[1] - at(b)?[1]).powi(2)).sqrt());
        let mid = |a: usize, b: usize| Some([(at(a)?[0] + at(b)?[0]) * 0.5, (at(a)?[1] + at(b)?[1]) * 0.5]);
        // The unit of length: a torso, or a bone that stands in for one (its usual ratio to a torso).
        let torso = mid(5, 6).zip(mid(11, 12)).map(|(s, h)| ((s[0] - h[0]).powi(2) + (s[1] - h[1]).powi(2)).sqrt());
        let scale = torso
            .or(dist(5, 11))
            .or(dist(6, 12))
            .or(dist(5, 6).map(|d| d * 1.4))
            .or(dist(5, 7).map(|d| d * 1.7))
            .or(dist(6, 8).map(|d| d * 1.7))
            .or(dist(7, 9).map(|d| d * 1.9))
            .or(dist(8, 10).map(|d| d * 1.9))
            .or(dist(11, 13).map(|d| d * 1.2))
            .or(dist(12, 14).map(|d| d * 1.2))
            .or(dist(11, 12).map(|d| d * 2.4))
            .filter(|l| *l >= 0.02)?;
        // A wrist above its shoulder, or (shoulder unseen) above the nose, which is about
        // half a torso higher than the shoulder line.
        let up = |wrist: usize, shoulder: usize| Some(match at(shoulder) {
            Some(s) => (s[1] - at(wrist)?[1]) / scale,
            None => (at(0)?[1] - at(wrist)?[1]) / scale + 0.46,
        });
        let over_head = |wrist: usize| Some((at(0)?[1] - at(wrist)?[1]) / scale);
        let out = |wrist: usize, shoulder: usize| Some((at(wrist)?[0] - at(shoulder)?[0]).abs() / scale);
        let height = |joint: usize| Some(at(joint)?[1] / scale);
        let shoulder_width = dist(5, 6).map(|d| d / scale).or(dist(11, 12).map(|d| d * 1.7 / scale));
        Some(Self {
            left_up: up(9, 5),
            right_up: up(10, 6),
            left_over_head: over_head(9),
            right_over_head: over_head(10),
            left_out: out(9, 5),
            right_out: out(10, 6),
            wrists_apart: dist(9, 10).map(|d| d / scale),
            left_wrist: height(9),
            right_wrist: height(10),
            left_knee: height(13),
            right_knee: height(14),
            shoulders: shoulder_width,
        })
    }

    /// One reading from several cameras' views of the subject: each feature comes from the
    /// view that sees the most, so no camera has to see the whole body (one may see only
    /// the arms and another only the legs).
    pub fn measure_all(views: &[(&[[f32; 3]], f32)]) -> Option<Self> {
        let mut all: Vec<Self> = views.iter().filter_map(|(points, aspect)| Self::measure(points, *aspect)).collect();
        all.sort_by_key(|f| std::cmp::Reverse(f.known()));
        let pick = |get: fn(&Self) -> Option<f32>| all.iter().find_map(get);
        (!all.is_empty()).then(|| Self {
            left_up: pick(|f| f.left_up),
            right_up: pick(|f| f.right_up),
            left_over_head: pick(|f| f.left_over_head),
            right_over_head: pick(|f| f.right_over_head),
            left_out: pick(|f| f.left_out),
            right_out: pick(|f| f.right_out),
            wrists_apart: pick(|f| f.wrists_apart),
            left_wrist: pick(|f| f.left_wrist),
            right_wrist: pick(|f| f.right_wrist),
            left_knee: pick(|f| f.left_knee),
            right_knee: pick(|f| f.right_knee),
            shoulders: pick(|f| f.shoulders),
        })
    }

    /// How many features are known.
    fn known(&self) -> usize {
        [self.left_up, self.right_up, self.left_over_head, self.right_over_head, self.left_out, self.right_out, self.wrists_apart, self.left_wrist, self.right_wrist, self.left_knee, self.right_knee, self.shoulders]
            .iter()
            .flatten()
            .count()
    }
}

/// How the guide is doing.
#[derive(Clone, Debug, PartialEq)]
pub enum GuideState {
    /// Waiting for (or watching) a step.
    Step { step: Step, progress: f32, detected: bool },
    /// Every step is done.
    Finished,
}

/// A recent window of one signal.
#[derive(Default)]
struct Window {
    samples: Vec<(f64, f32)>,
}

impl Window {
    fn push(&mut self, t: f64, v: f32, seconds: f64) {
        self.samples.push((t, v));
        self.samples.retain(|(s, _)| t - s <= seconds);
    }

    fn range(&self) -> f32 {
        let (lo, hi) = self.samples.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), (_, v)| (lo.min(*v), hi.max(*v)));
        if lo > hi { 0.0 } else { hi - lo }
    }

    /// Half-cycles: how many times the signal crosses its mean by more than a hysteresis band.
    fn half_cycles(&self) -> usize {
        let n = self.samples.len();
        if n < 4 {
            return 0;
        }
        let mean = self.samples.iter().map(|(_, v)| v).sum::<f32>() / n as f32;
        let band = self.range() * 0.2;
        let (mut count, mut side) = (0, 0i8);
        for (_, v) in &self.samples {
            let now = if *v > mean + band { 1 } else if *v < mean - band { -1 } else { 0 };
            if now != 0 && now != side {
                count += usize::from(side != 0);
                side = now;
            }
        }
        count
    }

    fn duration(&self) -> f64 {
        match (self.samples.first(), self.samples.last()) {
            (Some(a), Some(b)) => b.0 - a.0,
            _ => 0.0,
        }
    }
}

/// Seconds a pose must be held.
const HOLD_S: f64 = 0.7;
/// A pose is still being held through a gap in detection this short (a detector flickers).
const GRACE_S: f64 = 0.4;
/// A raised hand counts when its wrist is this many torso lengths above its shoulder.
const HAND_UP: f32 = 0.6;
/// Seconds a motion is judged over.
const WINDOW_S: f64 = 3.0;

/// Walks the subject through the steps.
pub struct Guide {
    index: usize,
    skipped: Vec<Step>,
    held_since: Option<f64>,
    a: Window,
    b: Window,
    /// Turn tracking: widest and narrowest shoulder width seen, and the peaks and valleys passed.
    turn: TurnTrack,
    progress: f32,
    detected: bool,
    /// When the pose was last seen, to ride out a flicker.
    last_present: f64,
    /// What the subject should do differently, or how it is going.
    hint: String,
}

#[derive(Default)]
struct TurnTrack {
    max: f32,
    last: Option<f32>,
    mode: i8,
    peaks: usize,
    valleys: usize,
}

impl Default for Guide {
    fn default() -> Self {
        Self::new()
    }
}

impl Guide {
    /// A guide at the first step.
    pub fn new() -> Self {
        Self { index: 0, skipped: Vec::new(), held_since: None, a: Window::default(), b: Window::default(), turn: TurnTrack::default(), progress: 0.0, detected: false, last_present: f64::NEG_INFINITY, hint: String::new() }
    }

    /// Why the step is not completing yet, or how it is going: for the screen.
    pub fn hint(&self) -> &str {
        &self.hint
    }

    /// The step in progress.
    pub fn current(&self) -> Option<Step> {
        Step::ALL.get(self.index).copied()
    }

    /// Steps done, in order (skipped ones excluded).
    pub fn completed(&self) -> Vec<Step> {
        Step::ALL.iter().take(self.index).filter(|s| !self.skipped.contains(s)).copied().collect()
    }

    /// Where the guide is.
    pub fn state(&self) -> GuideState {
        match self.current() {
            Some(step) => GuideState::Step { step, progress: self.progress, detected: self.detected },
            None => GuideState::Finished,
        }
    }

    /// True while the subject is doing the current step (samples are wanted).
    pub fn collecting(&self) -> bool {
        self.detected
    }

    /// Gives up on the current step.
    pub fn skip(&mut self) {
        if let Some(step) = self.current() {
            self.skipped.push(step);
            self.next();
        }
    }

    fn next(&mut self) {
        self.index += 1;
        self.held_since = None;
        self.a = Window::default();
        self.b = Window::default();
        self.turn = TurnTrack::default();
        self.progress = 0.0;
        self.detected = false;
        self.last_present = f64::NEG_INFINITY;
        self.hint.clear();
    }

    /// Feeds one measurement (or `None` when nobody is seen) at time `t`.
    /// Returns the step that just completed, if one did.
    pub fn update(&mut self, t: f64, features: Option<&Features>) -> Option<Step> {
        let step = self.current()?;
        let Some(f) = features else {
            self.detected = false;
            // A gap in detection is ridden out for a moment; a long one restarts the hold.
            if t - self.last_present > GRACE_S {
                self.held_since = None;
                self.progress = 0.0;
            }
            self.hint = "I can't see you: stand where the cameras can see your upper body".to_owned();
            return None;
        };
        let done = match step {
            Step::Neutral => self.hold(t, f.left_up.zip(f.right_up).is_some_and(|(l, r)| l < -0.2 && r < -0.2)),
            Step::RaiseLeftHand => self.raise_hand(t, "left", (f.left_up, f.left_over_head), f.right_up),
            Step::RaiseRightHand => self.raise_hand(t, "right", (f.right_up, f.right_over_head), f.left_up),
            Step::ArmsOut => {
                let level = f.left_up.zip(f.right_up).is_none_or(|(l, r)| l.abs() < 0.45 && r.abs() < 0.45);
                let wide = f.wrists_apart.is_some_and(|a| a > 2.0) || f.left_out.zip(f.right_out).is_some_and(|(l, r)| l > 0.7 && r > 0.7);
                self.hold(t, wide && level)
            }
            Step::SwingArms => self.motion(t, f.left_wrist, f.right_wrist, 0.45, 4, 0.0),
            Step::WalkInPlace => self.motion(t, f.left_knee, f.right_knee, 0.22, 5, 0.0),
            Step::JogInPlace => self.motion(t, f.left_knee, f.right_knee, 0.40, 7, 2.2),
            Step::TurnAround => self.turn_around(f.shoulders),
        };
        if done {
            self.next();
            return Some(step);
        }
        None
    }

    /// A hand raised (above its shoulder, or above the head) and held, with a hint when it is
    /// not yet working: which hand, how high, or that the wrist cannot be seen.
    fn raise_hand(&mut self, t: f64, side: &str, mine: (Option<f32>, Option<f32>), other_up: Option<f32>) -> bool {
        let (up, over_head) = mine;
        let raised = up.is_some_and(|u| u > HAND_UP) || over_head.is_some_and(|h| h > -0.05);
        let other_raised = other_up.is_some_and(|u| u > HAND_UP);
        self.hint = match (raised, up.or(over_head), other_raised) {
            (true, ..) => "Good: hold it there".to_owned(),
            (false, None, _) if other_raised => format!("That is the other hand: raise your {} hand (the one on your {side})", side.to_uppercase()),
            (false, None, _) => format!("I can't see your {side} wrist clearly: keep your {side} arm in view of a camera"),
            (false, Some(_), true) => format!("That is the other hand: raise your {} hand (the one on your {side})", side.to_uppercase()),
            (false, Some(u), false) => format!("Raise your {side} hand higher: {:.0}% of the way", (u / HAND_UP * 100.0).clamp(0.0, 99.0)),
        };
        self.hold(t, raised)
    }

    /// A pose held for [`HOLD_S`].
    fn hold(&mut self, t: f64, present: bool) -> bool {
        self.detected = present;
        if present {
            self.last_present = t;
        } else if t - self.last_present > GRACE_S {
            self.held_since = None;
            self.progress = 0.0;
            return false;
        } else {
            // A flicker: keep what has been held so far.
            return false;
        }
        let since = *self.held_since.get_or_insert(t);
        self.progress = (((t - since) / HOLD_S) as f32).min(1.0);
        t - since >= HOLD_S
    }

    /// Two signals both swinging by `amplitude` torso lengths, with at least
    /// `half_cycles` crossings in the window, at `min_rate` half-cycles per second when given.
    fn motion(&mut self, t: f64, a: Option<f32>, b: Option<f32>, amplitude: f32, half_cycles: usize, min_rate: f64) -> bool {
        let (Some(a), Some(b)) = (a, b) else {
            self.detected = false;
            return false;
        };
        self.a.push(t, a, WINDOW_S);
        self.b.push(t, b, WINDOW_S);
        let swing = self.a.range().min(self.b.range()) >= amplitude;
        let cycles = self.a.half_cycles().min(self.b.half_cycles());
        let rate = if self.a.duration() > 0.5 { cycles as f64 / self.a.duration() } else { 0.0 };
        self.detected = swing && cycles >= 2 && rate >= min_rate * 0.6;
        self.progress = if swing { (cycles as f32 / half_cycles as f32).min(1.0) } else { 0.0 };
        swing && cycles >= half_cycles && rate >= min_rate
    }

    /// A full turn: the shoulders go wide and narrow twice each.
    fn turn_around(&mut self, shoulders: Option<f32>) -> bool {
        let Some(w) = shoulders else {
            self.detected = false;
            return false;
        };
        self.turn.max = self.turn.max.max(w);
        let (wide, narrow) = (self.turn.max * 0.8, self.turn.max * 0.45);
        let mode = if w >= wide { 1 } else if w <= narrow { -1 } else { 0 };
        if mode != 0 && mode != self.turn.mode {
            if self.turn.mode != 0 || self.turn.last.is_some() {
                if mode == 1 {
                    self.turn.peaks += 1;
                } else {
                    self.turn.valleys += 1;
                }
            }
            self.turn.mode = mode;
        }
        self.turn.last = Some(w);
        self.detected = self.turn.last.is_some() && self.turn.max > 0.2;
        self.progress = ((self.turn.peaks + self.turn.valleys) as f32 / 4.0).min(1.0);
        self.turn.valleys >= 2 && self.turn.peaks >= 2
    }
}
