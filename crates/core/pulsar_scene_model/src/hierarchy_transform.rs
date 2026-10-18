//! Moving an object moves what it contains.
//!
//! [`Transform`] stays a flat world-space value (the renderer reads it by
//! entity index, with no per-frame parent resolution). So a parent's edit is
//! carried to its descendants at write time: the change from the parent's old
//! world transform to its new one is applied to every object beneath it.
//! The cost is paid only on an edit, in proportion to the subtree, and the
//! renderer sees ordinary dirty-tracked rows.
//!
//! Grouping needs nothing here: reparenting keeps each object's world
//! transform, so an object put in a folder does not jump.

use glam::{EulerRot, Mat4, Quat, Vec3};
use pulsar_scenedb::{Entity, World};
use std::collections::HashMap;

use crate::components::{Parent, Transform};

fn to_matrix(t: &Transform) -> Mat4 {
    let q = Quat::from_euler(
        EulerRot::YXZ,
        t.rotation[1].to_radians(),
        t.rotation[0].to_radians(),
        t.rotation[2].to_radians(),
    );
    Mat4::from_scale_rotation_translation(
        Vec3::from_array(t.scale),
        q,
        Vec3::from_array(t.position),
    )
}

fn from_matrix(m: Mat4) -> Transform {
    let (scale, rotation, translation) = m.to_scale_rotation_translation();
    let (y, x, z) = rotation.to_euler(EulerRot::YXZ);
    Transform {
        position: translation.to_array(),
        rotation: [x.to_degrees(), y.to_degrees(), z.to_degrees()],
        scale: scale.to_array(),
    }
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

/// Carry the change `old` -> `new` of `entity`'s world transform to every
/// object beneath it. Writes only the descendants; the caller writes `entity`.
///
/// A pure move is applied as an exact offset. Any rotation or scale change goes
/// through matrices (a descendant under a non-uniform scale loses any shear it
/// would pick up, since a `Transform` cannot hold one).
pub fn propagate_to_descendants(world: &mut World, entity: Entity, old: &Transform, new: &Transform) {
    if old == new {
        return;
    }
    let descendants = descendants_of(world, entity);
    if descendants.is_empty() {
        return;
    }
    if old.rotation == new.rotation && old.scale == new.scale {
        let offset = [
            new.position[0] - old.position[0],
            new.position[1] - old.position[1],
            new.position[2] - old.position[2],
        ];
        for child in descendants {
            if let Some(mut t) = world.get_mut::<Transform>(child) {
                for axis in 0..3 {
                    t.position[axis] += offset[axis];
                }
            }
        }
        return;
    }
    let old_matrix = to_matrix(old);
    if old_matrix.determinant().abs() < 1e-12 {
        // A collapsed parent has no inverse to carry a change through.
        return;
    }
    let delta = to_matrix(new) * old_matrix.inverse();
    for child in descendants {
        if let Some(mut t) = world.get_mut::<Transform>(child) {
            *t = from_matrix(delta * to_matrix(&t));
        }
    }
}

/// Set `entity`'s world transform and carry the change to its descendants.
pub fn set_transform_with_descendants(world: &mut World, entity: Entity, new: Transform) {
    let Some(old) = world.get::<Transform>(entity).copied() else {
        return;
    };
    if let Some(mut t) = world.get_mut::<Transform>(entity) {
        *t = new;
    }
    propagate_to_descendants(world, entity, &old, &new);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world_ext::{SceneWorldExt, SpawnObject};

    fn pair(world: &mut World, child_at: [f32; 3]) -> (Entity, Entity) {
        let parent = world.spawn_object(SpawnObject::new("folder")).unwrap();
        let child = world
            .spawn_object(
                SpawnObject::new("child")
                    .with_parent(Some(parent))
                    .with_transform(Transform {
                        position: child_at,
                        ..Transform::default()
                    }),
            )
            .unwrap();
        (parent, child)
    }

    #[test]
    fn moving_a_folder_moves_its_contents() {
        let mut world = World::new();
        let (parent, child) = pair(&mut world, [1.0, 2.0, 3.0]);
        let moved = Transform {
            position: [10.0, 0.0, 0.0],
            ..Transform::default()
        };
        set_transform_with_descendants(&mut world, parent, moved);
        assert_eq!(world.get::<Transform>(child).unwrap().position, [11.0, 2.0, 3.0]);
    }

    #[test]
    fn rotating_a_folder_swings_its_contents_about_it() {
        let mut world = World::new();
        let (parent, child) = pair(&mut world, [1.0, 0.0, 0.0]);
        let turned = Transform {
            rotation: [0.0, 90.0, 0.0],
            ..Transform::default()
        };
        set_transform_with_descendants(&mut world, parent, turned);
        let p = world.get::<Transform>(child).unwrap().position;
        assert!(p[0].abs() < 1e-4 && p[2].abs() - 1.0 < 1e-4, "{p:?}");
    }
}
