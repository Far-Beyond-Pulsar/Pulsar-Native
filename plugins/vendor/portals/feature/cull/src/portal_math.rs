//! Portal pair math: `pair_map`, teleport, crossing detection.
//!
//! A portal pair has two surfaces A and B in the same world. `pair_map` is the
//! rigid transform that maps A's frame to B's frame; teleporting a point
//! through A maps it by `pair_map`. All math here is CPU-side and unit-tested.
//!
//! This crate is deliberately narrow: it captures only the portal-pose algebra
//! that is unaffected by *how* a portal is rendered — the pair transform,
//! crossing detection for teleport, and ray mapping. The GPU-facing mechanism
//! (drawing a clipped duplicate of nearby geometry through this same
//! `pair_map_inverse` as one more coordinate space) lives in `helio`'s scene
//! layer, not here; an earlier version of this crate also carried an "eye
//! camera" + oblique-near-clip construction for a since-abandoned
//! off-screen-camera rendering approach — that part is gone, the pair algebra
//! below is unchanged from it.
//!
//! Conventions:
//! - A pose is a rigid `Mat4` built from a look-at; the surface's outward
//!   normal (`forward`) is the -Z axis, matching how Helio derives camera
//!   forward from `-view.z_axis`.

#![warn(missing_docs)]
#![forbid(unsafe_code)]

use glam::{Mat4, Vec2, Vec3};

/// Builds a portal pose from a position, outward-facing direction and up vector.
pub fn portal_pose_facing(position: Vec3, forward: Vec3, up: Vec3) -> PortalPose {
    PortalPose::from_look_at(position, position + forward, up)
}

/// Build the render-space map that places target-level contents beyond the
/// source portal opening.
///
/// This is intentionally different from the teleport map. A portal view
/// crosses from the target portal's local front side into the source portal's
/// local back side, so the standard 180-degree turn in portal-local space is
/// required before placing the target contents at the source. Without it,
/// opposing room portals map their far room onto the camera side of the source
/// surface (or behind the camera), which can make the opening look empty even
/// though the peer transform itself is algebraically valid.
pub fn portal_view_map(source: Mat4, target: Mat4) -> Mat4 {
    source * Mat4::from_rotation_y(std::f32::consts::PI) * target.inverse()
}

// ── Portal pose ───────────────────────────────────────────────────────────────

/// Position + orientation of a portal surface.
///
/// Rigid (rotation + translation, unit scale). Built from a look-at so that
/// `forward()` follows the -Z convention.
#[derive(Debug, Clone, Copy)]
pub struct PortalPose {
    /// Local-frame → world transform. Must be rigid.
    pub transform: Mat4,
}

impl PortalPose {
    /// World-space position of the surface.
    pub fn position(&self) -> Vec3 {
        self.transform.w_axis.truncate()
    }

    /// Surface normal pointing into the portal's own world (the -Z axis).
    pub fn forward(&self) -> Vec3 {
        -self.transform.z_axis.truncate().normalize()
    }

    /// Surface up vector (the +Y axis).
    pub fn up(&self) -> Vec3 {
        self.transform.y_axis.truncate().normalize()
    }

    /// A pose at `position` looking toward `look_at` (the surface's forward).
    pub fn from_look_at(position: Vec3, look_at: Vec3, up: Vec3) -> Self {
        // `look_at_mat4` builds a *view* matrix (world → local — the same
        // matrix `Camera::perspective_look_at` uses directly as `Camera.view`
        // for GPU rendering, confirmed empirically: it sends `position`
        // itself to the local origin). Every formula below (`position()`,
        // `forward()`, `pair_map()`) needs the opposite: a *placement*
        // matrix (local → world, i.e. this pose's own "camera-to-world"),
        // so its w_axis is directly this pose's world position and its
        // z_axis is directly its world-space orientation. Inverting once
        // here — a rigid transform, so cheap and exact — keeps every other
        // formula in this file simple and correct instead of threading a
        // view/placement distinction through all of them.
        Self {
            transform: glam::camera::rh::view::look_at_mat4(position, look_at, up).inverse(),
        }
    }
}

