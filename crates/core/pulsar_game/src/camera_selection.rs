//! Camera selection for play-mode frames (Pulsar-Native#637).
//!
//! Reads the frame's view camera from the ONE shared world instead of
//! special-casing freecam everywhere: an entity the editor classified as
//! `ObjectType::Camera` drives the view, positioned/oriented by its own
//! `Transform` -- so moving that entity (editor gizmo, script, gameplay
//! code) moves the rendered camera on the next frame. Gameplay code can
//! still override per-window via `WindowManager::set_camera`, which keeps
//! highest precedence; the freecam remains the fallback when the world has
//! no camera at all.
//!
//! There is no dedicated `CameraComponent` class yet -- `ObjectType::Camera`
//! + `Transform` is today's whole camera contract. When a real component
//! lands (projection fields, per-camera near/far), this module is the one
//! place that switches over.

use engine_backend::scene::SceneWorldExt;
use crate::freecam::FreeCam;
use crate::window::RenderCamera;


/// Resolve the shared-world camera, if any: the first live
/// `ObjectType::Camera` entity's transform as a [`RenderCamera`].
///
/// Rotation convention matches the scene format's (`Transform.rotation` =
/// Euler degrees YXZ, `[pitch, yaw, roll]`), reusing [`FreeCam`]'s look-dir
/// math so a world camera and the freecam agree on orientation semantics.
pub fn select_world_camera(store: &pulsar_scenedb::SceneDb) -> Option<RenderCamera> {
    let world = &store.world;
    for entity in world.camera_entities() {
        let Some(transform) = world.get::<engine_backend::scene::Transform>(entity) else {
            continue;
        };
        // Degrees -> radians; yaw around Y, pitch around X (FPS-style).
        let yaw = transform.rotation[1].to_radians();
        let pitch = transform.rotation[0].to_radians();
        return Some(camera_from_pose(transform.position, yaw, pitch));
    }
    None
}

/// Build a [`RenderCamera`] from a world-space position + yaw/pitch, with
/// the same defaults the freecam uses for anything a bare transform can't
/// express (fov/near/far).
fn camera_from_pose(position: [f32; 3], yaw: f32, pitch: f32) -> RenderCamera {
    let freecam = FreeCam::default().place(glam::Vec3::from_array(position), yaw, pitch);
    freecam.to_render_camera()
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_backend::scene::{MeshType, ObjectType, Transform, Visibility};

    /// #637: the rendered camera comes from the shared world, not a
    /// renderer-private copy -- an ObjectType::Camera entity's transform
    /// drives the view.
    #[test]
    fn a_camera_typed_entity_drives_the_view_from_the_shared_world() {
        let mut store = engine_backend::scene::new_scene();
        let cam = store.world.spawn_object(engine_backend::scene::SpawnObject::new("View").with_id("view")).unwrap();
        store.world.insert(cam, ObjectType::Camera);
        store.world.insert(
            cam,
            Transform {
                position: [3.0, 4.0, 5.0],
                rotation: [0.0; 3],
                scale: [1.0; 3],
            },
        );

        let resolved = select_world_camera(&store).expect("world camera");
        assert_eq!(resolved.position, [3.0, 4.0, 5.0]);
        // Zero rotation looks down -Z, matching the editor camera convention.
        assert!(
            resolved.target[2] < resolved.position[2],
            "must face -Z at zero yaw"
        );
    }

    #[test]
    fn non_camera_objects_and_empty_worlds_yield_none() {
        let mut store = engine_backend::scene::new_scene();
        assert!(select_world_camera(&store).is_none());

        let plain = store.world.spawn_object(engine_backend::scene::SpawnObject::new("Cube")).unwrap();
        store.world.insert(plain, ObjectType::Mesh(MeshType::Cube));
        store.world.insert(
            plain,
            Visibility {
                visible: true,
                locked: false,
            },
        );
        assert!(
            select_world_camera(&store).is_none(),
            "meshes must not be mistaken for cameras"
        );
    }

    /// A hidden camera still drives the view (visibility culls its *body*
    /// from rendering, not its viewpoint) -- matching editor behavior where
    /// hiding the camera object doesn't blank the viewport.
    #[test]
    fn a_hidden_camera_still_drives_the_view() {
        let mut store = engine_backend::scene::new_scene();
        let cam = store.world.spawn_object(engine_backend::scene::SpawnObject::new("Cam")).unwrap();
        store.world.insert(cam, ObjectType::Camera);
        store.world.insert(
            cam,
            Visibility {
                visible: false,
                locked: false,
            },
        );
        assert!(select_world_camera(&store).is_some());
    }
}
