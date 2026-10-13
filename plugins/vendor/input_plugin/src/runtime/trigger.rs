//! Trigger state machines: given whether a value is actuated this frame,
//! decide whether the trigger is firing, building toward firing, or idle.

use crate::model::behavior::Trigger;

/// What a trigger says this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TriggerState {
    /// Nothing is happening.
    None,
    /// Building toward firing (holding, waiting for the second tap).
    Ongoing,
    /// Firing.
    Triggered,
}

/// One trigger and what it remembers between frames.
#[derive(Clone, Debug)]
pub struct TriggerRt {
    kind: Trigger,
    /// How long the value has been actuated.
    held: f32,
    prev_actuated: bool,
    /// Hold: fired already this press. Tap: held too long to count.
    flag: bool,
    count: u32,
    timer: f32,
    /// Double tap: time since the first press, while waiting for the second.
    armed: Option<f32>,
}

/// Floor for intervals, so a 0 can't fire a pulse many times in a frame.
const MIN_INTERVAL: f32 = 0.001;

impl TriggerRt {
    /// `None` for [`Trigger::Chorded`], which is a gate and not a state
    /// machine.
    pub fn new(kind: Trigger) -> Option<Self> {
        if matches!(kind, Trigger::Chorded { .. }) {
            return None;
        }
        Some(Self { kind, held: 0.0, prev_actuated: false, flag: false, count: 0, timer: 0.0, armed: None })
    }