// ── Portal pair ───────────────────────────────────────────────────────────────

/// Two portal surfaces connected by a `pair_map`.
#[derive(Debug, Clone, Copy)]
pub struct PortalPair {
    /// Source surface.
    pub a: PortalPose,
    /// Destination surface.
    pub b: PortalPose,
}

impl PortalPair {
    /// The rigid transform mapping A's frame to B's frame.
    ///
    /// Rendering-side use: content actually near B is drawn a second time
    /// through this transform (as one more coordinate space, see
    /// `helio_pass_object_batch::coordinate_space`) to produce what's visible through A.
    pub fn pair_map(&self) -> Mat4 {
        self.b.transform * self.a.transform.inverse()
    }

    /// The inverse mapping (B's frame to A's frame).
    pub fn pair_map_inverse(&self) -> Mat4 {
        self.a.transform * self.b.transform.inverse()
    }

    /// The target-content-to-source-opening map used by portal rendering.
    pub fn view_map_inverse(&self) -> Mat4 {
        portal_view_map(self.a.transform, self.b.transform)
    }

    /// Map a world point through the portal (A's frame → B's frame).
    pub fn map_point(&self, world: Vec3) -> Vec3 {
        self.pair_map().transform_point3(world)
    }

    /// Map a world direction through the portal (rotation only, no translation).
    pub fn map_direction(&self, world_dir: Vec3) -> Vec3 {
        self.pair_map().transform_vector3(world_dir)
    }

    /// Teleport a ray through the portal — used to carry the camera/player
    /// through on crossing, and for CPU-side pick-through queries.
    pub fn teleport_ray(&self, origin: Vec3, direction: Vec3) -> (Vec3, Vec3) {
        (self.map_point(origin), self.map_direction(direction))
    }
}

// ── Crossing detection ────────────────────────────────────────────────────────

/// Signed distance from `point` to the plane through `plane_origin` with normal
/// `plane_normal` (positive on the normal side).
pub fn plane_signed_distance(plane_origin: Vec3, plane_normal: Vec3, point: Vec3) -> f32 {
    (point - plane_origin).dot(plane_normal)
}

