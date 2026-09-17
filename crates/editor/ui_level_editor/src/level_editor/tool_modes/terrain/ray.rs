//! Viewport → world ray construction for terrain tool modes.
//!
//! The renderer builds its own pick ray on the render thread
//! (`HelioRenderer::build_pick_ray`) from the live camera. A tool mode runs on
//! the UI thread and only has the [`CameraFrame`]/[`ViewportFrame`] the shell
//! handed it, so it must reconstruct the same ray itself. The projection
//! parameters below are duplicated from that function on purpose: if the two
//! disagree, the brush lands somewhere other than where the user clicked.

use glam::{Mat4, Vec3};

use engine_backend::services::terrain_edit::Ray3;

use crate::level_editor::tool_modes::{CameraFrame, ViewportFrame};

/// Vertical field of view used by the editor viewport camera.
///
/// Must match `HelioRenderer::build_pick_ray` and the `Camera::perspective_look_at`
/// call in `render_frame`. Note this is *not* `CameraFrame::fov` — that field
/// carries a degrees value the shell fills in with a placeholder and the
/// renderer never reads.
const VIEWPORT_FOV_RADIANS: f32 = std::f32::consts::FRAC_PI_4;
const NEAR_PLANE_M: f32 = 0.1;
const FAR_PLANE_M: f32 = 10_000.0;

/// Build a world-space ray from normalized viewport coordinates.
///
/// `norm_x`/`norm_y` are the shell's `[0,1]` viewport-local coordinates, with
/// the origin at the top-left — the same convention the pointer mailbox uses.
/// Returns `None` for a degenerate viewport.
pub fn viewport_ray(
    camera: CameraFrame,
    viewport: ViewportFrame,
    norm_x: f32,
    norm_y: f32,
) -> Option<Ray3> {
    if !(viewport.width > 0.0 && viewport.height > 0.0) {
        return None;
    }
    let ndc_x = norm_x.clamp(0.0, 1.0) * 2.0 - 1.0;
    let ndc_y = 1.0 - norm_y.clamp(0.0, 1.0) * 2.0;

    let position = Vec3::from_array(camera.position);
    let forward = camera_forward(camera);
    let projection = Mat4::perspective_rh(
        VIEWPORT_FOV_RADIANS,
        viewport.width / viewport.height,
        NEAR_PLANE_M,
        FAR_PLANE_M,
    );
    let view = Mat4::look_at_rh(position, position + forward, Vec3::Y);
    let inverse = (projection * view).inverse();
    let near = inverse.project_point3(Vec3::new(ndc_x, ndc_y, 0.0));
    let far = inverse.project_point3(Vec3::new(ndc_x, ndc_y, 1.0));
    let direction = (far - near).normalize_or_zero();
    if direction == Vec3::ZERO {
        return None;
    }
    Some(Ray3 {
        origin: near.to_array(),
        direction: direction.to_array(),
    })
}

/// The camera's forward vector, matching the renderer's yaw/pitch convention.
pub fn camera_forward(camera: CameraFrame) -> Vec3 {
    let (sin_yaw, cos_yaw) = camera.yaw.sin_cos();
    let (sin_pitch, cos_pitch) = camera.pitch.sin_cos();
    Vec3::new(sin_yaw * cos_pitch, sin_pitch, -cos_yaw * cos_pitch)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn viewport() -> ViewportFrame {
        ViewportFrame {
            width: 1920.0,
            height: 1080.0,
        }
    }

    #[test]
    fn a_ray_through_the_viewport_centre_points_along_the_camera_forward() {
        let camera = CameraFrame {
            position: [10.0, 5.0, -3.0],
            yaw: 0.7,
            pitch: -0.2,
            fov: 60.0,
        };
        let ray = viewport_ray(camera, viewport(), 0.5, 0.5).unwrap();
        let direction = Vec3::from_array(ray.direction);
        let expected = camera_forward(camera);
        assert!(
            direction.dot(expected) > 0.9999,
            "centre ray {direction:?} should match forward {expected:?}"
        );
    }

    #[test]
    fn the_ray_origin_sits_on_the_near_plane_in_front_of_the_camera() {
        let camera = CameraFrame::default();
        let ray = viewport_ray(camera, viewport(), 0.5, 0.5).unwrap();
        let offset = Vec3::from_array(ray.origin) - Vec3::from_array(camera.position);
        assert!(
            (offset.length() - NEAR_PLANE_M).abs() < 1e-3,
            "origin should be one near-plane away, got {}",
            offset.length()
        );
    }

    #[test]
    fn horizontal_screen_position_maps_to_horizontal_ray_deflection() {
        let camera = CameraFrame::default();
        let left = viewport_ray(camera, viewport(), 0.1, 0.5).unwrap();
        let right = viewport_ray(camera, viewport(), 0.9, 0.5).unwrap();
        // Default camera looks down -Z, so +X in the viewport is +X in world.
        assert!(left.direction[0] < 0.0, "got {:?}", left.direction);
        assert!(right.direction[0] > 0.0, "got {:?}", right.direction);
    }

    #[test]
    fn vertical_screen_position_maps_to_vertical_ray_deflection() {
        let camera = CameraFrame::default();
        let top = viewport_ray(camera, viewport(), 0.5, 0.05).unwrap();
        let bottom = viewport_ray(camera, viewport(), 0.5, 0.95).unwrap();
        assert!(top.direction[1] > 0.0, "got {:?}", top.direction);
        assert!(bottom.direction[1] < 0.0, "got {:?}", bottom.direction);
    }

    #[test]
    fn a_degenerate_viewport_yields_no_ray_rather_than_a_nan_one() {
        let camera = CameraFrame::default();
        assert!(viewport_ray(
            camera,
            ViewportFrame {
                width: 0.0,
                height: 0.0,
            },
            0.5,
            0.5,
        )
        .is_none());
    }
}
