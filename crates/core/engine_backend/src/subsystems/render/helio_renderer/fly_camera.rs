//! The free-flying camera pose shared by the editor viewport and the game's
//! free camera: an f64 eye, and a reference frame whose up follows the local
//! vertical of the ground below (a planet's radial, else +Y), carried along
//! by the smallest rotation as the camera moves, so the horizon stays level
//! and "up" is away from the ground anywhere on a planet. Yaw and pitch are
//! within that frame; yaw 0 looks down the frame's -Z.

use glam::{DVec3, Quat, Vec3};

use super::scene_renderer::VoxelGround;

/// Pitch is kept within this many radians of level.
pub const PITCH_LIMIT: f32 = 1.5;

#[derive(Clone, Copy, Debug)]
pub struct FlyCamera {
    /// The eye in world space, in f64: a planet's surface is millions of
    /// metres from the origin.
    pub position: DVec3,
    /// Yaw and pitch relative to `frame`.
    yaw: f32,
    pitch: f32,
    /// The reference frame (local to world).
    frame: Quat,
    /// A world-space view direction to restore once the frame at a newly set
    /// position is known (the next [`Self::follow_ground`]).
    pending_forward: Option<Vec3>,
}

impl FlyCamera {
    /// At `position`, looking along world-space yaw and pitch (about +Y,
    /// yaw 0 = -Z): the convention a level's saved camera uses.
    pub fn new(position: DVec3, yaw: f32, pitch: f32) -> Self {
        let mut camera = Self {
            position,
            yaw: 0.0,
            pitch: 0.0,
            frame: Quat::IDENTITY,
            pending_forward: None,
        };
        camera.set_pose(position, direction(yaw, pitch));
        camera
    }

    /// Move to `position` looking along the world-space `forward`. Over a
    /// voxel world the frame there is known at the next
    /// [`Self::follow_ground`], which keeps `forward`.
    pub fn set_pose(&mut self, position: DVec3, forward: Vec3) {
        self.position = position;
        self.frame = Quat::IDENTITY;
        self.set_forward(forward);
        self.pending_forward = Some(forward);
    }

    /// Follow the local vertical `up` of the ground at the eye (`None`: no
    /// voxel ground, +Y).
    pub fn follow_ground(&mut self, up: Option<DVec3>) {
        let target = up.map_or(Vec3::Y, |up| up.as_vec3()).normalize_or(Vec3::Y);
        match self.pending_forward.take() {
            // A pose set from outside: its view direction within the frame at it.
            Some(forward) => {
                self.frame = Quat::from_rotation_arc(Vec3::Y, target);
                self.set_forward(forward);
            }
            None => self.frame = transported(self.frame, target),
        }
    }

    /// Turn by `yaw` and `pitch` radians within the frame.
    pub fn look(&mut self, yaw: f32, pitch: f32) {
        self.yaw += yaw;
        self.pitch = (self.pitch + pitch).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }

    /// Point the camera along a world-space direction within its frame.
    pub fn set_forward(&mut self, forward: Vec3) {
        (self.yaw, self.pitch) = local_yaw_pitch(self.frame, forward);
    }

    /// World-space forward, right and up of the view.
    pub fn basis(&self) -> (Vec3, Vec3, Vec3) {
        basis(self.frame, self.yaw, self.pitch)
    }

    /// The view direction in world space.
    pub fn forward(&self) -> Vec3 {
        self.basis().0
    }

    /// The frame's up: the local vertical the view is built with.
    pub fn frame_up(&self) -> Vec3 {
        self.frame * Vec3::Y
    }

    /// The level movement axes: forward along the horizontal view direction,
    /// right, and the local vertical.
    pub fn level_axes(&self) -> (Vec3, Vec3, Vec3) {
        let (sy, cy) = self.yaw.sin_cos();
        (
            self.frame * Vec3::new(sy, 0.0, -cy),
            self.frame * Vec3::new(cy, 0.0, sy),
            self.frame_up(),
        )
    }

    /// Move by `velocity` (right, up, forward along [`Self::level_axes`],
    /// units per second) for `dt` seconds.
    pub fn fly(&mut self, velocity: Vec3, dt: f32) {
        let (forward, right, up) = self.level_axes();
        self.position += (right * velocity.x * dt).as_dvec3()
            + (up * velocity.y * dt).as_dvec3()
            + (forward * velocity.z * dt).as_dvec3();
    }

    /// Yaw and pitch of the world-space view direction (about +Y),
    /// independent of the frame: what a level saves.
    pub fn world_yaw_pitch(&self) -> (f32, f32) {
        yaw_pitch(self.forward())
    }

    /// Yaw and pitch within the frame.
    pub fn local_yaw_pitch(&self) -> (f32, f32) {
        (self.yaw, self.pitch)
    }
}

/// How much faster than its base speed a camera flies at `ground`: the base
/// speed within 20 m of the voxel ground, 50x at 1 km, orbit in seconds.
pub fn altitude_speed_scale(ground: &VoxelGround) -> f64 {
    ground
        .altitude
        .map_or(1.0, |h| (h / 20.0).clamp(1.0, 1.0e6))
}

/// View direction for yaw (about +Y, 0 = -Z) and pitch.
pub fn direction(yaw: f32, pitch: f32) -> Vec3 {
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    Vec3::new(sy * cp, sp, -cy * cp)
}

/// Inverse of [`direction`].
pub fn yaw_pitch(forward: Vec3) -> (f32, f32) {
    let f = forward.normalize_or(Vec3::NEG_Z);
    (f.x.atan2(-f.z), f.y.clamp(-1.0, 1.0).asin())
}

