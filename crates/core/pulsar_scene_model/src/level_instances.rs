//! Level instances.
//!
//! The default level is a **level instance** that nobody thinks about: its
//! objects are simply the world. Others exist alongside it, and an instance is
//! made in one of two ways:
//!
//! * **from a level file**, by a display component (`SubLevelDisplayComponent`,
//!   whose behaviour lives in the engine): the file's objects are instantiated
//!   under an object, which is the instance's origin;
//! * **in memory**, by [`create_level_instance`] and then adding objects to it
//!   ([`spawn_in_level_instance`]): a temporary level that exists only in the
//!   running world.
//!
//! Either way an instance is an object (its **root**, the origin of the
//! instance) with the instance's objects as descendants. The root is a space
//! (`crate::frames`): its contents are drawn through one matrix, so moving the
//! root moves the whole instance and rewrites none of it. Contents are
//! [`LevelInstanceMember`]s: part of the instance, not of the level that shows
//! it, so they are never saved with that level.

use pulsar_scenedb::{Entity, World};

use crate::frames::{sync_spaces, SpaceDefiner};
use crate::world_ext::{SceneError, SceneWorldExt, SpawnObject};
use crate::ObjectType;

/// The object is the root of an in-memory level instance (the origin of its
/// contents). Instances shown from a level file get their root from the
/// display component instead.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LevelInstanceRoot;

/// The object belongs to the level instance rooted at `root`: it is generated
/// content, rebuilt from its source and never saved with the level that shows
/// the instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LevelInstanceMember {
    pub root: Entity,
}

fn roots(world: &World) -> Vec<Entity> {
    world
        .query::<&LevelInstanceRoot>()
        .map(|(entity, _)| entity)
        .collect()
}

inventory::submit! {
    SpaceDefiner {
        name: "level instance",
        entities: roots,
        is_normalized: None,
        normalize: None,
    }
}

/// Create an empty in-memory level instance and return its root. Add objects
/// with [`spawn_in_level_instance`]; place the instance by placing the root.
pub fn create_level_instance(world: &mut World, name: &str) -> Result<Entity, SceneError> {
    let root = world.spawn_object(SpawnObject::new(name).with_object_type(ObjectType::Folder))?;
    world.insert(root, LevelInstanceRoot);
    sync_spaces(world);
    Ok(root)
}

/// Add an object to the instance rooted at `root`. Its transform is relative to
/// the instance's origin (the root), or to its `parent` when that is another
/// object of the instance.
pub fn spawn_in_level_instance(
    world: &mut World,
    root: Entity,
    mut spec: SpawnObject,
) -> Result<Entity, SceneError> {
    if spec.parent.is_none() {
        spec.parent = Some(root);
    }
    let entity = world.spawn_object(spec)?;
    world.insert(entity, LevelInstanceMember { root });
    Ok(entity)
}

/// Mark an object that was spawned some other way as belonging to the instance.
pub fn mark_member(world: &mut World, root: Entity, entity: Entity) {
    world.insert(entity, LevelInstanceMember { root });
}

/// Whether `entity` is generated content of some level instance.
pub fn is_member(world: &World, entity: Entity) -> bool {
    world.get::<LevelInstanceMember>(entity).is_some()
}

/// Every object of the instance rooted at `root` (not the root itself).
pub fn members_of(world: &World, root: Entity) -> Vec<Entity> {
    world
        .query::<&LevelInstanceMember>()
        .filter(|(_, member)| member.root == root)
        .map(|(entity, _)| entity)
        .collect()
}

/// Despawn the contents of the instance rooted at `root`, leaving the root.
/// Returns how many objects went.
pub fn clear_members(world: &mut World, root: Entity) -> usize {
    let members = members_of(world, root);
    let before = members.len();
    // Despawning a tree takes its descendants too, so start from the tops.
    let tops: Vec<Entity> = members
        .into_iter()
        .filter(|&entity| {
            world.parent_of(entity).is_none_or(|parent| {
                world
                    .get::<LevelInstanceMember>(parent)
                    .is_none_or(|m| m.root != root)
            })
        })
        .collect();
    for top in tops {
        world.despawn_tree(top);
    }
    before
}

/// Remove an in-memory instance entirely: its contents and its root.
pub fn remove_level_instance(world: &mut World, root: Entity) {
    clear_members(world, root);
    world.despawn_tree(root);
    sync_spaces(world);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frames::{frame_table, space_of, SpaceSlot};
    use crate::Transform;

    fn at(x: f64) -> Transform {
        Transform {
            position: [x, 0.0, 0.0],
            ..Transform::default()
        }
    }

    #[test]
    fn an_in_memory_instance_is_a_space_holding_what_is_added_to_it() {
        let mut world = World::new();
        let root = create_level_instance(&mut world, "temporary").unwrap();
        let slot = world
            .get::<SpaceSlot>(root)
            .expect("the root is a space")
            .slot;

        let wall = spawn_in_level_instance(
            &mut world,
            root,
            SpawnObject::new("wall").with_transform(at(2.0)),
        )
        .unwrap();
        let lamp = spawn_in_level_instance(
            &mut world,
            root,
            SpawnObject::new("lamp")
                .with_parent(Some(wall))
                .with_transform(at(1.0)),
        )
        .unwrap();
        assert!(is_member(&world, wall) && is_member(&world, lamp));
        assert_eq!(
            space_of(&world, lamp),
            slot,
            "drawn through the instance's one matrix"
        );
        assert_eq!(members_of(&world, root).len(), 2);

        // Placing the root places the instance: one matrix, no member rewritten.
        world.get_mut::<Transform>(root).unwrap().position = [100.0, 0.0, 0.0];
        assert_eq!(
            world.get::<Transform>(wall).unwrap().position,
            [2.0, 0.0, 0.0]
        );
        let table = frame_table(&world, glam::DVec3::ZERO);
        assert_eq!(table[slot as usize].w_axis.x, 100.0);
    }

    #[test]
    fn clearing_and_removing_an_instance() {
        let mut world = World::new();
        let root = create_level_instance(&mut world, "temporary").unwrap();
        let wall = spawn_in_level_instance(&mut world, root, SpawnObject::new("wall")).unwrap();
        spawn_in_level_instance(
            &mut world,
            root,
            SpawnObject::new("lamp").with_parent(Some(wall)),
        )
        .unwrap();
        let outside = world.spawn_object(SpawnObject::new("outside")).unwrap();

        assert_eq!(clear_members(&mut world, root), 2);
        assert!(world.is_alive(root) && world.is_alive(outside));
        assert!(members_of(&world, root).is_empty());

        spawn_in_level_instance(&mut world, root, SpawnObject::new("again")).unwrap();
        remove_level_instance(&mut world, root);
        assert!(!world.is_alive(root));
        assert!(world.is_alive(outside), "the default level is untouched");
        assert!(world.get::<SpaceSlot>(outside).is_none());
    }
}
