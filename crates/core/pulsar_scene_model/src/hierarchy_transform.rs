//! Parented transforms.
//!
//! An object's [`Transform`] is **relative to its parent** (to the world for an
//! object with none, so a flat scene is unchanged). The renderer composes each
//! object's world transform from its parent chain on the GPU (`HierarchyResolve`
//! in Helio, fed by the [`ObjectParent`](crate::ObjectParent) row), so moving a
//! parent is one row write however many objects it holds.
//!
//! This module is the CPU side of the same arithmetic, for what needs an
//! object's world pose without the GPU: gizmos, picking, camera selection,
//! reparenting without a jump, and loading a scene saved before transforms
//! were parent-relative. It uses the same Euler convention as the shaders
//! (`Quat::from_euler(YXZ, yaw = rotation[1], pitch = rotation[0], roll =
//! rotation[2])`, degrees).
//!
//! A [`Transform`] cannot hold shear, so a rotated, non-uniformly scaled
//! parent over a rotated child resolves to the nearest shear-free transform
//! (the same on the GPU and here). The arithmetic is f64, as positions are.

use glam::{DMat4, DQuat, DVec3, EulerRot};
use pulsar_scenedb::{Entity, World};
use std::collections::HashMap;

use crate::components::{Parent, Transform};
use crate::world_ext::{SceneError, SceneWorldExt};

/// Deepest parent chain walked. Matches the GPU resolve; reparenting refuses
/// cycles, so this only bounds a corrupt world.
pub const MAX_DEPTH: usize = 64;

/// `t` as a matrix.
pub fn to_matrix(t: &Transform) -> DMat4 {
    let q = DQuat::from_euler(
        EulerRot::YXZ,
        f64::from(t.rotation[1]).to_radians(),
        f64::from(t.rotation[0]).to_radians(),
        f64::from(t.rotation[2]).to_radians(),
    );
    DMat4::from_scale_rotation_translation(
        DVec3::from_array(t.scale.map(f64::from)),
        q,
        DVec3::from_array(t.position),
    )
}

/// The nearest [`Transform`] to `m`.
pub fn from_matrix(m: DMat4) -> Transform {
    let (scale, rotation, translation) = m.to_scale_rotation_translation();
    let (y, x, z) = rotation.to_euler(EulerRot::YXZ);
    Transform {
        position: translation.to_array(),
        rotation: [
            x.to_degrees() as f32,
            y.to_degrees() as f32,
            z.to_degrees() as f32,
        ],
        scale: scale.as_vec3().to_array(),
    }
}

/// The matrix taking `entity`'s space to the world: its own transform under
/// every ancestor's.
pub fn world_matrix_of(world: &World, entity: Entity) -> Option<DMat4> {
    let mut matrix = to_matrix(&world.get::<Transform>(entity).copied()?);
    let mut at = world.get::<Parent>(entity).map(|parent| parent.0);
    for _ in 0..MAX_DEPTH {
        let Some(parent) = at else {
            break;
        };
        let Some(transform) = world.get::<Transform>(parent) else {
            break;
        };
        matrix = to_matrix(&transform) * matrix;
        at = world.get::<Parent>(parent).map(|p| p.0);
    }
    Some(matrix)
}

/// `entity`'s transform in world space.
pub fn world_transform_of(world: &World, entity: Entity) -> Option<Transform> {
    let own = world.get::<Transform>(entity).copied()?;
    if world.get::<Parent>(entity).is_none() {
        // The world pose of an object with no parent is its stored value, not
        // a matrix round trip of it.
        return Some(own);
    }
    world_matrix_of(world, entity).map(from_matrix)
}

/// The matrix taking `parent`'s space to the world (identity for no parent).
pub fn parent_matrix(world: &World, parent: Option<Entity>) -> DMat4 {
    parent
        .and_then(|parent| world_matrix_of(world, parent))
        .unwrap_or(DMat4::IDENTITY)
}

/// The local transform that places an object under `parent` at `world_pose`.
pub fn local_for_world(world: &World, parent: Option<Entity>, world_pose: &Transform) -> Transform {
    from_matrix(parent_matrix(world, parent).inverse() * to_matrix(world_pose))
}