/// Yaw and (clamped) pitch of a world-space direction within `frame`.
fn local_yaw_pitch(frame: Quat, forward: Vec3) -> (f32, f32) {
    let (yaw, pitch) = yaw_pitch(frame.inverse() * forward);
    (yaw, pitch.clamp(-PITCH_LIMIT, PITCH_LIMIT))
}

/// World-space forward, right and up for yaw and pitch within `frame`.
fn basis(frame: Quat, yaw: f32, pitch: f32) -> (Vec3, Vec3, Vec3) {
    let forward = frame * direction(yaw, pitch);
    let (sy, cy) = yaw.sin_cos();
    let right = frame * Vec3::new(cy, 0.0, sy);
    let up = right.cross(forward).normalize_or_zero();
    (forward, right, up)
}

/// `frame` turned by the smallest rotation that takes its up to `up`.
fn transported(frame: Quat, up: Vec3) -> Quat {
    (Quat::from_rotation_arc(frame * Vec3::Y, up) * frame).normalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_frame_matches_the_legacy_yaw_pitch_convention() {
        let (forward, right, up) = basis(Quat::IDENTITY, 0.0, 0.0);
        assert!(
            forward.abs_diff_eq(Vec3::NEG_Z, 1e-6)
                && right.abs_diff_eq(Vec3::X, 1e-6)
                && up.abs_diff_eq(Vec3::Y, 1e-6)
        );
        let d = direction(0.7, -0.3);
        let (yaw, pitch) = yaw_pitch(d);
        assert!((yaw - 0.7).abs() < 1e-5 && (pitch + 0.3).abs() < 1e-5);
    }

    #[test]
    fn the_frame_follows_the_local_vertical_with_a_level_horizon() {
        // 30 degrees from the pole of a planet centred at the origin.
        let local_up = Vec3::new(0.5, 0.866_025_4, 0.0);
        let frame = transported(Quat::IDENTITY, local_up);
        assert!((frame * Vec3::Y).abs_diff_eq(local_up, 1e-5));
        for yaw in [0.0, 1.0, 2.5, -2.0] {
            let (forward, right, up) = basis(frame, yaw, 0.0);
            // Level: forward and right horizontal, up is the local vertical.
            assert!(
                forward.dot(local_up).abs() < 1e-5 && right.dot(local_up).abs() < 1e-5,
                "yaw {yaw}"
            );
            assert!(up.abs_diff_eq(local_up, 1e-5), "yaw {yaw}");
        }
        // A world-space direction survives the round trip through the frame.
        let wanted = Vec3::new(0.3, -0.4, -0.8).normalize();
        let (yaw, pitch) = local_yaw_pitch(frame, wanted);
        assert!(basis(frame, yaw, pitch).0.abs_diff_eq(wanted, 1e-5));
    }

    #[test]
    fn carrying_the_frame_over_the_planet_has_no_jumps() {
        let mut frame = Quat::IDENTITY;
        let mut previous = basis(frame, 0.4, -0.2).0;
        for step in 1..=450 {
            let angle = (step as f32 * 0.1).to_radians();
            let up = Vec3::new(angle.sin(), angle.cos(), 0.0);
            frame = transported(frame, up);
            let forward = basis(frame, 0.4, -0.2).0;
            // The view turns only as much as the vertical does.
            assert!(
                forward.angle_between(previous) <= 0.1f32.to_radians() * 1.01,
                "step {step}"
            );
            previous = forward;
        }
    }

    /// A pose set on a planet keeps its view direction once the frame there
    /// is known, and then stands on the local vertical.
    #[test]
    fn a_set_pose_keeps_its_view_and_takes_the_local_vertical() {
        let up = DVec3::new(1.0, 1.0, 0.3).normalize();
        let eye = up * 6_371_002.0;
        let wanted = Vec3::new(0.2, 0.1, -0.9).normalize();
        let mut camera = FlyCamera::new(DVec3::ZERO, 0.0, 0.0);
        camera.set_pose(eye, wanted);
        camera.follow_ground(Some(up));
        assert!(camera.forward().abs_diff_eq(wanted, 1e-5));
        assert!(camera.frame_up().abs_diff_eq(up.as_vec3(), 1e-5));
        // Flying forward stays level: the height above the centre holds to
        // first order, and the f64 eye moves by sub-millimetre steps.
        let before = camera.position;
        camera.fly(Vec3::new(0.0, 0.0, 0.001), 1.0);
        let moved = camera.position - before;
        assert!((moved.length() - 0.001).abs() < 1e-6, "{moved:?}");
        assert!(moved.dot(up).abs() < 1e-6);
    }

    #[test]
    fn flat_worlds_keep_y_up() {
        let mut camera = FlyCamera::new(DVec3::new(1.0, 2.0, 3.0), 0.4, -0.2);
        camera.follow_ground(None);
        assert!(camera.frame_up().abs_diff_eq(Vec3::Y, 1e-6));
        let (yaw, pitch) = camera.world_yaw_pitch();
        assert!((yaw - 0.4).abs() < 1e-5 && (pitch + 0.2).abs() < 1e-5);
    }

    #[test]
    fn speed_grows_with_altitude() {
        let at = |altitude| VoxelGround { up: None, altitude };
        assert_eq!(altitude_speed_scale(&at(None)), 1.0);
        assert_eq!(altitude_speed_scale(&at(Some(2.0))), 1.0);
        assert_eq!(altitude_speed_scale(&at(Some(1_000.0))), 50.0);
    }
}
