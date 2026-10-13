//! What the devices are doing right now: one value per catalog key.
//!
//! Events arrive between frames (a key went down, the mouse moved); the
//! evaluator asks once per frame what every key reads. Three details make
//! that dependable:
//!
//! - A button pressed and released between two frames still reads as pressed
//!   for one frame (it is *latched*), so a fast tap is never lost.
//! - Mouse motion and the wheel are the total since the last frame, and read
//!   zero again after it.
//! - Keys that are derived from others (a stick as a pair, a stick pushed
//!   left, the D-pad as a pair, the wheel as buttons) are worked out once per
//!   frame, in [`RawInput::derive`].

use crate::keys::{catalog, MouseAxis, Pad, Source};
use crate::model::behavior::Value3;

/// How far a stick must be pushed for its direction buttons to read as down.
const STICK_DIRECTION: f32 = 0.5;

#[derive(Clone)]
pub struct RawInput {
    values: Vec<Value3>,
    latched: Vec<bool>,
    /// Keys that hold a per-frame total rather than a state.
    per_frame: Vec<bool>,
    derived: Derived,
}

#[derive(Clone)]
struct Derived {
    mouse_x: usize,
    mouse_y: usize,
    mouse_xy: usize,
    wheel: usize,
    wheel_up: usize,
    wheel_down: usize,
    dpad: [usize; 4],
    dpad_xy: usize,
    sticks: [Stick; 2],
}

#[derive(Clone)]
struct Stick {
    x: usize,
    y: usize,
    xy: usize,
    /// Up, down, left, right.
    directions: [usize; 4],
}

fn index(id: &str) -> usize {
    catalog().index_of(id).unwrap_or_else(|| panic!("`{id}` is missing from the key catalog"))
}

impl Derived {
    fn new() -> Self {
        let stick = |name: &str| Stick {
            x: index(&format!("Gamepad.{name}StickX")),
            y: index(&format!("Gamepad.{name}StickY")),
            xy: index(&format!("Gamepad.{name}Stick")),
            directions: [
                index(&format!("Gamepad.{name}StickUp")),
                index(&format!("Gamepad.{name}StickDown")),
                index(&format!("Gamepad.{name}StickLeft")),
                index(&format!("Gamepad.{name}StickRight")),
            ],
        };
        Self {
            mouse_x: index("Mouse.X"),
            mouse_y: index("Mouse.Y"),
            mouse_xy: index("Mouse.XY"),
            wheel: index("Mouse.Wheel"),
            wheel_up: index("Mouse.WheelUp"),
            wheel_down: index("Mouse.WheelDown"),
            dpad: [index("Gamepad.DpadUp"), index("Gamepad.DpadDown"), index("Gamepad.DpadLeft"), index("Gamepad.DpadRight")],
            dpad_xy: index("Gamepad.Dpad"),
            sticks: [stick("Left"), stick("Right")],
        }
    }
}

impl Default for RawInput {
    fn default() -> Self {
        Self::new()
    }
}

impl RawInput {
    pub fn new() -> Self {
        let keys = catalog().all();
        Self {
            values: vec![[0.0; 3]; keys.len()],
            latched: vec![false; keys.len()],
            per_frame: keys.iter().map(|k| matches!(k.source, Source::MouseAxis(_))).collect(),
            derived: Derived::new(),
        }
    }

    // ── Feeding ────────────────────────────────────────────────────────────

    /// A button went down (or up).
    pub fn set_button(&mut self, key: usize, down: bool) {
        self.values[key] = [if down { 1.0 } else { 0.0 }, 0.0, 0.0];
        if down {
            self.latched[key] = true;
        }
    }

    /// An axis now reads `value`.
    pub fn set_axis(&mut self, key: usize, value: f32) {
        self.values[key][0] = value;
    }

    /// Add to a per-frame total (mouse motion, wheel).
    pub fn add_motion(&mut self, key: usize, delta: f32) {
        self.values[key][0] += delta;
    }

    /// The key with this winit code went down or up.
    pub fn key_code(&mut self, code: i64, down: bool) {
        if let Some(key) = catalog().index_for_code(code) {
            self.set_button(key, down);
        }
    }