/// Place `entity` at `world_pose` in the world: writes the local transform
/// that gives it, under its current parent.
pub fn set_world_transform(world: &mut World, entity: Entity, world_pose: Transform) {
    let parent = world.parent_of(entity);
    // With no parent the world pose is the local one; written as given, not
    // round-tripped through a matrix (a drag would drift its Euler angles).
    let local = if parent.is_some() {
        local_for_world(world, parent, &world_pose)
    } else {
        world_pose
    };
    if let Some(mut transform) = world.get_mut::<Transform>(entity) {
        *transform = local;
    }
}

/// Reparent `entity` without moving it in the world: its local transform is
/// rewritten for the new parent. Refuses cycles like
/// [`SceneWorldExt::reparent`].
pub fn reparent_keeping_world(
    world: &mut World,
    entity: Entity,
    new_parent: Option<Entity>,
) -> Result<(), SceneError> {
    let pose = world_transform_of(world, entity);
    world.reparent(entity, new_parent)?;
    if let Some(pose) = pose {
        let local = local_for_world(world, new_parent, &pose);
        if let Some(mut transform) = world.get_mut::<Transform>(entity) {
            *transform = local;
        }
    }
    Ok(())
}

/// Every object beneath `root`, nearest first. One pass over the parent links,
/// not a scan per level.
pub fn descendants_of(world: &World, root: Entity) -> Vec<Entity> {
    let mut children: HashMap<Entity, Vec<Entity>> = HashMap::new();
    for (entity, parent) in world.query::<&Parent>() {
        children.entry(parent.0).or_default().push(entity);
    }
    let mut found = Vec::new();
    let mut at = vec![root];
    while let Some(entity) = at.pop() {
        if let Some(kids) = children.get(&entity) {
            found.extend(kids.iter().copied());
            at.extend(kids.iter().copied());
        }
    }
    found
}

/// The scene file version that introduced parent-relative transforms. Older
/// files hold the world-space value of every object, parented or not.
pub const PARENT_RELATIVE_FILE_VERSION: &str = "2.2";

/// Whether a scene file of `version` (`"1"`, `"2.1"`, ...) predates
/// parent-relative transforms, so its parented objects hold world-space
/// values that [`convert_legacy_world_transforms`] must turn local after the
/// objects are loaded.
pub fn file_has_world_space_transforms(version: &str) -> bool {
    let mut parts = version.trim().split('.');
    let major = parts
        .next()
        .and_then(|p| p.parse::<u32>().ok())
        .unwrap_or(0);
    let minor = parts
        .next()
        .and_then(|p| p.parse::<u32>().ok())
        .unwrap_or(0);
    (major, minor) < (2, 2)
}

/// Turn the parented objects among `entities` that hold **world-space**
/// transforms (loaded from a scene saved before transforms were
/// parent-relative) into parent-relative ones, keeping every object where it
/// was. Objects outside `entities` are neither converted nor changed; their
/// world poses, where a listed object's parent is one of them, are read as
/// they stand now.
pub fn convert_legacy_world_transforms(world: &mut World, entities: &[Entity]) {
    let listed: std::collections::HashSet<Entity> = entities.iter().copied().collect();
    // Every object's pose as loaded, before any is rewritten: a child is
    // converted against its parent's world-space value, not its new local one.
    let poses: HashMap<Entity, DMat4> = world
        .query::<&Transform>()
        .map(|(entity, transform)| (entity, to_matrix(transform)))
        .collect();
    for &entity in entities {
        let Some(parent) = world.get::<Parent>(entity).map(|p| p.0) else {
            continue;
        };
        let (Some(own), Some(parent_pose)) = (poses.get(&entity), poses.get(&parent)) else {
            continue;
        };
        // A parent outside the list is already parent-relative: its world pose
        // is its chain, not its stored value.
        let parent_pose = if listed.contains(&parent) {
            *parent_pose
        } else {
            world_matrix_of(world, parent).unwrap_or(*parent_pose)
        };
        let local = from_matrix(parent_pose.inverse() * *own);
        if let Some(mut transform) = world.get_mut::<Transform>(entity) {
            *transform = local;
        }
    }
}

