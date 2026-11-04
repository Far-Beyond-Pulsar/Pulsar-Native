//! Camera data extraction from SolidRS

use solid_rs::scene::{Camera, Projection};

/// Camera data from SolidRS (informational only)
///
/// Helio manages cameras independently, so this just extracts
/// the camera parameters for the user to apply if desired.
#[derive(Debug, Clone)]
pub struct CameraData {
    pub name: String,
    pub fov_y: Option<f32>, // Vertical FOV in radians (for perspective)
    pub near: f32,
    pub far: Option<f32>,
    pub is_perspective: bool,
}

/// Extract camera data from a SolidRS camera
pub fn extract_camera_data(camera: &Camera) -> CameraData {
    match &camera.projection {
        Projection::Perspective(persp) => CameraData {
            name: camera.name.clone(),
            fov_y: Some(persp.fov_y),
            near: persp.z_near,
            far: persp.z_far,
            is_perspective: true,
        },
        Projection::Orthographic(ortho) => {
            log::warn!(
                "Orthographic camera '{}' not directly supported - storing as info only",
                camera.name
            );
            CameraData {
                name: camera.name.clone(),
                fov_y: None,
                near: ortho.z_near,
                far: Some(ortho.z_far),
                is_perspective: false,
            }
        }
    }
}