/// True when the camera crossed `pose`'s plane between `prev_pos` and `cur_pos`
/// and `cur_pos` projects within `±half_extent` of the portal's local X/Y.
pub fn crossing_detected(
    prev_pos: Vec3,
    cur_pos: Vec3,
    pose: &PortalPose,
    half_extent: Vec2,
) -> bool {
    let n = pose.forward();
    let d_prev = plane_signed_distance(pose.position(), n, prev_pos);
    let d_cur = plane_signed_distance(pose.position(), n, cur_pos);
    // Sign change with an epsilon: landing exactly on the plane counts as a
    // crossing (otherwise the player can wedge against the surface).
    let crossed = (d_prev > 0.0) != (d_cur > 0.0) || d_cur.abs() < 1e-5;
    if !crossed {
        return false;
    }
    let local = pose.transform.inverse().transform_point3(cur_pos);
    local.x.abs() <= half_extent.x && local.y.abs() <= half_extent.y
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pose_at(pos: Vec3, forward: Vec3) -> PortalPose {
        PortalPose::from_look_at(pos, pos + forward, Vec3::Y)
    }

    #[test]
    fn pose_forward_follows_negative_z() {
        let p = pose_at(Vec3::ZERO, Vec3::Z);
        assert!((p.forward() - Vec3::Z).length() < 1e-5);
    }

    #[test]
    fn translation_pair_maps_a_to_b() {
        let a = pose_at(Vec3::ZERO, Vec3::Z);
        let b = pose_at(Vec3::new(10.0, 0.0, 0.0), Vec3::Z);
        let pair = PortalPair { a, b };

        // A's own position maps exactly onto B's position.
        let mapped = pair.map_point(a.position());
        assert!((mapped - b.position()).length() < 1e-4);

        // A's forward maps onto B's forward.
        let fwd = pair.map_direction(a.forward());
        assert!((fwd - b.forward()).length() < 1e-4);

        // Round-trip through the inverse restores the original.
        let back = pair.pair_map_inverse().transform_point3(b.position());
        assert!((back - a.position()).length() < 1e-4);
    }

    #[test]
    fn pair_map_inverse_is_the_actual_inverse() {
        let a = pose_at(Vec3::new(1.0, 2.0, 3.0), Vec3::new(1.0, 0.0, 1.0));
        let b = pose_at(Vec3::new(-4.0, 0.0, 6.0), Vec3::new(-1.0, 0.5, 0.2));
        let pair = PortalPair { a, b };

        let round = pair.pair_map() * pair.pair_map_inverse();
        let id = Mat4::IDENTITY.to_cols_array();
        let round = round.to_cols_array();
        for i in 0..16 {
            assert!((round[i] - id[i]).abs() < 1e-3, "index {i}: {round:?}");
        }
    }

    #[test]
    fn view_map_places_target_contents_beyond_an_opposing_source_portal() {
        let source = pose_at(Vec3::new(0.0, 0.0, 6.0), Vec3::NEG_Z);
        let target = pose_at(Vec3::new(0.0, 0.0, -6.0), Vec3::Z);
        let map = PortalPair {
            a: source,
            b: target,
        }
        .view_map_inverse();

        // The target-room center is six units in front of the target portal.
        // It must land six units beyond the source portal, not on the camera
        // side of it.
        let mapped = map.transform_point3(Vec3::ZERO);
        assert!((mapped - Vec3::new(0.0, 0.0, 12.0)).length() < 1e-4);
    }

    #[test]
    fn teleport_ray_maps_point_and_direction() {
        let a = pose_at(Vec3::ZERO, Vec3::Z);
        let b = pose_at(Vec3::new(10.0, 0.0, 0.0), Vec3::Z);
        let pair = PortalPair { a, b };
        let (o, d) = pair.teleport_ray(Vec3::new(0.0, 0.0, 5.0), Vec3::Z);
        assert!((o - Vec3::new(10.0, 0.0, 5.0)).length() < 1e-4);
        assert!((d - Vec3::Z).length() < 1e-4);
    }

    #[test]
    fn crossing_triggers_when_walking_through_inside_bounds() {
        let portal = pose_at(Vec3::ZERO, Vec3::Z);
        let prev = Vec3::new(0.0, 0.0, -1.0);
        let cur = Vec3::new(0.0, 0.0, 1.0);
        assert!(crossing_detected(prev, cur, &portal, Vec2::splat(2.0)));
    }

    #[test]
    fn crossing_ignored_outside_bounds() {
        let portal = pose_at(Vec3::ZERO, Vec3::Z);
        let prev = Vec3::new(5.0, 0.0, -1.0);
        let cur = Vec3::new(5.0, 0.0, 1.0);
        assert!(!crossing_detected(prev, cur, &portal, Vec2::splat(2.0)));
    }

    #[test]
    fn crossing_ignored_for_parallel_motion() {
        let portal = pose_at(Vec3::ZERO, Vec3::Z);
        let prev = Vec3::new(0.0, 0.0, -1.0);
        let cur = Vec3::new(0.0, 0.0, -0.5);
        assert!(!crossing_detected(prev, cur, &portal, Vec2::splat(2.0)));
    }

    #[test]
    fn crossing_not_detected_when_stopping_short() {
        let portal = pose_at(Vec3::ZERO, Vec3::Z);
        let prev = Vec3::new(0.0, 0.0, -1.0);
        let cur = Vec3::new(0.0, 0.0, -1e-3);
        // 1mm short of the surface: outside the on-plane epsilon, no sign change.
        assert!(!crossing_detected(prev, cur, &portal, Vec2::splat(2.0)));
    }

    #[test]
    fn crossing_detected_when_landing_within_epsilon() {
        let portal = pose_at(Vec3::ZERO, Vec3::Z);
        let prev = Vec3::new(0.0, 0.0, -1.0);
        let cur = Vec3::new(0.0, 0.0, -1e-6);
        // Within the on-plane epsilon: counts as a crossing so the player can
        // not wedge against the surface.
        assert!(crossing_detected(prev, cur, &portal, Vec2::splat(2.0)));
    }

    /// The regression this whole fix exists for: `from_look_at` used to store
    /// the raw view matrix `look_at_mat4` returns (world → local — confirmed
    /// empirically by checking it sends `position` itself to the origin,
    /// same as `Camera.view`), not its inverse. That bug was invisible in
    /// every other test above because they either place `a` at the world
    /// origin (where the view/placement distinction vanishes: `-R*0 = 0`
    /// either way) or only check round-trip identities (`pair_map() *
    /// pair_map_inverse() == I`), which hold for *any* invertible matrix
    /// regardless of what it represents. This test uses a non-trivial,
    /// non-axis-aligned pose and checks `position()`/`forward()` against the
    /// values actually passed in — the one thing the others couldn't catch.
    #[test]
    fn position_and_forward_match_the_requested_pose_off_axis() {
        let eye = Vec3::new(3.0, 1.0, -2.0);
        let dir = Vec3::new(1.0, 0.0, 1.0).normalize();
        let pose = PortalPose::from_look_at(eye, eye + dir, Vec3::Y);
        assert!(
            (pose.position() - eye).length() < 1e-4,
            "got {:?}",
            pose.position()
        );
        assert!(
            (pose.forward() - dir).length() < 1e-4,
            "got {:?}",
            pose.forward()
        );
    }
}

