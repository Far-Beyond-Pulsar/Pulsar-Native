//! Showing what an action does before the game runs.
//!
//! [`simulate`] feeds a made-up input (a tap, a hold, a double tap, a slow
//! push of a trigger) through the real evaluator with the action alone
//! bound to one key, and records what the action did at every step. The
//! editor draws the result as a timeline, so a hold time or a dead zone can
//! be judged by looking at it.

use std::collections::HashMap;
use std::sync::Arc;

use crate::keys::{catalog, Pad};
use crate::model::behavior::{magnitude, Value3};
use crate::model::{ActionAsset, ActionRef, MappingAsset};
use crate::runtime::raw::RawInput;
use crate::runtime::system::{ActionLookup, InputSystem, Phase};
use crate::runtime::trigger::TriggerState;

/// A made-up input to try the action with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pattern {
    /// One quick press.
    Tap,
    /// Held for a second.
    Hold,
    /// Two quick presses.
    DoubleTap,
    /// A trigger pulled slowly from rest to full.
    Ramp,
}

impl Pattern {
    pub const ALL: [Pattern; 4] = [Pattern::Tap, Pattern::Hold, Pattern::DoubleTap, Pattern::Ramp];

    pub fn label(self) -> &'static str {
        match self {
            Pattern::Tap => "Tap",
            Pattern::Hold => "Hold",
            Pattern::DoubleTap => "Double tap",
            Pattern::Ramp => "Slow push",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Pattern::Tap => "A key pressed and released after 0.1 seconds",
            Pattern::Hold => "A key held for a second",
            Pattern::DoubleTap => "Two taps, 0.15 seconds apart",
            Pattern::Ramp => "A trigger pulled from rest to full over a second and a half",
        }
    }

    /// How long the preview runs, in seconds.
    pub fn seconds(self) -> f32 {
        match self {
            Pattern::Tap => 1.2,
            Pattern::Hold => 2.0,
            Pattern::DoubleTap => 1.5,
            Pattern::Ramp => 2.0,
        }
    }

    /// The input `t` seconds in.
    fn input(self, t: f32) -> f32 {
        let window = |from: f32, to: f32| if t >= from && t < to { 1.0 } else { 0.0 };
        match self {
            Pattern::Tap => window(0.2, 0.3),
            Pattern::Hold => window(0.2, 1.2),
            Pattern::DoubleTap => window(0.2, 0.3).max(window(0.45, 0.55)),
            Pattern::Ramp => ((t - 0.2) / 1.5).clamp(0.0, 1.0),
        }
    }
}

/// One step of the preview.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    pub time: f32,
    pub input: f32,
    pub value: Value3,
    pub state: TriggerState,
    pub phases: Vec<Phase>,
}

/// Seconds per step.
pub const STEP: f32 = 1.0 / 60.0;

/// What `action` does with `pattern`.
pub fn simulate(action: &ActionAsset, pattern: Pattern) -> Vec<Sample> {
    let mut action = action.clone();
    action.consume_input = false;
    let reference = ActionRef::new(action.id.clone(), "");
    let mut mapping = MappingAsset::new();
    let category = mapping.categories[0].id;
    let key = match pattern {
        Pattern::Ramp => "Gamepad.RightTrigger",
        _ => "Keyboard.Space",
    };
    mapping.add_entry(category, key, Some(reference));
    let mut lookup: HashMap<_, (Arc<ActionAsset>, String)> = HashMap::new();
    lookup.insert(action.id.clone(), (Arc::new(action.clone()), "Preview".to_string()));
    let lookup: &dyn ActionLookup = &lookup;

    let mut system = InputSystem::new();
    system.add(&mapping, lookup, 0, None);
    let mut raw = RawInput::new();
    let space = catalog().index_of("Keyboard.Space").unwrap_or_default();
    let steps = (pattern.seconds() / STEP).round() as usize;
    let mut was_down = false;
    let mut samples = Vec::with_capacity(steps);
    for step in 0..steps {
        let time = step as f32 * STEP;
        let input = pattern.input(time);
        match pattern {
            Pattern::Ramp => raw.pad(Pad::RightTrigger, input),
            _ => {
                let down = input > 0.5;
                if down != was_down {
                    raw.set_button(space, down);
                    was_down = down;
                }
            }
        }
        raw.derive();
        let events = system.update(&raw, STEP, false);
        raw.end_frame();
        let (state, value) = system.value_of(&action.id).unwrap_or((TriggerState::None, [0.0; 3]));
        samples.push(Sample { time, input, value, state, phases: events.iter().map(|e| e.phase).collect() });
    }
    samples
}

