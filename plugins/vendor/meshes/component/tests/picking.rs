//! Picking and framing a mesh object: the meshes' content geometry
//! (`engine_backend`'s `ContentGeometryRegistration`) tests the ray against
//! the mesh's triangles and frames its bounding sphere.

use engine_backend::subsystems::render::helio_renderer::SceneInteraction;
use glam::{DVec3, Vec3};
use meshes_component::components::{MeshGeometryKey, StaticMeshComponent};
use pulsar_scene_model::world_ext::SceneWorldExt;
use pulsar_scene_model::{MeshType, ObjectType, SpawnObject, Transform};
use pulsar_scenedb::{Entity, World};

fn spawn_cube(world: &mut World, id: &str, z: f64) {
    world
        .spawn_object(
            SpawnObject::new(id)
                .with_id(id)
                .with_object_type(ObjectType::Mesh(MeshType::Cube))
                .with_transform(Transform {
                    position: [0.0, 0.0, z],
                    ..Default::default()
                }),
        )
        .unwrap();
}

#[test]
fn scene_pick_does_not_select_empty_aabb_space_inside_mesh_bounds() {
    let mut world = World::new();
    spawn_cube(&mut world, "triangle", -3.0);
    let entity = world.entity_for("triangle").unwrap();
    {
        let mut transform = world.get_mut::<Transform>(entity).unwrap();
        transform.rotation = [0.0, 35.0, 0.0];
        transform.scale = [-2.0, 0.5, 1.5];
    }
    let vertex = |position| {
        helio_mesh_api::PackedVertex::from_components(
            position,
            [0.0, 0.0, 1.0],
            [0.0; 2],
            [1.0, 0.0, 0.0],
            1.0,
        )
    };
    let mesh = StaticMeshComponent {
        bounds_local: [0.0, 0.0, 0.0, 2.0],
        vertices: vec![
            vertex([-1.0, -1.0, 0.0]),
            vertex([1.0, -1.0, 0.0]),
            vertex([-1.0, 1.0, 0.0]),
        ],
        indices: vec![0, 1, 2],
        geometry_key: MeshGeometryKey(0x705),
        ..Default::default()
    };
    pulsar_world_registry::attach_value(&mut world, entity, mesh).unwrap();

    let interaction = SceneInteraction::default();
    assert_eq!(
        interaction.pick(&world, DVec3::ZERO, -Vec3::Z),
        Some("triangle".into())
    );
    // This ray is inside the component's broad-phase box but in the
    // missing corner of its triangle, so it must not select the mesh.
    assert_eq!(
        interaction.pick(&world, DVec3::ZERO, Vec3::new(0.8, 0.8, -3.0)),
        None
    );
}

/// A mesh object attached a `StaticMeshComponent` whose local bounding
/// sphere has `radius`, at `z`.
fn spawn_mesh(world: &mut World, id: &str, z: f64, radius: f32) -> Entity {
    spawn_cube(world, id, z);
    let owner = world.entity_for(id).unwrap();
    let mesh = StaticMeshComponent {
        bounds_local: [0.0, 0.0, 0.0, radius],
        ..Default::default()
    };
    pulsar_world_registry::attach_value(world, owner, mesh).unwrap();
    owner
}

/// A click on a mesh's visible extent picks it, not only a click within
/// a unit box around its origin.
#[test]
fn a_mesh_is_picked_anywhere_in_its_bounds() {
    let mut world = World::new();
    spawn_mesh(&mut world, "wide", -10.0, 5.0);
    let towards_its_side = Vec3::new(3.0, 0.0, -10.0);
    assert_eq!(
        SceneInteraction::default().pick(&world, DVec3::ZERO, towards_its_side),
        Some("wide".into())
    );
    // Outside its bounds nothing is hit.
    assert_eq!(
        SceneInteraction::default().pick(&world, DVec3::ZERO, Vec3::new(12.0, 0.0, -10.0)),
        None
    );
}

/// A duplicate starts on its source. A click there keeps the selected
/// one of the two, so the copy the editor selected can be picked and
/// dragged instead of always resolving to the same object.
#[test]
fn overlapping_objects_keep_the_selected_one() {
    let mut world = World::new();
    spawn_mesh(&mut world, "a", -10.0, 1.0);
    let copy = spawn_mesh(&mut world, "b", -10.0, 1.0);
    let interaction = SceneInteraction::default();
    assert_eq!(
        interaction.pick(&world, DVec3::ZERO, -Vec3::Z),
        Some("a".into())
    );
    world.select(Some(copy));
    assert_eq!(
        interaction.pick(&world, DVec3::ZERO, -Vec3::Z),
        Some("b".into())
    );
}
