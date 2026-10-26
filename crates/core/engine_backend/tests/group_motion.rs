//! Moving a group moves what is in it, so the editor must know when that
//! includes something authored fixed -- unless the group is a rigid frame,
//! whose members stay fixed in its own space.

use engine_backend::scene::frames::make_frame;
use engine_backend::scene::{first_fixed_in_group, SceneWorldExt, SpawnObject};
use helio_component::components::{ObjectMovability, StaticMeshComponent};
use pulsar_scenedb::{Entity, World};

fn object(world: &mut World, parent: Option<Entity>) -> Entity {
    world
        .spawn_object(SpawnObject::new("o").with_parent(parent))
        .unwrap()
}

fn with_mesh(world: &mut World, entity: Entity, movability: ObjectMovability) {
    let mesh = StaticMeshComponent {
        movability,
        ..Default::default()
    };
    pulsar_world_registry::attach_value(world, entity, mesh).unwrap();
}

#[test]
fn a_group_holding_a_static_mesh_reports_it() {
    let mut world = World::new();
    let group = object(&mut world, None);
    let movable = object(&mut world, Some(group));
    with_mesh(&mut world, movable, ObjectMovability::Movable);
    assert!(first_fixed_in_group(&world, group).is_none());

    let wall = object(&mut world, Some(movable));
    with_mesh(&mut world, wall, ObjectMovability::Static);
    let (found, _) = first_fixed_in_group(&world, group).expect("the nested static wall");
    assert_eq!(found, wall);
}

#[test]
fn a_rigid_frames_static_members_stay_fixed_in_its_space() {
    let mut world = World::new();
    let ship = object(&mut world, None);
    let wall = object(&mut world, Some(ship));
    with_mesh(&mut world, wall, ObjectMovability::Static);
    assert!(
        first_fixed_in_group(&world, ship).is_some(),
        "an ordinary group moves it"
    );

    make_frame(&mut world, ship).unwrap();
    assert!(
        first_fixed_in_group(&world, ship).is_none(),
        "as a rigid frame the wall does not move relative to it"
    );
    // A frame is itself placed in its parent's space.
    let hull = object(&mut world, None);
    with_mesh(&mut world, hull, ObjectMovability::Static);
    make_frame(&mut world, hull).unwrap();
    assert!(
        first_fixed_in_group(&world, hull).is_some(),
        "the frame's own mesh is fixed"
    );
}

#[test]
fn a_group_around_a_frame_does_not_count_the_frames_members() {
    let mut world = World::new();
    let fleet = object(&mut world, None);
    let ship = object(&mut world, Some(fleet));
    let wall = object(&mut world, Some(ship));
    with_mesh(&mut world, wall, ObjectMovability::Static);
    make_frame(&mut world, ship).unwrap();
    assert!(
        first_fixed_in_group(&world, fleet).is_none(),
        "the wall rides the ship's matrix; nothing is rewritten"
    );
}
