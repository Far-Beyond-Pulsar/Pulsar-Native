//! Keyboard and mouse state, reduced to what the game asks each frame.

use std::collections::HashSet;
use winit::event::{ElementState, MouseButton, MouseScrollDelta};
use winit::keyboard::KeyCode;

#[derive(Default)]
pub struct Input {
    held: HashSet<KeyCode>,
    pressed: HashSet<KeyCode>,
    buttons: HashSet<MouseButton>,
    clicked: HashSet<MouseButton>,
    look: (f64, f64),
    scroll: f64,
    /// The cursor is captured by the game.
    pub captured: bool,
}

/// What the player asked for this frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct Controls {
    /// Forward (+1) / back (-1) and right (+1) / left (-1).
    pub forward: f64,
    pub strafe: f64,
    /// Held jump (fly: up) and sneak (fly: down).
    pub jump: bool,
    pub sneak: bool,
    pub sprint: bool,
    /// Mouse motion since the last frame, in pixels.
    pub look: (f64, f64),
    pub mine: bool,
    pub place: bool,
    pub pick: bool,
    /// An explicit world-space pointing ray (VR); desktop uses the player's gaze.
    pub aim: Option<(glam::DVec3, glam::DVec3)>,
}

impl Input {
    pub fn key(&mut self, code: KeyCode, state: ElementState) {
        match state {
            ElementState::Pressed => {
                if self.held.insert(code) {
                    self.pressed.insert(code);
                }
            }
            ElementState::Released => {
                self.held.remove(&code);
            }
        }
    }

    pub fn button(&mut self, button: MouseButton, state: ElementState) {
        match state {
            ElementState::Pressed => {
                self.buttons.insert(button);
                self.clicked.insert(button);
            }
            ElementState::Released => {
                self.buttons.remove(&button);
            }
        }
    }

    pub fn mouse_motion(&mut self, dx: f64, dy: f64) {
        if self.captured {
            self.look.0 += dx;
            self.look.1 += dy;
        }
    }

    pub fn wheel(&mut self, delta: MouseScrollDelta) {
        self.scroll += match delta {
            MouseScrollDelta::LineDelta(_, y) => f64::from(y),
            MouseScrollDelta::PixelDelta(p) => p.y / 40.0,
        };
    }

    /// Whether `code` went down since the last `end_frame`.
    pub fn just_pressed(&self, code: KeyCode) -> bool {
        self.pressed.contains(&code)
    }

    pub fn held(&self, code: KeyCode) -> bool {
        self.held.contains(&code)
    }

    /// Lines of wheel scroll since the last frame (up positive).
    pub fn scroll(&self) -> f64 {
        self.scroll
    }

    /// The hotbar slot (0..9) a number key chose this frame.
    pub fn hotbar_key(&self) -> Option<usize> {
        [
            KeyCode::Digit1,
            KeyCode::Digit2,
            KeyCode::Digit3,
            KeyCode::Digit4,
            KeyCode::Digit5,
            KeyCode::Digit6,
            KeyCode::Digit7,
            KeyCode::Digit8,
            KeyCode::Digit9,
        ]
        .iter()
        .position(|k| self.pressed.contains(k))
    }

    pub fn controls(&self) -> Controls {
        let axis = |plus: KeyCode, minus: KeyCode| {
            f64::from(i32::from(self.held(plus)) - i32::from(self.held(minus)))
        };
        Controls {
            forward: axis(KeyCode::KeyW, KeyCode::KeyS),
            strafe: axis(KeyCode::KeyD, KeyCode::KeyA),
            jump: self.held(KeyCode::Space),
            sneak: self.held(KeyCode::ShiftLeft),
            sprint: self.held(KeyCode::ControlLeft),
            look: if self.captured { self.look } else { (0.0, 0.0) },
            mine: self.captured && self.buttons.contains(&MouseButton::Left),
            place: self.captured && self.clicked.contains(&MouseButton::Right),
            pick: self.captured && self.clicked.contains(&MouseButton::Middle),
            aim: None,
        }
    }

    /// Forget one-frame events.
    pub fn end_frame(&mut self) {
        self.pressed.clear();
        self.clicked.clear();
        self.look = (0.0, 0.0);
        self.scroll = 0.0;
    }

    pub fn release_all(&mut self) {
        self.held.clear();
        self.buttons.clear();
        self.end_frame();
    }
}