    /// A mouse button, numbered as `MouseButtonDown` does.
    pub fn mouse_button(&mut self, button: i64, down: bool) {
        let target = catalog().all().iter().position(|k| k.source == Source::MouseButton(button as u8));
        if let Some(key) = target.filter(|_| (0..=255).contains(&button)) {
            self.set_button(key, down);
        }
    }

    /// The mouse moved by `dx`, `dy` window pixels (down is positive, as
    /// windows count). Reads as up-positive, like the sticks.
    pub fn mouse_moved(&mut self, dx: f64, dy: f64) {
        self.add_motion(self.derived.mouse_x, dx as f32);
        self.add_motion(self.derived.mouse_y, -dy as f32);
    }

    /// The wheel turned by `x`, `y` notches.
    pub fn wheel(&mut self, x: f64, y: f64) {
        let axes = catalog().all().iter().enumerate().filter_map(|(i, k)| match k.source {
            Source::MouseAxis(MouseAxis::WheelX) => Some((i, x)),
            Source::MouseAxis(MouseAxis::WheelY) => Some((i, y)),
            _ => None,
        });
        for (key, amount) in axes.collect::<Vec<_>>() {
            self.add_motion(key, amount as f32);
        }
    }

    /// A gamepad control changed. Buttons use 0 or 1; triggers 0 to 1; sticks
    /// -1 to 1 with up and right positive.
    pub fn pad(&mut self, control: Pad, value: f32) {
        let Some(key) = catalog().all().iter().position(|k| k.source == Source::Pad(control)) else { return };
        match catalog().at(key).kind {
            crate::keys::KeyKind::Button => self.set_button(key, value > 0.5),
            _ => self.set_axis(key, value),
        }
    }

    // ── Reading ────────────────────────────────────────────────────────────

    /// What `key` reads now.
    pub fn value(&self, key: usize) -> Value3 {
        let mut v = self.values[key];
        if self.latched[key] && v[0] == 0.0 && !self.per_frame[key] {
            v[0] = 1.0;
        }
        v
    }

    /// Whether `key` is down, for buttons.
    pub fn is_down(&self, key: usize) -> bool {
        self.value(key)[0] != 0.0
    }

    /// Work out the derived keys from the ones that are fed. Call once per
    /// frame, after the events and before reading.
    pub fn derive(&mut self) {
        let d = self.derived.clone();
        self.values[d.mouse_xy] = [self.values[d.mouse_x][0], self.values[d.mouse_y][0], 0.0];
        let wheel = self.values[d.wheel][0];
        self.values[d.wheel_up] = [if wheel > 0.0 { 1.0 } else { 0.0 }, 0.0, 0.0];
        self.values[d.wheel_down] = [if wheel < 0.0 { 1.0 } else { 0.0 }, 0.0, 0.0];
        // A wheel notch lasts the frame it came in, so these never latch.
        let axis = |me: &Self, up: usize, down: usize| me.value(up)[0] - me.value(down)[0];
        let dx = axis(self, d.dpad[3], d.dpad[2]);
        let dy = axis(self, d.dpad[0], d.dpad[1]);
        self.values[d.dpad_xy] = [dx, dy, 0.0];
        for stick in &d.sticks {
            let (x, y) = (self.values[stick.x][0], self.values[stick.y][0]);
            self.values[stick.xy] = [x, y, 0.0];
            let flags = [y > STICK_DIRECTION, y < -STICK_DIRECTION, x < -STICK_DIRECTION, x > STICK_DIRECTION];
            for (key, on) in stick.directions.iter().zip(flags) {
                self.values[*key] = [if on { 1.0 } else { 0.0 }, 0.0, 0.0];
            }
        }
    }

    /// Finish a frame: motion starts again from zero and latched presses end.
    pub fn end_frame(&mut self) {
        for (i, per_frame) in self.per_frame.iter().enumerate() {
            if *per_frame {
                self.values[i] = [0.0; 3];
            }
        }
        for latch in &mut self.latched {
            *latch = false;
        }
    }

