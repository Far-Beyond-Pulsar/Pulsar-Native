//! WASD + mouse-look free camera for the VR demo.
//!
//! Used by the desktop-mirror fallback (no headset). In XR mode the head
//! position/orientation comes straight from OpenXR inside `render_xr`, so this
//! controller is only consulted for the window mirror camera.

use std::collections::HashSet;

use glam::{EulerRot, Quat, Vec3};
use helio::Camera;
use winit::keyboard::KeyCode;

const FLY_SPEED: f32 = 4.0;
const LOOK_SENS: f32 = 0.002;
const DRAG: f32 = 6.0;
/// Head height above the floor when standing at a stage origin (OpenXR's Y+ is up).
const EYE_HEIGHT: f32 = 1.6;

pub struct FreeCam {
    pub position: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub velocity: Vec3,
    pub keys: HashSet<KeyCode>,
    pub cursor_grabbed: bool,
    pub mouse_delta: (f32, f32),
}

impl FreeCam {
    pub fn new() -> Self {
        Self {
            position: Vec3::new(0.0, EYE_HEIGHT, 3.0),
            yaw: 0.0,
            pitch: 0.0,
            velocity: Vec3::ZERO,
            keys: HashSet::new(),
            cursor_grabbed: false,
            mouse_delta: (0.0, 0.0),
        }
    }

    pub fn orientation(&self) -> Quat {
        Quat::from_euler(EulerRot::YXZ, self.yaw, self.pitch, 0.0)
    }

    /// Forward direction of the view orientation.
    #[allow(dead_code)]
    pub fn forward(&self) -> Vec3 {
        self.orientation() * -Vec3::Z
    }

    /// Apply WASD / mouse input for `dt` seconds.
    pub fn update(&mut self, dt: f32) {
        let (dx, dy) = self.mouse_delta;
        self.mouse_delta = (0.0, 0.0);
        self.yaw -= dx * LOOK_SENS;
        self.pitch = (self.pitch - dy * LOOK_SENS).clamp(-1.5, 1.5);

        let orientation = self.orientation();
        let forward = orientation * -Vec3::Z;
        let right = orientation * Vec3::X;

        let mut accel = Vec3::ZERO;
        if self.keys.contains(&KeyCode::KeyW) {
            accel += forward;
        }
        if self.keys.contains(&KeyCode::KeyS) {
            accel -= forward;
        }
        if self.keys.contains(&KeyCode::KeyA) {
            accel -= right;
        }
        if self.keys.contains(&KeyCode::KeyD) {
            accel += right;
        }
        if self.keys.contains(&KeyCode::Space) {
            accel += Vec3::Y;
        }
        if self.keys.contains(&KeyCode::ShiftLeft) {
            accel -= Vec3::Y;
        }

        self.velocity += accel * FLY_SPEED * dt;
        self.velocity /= 1.0 + DRAG * dt;
        self.position += self.velocity * dt;
    }

    pub fn camera(&self, aspect: f32) -> Camera {
        let orientation = self.orientation();
        Camera::perspective_look_at(
            self.position,
            self.position + orientation * -Vec3::Z,
            orientation * Vec3::Y,
            std::f32::consts::FRAC_PI_4,
            aspect,
            0.05,
            200.0,
        )
    }

    /// Controller aim ray stub. The full controller tracking pipeline (action
    /// sets → spaces → poses) is not wired up yet; returning `None` keeps the
    /// demo rendering while the interface stays ready for real hands.
    #[allow(dead_code)]
    pub fn controller_aim(&self) -> Option<(Vec3, Quat)> {
        None
    }
}