    pub fn update(&mut self, actuated: bool, dt: f32) -> TriggerState {
        use TriggerState::*;
        let pressed = actuated && !self.prev_actuated;
        let released = !actuated && self.prev_actuated;
        let state = match self.kind {
            Trigger::Down => {
                if actuated {
                    Triggered
                } else {
                    None
                }
            }
            Trigger::Pressed => {
                if pressed {
                    Triggered
                } else {
                    None
                }
            }
            Trigger::Released => {
                if released {
                    Triggered
                } else if actuated {
                    Ongoing
                } else {
                    None
                }
            }
            Trigger::Hold { seconds, once } => {
                if actuated {
                    self.held += dt;
                    if self.held >= seconds {
                        if once && self.flag {
                            None
                        } else {
                            self.flag = true;
                            Triggered
                        }
                    } else {
                        Ongoing
                    }
                } else {
                    self.held = 0.0;
                    self.flag = false;
                    None
                }
            }
            Trigger::HoldAndRelease { seconds } => {
                if actuated {
                    self.held += dt;
                    Ongoing
                } else {
                    let fired = released && self.held >= seconds;
                    self.held = 0.0;
                    if fired {
                        Triggered
                    } else {
                        None
                    }
                }
            }
            Trigger::Tap { max_seconds } => {
                if actuated {
                    self.held += dt;
                    if self.held > max_seconds {
                        self.flag = true;
                    }
                    if self.flag {
                        None
                    } else {
                        Ongoing
                    }
                } else {
                    let fired = released && !self.flag && self.held <= max_seconds;
                    self.held = 0.0;
                    self.flag = false;
                    if fired {
                        Triggered
                    } else {
                        None
                    }
                }
            }
            Trigger::Pulse { interval, limit, on_start } => {
                if actuated {
                    if pressed {
                        self.count = 0;
                        self.timer = 0.0;
                        if on_start {
                            self.count = 1;
                            Triggered
                        } else {
                            Ongoing
                        }
                    } else {
                        self.timer += dt;
                        let interval = interval.max(MIN_INTERVAL);
                        if self.timer >= interval {
                            // At most one pulse a frame, however long the frame.
                            self.timer %= interval;
                            if limit == 0 || self.count < limit {
                                self.count += 1;
                                Triggered
                            } else {
                                Ongoing
                            }
                        } else {
                            Ongoing
                        }
                    }
                } else {
                    self.count = 0;
                    self.timer = 0.0;
                    None
                }
            }
            Trigger::DoubleTap { window } => {
                if let Some(elapsed) = self.armed.as_mut() {
                    *elapsed += dt;
                    if *elapsed > window {
                        self.armed = Option::None;
                    }
                }
                if pressed {
                    if self.armed.take().is_some() {
                        Triggered
                    } else {
                        self.armed = Some(0.0);
                        Ongoing
                    }
                } else if self.armed.is_some() {
                    Ongoing
                } else {
                    None
                }
            }
            Trigger::Chorded { .. } => None,
        };
        self.prev_actuated = actuated;
        state
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use TriggerState::*;

    /// Feed one actuation pattern at `dt` per frame; return each frame's state.
    fn run(trigger: Trigger, dt: f32, frames: &[bool]) -> Vec<TriggerState> {
        let mut t = TriggerRt::new(trigger).unwrap();
        frames.iter().map(|&a| t.update(a, dt)).collect()
    }

    const U: bool = false;
    const D: bool = true;

    #[test]
    fn down_follows_the_input() {
        assert_eq!(run(Trigger::Down, 0.1, &[U, D, D, U]), [None, Triggered, Triggered, None]);
    }

    #[test]
    fn pressed_fires_on_the_edge_only_and_again_after_release() {
        assert_eq!(run(Trigger::Pressed, 0.1, &[D, D, D, U, D]), [Triggered, None, None, None, Triggered]);
    }

    #[test]
    fn released_fires_when_the_input_lets_go() {
        assert_eq!(run(Trigger::Released, 0.1, &[D, D, U, U]), [Ongoing, Ongoing, Triggered, None]);
    }

    #[test]
    fn hold_builds_then_fires_every_frame_or_once() {
        let hold = Trigger::Hold { seconds: 0.25, once: false };
        assert_eq!(run(hold, 0.1, &[D, D, D, D, U]), [Ongoing, Ongoing, Triggered, Triggered, None]);
        let once = Trigger::Hold { seconds: 0.25, once: true };
        assert_eq!(run(once.clone(), 0.1, &[D, D, D, D, D]), [Ongoing, Ongoing, Triggered, None, None]);
        assert_eq!(run(once, 0.1, &[D, D, D, U, D, D, D]), [Ongoing, Ongoing, Triggered, None, Ongoing, Ongoing, Triggered], "fires again on the next hold");
    }

    #[test]
    fn hold_that_lets_go_early_never_fires() {
        let hold = Trigger::Hold { seconds: 0.5, once: false };
        assert!(!run(hold, 0.1, &[D, D, U, D, D]).contains(&Triggered));
    }

    #[test]
    fn hold_and_release_fires_only_on_release_after_long_enough() {
        let t = Trigger::HoldAndRelease { seconds: 0.25 };
        assert_eq!(run(t.clone(), 0.1, &[D, D, D, U, U]), [Ongoing, Ongoing, Ongoing, Triggered, None]);
        assert_eq!(run(t, 0.1, &[D, U, U]), [Ongoing, None, None], "released too early");
    }

    #[test]
    fn tap_fires_on_a_quick_release_but_not_after_a_long_hold() {
        let t = Trigger::Tap { max_seconds: 0.25 };
        assert_eq!(run(t.clone(), 0.1, &[D, D, U]), [Ongoing, Ongoing, Triggered]);
        let long = run(t.clone(), 0.1, &[D, D, D, D, U]);
        assert!(!long.contains(&Triggered), "{long:?}");
        assert_eq!(run(t, 0.1, &[D, D, D, D, U, D, U]).last(), Some(&Triggered), "a later quick tap works again");
    }

    #[test]
    fn pulse_fires_at_its_interval_up_to_the_limit() {
        let p = Trigger::Pulse { interval: 0.2, limit: 3, on_start: true };
        let states = run(p, 0.1, &[D; 12]);
        assert_eq!(states.iter().filter(|s| **s == Triggered).count(), 3);
        assert_eq!(states[0], Triggered, "on_start fires at once");
        assert_eq!(states[1], Ongoing);
        assert_eq!(states[2], Triggered);
        let lazy = Trigger::Pulse { interval: 0.2, limit: 0, on_start: false };
        let states = run(lazy, 0.1, &[D; 6]);
        assert_eq!(states, [Ongoing, Ongoing, Triggered, Ongoing, Triggered, Ongoing]);
    }

    #[test]
    fn a_long_frame_gives_one_pulse_not_a_burst() {
        let p = Trigger::Pulse { interval: 0.01, limit: 0, on_start: false };
        let mut t = TriggerRt::new(p).unwrap();
        t.update(D, 0.0);
        assert_eq!(t.update(D, 1.0), Triggered);
        assert_eq!(t.update(D, 0.0), Ongoing);
    }

    #[test]
    fn pulse_starts_over_after_a_release() {
        let p = Trigger::Pulse { interval: 0.2, limit: 1, on_start: true };
        let states = run(p, 0.1, &[D, D, D, U, D]);
        assert_eq!(states[0], Triggered);
        assert_eq!(states[4], Triggered);
    }

    #[test]
    fn double_tap_needs_the_second_press_inside_the_window() {
        let t = Trigger::DoubleTap { window: 0.35 };
        assert_eq!(run(t.clone(), 0.1, &[D, U, D, U]), [Ongoing, Ongoing, Triggered, None]);
        let slow = run(t, 0.1, &[D, U, U, U, U, D]);
        assert_eq!(slow[5], Ongoing, "too late: this is a new first tap");
    }

    #[test]
    fn chords_are_gates_not_state_machines() {
        assert!(TriggerRt::new(Trigger::Chorded { action: Default::default() }).is_none());
    }
}
