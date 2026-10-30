//! Built-in free-look camera for the game runtime.
//!
//! Active whenever no game camera has been pushed to a window via
//! [`WindowManager::set_camera`].  Provides standard FPS controls:
//!
//! | Input | Action |
//! |-------|--------|
//! | W / S | Forward / Backward |
//! | A / D | Strafe left / right |
//! | Space | Ascend (along the local vertical) |
//! | Left Shift | Descend |
//! | Mouse (captured) | Look |
//! | Left-click | Capture / re-capture cursor |
//! | Escape | Release cursor |
//!
//! It flies as the editor viewport's camera does ([`FlyCamera`]): an f64 eye,
//! level with the ground below (over a planet its up is the local vertical,
//! elsewhere +Y), and faster the higher it is above voxel ground.
//!
//! While a possessed pawn owns the player's input ([`FreeCam::set_suspended`],
//! driven by the runtime's [`PossessionGate`](crate::player_input::PossessionGate))
//! it neither moves nor looks, so it does not fly on the keys the pawn hears.
//!
//! This is a *placeholder* — it is automatically superseded per-window as soon
//! as the game pushes its own camera.

use engine_backend::subsystems::render::{altitude_speed_scale, FlyCamera, VoxelGround};
use glam::{DVec3, Vec3};
use winit::keyboard::{KeyCode, PhysicalKey};

use crate::window::RenderCamera;

/// Bitflags for currently-held movement keys.
#[derive(Default, Clone, Copy)]
struct KeyState {
    forward: bool,
    backward: bool,
    left: bool,
    right: bool,
    up: bool,
    down: bool,
}

/// Free-look camera — lives on the main thread, updated from winit events.
pub struct FreeCam {
    /// The pose: the eye, and yaw/pitch within the frame of the ground below.
    pub pose: FlyCamera,

    /// Movement speed in units per second near the ground; it grows with the
    /// height above voxel ground.
    pub move_speed: f32,
    /// Mouse sensitivity — pixels-to-radians scale.
    pub mouse_sensitivity: f32,

    /// Vertical field of view in radians.
    pub fov_y: f32,
    pub near: f32,
    pub far: f32,

    /// The voxel ground under the eye, from the last frame.
    ground: VoxelGround,
    keys: KeyState,
    /// A possessed pawn owns the input: see [`set_suspended`](Self::set_suspended).
    suspended: bool,
}

impl Default for FreeCam {
    fn default() -> Self {
        Self {
            // Sane default — positioned slightly above and behind origin,
            // facing toward the scene (yaw 0 faces -Z) with a very slight
            // downward tilt. The first rendered frame will show something
            // sensible even with an empty scene.
            pose: FlyCamera::new(DVec3::new(0.0, 3.0, 10.0), 0.0, -0.15),
            move_speed: 8.0,
            mouse_sensitivity: 0.002,
            fov_y: std::f32::consts::FRAC_PI_4,
            near: 0.05,
            far: 2000.0,
            ground: VoxelGround::default(),
            keys: KeyState::default(),
            suspended: false,
        }
    }
}

impl FreeCam {
    /// Place the freecam at a world position, looking along world-space yaw
    /// and pitch (about +Y, yaw 0 = -Z), as a level saves its camera. Over a
    /// planet the view is kept once the frame there is known.
    pub fn place(mut self, position: DVec3, yaw: f32, pitch: f32) -> Self {
        self.pose = FlyCamera::new(position, yaw, pitch);
        self
    }

    /// The eye in world space.
    pub fn position(&self) -> DVec3 {
        self.pose.position
    }

    // ── Input ─────────────────────────────────────────────────────────────────

    /// Stand down (or take the input back). A suspended camera ignores keys
    /// and mouse motion and does not move; keys held at the moment it is
    /// suspended are forgotten. The hosts call it every frame with
    /// [`PossessionGate::is_possessed`](crate::player_input::PossessionGate::is_possessed):
    /// the pawn that is possessed hears those keys, so the camera must not
    /// fly on them too.
    pub fn set_suspended(&mut self, suspended: bool) {
        if suspended && !self.suspended {
            self.keys = KeyState::default();
        }
        self.suspended = suspended;
    }

    pub fn is_suspended(&self) -> bool {
        self.suspended
    }

    /// Call from `window_event(KeyboardInput { .. })`.
    pub fn on_key(&mut self, key: &PhysicalKey, pressed: bool) {
        if self.suspended {
            return;
        }
        let PhysicalKey::Code(code) = key else { return };
        match code {
            KeyCode::KeyW | KeyCode::ArrowUp => self.keys.forward = pressed,
            KeyCode::KeyS | KeyCode::ArrowDown => self.keys.backward = pressed,
            KeyCode::KeyA | KeyCode::ArrowLeft => self.keys.left = pressed,
            KeyCode::KeyD | KeyCode::ArrowRight => self.keys.right = pressed,
            KeyCode::Space => self.keys.up = pressed,
            KeyCode::ShiftLeft | KeyCode::ShiftRight => self.keys.down = pressed,
            _ => {}
        }
    }

    /// Call from `DeviceEvent::MouseMotion { delta }` when the cursor is captured.
    ///
    /// `dx` is raw horizontal pixels (right = positive), `dy` is raw vertical
    /// pixels (down = positive in winit).
    pub fn on_mouse_delta(&mut self, dx: f64, dy: f64) {
        if self.suspended {
            return;
        }
        // Match the editor camera: moving the mouse right increases yaw, and
        // moving it down (dy > 0) looks down.
        self.pose.look(
            dx as f32 * self.mouse_sensitivity,
            -dy as f32 * self.mouse_sensitivity,
        );
    }

