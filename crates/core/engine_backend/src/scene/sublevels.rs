//! Sublevel placements as spaces.
//!
//! A sublevel placement (an object carrying
//! [`SubLevelActorComponent`](helio_pass_gbuffer::SubLevelActorComponent)) is one
//! of the things that define a **space** (`.agents/FRAMES.md`): the content
//! placed there is the object's descendants, drawn through one entry of the
//! coordinate-space table, so moving the placement moves the whole sublevel with
//! one matrix and rewrites none of its members.
//!
//! The component stays the only thing authored. It registers itself as a
//! [`SpaceDefiner`] and the one allocator in `pulsar_scene_model::frames` gives
//! the object its slot, exactly as for a rigid frame; there is no separate
//! "adopt" step and the placement does not know any other kind of space exists.
//!
//! The one thing the definer does is **normalize**: a space has exactly one
//! placement, the object's own `Transform`, so a placement's authored matrix is
//! folded into it once and the component keeps the identity afterwards.
//!
//! Not here yet, because it needs decisions about authoring: a level-asset format
//! for sublevel content, the editor UI that places one, and instancing one
//! content several times (today each placement holds its own objects).

use glam::Mat4;
use helio_pass_gbuffer::SubLevelActorComponent;
use pulsar_scene_model::frames::SpaceDefiner;
use pulsar_scene_model::hierarchy_transform::{from_matrix, to_matrix};
use pulsar_scene_model::Transform;
use pulsar_scenedb::{Entity, World};

fn placements(world: &World) -> Vec<Entity> {
    world
        .query::<&SubLevelActorComponent>()
        .map(|(entity, _)| entity)
        .collect()
}

fn is_folded(world: &World, entity: Entity) -> bool {
    world
        .get::<SubLevelActorComponent>(entity)
        .is_none_or(|actor| actor.transform() == Mat4::IDENTITY)
}

/// Fold the placement's authored matrix into the object's own transform.
fn fold(world: &mut World, entity: Entity) {
    let Some(actor) = world.get::<SubLevelActorComponent>(entity).copied() else {
        return;
    };
    if let Some(own) = world.get::<Transform>(entity).copied() {
        let folded = from_matrix(to_matrix(&own) * actor.transform().as_dmat4());
        if let Some(mut transform) = world.get_mut::<Transform>(entity) {
            *transform = folded;
        }
    }
    if let Some(mut component) = world.get_mut::<SubLevelActorComponent>(entity) {
        component.transform = Mat4::IDENTITY.to_cols_array_2d();
    }
}

inventory::submit! {
    SpaceDefiner {
        name: "sublevel placement",
        entities: placements,
        is_normalized: Some(is_folded),
        normalize: Some(fold),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulsar_scene_model::frames::{frame_table, space_of, sync_spaces, SpaceSlot};
    use pulsar_scene_model::world_ext::{SceneWorldExt, SpawnObject};

    fn near(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-4)
    }

    #[test]
    fn a_placement_is_a_space_holding_its_contents() {
        let mut world = World::new();
        let actor = world.spawn_object(SpawnObject::new("room")).unwrap();
        world.insert(
            actor,
            SubLevelActorComponent::new(3, Mat4::from_translation(glam::Vec3::new(10.0, 0.0, 0.0))),
        );
        let wall = world
            .spawn_object(SpawnObject::new("wall").with_parent(Some(actor)))
            .unwrap();

        assert!(sync_spaces(&mut world));
        let slot = world
            .get::<SpaceSlot>(actor)
            .expect("the placement has a slot")
            .slot;
        assert_eq!(space_of(&world, wall), slot);
        // The authored placement now lives in the object's transform, once.
        assert!(near(
            world
                .get::<Transform>(actor)
                .unwrap()
                .position
                .map(|v| v as f32),
            [10.0, 0.0, 0.0]
        ));
        assert_eq!(
            world
                .get::<SubLevelActorComponent>(actor)
                .unwrap()
                .transform(),
            Mat4::IDENTITY
        );
        // The table places the whole sublevel by that one matrix.
        let table = frame_table(&world, glam::DVec3::ZERO);
        assert!(near(
            table[slot as usize].w_axis.truncate().to_array(),
            [10.0, 0.0, 0.0]
        ));

        assert!(!sync_spaces(&mut world), "idempotent");
        assert!(near(
            world
                .get::<Transform>(actor)
                .unwrap()
                .position
                .map(|v| v as f32),
            [10.0, 0.0, 0.0]
        ));
    }

    #[test]
    fn a_placement_and_a_rigid_frame_get_different_slots() {
        use pulsar_scene_model::frames::make_frame;
        let mut world = World::new();
        let actor = world.spawn_object(SpawnObject::new("room")).unwrap();
        world.insert(actor, SubLevelActorComponent::new(1, Mat4::IDENTITY));
        let folder = world.spawn_object(SpawnObject::new("ship")).unwrap();
        make_frame(&mut world, folder).unwrap();
        let a = world.get::<SpaceSlot>(actor).unwrap().slot;
        let b = world.get::<SpaceSlot>(folder).unwrap().slot;
        assert_ne!(a, b, "one allocator, no overlap");
    }
}
