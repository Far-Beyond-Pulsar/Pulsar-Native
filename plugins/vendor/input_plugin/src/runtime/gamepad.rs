//! Game controllers.
//!
//! Controllers are read on a thread of their own, because polling a device
//! can block and must not cost the game a frame. The thread turns what the
//! controllers report into [`Pad`] changes and leaves them in a queue; the
//! game thread takes them each frame and feeds them to the key state.
//!
//! Every connected controller drives the same keys (the last to change
//! wins), which is what a single-player game wants. A controller that is
//! unplugged lets go of everything it held.
//!
//! Without the `gamepad` feature this is an empty stand-in.

use crate::keys::Pad;
use crate::runtime::raw::RawInput;

/// One change of one control.
pub type PadChange = (Pad, f32);

/// Feed `changes` to `raw`.
pub fn apply(raw: &mut RawInput, changes: impl IntoIterator<Item = PadChange>) {
    for (pad, value) in changes {
        raw.pad(pad, value);
    }
}

#[cfg(feature = "gamepad")]
mod imp {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;
    use std::time::Duration;

    use gilrs::{Axis, Button, EventType, Gilrs};

    use super::{Pad, PadChange};

    /// How long the thread waits for a report before checking whether to stop.
    const WAIT: Duration = Duration::from_millis(10);
    /// Changes kept for a frame the game has not collected yet.
    const MAX_QUEUED: usize = 4096;

    /// The controller thread and its queue.
    pub struct Gamepads {
        queue: Arc<Mutex<Vec<PadChange>>>,
        stop: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    }

    /// The control a gilrs button is, if it is one we expose. Analog
    /// triggers are reported separately as values.
    pub fn button(button: Button) -> Option<Pad> {
        Some(match button {
            Button::South => Pad::FaceBottom,
            Button::East => Pad::FaceRight,
            Button::West => Pad::FaceLeft,
            Button::North => Pad::FaceTop,
            Button::LeftTrigger => Pad::LeftShoulder,
            Button::RightTrigger => Pad::RightShoulder,
            Button::LeftTrigger2 => Pad::LeftTrigger,
            Button::RightTrigger2 => Pad::RightTrigger,
            Button::Select => Pad::Select,
            Button::Start => Pad::Start,
            Button::Mode => Pad::Mode,
            Button::LeftThumb => Pad::LeftThumb,
            Button::RightThumb => Pad::RightThumb,
            Button::DPadUp => Pad::DpadUp,
            Button::DPadDown => Pad::DpadDown,
            Button::DPadLeft => Pad::DpadLeft,
            Button::DPadRight => Pad::DpadRight,
            _ => return None,
        })
    }

    pub fn axis(axis: Axis) -> Option<Pad> {
        Some(match axis {
            Axis::LeftStickX => Pad::LeftStickX,
            Axis::LeftStickY => Pad::LeftStickY,
            Axis::RightStickX => Pad::RightStickX,
            Axis::RightStickY => Pad::RightStickY,
            _ => return None,
        })
    }

    fn is_analog_trigger(pad: Pad) -> bool {
        matches!(pad, Pad::LeftTrigger | Pad::RightTrigger)
    }

