//! Sublevels on rigid frames.
//!
//! A sublevel placement (an object carrying
//! [`SubLevelActorComponent`](helio_pass_gbuffer::SubLevelActorComponent)) is a
//! **rigid frame** (see `.agents/FRAMES.md`): the content placed there is that
//! object's descendants, drawn through one entry of the frame table, so moving
//! the placement moves the whole sublevel with one matrix and rewrites none of
//! its members. This replaces tagging every instance with a space id on the CPU:
//! an object's space is simply its nearest frame ancestor, found on the GPU.
//!
//! [`adopt_sublevel_actors`] is the bridge from the authored component to that
//! model. What is *not* here yet, because it needs decisions about authoring:
//! a level-asset format for sublevel content, the editor UI that places one,
//! and instancing one content several times (today each placement holds its
//! own objects).

use glam::Mat4;
use helio_pass_gbuffer::SubLevelActorComponent;
use pulsar_scene_model::frames::{make_frame, Frame};
use pulsar_scene_model::hierarchy_transform::{from_matrix, to_matrix};
use pulsar_scene_model::Transform;
use pulsar_scenedb::{Entity, World};

/// Make every sublevel placement in `world` a rigid frame. A placement's
/// authored matrix is folded into its object's own [`Transform`] (the object's
/// transform is the one placement of a frame), and the component keeps the
/// identity from then on. Returns how many objects became frames. Run it after
/// a level loads and whenever placements are authored; it is idempotent.
pub fn adopt_sublevel_actors(world: &mut World) -> usize {
    let actors: Vec<(Entity, SubLevelActorComponent)> = world
        .query::<&SubLevelActorComponent>()
        .map(|(entity, actor)| (entity, *actor))
        .collect();
    let mut adopted = 0;
    for (entity, actor) in actors {
        let placement = actor.transform();
        if placement != Mat4::IDENTITY {
            if let Some(own) = world.get::<Transform>(entity).copied() {
                let folded = from_matrix(to_matrix(&own) * placement);
                if let Some(mut transform) = world.get_mut::<Transform>(entity) {
                    *transform = folded;
                }
            }
            if let Some(mut component) = world.get_mut::<SubLevelActorComponent>(entity) {
                component.transform = Mat4::IDENTITY.to_cols_array_2d();
            }
        }
        if world.get::<Frame>(entity).is_none() && make_frame(world, entity).is_ok() {
            adopted += 1;
        }
    }
    adopted
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulsar_scene_model::frames::{frame_table, space_of};
    use pulsar_scene_model::world_ext::{SceneWorldExt, SpawnObject};

    fn near(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-4)
    }

    #[test]
    fn a_placement_becomes_a_frame_holding_its_contents() {
        let mut world = World::new();
        let actor = world.spawn_object(SpawnObject::new("room")).unwrap();
        world.insert(
            actor,
            SubLevelActorComponent::new(3, Mat4::from_translation(glam::Vec3::new(10.0, 0.0, 0.0))),
        );
        let wall = world
            .spawn_object(SpawnObject::new("wall").with_parent(Some(actor)))
            .unwrap();

        assert_eq!(adopt_sublevel_actors(&mut world), 1);
        assert!(world.get::<Frame>(actor).is_some());
        assert_eq!(space_of(&world, wall), world.get::<Frame>(actor).unwrap().slot);
        // The authored placement now lives in the object's transform, once.
        assert!(near(world.get::<Transform>(actor).unwrap().position, [10.0, 0.0, 0.0]));
        assert_eq!(
            world.get::<SubLevelActorComponent>(actor).unwrap().transform(),
            Mat4::IDENTITY
        );
        // The frame table places the whole sublevel by that one matrix.
        let table = frame_table(&world, glam::DVec3::ZERO);
        assert!(near(table[1].w_axis.truncate().to_array(), [10.0, 0.0, 0.0]));

        assert_eq!(adopt_sublevel_actors(&mut world), 0, "idempotent");
        assert!(near(world.get::<Transform>(actor).unwrap().position, [10.0, 0.0, 0.0]));
    }
}