/// [`convert_legacy_world_transforms`] over every object in `world`.
pub fn convert_all_legacy_world_transforms(world: &mut World) {
    let entities: Vec<Entity> = world
        .query::<&Transform>()
        .map(|(entity, _)| entity)
        .collect();
    convert_legacy_world_transforms(world, &entities);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world_ext::SpawnObject;

    fn spawn(world: &mut World, parent: Option<Entity>, t: Transform) -> Entity {
        world
            .spawn_object(SpawnObject::new("o").with_parent(parent).with_transform(t))
            .unwrap()
    }

    fn at(position: [f64; 3]) -> Transform {
        Transform {
            position,
            ..Transform::default()
        }
    }

    fn near(a: [f64; 3], b: [f64; 3]) -> bool {
        a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-4)
    }

    #[test]
    fn a_child_is_placed_relative_to_its_parent() {
        let mut world = World::new();
        let parent = spawn(&mut world, None, at([10.0, 0.0, 0.0]));
        let child = spawn(&mut world, Some(parent), at([1.0, 2.0, 3.0]));
        let w = world_transform_of(&world, child).unwrap();
        assert!(near(w.position, [11.0, 2.0, 3.0]), "{w:?}");
    }

    #[test]
    fn a_rotated_parent_swings_its_child() {
        let mut world = World::new();
        let parent = spawn(
            &mut world,
            None,
            Transform {
                rotation: [0.0, 90.0, 0.0],
                ..Transform::default()
            },
        );
        let child = spawn(&mut world, Some(parent), at([1.0, 0.0, 0.0]));
        let w = world_transform_of(&world, child).unwrap();
        assert!(near(w.position, [0.0, 0.0, -1.0]), "{w:?}");
    }

    #[test]
    fn reparenting_keeps_the_world_pose() {
        let mut world = World::new();
        let folder = spawn(
            &mut world,
            None,
            Transform {
                position: [5.0, 1.0, 0.0],
                rotation: [0.0, 30.0, 0.0],
                scale: [2.0, 2.0, 2.0],
            },
        );
        let object = spawn(&mut world, None, at([3.0, 4.0, 5.0]));
        let before = world_transform_of(&world, object).unwrap();
        reparent_keeping_world(&mut world, object, Some(folder)).unwrap();
        let after = world_transform_of(&world, object).unwrap();
        assert!(
            near(before.position, after.position),
            "{before:?} {after:?}"
        );
        // Moving the folder now moves the object, and only because of the link.
        let mut moved = *world.get::<Transform>(folder).unwrap();
        moved.position[0] += 10.0;
        *world.get_mut::<Transform>(folder).unwrap() = moved;
        let later = world_transform_of(&world, object).unwrap();
        assert!(near(
            later.position,
            [
                after.position[0] + 10.0,
                after.position[1],
                after.position[2]
            ]
        ));
    }

    #[test]
    fn setting_a_world_pose_writes_the_local_one() {
        let mut world = World::new();
        let parent = spawn(&mut world, None, at([10.0, 0.0, 0.0]));
        let child = spawn(&mut world, Some(parent), Transform::default());
        set_world_transform(&mut world, child, at([12.0, 0.0, 0.0]));
        assert!(near(
            world.get::<Transform>(child).unwrap().position,
            [2.0, 0.0, 0.0]
        ));
    }

    #[test]
    fn legacy_world_values_become_parent_relative() {
        let mut world = World::new();
        let parent = spawn(&mut world, None, at([10.0, 0.0, 0.0]));
        // Saved as world-space: the child sat at x = 11.
        let child = spawn(&mut world, Some(parent), at([11.0, 2.0, 3.0]));
        convert_all_legacy_world_transforms(&mut world);
        assert!(near(
            world.get::<Transform>(child).unwrap().position,
            [1.0, 2.0, 3.0]
        ));
        assert!(near(
            world.get::<Transform>(parent).unwrap().position,
            [10.0, 0.0, 0.0]
        ));
    }
}