/// How large the value gets, for scaling the chart (at least 1).
pub fn peak(samples: &[Sample]) -> f32 {
    samples.iter().map(|s| magnitude(s.value)).fold(0.0, f32::max).max(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::behavior::{Modifier, Tolerant, Trigger};

    fn count(samples: &[Sample], phase: Phase) -> usize {
        samples.iter().flat_map(|s| &s.phases).filter(|p| **p == phase).count()
    }

    #[test]
    fn a_plain_action_fires_for_as_long_as_the_key_is_down() {
        let s = simulate(&ActionAsset::new(), Pattern::Hold);
        assert_eq!(count(&s, Phase::Started), 1);
        assert_eq!(count(&s, Phase::Completed), 1);
        let firing = s.iter().filter(|s| s.state == TriggerState::Triggered).count();
        assert!((55..=65).contains(&firing), "about a second at 60 steps a second: {firing}");
    }

    #[test]
    fn a_hold_trigger_needs_the_hold_and_a_tap_does_not_reach_it() {
        let mut a = ActionAsset::new();
        a.triggers = vec![Tolerant::Known(Trigger::Hold { seconds: 0.5, once: true })];
        assert_eq!(count(&simulate(&a, Pattern::Hold), Phase::Triggered), 1);
        assert_eq!(count(&simulate(&a, Pattern::Tap), Phase::Triggered), 0);
        assert_eq!(count(&simulate(&a, Pattern::Tap), Phase::Canceled), 1, "a tap on a hold action is a cancel");
    }

    #[test]
    fn a_double_tap_trigger_fires_only_for_two_taps() {
        let mut a = ActionAsset::new();
        a.triggers = vec![Tolerant::Known(Trigger::DoubleTap { window: 0.4 })];
        assert_eq!(count(&simulate(&a, Pattern::DoubleTap), Phase::Triggered), 1);
        assert_eq!(count(&simulate(&a, Pattern::Tap), Phase::Triggered), 0);
    }

    #[test]
    fn a_dead_zone_delays_a_slow_push() {
        let mut a = ActionAsset::new();
        a.value_type = crate::model::ValueType::Axis1D;
        a.actuation_threshold = 0.05;
        let plain = simulate(&a, Pattern::Ramp);
        a.modifiers = vec![Tolerant::Known(Modifier::DeadZone { lower: 0.5, upper: 1.0, radial: false })];
        let dead = simulate(&a, Pattern::Ramp);
        let first = |s: &[Sample]| s.iter().position(|s| s.state == TriggerState::Triggered).unwrap();
        assert!(first(&dead) > first(&plain) + 20, "the dead zone swallows the first half of the push");
        let end = dead.last().unwrap().value[0];
        assert!((end - 1.0).abs() < 0.01, "and still reaches full: {end}");
    }

    #[test]
    fn every_pattern_runs_and_reports_its_length() {
        for pattern in Pattern::ALL {
            let s = simulate(&ActionAsset::new(), pattern);
            assert_eq!(s.len(), (pattern.seconds() / STEP).round() as usize);
            assert!(s.windows(2).all(|w| w[1].time > w[0].time));
        }
    }
}