    /// Release everything (the window lost focus, the game paused input).
    pub fn release_all(&mut self) {
        for v in &mut self.values {
            *v = [0.0; 3];
        }
        self.latched.iter_mut().for_each(|l| *l = false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(id: &str) -> usize {
        index(id)
    }

    #[test]
    fn a_key_reads_down_until_it_comes_up() {
        let mut raw = RawInput::new();
        raw.key_code(41, true);
        raw.derive();
        assert!(raw.is_down(key("Keyboard.KeyW")));
        raw.end_frame();
        raw.derive();
        assert!(raw.is_down(key("Keyboard.KeyW")), "stays down across frames");
        raw.key_code(41, false);
        assert!(!raw.is_down(key("Keyboard.KeyW")));
    }

    #[test]
    fn a_tap_inside_one_frame_still_reads_as_pressed_for_that_frame() {
        let mut raw = RawInput::new();
        raw.key_code(62, true);
        raw.key_code(62, false);
        raw.derive();
        assert!(raw.is_down(key("Keyboard.Space")));
        raw.end_frame();
        raw.derive();
        assert!(!raw.is_down(key("Keyboard.Space")), "and not for the next");
    }

    #[test]
    fn mouse_motion_accumulates_within_a_frame_and_resets_after() {
        let mut raw = RawInput::new();
        raw.mouse_moved(3.0, 2.0);
        raw.mouse_moved(1.0, -5.0);
        raw.derive();
        assert_eq!(raw.value(key("Mouse.X"))[0], 4.0);
        assert_eq!(raw.value(key("Mouse.Y"))[0], 3.0, "up is positive");
        assert_eq!(raw.value(key("Mouse.XY"))[..2], [4.0, 3.0]);
        raw.end_frame();
        raw.derive();
        assert_eq!(raw.value(key("Mouse.XY")), [0.0; 3]);
    }

    #[test]
    fn the_wheel_is_an_axis_and_two_buttons() {
        let mut raw = RawInput::new();
        raw.wheel(0.0, 2.0);
        raw.derive();
        assert_eq!(raw.value(key("Mouse.Wheel"))[0], 2.0);
        assert!(raw.is_down(key("Mouse.WheelUp")));
        assert!(!raw.is_down(key("Mouse.WheelDown")));
        raw.end_frame();
        raw.wheel(0.0, -1.0);
        raw.derive();
        assert!(raw.is_down(key("Mouse.WheelDown")));
        assert!(!raw.is_down(key("Mouse.WheelUp")));
    }

    #[test]
    fn a_stick_is_a_pair_and_four_direction_buttons() {
        let mut raw = RawInput::new();
        raw.pad(Pad::LeftStickX, -0.8);
        raw.pad(Pad::LeftStickY, 0.2);
        raw.derive();
        assert_eq!(raw.value(key("Gamepad.LeftStick"))[..2], [-0.8, 0.2]);
        assert!(raw.is_down(key("Gamepad.LeftStickLeft")));
        assert!(!raw.is_down(key("Gamepad.LeftStickUp")), "0.2 is below the direction threshold");
        assert!(!raw.is_down(key("Gamepad.RightStickLeft")));
    }

    #[test]
    fn the_dpad_is_a_pair_that_cancels_opposite_presses() {
        let mut raw = RawInput::new();
        raw.pad(Pad::DpadUp, 1.0);
        raw.pad(Pad::DpadRight, 1.0);
        raw.derive();
        assert_eq!(raw.value(key("Gamepad.Dpad"))[..2], [1.0, 1.0]);
        raw.pad(Pad::DpadLeft, 1.0);
        raw.derive();
        assert_eq!(raw.value(key("Gamepad.Dpad"))[0], 0.0);
    }

    #[test]
    fn pad_triggers_are_axes_and_face_buttons_are_buttons() {
        let mut raw = RawInput::new();
        raw.pad(Pad::RightTrigger, 0.7);
        raw.pad(Pad::FaceBottom, 1.0);
        assert_eq!(raw.value(key("Gamepad.RightTrigger"))[0], 0.7);
        assert!(raw.is_down(key("Gamepad.FaceBottom")));
    }

    #[test]
    fn mouse_buttons_and_unknown_codes_are_handled() {
        let mut raw = RawInput::new();
        raw.mouse_button(1, true);
        assert!(raw.is_down(key("Mouse.Right")));
        raw.mouse_button(99, true);
        raw.mouse_button(-1, true);
        raw.key_code(100_000, true);
        raw.release_all();
        assert!(!raw.is_down(key("Mouse.Right")));
    }
}
