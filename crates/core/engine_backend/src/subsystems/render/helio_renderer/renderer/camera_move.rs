//! Flying the editor camera from the captured input.

use super::*;

impl HelioRenderer {
    pub(super) fn apply_camera_input(&mut self, dt: f32) {
        let Some(input) = self.take_camera_input() else {
            return;
        };
        if self.active_view.is_orthographic() && self.apply_ortho_input(&input, dt) {
            return;
        }
        let fly = self.fly_camera(&input, dt);
        if self.traverse_through_aperture(&fly) {
            return;
        }
        self.keep_camera_out_of_ground(&fly);
        self.cam.position += fly.pan;
    }

    /// Snapshot the camera input and clear its per-frame deltas; `None` when
    /// the input lock is poisoned.
    pub(super) fn take_camera_input(&self) -> Option<CameraInput> {
        match self.camera_input.lock() {
            Ok(mut lock) => {
                let snap = lock.clone();
                lock.clear_transient_deltas();
                Some(snap)
            }
            Err(_) => None,
        }
    }

    /// Orthographic zoom and axis-view input. Returns true when an
    /// axis-aligned view consumed the input.
    pub(super) fn apply_ortho_input(&mut self, input: &CameraInput, dt: f32) -> bool {
        if input.zoom_delta != 0.0 {
            // Scrolling lengthens or shortens the boom arm.
            self.ortho_boom = (self.ortho_boom * (-input.zoom_delta * 0.1).exp())
                .clamp(ORTHO_MIN_BOOM, ORTHO_MAX_BOOM);
        }
        if self.active_view.is_axis_aligned() {
            self.apply_axis_view_input(input, dt);
            return true;
        }
        false
    }

    /// Look and fly the camera from the input; the pan is returned, not applied.
    pub(super) fn fly_camera(&mut self, input: &CameraInput, dt: f32) -> FlyMove {
        const LOOK: f32 = 0.0025;

        self.cam
            .look(input.mouse_delta_x * LOOK, -input.mouse_delta_y * LOOK);

        // Ordinary flight stays level. While traversing, aim W/S through
        // the aperture even when it is above or below the eye.
        let (level_forward, right, frame_up) = self.cam.level_axes();
        let fwd = if self.active_space_traversal && !self.active_view.is_orthographic() {
            self.cam.forward()
        } else {
            level_forward
        };
        // Over ground the scene reports, speed grows with height above it.
        let altitude = altitude_speed_scale(&self.camera_ground) as f32;
        let speed = altitude
            * if input.boost {
                input.move_speed * 3.0
            } else {
                input.move_speed
            };

        // Target local velocity from input (units/sec).
        let target_velocity =
            Vec3::new(input.right * speed, input.up * speed, input.forward * speed);

        // Keyboard velocity applies instantly — matching the crisp, zero-latency
        // behavior of mouse look. The previous exponential ease-in/out
        // (ACCEL_RATE 10 / DECEL_RATE 14) took ~230ms to reach 90% of target
        // speed, which is what made WASD feel "mushy" next to the mouse.
        // Camera position stays continuous (velocity * dt integration), so an
        // instant velocity step produces no visible jump.
        self.cam_local_velocity = target_velocity;

        let before = self.cam.position;
        let steps = [
            (right * self.cam_local_velocity.x * dt).as_dvec3(),
            (frame_up * self.cam_local_velocity.y * dt).as_dvec3(),
            (fwd * self.cam_local_velocity.z * dt).as_dvec3(),
        ];
        self.cam.position += steps[0] + steps[1] + steps[2];
        // Middle-mouse (or Shift + right-click) pans in the view plane.
        let pan = if input.pan_delta_x != 0.0 || input.pan_delta_y != 0.0 {
            const PAN: f32 = 0.01;
            let (_, _, screen_up) = self.camera_basis();
            let pan_speed = PAN * input.move_speed.max(1.0);
            (right * (-input.pan_delta_x) * pan_speed).as_dvec3()
                + (screen_up * input.pan_delta_y * pan_speed).as_dvec3()
        } else {
            DVec3::ZERO
        };
        let end = self.cam.position + pan;
        FlyMove {
            before,
            end,
            steps,
            pan,
        }
    }

    /// Carry the camera through an aperture its move crossed. Returns true
    /// when it was transported (the move is then complete).
    pub(super) fn traverse_through_aperture(&mut self, fly: &FlyMove) -> bool {
        if fly.before != fly.end
            && !self.active_view.is_orthographic()
            && self.active_space_traversal
        {
            // The aperture can be inset into terrain. Sweep before the source
            // ground's endpoint check can clamp the eye back to its own side.
            if let Some((position, rotation, space)) = self.inner.as_ref().and_then(|inner| {
                inner
                    .scene
                    .traverse_camera(fly.before, fly.end, self.camera_space)
            }) {
                self.cam.transport(position, rotation);
                self.camera_space = space;
                self.reset_taa_next_frame = true;
                return true;
            }
        }
        false
    }

    /// The editor camera never enters solid ground. From air (a
    /// cave, a dig, above ground) it slides along what it touches: each
    /// axis of the move is kept only if it stays in air. Lifting to the
    /// surface is only for a camera already buried (spawned or teleported
    /// into rock): lifting on contact threw it out of caves.
    pub(super) fn keep_camera_out_of_ground(&mut self, fly: &FlyMove) {
        let lift = |renderer: &mut Self, eye: DVec3| {
            renderer
                .inner
                .as_mut()
                .and_then(|inner| inner.scene.lift_out_of_ground(eye))
        };
        if lift(self, self.cam.position).is_some() {
            if lift(self, fly.before).is_none() {
                let mut pos = fly.before;
                for step in fly.steps {
                    if lift(self, pos + step).is_none() {
                        pos += step;
                    }
                }
                self.cam.position = pos;
            } else if let Some(lifted) = lift(self, self.cam.position) {
                self.cam.position = lifted;
            }
        }
    }
}
