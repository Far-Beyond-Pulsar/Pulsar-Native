//! Camera selection for play-mode frames (Pulsar-Native#637).
//!
//! Reads the frame's view camera from the ONE shared world instead of
//! special-casing freecam everywhere. In order of precedence:
//!
//! 1. a camera gameplay pushed per window with `WindowManager::set_camera`
//!    (the hosts check that before calling in here);
//! 2. the **view target**'s camera: the player's view target
//!    ([`ViewTargets`](crate::view_target::ViewTargets), set by possession to
//!    the pawn) looks through the best enabled `CameraComponent` on that
//!    object, else on an object beneath it;
//! 3. the best enabled `CameraComponent` in the world: the highest priority,
//!    ties to the earlier object;
//! 4. the first live `ObjectType::Camera` entity (the editor's camera
//!    object), positioned by its own `Transform`, with the free camera's lens;
//! 5. the freecam (the hosts' fallback when this returns `None`).
//!
//! A camera is positioned by its owner object's world matrix, resolved
//! through the hierarchy, so a camera parented under a character follows it
//! the next frame. Which `CameraComponent` is the better one, and what its
//! projection means, is Helio's (`helio_component::components::camera_component`).
//!
//! Where the lens and the camera post-process come from is not decided here:
//! they are matched to the render view (`view_id`, 0 for these windows), not
//! to the camera that is the view.

use crate::freecam::FreeCam;
use crate::window::RenderCamera;
use engine_backend::scene::{hierarchy_transform::world_matrix_of, SceneWorldExt};
use helio_component::components::{
    camera_on_object, highest_priority_camera, ResolvedProjection, SelectedCamera,
};
use pulsar_scenedb::{Entity, World};

/// `owner`'s world pose as a [`RenderCamera`] with the lens `lens` describes.
///
/// The camera looks down its own -Z with its own +Y up, rotated as every
/// object is (`Transform.rotation` = Euler degrees YXZ, `[pitch, yaw,
/// roll]`), so a camera placed on a planet's side stands on it. The eye is
/// the f64 world position.
fn render_camera(world: &World, owner: Entity, lens: Option<ResolvedProjection>) -> Option<RenderCamera> {
    let matrix = world_matrix_of(world, owner)?;
    let (_, rotation, position) = matrix.to_scale_rotation_translation();
    let rotation = rotation.as_quat();
    let free = FreeCam::default();
    let mut camera = RenderCamera {
        position,
        forward: rotation * glam::Vec3::NEG_Z,
        up: rotation * glam::Vec3::Y,
        fov_y: free.fov_y,
        near: free.near,
        far: free.far,
        orthographic_half_height: None,
    };
    match lens {
        Some(ResolvedProjection::Perspective { fov_y, near, far }) => {
            camera.fov_y = fov_y;
            camera.near = near;
            camera.far = far;
        }
        Some(ResolvedProjection::Orthographic { half_height, near, far }) => {
            camera.near = near;
            camera.far = far;
            camera.orthographic_half_height = Some(half_height);
        }
        None => {}
    }
    Some(camera)
}

fn component_camera(world: &World, selected: Option<SelectedCamera>) -> Option<RenderCamera> {
    let selected = selected?;
    render_camera(
        world,
        selected.owner,
        Some(selected.camera.resolved_projection()),
    )
}

/// The camera `view_target` looks through, if it (or an object beneath it)
/// has an enabled camera that can be placed.
fn view_target_camera(world: &World, view_target: Entity) -> Option<RenderCamera> {
    component_camera(world, camera_on_object(world, view_target))
}

/// Whether the view target's own camera is the frame's camera (the free camera
/// stands down for it, see `player_input::PossessionGate`). `false` for no
/// target, or a target with no usable camera: then the world's other cameras,
/// or the free camera, show the game.
pub fn view_target_drives_camera(
    store: &pulsar_scenedb::SceneDb,
    view_target: Option<Entity>,
) -> bool {
    view_target.is_some_and(|target| view_target_camera(&store.world, target).is_some())
}

