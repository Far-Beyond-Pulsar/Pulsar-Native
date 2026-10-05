//! Helio's `Movability` gates runtime moves of an object's `Transform`
//! (the gate lives in `helio_component::motion_gate`, registered through
//! `pulsar_scene_model::motion`).

use helio::Movability;
use pulsar_scene_model::motion::ensure_can_move;
use pulsar_scene_model::Transform;
use pulsar_scenedb::World;

// Linked for its `inventory` registration; nothing is used by name.
use helio_component as _;

#[test]
fn only_objects_that_can_move_may_be_moved() {
    let mut world = World::new();
    let object = world.spawn();
    world.insert(object, Transform::default());
    assert!(ensure_can_move(&world, object).is_ok(), "no Movability means no restriction");

    for (movability, allowed) in [
        (Movability::Static, false),
        (Movability::Stationary, false),
        (Movability::Movable, true),
        (Movability::Dynamic, true),
    ] {
        world.insert(object, movability);
        let result = ensure_can_move(&world, object);
        assert_eq!(result.is_ok(), allowed, "{movability:?}: {result:?}");
        if !allowed {
            assert!(result.unwrap_err().contains("Movability"), "the gate is named in the message");
        }
    }
}