    // ── Update ────────────────────────────────────────────────────────────────

    /// Follow the voxel ground under the eye (`SceneRenderer::voxel_ground`):
    /// its local vertical becomes the camera's up, its height the speed.
    pub fn follow_ground(&mut self, ground: VoxelGround) {
        self.pose.follow_ground(ground.up);
        self.ground = ground;
    }

    /// Advance the camera by `dt` seconds according to currently-held keys:
    /// level within the camera frame (no pitch for movement — FPS style),
    /// up and down along the local vertical.
    pub fn update(&mut self, dt: f32) {
        if self.suspended {
            return;
        }
        let axis = |positive: bool, negative: bool| f32::from(positive) - f32::from(negative);
        let direction = Vec3::new(
            axis(self.keys.right, self.keys.left),
            axis(self.keys.up, self.keys.down),
            axis(self.keys.forward, self.keys.backward),
        );
        if direction == Vec3::ZERO {
            return;
        }
        let speed = self.move_speed * altitude_speed_scale(&self.ground) as f32;
        self.pose.fly(direction * speed, dt);
    }

    // ── Output ────────────────────────────────────────────────────────────────

    /// Produce a [`RenderCamera`] for this frame.
    pub fn to_render_camera(&self) -> RenderCamera {
        RenderCamera {
            position: self.pose.position,
            forward: self.pose.forward(),
            up: self.pose.frame_up(),
            fov_y: self.fov_y,
            near: self.near,
            far: self.far,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_backend::subsystems::render::helio_renderer::fly_camera::yaw_pitch;

    fn press(camera: &mut FreeCam, code: KeyCode) {
        camera.on_key(&PhysicalKey::Code(code), true);
    }

    /// While a pawn is possessed the camera neither flies nor looks, and keys
    /// it was holding when it stood down do not resume by themselves.
    #[test]
    fn a_suspended_camera_ignores_the_keys_and_the_mouse_and_resumes_clean() {
        let start = DVec3::new(0.0, 2.0, 0.0);
        let mut camera = FreeCam::default().place(start, 0.0, 0.0);
        press(&mut camera, KeyCode::KeyW);
        camera.set_suspended(true);
        assert!(camera.is_suspended());
        press(&mut camera, KeyCode::Space);
        camera.on_mouse_delta(500.0, 500.0);
        camera.update(1.0);
        assert_eq!(camera.position(), start, "did not move");
        let forward = camera.to_render_camera().forward;
        assert!(forward.abs_diff_eq(FreeCam::default().place(start, 0.0, 0.0).to_render_camera().forward, 1e-6), "did not look");

        // Taken back: nothing held over from before, and it flies as ever.
        camera.set_suspended(false);
        camera.update(1.0);
        assert_eq!(camera.position(), start, "W was forgotten while suspended");
        press(&mut camera, KeyCode::Space);
        camera.update(1.0);
        assert!(camera.position().y > start.y);
        camera.on_mouse_delta(100.0, 0.0);
        assert!(!camera.to_render_camera().forward.abs_diff_eq(forward, 1e-3));
    }

    /// Without voxel ground the camera keeps +Y up and its base speed.
    #[test]
    fn a_flat_world_flies_with_y_up() {
        let mut camera = FreeCam::default().place(DVec3::new(0.0, 2.0, 0.0), 0.0, 0.0);
        camera.follow_ground(VoxelGround::default());
        press(&mut camera, KeyCode::Space);
        camera.update(1.0);
        assert!(camera.to_render_camera().up.abs_diff_eq(Vec3::Y, 1e-6));
        assert!((camera.position() - DVec3::new(0.0, 10.0, 0.0)).length() < 1e-5);
    }

    /// Over a planet the camera stands on the local vertical, keeps the view
    /// it was placed with, flies level with the ground and speeds up with the
    /// height above it; its eye is f64, millions of metres out.
    #[test]
    fn over_a_planet_the_camera_takes_the_local_vertical_and_altitude_speed() {
        let up = DVec3::new(1.0, 1.0, 0.3).normalize();
        let eye = up * 6_371_002.0;
        let forward = Vec3::new(-0.3, 0.2, 0.9).normalize();
        let (yaw, pitch) = yaw_pitch(forward);
        let mut camera = FreeCam::default().place(eye, yaw, pitch);
        camera.follow_ground(VoxelGround {
            up: Some(up),
            altitude: Some(2.0),
        });
        let view = camera.to_render_camera();
        assert_eq!(view.position, eye, "the eye is kept exactly");
        assert!(view.up.abs_diff_eq(up.as_vec3(), 1e-5));
        assert!(view.forward.abs_diff_eq(forward, 1e-5));

        // 2 m above the ground: the base speed, level. A millimetre step
        // stays a millimetre (an f32 eye there moves by half metres).
        press(&mut camera, KeyCode::KeyW);
        camera.update(0.001 / camera.move_speed);
        let moved = camera.position() - eye;
        assert!((moved.length() - 0.001).abs() < 1e-6, "{moved:?}");
        assert!(moved.dot(up).abs() < 1e-6, "level with the ground");

        // 1 km up: fifty times faster.
        camera.follow_ground(VoxelGround {
            up: Some(up),
            altitude: Some(1_000.0),
        });
        let before = camera.position();
        camera.update(1.0);
        let moved = (camera.position() - before).length();
        assert!(
            (moved - 50.0 * f64::from(camera.move_speed)).abs() < 1e-3,
            "{moved}"
        );
    }
}