/// The frame's camera from the top of the precedence: `pushed` (gameplay's
/// `WindowManager::set_camera` for the window) wins, then the shared world's
/// ([`select_world_camera`]). `None` means the host's free camera.
pub fn camera_for_frame(
    pushed: Option<RenderCamera>,
    store: &pulsar_scenedb::SceneDb,
    view_target: Option<Entity>,
) -> Option<RenderCamera> {
    pushed.or_else(|| select_world_camera(store, view_target))
}

/// Resolve the shared-world camera, if any, in the precedence above.
/// `view_target` is the local player's view target.
pub fn select_world_camera(
    store: &pulsar_scenedb::SceneDb,
    view_target: Option<Entity>,
) -> Option<RenderCamera> {
    let world = &store.world;
    if let Some(camera) = view_target.and_then(|target| view_target_camera(world, target)) {
        return Some(camera);
    }
    if let Some(camera) = component_camera(world, highest_priority_camera(world)) {
        return Some(camera);
    }
    // The editor's camera object: only a transform, so the free camera's lens.
    world
        .camera_entities()
        .into_iter()
        .find_map(|entity| render_camera(world, entity, None))
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

        let resolved = select_world_camera(&store, None).expect("world camera");
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

        let resolved = select_world_camera(&store, None).expect("world camera");
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
        assert!(select_world_camera(&store, None).is_none());

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
            select_world_camera(&store, None).is_none(),
            "meshes must not be mistaken for cameras"
        );
    }

    use crate::scene::attachments::{self, NewInstance};
    use helio_component::components::{CameraComponent, CameraProjectionKind, CAMERA_CLASS_NAME};

    fn placed(store: &mut pulsar_scenedb::SceneDb, name: &str, position: [f64; 3]) -> Entity {
        let entity = store
            .world
            .spawn_object(engine_backend::scene::SpawnObject::new(name))
            .unwrap();
        store.world.insert(
            entity,
            Transform {
                position,
                rotation: [0.0; 3],
                scale: [1.0; 3],
            },
        );
        entity
    }

    fn give_camera(store: &mut pulsar_scenedb::SceneDb, owner: Entity, camera: CameraComponent) {
        let instance =
            attachments::spawn_instance(&mut store.world, owner, NewInstance::new(CAMERA_CLASS_NAME))
                .unwrap();
        store.world.insert(instance, camera);
    }

    fn priority(priority: i32) -> CameraComponent {
        CameraComponent {
            priority,
            ..Default::default()
        }
    }

    /// pushed camera > view target > highest priority CameraComponent >
    /// legacy Camera object > nothing (the free camera).
    #[test]
    fn precedence_runs_pushed_then_view_target_then_priority_then_legacy_then_free() {
        let mut store = engine_backend::scene::new_scene();
        assert!(camera_for_frame(None, &store, None).is_none(), "free camera");

        let legacy = placed(&mut store, "Legacy", [1.0, 0.0, 0.0]);
        store.world.insert(legacy, ObjectType::Camera);
        let legacy_eye = camera_for_frame(None, &store, None).unwrap().position;
        assert_eq!(legacy_eye, glam::DVec3::new(1.0, 0.0, 0.0));

        let low = placed(&mut store, "Low", [2.0, 0.0, 0.0]);
        let high = placed(&mut store, "High", [3.0, 0.0, 0.0]);
        let target = placed(&mut store, "Target", [4.0, 0.0, 0.0]);
        give_camera(&mut store, low, priority(1));
        give_camera(&mut store, high, priority(8));
        give_camera(&mut store, target, priority(-5));
        assert_eq!(
            camera_for_frame(None, &store, None).unwrap().position.x,
            3.0,
            "the highest priority component outranks the legacy camera"
        );
        assert_eq!(
            camera_for_frame(None, &store, Some(target)).unwrap().position.x,
            4.0,
            "the view target outranks any priority"
        );

        let pushed = RenderCamera::look_at(glam::DVec3::splat(9.0), glam::DVec3::ZERO, glam::Vec3::Y);
        assert_eq!(
            camera_for_frame(Some(pushed), &store, Some(target)).unwrap().position,
            glam::DVec3::splat(9.0),
            "a camera gameplay pushed outranks everything"
        );
    }

    #[test]
    fn a_view_target_without_a_camera_falls_through_to_the_rest() {
        let mut store = engine_backend::scene::new_scene();
        let bare = placed(&mut store, "Bare", [0.0; 3]);
        let legacy = placed(&mut store, "Legacy", [7.0, 0.0, 0.0]);
        store.world.insert(legacy, ObjectType::Camera);
        assert!(!view_target_drives_camera(&store, Some(bare)));
        assert_eq!(select_world_camera(&store, Some(bare)).unwrap().position.x, 7.0);
        // A dead target is no target.
        let gone = placed(&mut store, "Gone", [0.0; 3]);
        store.world.despawn_tree(gone);
        assert!(!view_target_drives_camera(&store, Some(gone)));
        assert_eq!(select_world_camera(&store, Some(gone)).unwrap().position.x, 7.0);
    }

    #[test]
    fn fov_near_far_and_projection_come_from_the_component() {
        let mut store = engine_backend::scene::new_scene();
        let owner = placed(&mut store, "Cam", [0.0; 3]);
        give_camera(
            &mut store,
            owner,
            CameraComponent {
                fov_y_deg: 60.0,
                near: 0.5,
                far: 300.0,
                ..Default::default()
            },
        );
        let seen = select_world_camera(&store, None).unwrap();
        assert!((seen.fov_y - 60f32.to_radians()).abs() < 1e-6);
        assert_eq!((seen.near, seen.far), (0.5, 300.0));
        assert_eq!(seen.orthographic_half_height, None);

        let ortho = placed(&mut store, "Ortho", [0.0; 3]);
        give_camera(
            &mut store,
            ortho,
            CameraComponent {
                priority: 1,
                projection: CameraProjectionKind::Orthographic,
                orthographic_size: 12.0,
                ..Default::default()
            },
        );
        let seen = select_world_camera(&store, None).unwrap();
        assert_eq!(seen.orthographic_half_height, Some(12.0));
        assert!(matches!(
            seen.frame_view(1.5).projection,
            engine_backend::subsystems::render::FrameProjection::Orthographic { half_height }
                if half_height == 12.0
        ));

        // The legacy camera object keeps the free camera's lens.
        let mut legacy_store = engine_backend::scene::new_scene();
        let legacy = placed(&mut legacy_store, "Legacy", [0.0; 3]);
        legacy_store.world.insert(legacy, ObjectType::Camera);
        let lens = FreeCam::default();
        let seen = select_world_camera(&legacy_store, None).unwrap();
        assert_eq!((seen.fov_y, seen.near, seen.far), (lens.fov_y, lens.near, lens.far));
    }

    /// A camera under a moving, turning object follows it through the
    /// hierarchy.
    #[test]
    fn a_camera_on_a_descendant_follows_its_parent() {
        let mut store = engine_backend::scene::new_scene();
        let pawn = placed(&mut store, "Pawn", [10.0, 0.0, 0.0]);
        let head = placed(&mut store, "Head", [0.0, 1.7, 0.0]);
        store.world.reparent(head, Some(pawn)).unwrap();
        give_camera(&mut store, head, CameraComponent::default());

        let eye = select_world_camera(&store, Some(pawn)).unwrap();
        assert_eq!(eye.position, glam::DVec3::new(10.0, 1.7, 0.0));
        assert!(eye.forward.abs_diff_eq(glam::Vec3::NEG_Z, 1e-6));

        // Move and turn the pawn 90 degrees to its left (yaw +90 faces -X).
        store.world.insert(
            pawn,
            Transform {
                position: [0.0, 0.0, 20.0],
                rotation: [0.0, 90.0, 0.0],
                scale: [1.0; 3],
            },
        );
        let eye = select_world_camera(&store, Some(pawn)).unwrap();
        assert_eq!(eye.position, glam::DVec3::new(0.0, 1.7, 20.0));
        assert!(eye.forward.abs_diff_eq(glam::Vec3::NEG_X, 1e-5), "{:?}", eye.forward);
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
        assert!(select_world_camera(&store, None).is_some());
    }
}