#[cfg(test)]
mod corridor_regression {
    //! Reproduces `examples/portals_demo.rs`'s exact two-portal hallway setup
    //! and checks the numbers the render pipeline actually depends on: that
    //! content near one portal's `b` surface lands, through
    //! `pair_map_inverse`, inside the *other* portal's clip window (the
    //! world→local check `gbuffer_portal.wgsl`'s fragment shader performs).
    //! A regression test for "the ends just end" — if this ever starts
    //! failing again, that bug is back.
    use super::*;
    use glam::Vec3;

    fn corridor_portal_poses() -> (PortalPose, PortalPose) {
        let pose_near = PortalPose::from_look_at(
            Vec3::new(0.0, 1.5, 17.0),
            Vec3::new(0.0, 1.5, 18.0),
            Vec3::Y,
        );
        let pose_far = PortalPose::from_look_at(
            Vec3::new(0.0, 1.5, -17.0),
            Vec3::new(0.0, 1.5, -18.0),
            Vec3::Y,
        );
        (pose_near, pose_far)
    }

    #[test]
    fn far_surface_maps_exactly_onto_near_surface() {
        let (pose_near, pose_far) = corridor_portal_poses();
        let pair = PortalPair {
            a: pose_near,
            b: pose_far,
        };
        let mapped = pair
            .pair_map_inverse()
            .transform_point3(pose_far.position());
        assert!(
            (mapped - pose_near.position()).length() < 1e-4,
            "got {mapped:?}"
        );
    }

    #[test]
    fn content_just_past_far_surface_lands_inside_near_portals_clip_window() {
        let (pose_near, pose_far) = corridor_portal_poses();
        let pair = PortalPair {
            a: pose_near,
            b: pose_far,
        };
        let half_extent = (2.0f32, 1.5f32);

        // A point slightly beyond pose_far, in its forward direction — the
        // kind of geometry that should duplicate through the near portal.
        let near_far_point = pose_far.position() + pose_far.forward() * 0.3;
        let mapped = pair.pair_map_inverse().transform_point3(near_far_point);
        let local = pose_near.transform.inverse().transform_point3(mapped);

        assert!(local.x.abs() <= half_extent.0, "local={local:?}");
        assert!(local.y.abs() <= half_extent.1, "local={local:?}");
        assert!(
            local.z <= 0.0,
            "local={local:?} — clip test discards local.z > 0"
        );
    }
}

