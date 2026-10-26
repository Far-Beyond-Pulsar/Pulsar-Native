//! Content spaces: which sublevel's content an object belongs to.
//!
//! A sublevel is *content*, not an entity (`.agents/FRAMES.md`): a set of
//! objects authored together that a level places somewhere. [`ContentSpace`]
//! tags the objects of one such set so the set can be listed, loaded and
//! unloaded as a unit while it all lives in the one world the renderer and
//! every query read. Objects with no tag belong to the default level (space 0).
//!
//! This is the scene-model half of the idea. The engine-level half, which
//! belongs in SceneDB itself, is a partition key on rows with per-partition
//! journals and mirror flush ranges; nothing here depends on it, and moving to
//! it later does not change these functions' callers.
//!
//! The tag is authored as an object prop ([`CONTENT_SPACE_PROP`]) so it
//! survives save and load with no file-format change.

use pulsar_scenedb::{Entity, World};
use std::collections::BTreeSet;

use crate::components::{Parent, RenderProps};
use crate::world_ext::SceneWorldExt;

/// The object prop that carries the tag in a saved scene.
pub const CONTENT_SPACE_PROP: &str = "content_space";

/// The sublevel content an object belongs to (`>= 1`; the default level is the
/// absence of the tag).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContentSpace(pub u32);

/// The space `entity` is tagged with (0 for the default level).
pub fn space_of(world: &World, entity: Entity) -> u32 {
    world.get::<ContentSpace>(entity).map_or(0, |space| space.0)
}

/// Tag `root` and everything beneath it with `space` (0 clears the tag), and
/// keep the saved prop in step.
pub fn assign_space(world: &mut World, root: Entity, space: u32) {
    let mut everything = crate::hierarchy_transform::descendants_of(world, root);
    everything.push(root);
    for entity in everything {
        if space == 0 {
            world.remove::<ContentSpace>(entity);
        } else {
            world.insert(entity, ContentSpace(space));
        }
        if let Some(mut props) = world.get_mut::<RenderProps>(entity) {
            if space == 0 {
                props.props.remove(CONTENT_SPACE_PROP);
            } else {
                props.props.insert(
                    CONTENT_SPACE_PROP.to_string(),
                    serde_json::Value::from(space),
                );
            }
        }
    }
}

/// Make `entity`'s tag match its saved prop (after a load or an edit).
pub fn apply_space_prop(world: &mut World, entity: Entity) {
    let saved = world
        .get::<RenderProps>(entity)
        .and_then(|props| props.props.get(CONTENT_SPACE_PROP).and_then(|v| v.as_u64()))
        .and_then(|space| u32::try_from(space).ok())
        .filter(|space| *space != 0);
    match saved {
        Some(space) => {
            world.insert(entity, ContentSpace(space));
        }
        None => {
            world.remove::<ContentSpace>(entity);
        }
    }
}

/// Every object of `space`.
pub fn objects_in_space(world: &World, space: u32) -> Vec<Entity> {
    world
        .query::<&ContentSpace>()
        .filter(|(_, tag)| tag.0 == space)
        .map(|(entity, _)| entity)
        .collect()
}

/// The objects of `space` whose parent is not in it: the tops of its trees.
pub fn space_roots(world: &World, space: u32) -> Vec<Entity> {
    objects_in_space(world, space)
        .into_iter()
        .filter(|&entity| {
            world
                .get::<Parent>(entity)
                .is_none_or(|parent| space_of(world, parent.0) != space)
        })
        .collect()
}

/// The content spaces present, in order (the default level, 0, is not listed).
pub fn spaces_present(world: &World) -> BTreeSet<u32> {
    world
        .query::<&ContentSpace>()
        .map(|(_, tag)| tag.0)
        .collect()
}

/// Unload a space: despawn every object of it (with its component instances).
/// Returns how many objects went. Objects of other spaces that were parented
/// under a removed object go with it, as despawning a tree does.
pub fn unload_space(world: &mut World, space: u32) -> usize {
    let before = objects_in_space(world, space).len();
    for root in space_roots(world, space) {
        world.despawn_tree(root);
    }
    before - objects_in_space(world, space).len().min(before)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world_ext::SpawnObject;

    fn object(world: &mut World, parent: Option<Entity>) -> Entity {
        world
            .spawn_object(SpawnObject::new("o").with_parent(parent))
            .unwrap()
    }

    #[test]
    fn a_space_is_listed_loaded_and_unloaded_as_a_unit() {
        let mut world = World::new();
        let room = object(&mut world, None);
        let table = object(&mut world, Some(room));
        let lamp = object(&mut world, Some(table));
        let outside = object(&mut world, None);
        assign_space(&mut world, room, 4);

        assert_eq!(spaces_present(&world), BTreeSet::from([4]));
        assert_eq!(space_of(&world, lamp), 4);
        assert_eq!(space_of(&world, outside), 0);
        assert_eq!(space_roots(&world, 4), vec![room]);
        assert_eq!(objects_in_space(&world, 4).len(), 3);

        assert_eq!(unload_space(&mut world, 4), 3);
        assert!(spaces_present(&world).is_empty());
        assert!(!world.is_alive(room) && !world.is_alive(lamp));
        assert!(world.is_alive(outside), "the default level is untouched");
    }

    #[test]
    fn the_saved_prop_round_trips() {
        let mut world = World::new();
        let room = object(&mut world, None);
        assign_space(&mut world, room, 2);
        world.remove::<ContentSpace>(room);
        apply_space_prop(&mut world, room);
        assert_eq!(space_of(&world, room), 2, "the prop restores the tag");

        assign_space(&mut world, room, 0);
        assert_eq!(space_of(&world, room), 0);
        apply_space_prop(&mut world, room);
        assert_eq!(space_of(&world, room), 0, "cleared for good");
    }
}