    /// What a controller reported, without gilrs's bookkeeping.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub enum Report {
        Pressed(Button),
        Released(Button),
        Changed(Button, f32),
        Axis(Axis, f32),
        Disconnected,
    }

    fn report_of(event: &EventType) -> Option<Report> {
        Some(match *event {
            EventType::ButtonPressed(b, _) => Report::Pressed(b),
            EventType::ButtonReleased(b, _) => Report::Released(b),
            EventType::ButtonChanged(b, v, _) => Report::Changed(b, v),
            EventType::AxisChanged(a, v, _) => Report::Axis(a, v),
            EventType::Disconnected => Report::Disconnected,
            _ => return None,
        })
    }

    /// What one report changes.
    pub fn changes_of(report: Report) -> Vec<PadChange> {
        match report {
            Report::Pressed(b) => button(b).filter(|p| !is_analog_trigger(*p)).map(|p| vec![(p, 1.0)]).unwrap_or_default(),
            Report::Released(b) => button(b).filter(|p| !is_analog_trigger(*p)).map(|p| vec![(p, 0.0)]).unwrap_or_default(),
            // Triggers report how far they are pulled as a button's value.
            Report::Changed(b, value) => button(b).filter(|p| is_analog_trigger(*p)).map(|p| vec![(p, value)]).unwrap_or_default(),
            Report::Axis(a, value) => axis(a).map(|p| vec![(p, value)]).unwrap_or_default(),
            Report::Disconnected => Pad::ALL.iter().map(|p| (*p, 0.0)).collect(),
        }
    }

    impl Gamepads {
        /// Start reading controllers. If the system has no controller
        /// support the result simply never reports anything.
        pub fn spawn() -> Self {
            let queue: Arc<Mutex<Vec<PadChange>>> = Arc::default();
            let stop = Arc::new(AtomicBool::new(false));
            let (q, s) = (queue.clone(), stop.clone());
            let thread = std::thread::Builder::new()
                .name("Pulsar-Gamepad".into())
                .spawn(move || {
                    let mut gilrs = match Gilrs::new() {
                        Ok(g) => g,
                        Err(gilrs::Error::NotImplemented(g)) => g,
                        Err(error) => {
                            tracing::warn!(%error, "input: controllers are unavailable");
                            return;
                        }
                    };
                    while !s.load(Ordering::Relaxed) {
                        let Some(event) = gilrs.next_event_blocking(Some(WAIT)) else { continue };
                        let changes = report_of(&event.event).map(changes_of).unwrap_or_default();
                        if changes.is_empty() {
                            continue;
                        }
                        let mut queue = q.lock().unwrap_or_else(|p| p.into_inner());
                        queue.extend(changes);
                        let excess = queue.len().saturating_sub(MAX_QUEUED);
                        queue.drain(..excess);
                    }
                })
                .map_err(|error| tracing::warn!(%error, "input: couldn't start the controller thread"))
                .ok();
            Self { queue, stop, thread }
        }

        /// Take the changes since the last call.
        pub fn take(&self) -> Vec<PadChange> {
            std::mem::take(&mut *self.queue.lock().unwrap_or_else(|p| p.into_inner()))
        }
    }

    impl Drop for Gamepads {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn buttons_map_to_the_same_controls_whatever_the_pad_is_called() {
            assert_eq!(button(Button::South), Some(Pad::FaceBottom));
            assert_eq!(button(Button::LeftTrigger2), Some(Pad::LeftTrigger));
            assert_eq!(button(Button::C), None);
            assert_eq!(axis(Axis::RightStickY), Some(Pad::RightStickY));
            assert_eq!(axis(Axis::LeftZ), None);
        }

        #[test]
        fn digital_buttons_press_and_release_and_triggers_report_how_far() {
            assert_eq!(changes_of(Report::Pressed(Button::South)), [(Pad::FaceBottom, 1.0)]);
            assert_eq!(changes_of(Report::Released(Button::South)), [(Pad::FaceBottom, 0.0)]);
            assert!(changes_of(Report::Pressed(Button::RightTrigger2)).is_empty(), "the value carries it");
            assert_eq!(changes_of(Report::Changed(Button::RightTrigger2, 0.6)), [(Pad::RightTrigger, 0.6)]);
            assert!(changes_of(Report::Changed(Button::South, 1.0)).is_empty(), "a pressed event already said so");
            assert_eq!(changes_of(Report::Axis(Axis::LeftStickX, -0.5)), [(Pad::LeftStickX, -0.5)]);
        }

        #[test]
        fn unplugging_lets_go_of_everything() {
            let changes = changes_of(Report::Disconnected);
            assert_eq!(changes.len(), Pad::ALL.len());
            assert!(changes.iter().all(|(_, v)| *v == 0.0));
        }
    }
}

#[cfg(not(feature = "gamepad"))]
mod imp {
    use super::PadChange;

    pub struct Gamepads;

    impl Gamepads {
        pub fn spawn() -> Self {
            Gamepads
        }

        pub fn take(&self) -> Vec<PadChange> {
            Vec::new()
        }
    }
}

pub use imp::Gamepads;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::catalog;

    #[test]
    fn changes_are_fed_to_the_key_state() {
        let mut raw = RawInput::new();
        apply(&mut raw, [(Pad::FaceBottom, 1.0), (Pad::LeftStickX, 0.5)]);
        raw.derive();
        assert!(raw.is_down(catalog().index_of("Gamepad.FaceBottom").unwrap()));
        assert_eq!(raw.value(catalog().index_of("Gamepad.LeftStick").unwrap())[0], 0.5);
    }
}