#[cfg(test)]
mod infinite_tunnel_regression {
    //! Replicates `examples/infinite_tunnel.rs`'s exact vertical-shift portal
    //! setup (both surfaces face the same direction, `pair_map_inverse` is a
    //! pure +Y lift) and checks the numbers the render pipeline depends on:
    //! that continuation copies buried HIDE_OFFSET below the corridor, once
    //! mapped through `pair_map_inverse` and read back through the near
    //! surface's inverse, pass `gbuffer_portal.wgsl`'s clip test
    //! (`|local.x| <= half_extent && |local.y| <= half_extent && local.z <= 0`).
    //! This is the pipeline's "the continuation actually lands inside the
    //! portal opening" guarantee for the infinite-tunnel demo.
    use super::*;
    use glam::Vec3;

    const HALF_WIDTH: f32 = 2.0;
    const HALF_HEIGHT: f32 = 1.5;
    const HALF_LENGTH: f32 = 8.0;
    const COPY_STRIDE: f32 = 2.0 * HALF_LENGTH;
    const PORTAL_Z: f32 = HALF_LENGTH - 1.0;
    const HIDE_OFFSET: f32 = 500.0;

    fn portal_poses() -> (PortalPose, PortalPose) {
        let near_a = PortalPose::from_look_at(
            Vec3::new(0.0, HALF_HEIGHT, PORTAL_Z),
            Vec3::new(0.0, HALF_HEIGHT, PORTAL_Z + 1.0),
            Vec3::Y,
        );
        let near_b = PortalPose::from_look_at(
            Vec3::new(0.0, HALF_HEIGHT - HIDE_OFFSET, PORTAL_Z),
            Vec3::new(0.0, HALF_HEIGHT - HIDE_OFFSET, PORTAL_Z + 1.0),
            Vec3::Y,
        );
        (near_a, near_b)
    }

    #[test]
    fn pair_map_inverse_is_a_pure_vertical_lift() {
        let (a, b) = portal_poses();
        let pair = PortalPair { a, b };
        let mapped = pair
            .pair_map_inverse()
            .transform_point3(Vec3::new(1.0, 2.0, 3.0));
        assert!(
            (mapped - Vec3::new(1.0, 502.0, 3.0)).length() < 1e-3,
            "got {mapped:?}"
        );
    }

    #[test]
    fn mapped_continuation_passes_the_fragment_clip() {
        let (a, b) = portal_poses();
        let pair = PortalPair { a, b };
        let half_extent = Vec2::new(HALF_WIDTH, HALF_HEIGHT);

        for copy in 1..=10i32 {
            // A buried copy's far-end floor vertex, in the copy's sublevel
            // frame: segment-local (x, floor_y, ±HALF_LENGTH) then placed
            // (0, -HIDE_OFFSET, copy*COPY_STRIDE).
            for z_local in [HALF_LENGTH, -HALF_LENGTH] {
                for x_local in [-1.5, 0.0, 1.5] {
                    let buried = Vec3::new(x_local, 0.02, copy as f32 * COPY_STRIDE + z_local);
                    let in_sublevel = Vec3::new(buried.x, buried.y - HIDE_OFFSET, buried.z);
                    // `own_space` is identity rotation + sublevel translation;
                    // `portal_space` is pair_map_inverse.
                    let portal_space = pair.pair_map_inverse();
                    let mapped = portal_space.transform_point3(in_sublevel);
                    let local = a.transform.inverse().transform_point3(mapped);

                    assert!(
                        local.z <= 0.0,
                        "copy={copy} buried={buried:?} -> mapped={mapped:?} local={local:?} — clip discards local.z > 0"
                    );
                    assert!(
                        local.x.abs() <= half_extent.x && local.y.abs() <= half_extent.y,
                        "copy={copy} local={local:?} — outside opening half-extent {half_extent:?}"
                    );
                }
            }
        }
    }
}
