//! One coordinate-space table, several producers: the spaces objects define
//! (rigid frames, sublevel placements) and a portal projection take their slots
//! from the same allocator, so none overwrites another.

use engine_backend::scene::frames::{frame_table, make_frame, SpaceSlot};
use engine_backend::scene::{SceneWorldExt, SpaceTable, SpawnObject, Transform};
use glam::{Mat4, Vec3};
use helio_pass_portal_cull::{
    components::PortalComponent, PortalProjectionBridge, SubLevelContents, SubLevelResolver,
};
use pulsar_scenedb::World;

#[test]
fn frames_and_a_portal_projection_share_one_table_without_overlap() {
    let mut world = World::new();
    let place = |world: &mut World, x: f64| {
        let object = world
            .spawn_object(SpawnObject::new("o").with_transform(Transform {
                position: [x, 0.0, 0.0],
                ..Transform::default()
            }))
            .unwrap();
        make_frame(world, object).unwrap();
        object
    };
    let ship = place(&mut world, 10.0);
    let room = place(&mut world, 20.0);
    let (ship_slot, room_slot) = (
        world.get::<SpaceSlot>(ship).unwrap().slot,
        world.get::<SpaceSlot>(room).unwrap().slot,
    );
    let mut table = SpaceTable::new(frame_table(&world, glam::DVec3::ZERO));
    assert_eq!(table.matrices().len(), 3, "world and two frames");

    let a = world.spawn();
    let b = world.spawn();
    let mut resolver = SubLevelResolver::new();
    resolver.insert_sublevel(
        0,
        SubLevelContents::new(
            [],
            [
                (
                    a,
                    PortalComponent::new(
                        Some(b),
                        Mat4::from_translation(Vec3::new(2.0, 0.0, 0.0)),
                        [1.0, 1.0],
                    ),
                ),
                (
                    b,
                    PortalComponent::new(
                        Some(a),
                        Mat4::from_translation(Vec3::new(-2.0, 0.0, 0.0)),
                        [1.0, 1.0],
                    ),
                ),
            ],
        ),
    );
    let projection = PortalProjectionBridge::new(1)
        .unwrap()
        .build_into(&resolver, &mut table)
        .unwrap();

    let matrices = table.matrices();
    assert_eq!(
        matrices.len(),
        5,
        "world, two frames, two projection spaces"
    );
    assert_eq!(
        matrices[ship_slot as usize].w_axis.x, 10.0,
        "the ship's slot is untouched"
    );
    assert_eq!(
        matrices[room_slot as usize].w_axis.x, 20.0,
        "the room's slot is untouched"
    );
    assert!(
        projection
            .portal_views
            .iter()
            .all(|view| view.coordinate_space > room_slot.max(ship_slot)),
        "every portal view uses a slot no frame holds"
    );
}
