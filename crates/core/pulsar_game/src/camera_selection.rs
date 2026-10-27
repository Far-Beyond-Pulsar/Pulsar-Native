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

use crate::freecam::FreeCam;
use crate::window::RenderCamera;
use engine_backend::scene::SceneWorldExt;

/// Resolve the shared-world camera, if any: the first live
/// `ObjectType::Camera` entity's world transform as a [`RenderCamera`].
///
/// The camera looks down its own -Z with its own +Y up, rotated as every
/// object is (`Transform.rotation` = Euler degrees YXZ, `[pitch, yaw,
/// roll]`), so a camera placed on a planet's side stands on it. The eye is
/// the f64 world position. Anything a bare transform can't express
/// (fov/near/far) is the freecam's.
pub fn select_world_camera(store: &pulsar_scenedb::SceneDb) -> Option<RenderCamera> {
    let world = &store.world;
    for entity in world.camera_entities() {
        let Some(matrix) =
            engine_backend::scene::hierarchy_transform::world_matrix_of(world, entity)
        else {
            continue;
        };
        let (_, rotation, position) = matrix.to_scale_rotation_translation();
        let rotation = rotation.as_quat();
        let lens = FreeCam::default();
        return Some(RenderCamera {
            position,
            forward: rotation * glam::Vec3::NEG_Z,
            up: rotation * glam::Vec3::Y,
            fov_y: lens.fov_y,
            near: lens.near,
            far: lens.far,
        });
    }
    None
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
        let cam = store
            .world
            .spawn_object(engine_backend::scene::SpawnObject::new("View").with_id("view"))
            .unwrap();
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
        assert_eq!(resolved.position, glam::DVec3::new(3.0, 4.0, 5.0));
        // Zero rotation looks down -Z, matching the editor camera convention.
        assert!(
            resolved.forward.abs_diff_eq(glam::Vec3::NEG_Z, 1e-6),
            "must face -Z at zero yaw"
        );
        assert!(resolved.up.abs_diff_eq(glam::Vec3::Y, 1e-6));
    }

    /// A camera on a planet's surface, millions of metres out and rolled to
    /// stand on it, keeps its f64 eye and its own up.
    #[test]
    fn a_camera_on_a_planet_keeps_its_eye_and_its_up() {
        let mut store = engine_backend::scene::new_scene();
        let cam = store
            .world
            .spawn_object(engine_backend::scene::SpawnObject::new("Surface").with_id("surface"))
            .unwrap();
        store.world.insert(cam, ObjectType::Camera);
        // On the planet's -X side (radius 6371 km), 1.7 m above it: rolled
        // 90 degrees, its up is -X.
        let eye = [-6_371_001.7, 0.25, 0.125];
        store.world.insert(
            cam,
            Transform {
                position: eye,
                rotation: [0.0, 0.0, 90.0],
                scale: [1.0; 3],
            },
        );

        let resolved = select_world_camera(&store).expect("world camera");
        assert!((resolved.position - glam::DVec3::from_array(eye)).length() < 1e-6);
        assert!(
            resolved.up.abs_diff_eq(glam::Vec3::NEG_X, 1e-5),
            "{:?}",
            resolved.up
        );
        assert!(resolved.forward.abs_diff_eq(glam::Vec3::NEG_Z, 1e-5));
    }

    #[test]
    fn non_camera_objects_and_empty_worlds_yield_none() {
        let mut store = engine_backend::scene::new_scene();
        assert!(select_world_camera(&store).is_none());

        let plain = store
            .world
            .spawn_object(engine_backend::scene::SpawnObject::new("Cube"))
            .unwrap();
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
        let cam = store
            .world
            .spawn_object(engine_backend::scene::SpawnObject::new("Cam"))
            .unwrap();
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
