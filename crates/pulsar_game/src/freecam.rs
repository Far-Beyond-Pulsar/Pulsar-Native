//! Built-in free-look camera for the game runtime.
//!
//! Active whenever no game camera has been pushed to a window via
//! [`WindowManager::set_camera`].  Provides standard FPS controls:
//!
//! | Input | Action |
//! |-------|--------|
//! | W / S | Forward / Backward |
//! | A / D | Strafe left / right |
//! | Space | Ascend |
//! | Left Shift | Descend |
//! | Mouse (captured) | Look |
//! | Left-click | Capture / re-capture cursor |
//! | Escape | Release cursor |
//!
//! This is a *placeholder* — it is automatically superseded per-window as soon
//! as the game pushes its own camera.

use std::f32::consts::{FRAC_PI_2, PI};
use winit::keyboard::{KeyCode, PhysicalKey};

use crate::window::RenderCamera;

/// Bitflags for currently-held movement keys.
#[derive(Default, Clone, Copy)]
struct KeyState {
    forward:  bool,
    backward: bool,
    left:     bool,
    right:    bool,
    up:       bool,
    down:     bool,
}

/// Free-look camera — lives on the main thread, updated from winit events.
pub struct FreeCam {
    /// World-space position.
    pub position: glam::Vec3,
    /// Horizontal angle in radians (0 = +Z, increases counter-clockwise).
    pub yaw: f32,
    /// Vertical angle in radians (0 = level, positive = look up).
    pub pitch: f32,

    /// Movement speed in units per second.
    pub move_speed: f32,
    /// Mouse sensitivity — pixels-to-radians scale.
    pub mouse_sensitivity: f32,

    /// Vertical field of view in radians.
    pub fov_y: f32,
    pub near: f32,
    pub far: f32,

    keys: KeyState,
}

impl Default for FreeCam {
    fn default() -> Self {
        Self {
            // Sane default — positioned slightly above and behind origin,
            // facing toward the scene.  The first rendered frame will show
            // something sensible even with an empty scene.
            position: glam::Vec3::new(0.0, 3.0, 10.0),
            yaw: PI,        // face -Z (into the scene)
            pitch: -0.15,   // very slight downward tilt
            move_speed: 8.0,
            mouse_sensitivity: 0.002,
            fov_y: std::f32::consts::FRAC_PI_4,
            near: 0.05,
            far: 2000.0,
            keys: KeyState::default(),
        }
    }
}

impl FreeCam {
    /// Place the freecam at a specific world position and look-direction.
    pub fn place(mut self, position: glam::Vec3, yaw: f32, pitch: f32) -> Self {
        self.position = position;
        self.yaw   = yaw;
        self.pitch = pitch;
        self
    }

    // ── Input ─────────────────────────────────────────────────────────────────

    /// Call from `window_event(KeyboardInput { .. })`.
    pub fn on_key(&mut self, key: &PhysicalKey, pressed: bool) {
        let PhysicalKey::Code(code) = key else { return };
        match code {
            KeyCode::KeyW | KeyCode::ArrowUp    => self.keys.forward  = pressed,
            KeyCode::KeyS | KeyCode::ArrowDown  => self.keys.backward = pressed,
            KeyCode::KeyA | KeyCode::ArrowLeft  => self.keys.left     = pressed,
            KeyCode::KeyD | KeyCode::ArrowRight => self.keys.right    = pressed,
            KeyCode::Space                       => self.keys.up       = pressed,
            KeyCode::ShiftLeft | KeyCode::ShiftRight => self.keys.down = pressed,
            _ => {}
        }
    }

    /// Call from `DeviceEvent::MouseMotion { delta }` when the cursor is captured.
    ///
    /// `dx` is raw horizontal pixels (right = positive), `dy` is raw vertical
    /// pixels (down = positive in winit).
    pub fn on_mouse_delta(&mut self, dx: f64, dy: f64) {
        self.yaw   -= dx as f32 * self.mouse_sensitivity;
        // Invert dy: moving mouse down (dy > 0) should look down (pitch decreases).
        self.pitch -= dy as f32 * self.mouse_sensitivity;

        // Keep yaw in [-π, π] to avoid float drift.
        self.yaw = (self.yaw + PI).rem_euclid(2.0 * PI) - PI;
        // Clamp pitch to avoid gimbal flip at ±90°.
        self.pitch = self.pitch.clamp(-FRAC_PI_2 + 0.01, FRAC_PI_2 - 0.01);
    }

    // ── Update ────────────────────────────────────────────────────────────────

    /// Advance the camera by `dt` seconds according to currently-held keys.
    pub fn update(&mut self, dt: f32) {
        let speed = self.move_speed * dt;

        // Forward direction lies in the XZ plane (no pitch for movement — FPS style).
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let forward = glam::Vec3::new(sin_yaw, 0.0, cos_yaw);
        let right   = glam::Vec3::new(cos_yaw, 0.0, -sin_yaw);
        let world_up = glam::Vec3::Y;

        if self.keys.forward  { self.position += forward   * speed; }
        if self.keys.backward { self.position -= forward   * speed; }
        if self.keys.right    { self.position += right     * speed; }
        if self.keys.left     { self.position -= right     * speed; }
        if self.keys.up       { self.position += world_up  * speed; }
        if self.keys.down     { self.position -= world_up  * speed; }
    }

    // ── Output ────────────────────────────────────────────────────────────────

    /// Compute the look-at target point from current position + yaw + pitch.
    fn look_dir(&self) -> glam::Vec3 {
        let (sin_yaw,   cos_yaw)   = self.yaw.sin_cos();
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        // Standard spherical → Cartesian, Y-up convention.
        glam::Vec3::new(
            cos_pitch * sin_yaw,
            sin_pitch,
            cos_pitch * cos_yaw,
        )
        .normalize()
    }

    /// Produce a [`RenderCamera`] for this frame.
    pub fn to_render_camera(&self) -> RenderCamera {
        let dir = self.look_dir();
        RenderCamera {
            position: self.position.to_array(),
            target:   (self.position + dir).to_array(),
            up:       [0.0, 1.0, 0.0],
            fov_y:    self.fov_y,
            near:     self.near,
            far:      self.far,
        }
    }
}
