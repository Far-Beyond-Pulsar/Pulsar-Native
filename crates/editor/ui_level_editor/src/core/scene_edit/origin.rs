//! Setting objects' origins: where the pivot an object moves, turns and
//! scales about sits (`engine_backend::scene::pivots`), chosen from what the
//! object is and where it is.
//!
//! The origin is a point in the world: bounds of the object's own content
//! (and its children's), a corner or face of it, the world origin, or, for
//! several objects, one point shared by the group. Each op returns the pivot
//! offset every object needs to put its origin at its point; writing them is
//! the `SetPivots` command's. Nothing here moves any geometry.
//!
//! Bounds come from the content geometry kinds register
//! (`ContentGeometryRegistration`: tight triangle bounds, the same data
//! picking uses), so any kind with content works and nothing here names one.

use engine_backend::scene::hierarchy_transform::{descendants_of, world_matrix_of, world_transform_of};
use engine_backend::scene::pivots::offset_for_point;
use engine_backend::scene::SceneWorldExt;
use engine_backend::subsystems::render::helio_renderer::content_triangles;
use glam::{DMat4, DVec3};
use pulsar_scenedb::{Entity, World};
use std::collections::HashMap;

/// Where each object's own origin goes ("Per Object").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectOrigin {
    /// Back on the object's transform: no pivot offset.
    Transform,
    /// The center of the object's bounds (its children's included).
    BoundsCenter,
    /// The middle of the bottom face of those bounds.
    BoundsBottom,
    /// The middle of the top face of those bounds.
    BoundsTop,
    /// The world origin.
    World,
}

/// Where the one origin shared by a selection goes ("Group").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupOrigin {
    /// The average of the objects' bounds centers.
    MedianCenter,
    /// The center of the box around every object.
    BoundsCenter,
    /// The middle of the bottom face of that box.
    BoundsBottom,
    /// The middle of the top face of that box.
    BoundsTop,
    /// The primary (last picked) object's transform position.
    Primary,
    /// The world origin.
    World,
}

/// A world-space box.
#[derive(Clone, Copy, Debug)]
struct Bounds {
    min: DVec3,
    max: DVec3,
}

impl Bounds {
    fn point(p: DVec3) -> Self {
        Self { min: p, max: p }
    }
    fn grow(&mut self, p: DVec3) {
        self.min = self.min.min(p);
        self.max = self.max.max(p);
    }
    fn union(self, other: Self) -> Self {
        Self {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
        }
    }
    fn center(self) -> DVec3 {
        (self.min + self.max) * 0.5
    }
    fn bottom(self) -> DVec3 {
        let c = self.center();
        DVec3::new(c.x, self.min.y, c.z)
    }
    fn top(self) -> DVec3 {
        let c = self.center();
        DVec3::new(c.x, self.max.y, c.z)
    }
}

/// The world bounds of each of `entities` (each with everything beneath it):
/// its content's triangles, or, with none, the positions of it and what is
/// beneath it. One pass over the world's triangles however many objects.
fn world_bounds(world: &World, entities: &[Entity]) -> Vec<Option<Bounds>> {
    // Which of `entities` each object belongs to (an object under two selected
    // ones counts for both).
    let mut owners: HashMap<Entity, Vec<usize>> = HashMap::new();
    for (index, &entity) in entities.iter().enumerate() {
        owners.entry(entity).or_default().push(index);
        for below in descendants_of(world, entity) {
            owners.entry(below).or_default().push(index);
        }
    }
    let mut bounds: Vec<Option<Bounds>> = vec![None; entities.len()];
    let mut matrices: HashMap<Entity, Option<DMat4>> = HashMap::new();
    content_triangles(world, &mut |object, triangle| {
        let Some(indices) = owners.get(&object) else {
            return true;
        };
        let Some(matrix) = *matrices
            .entry(object)
            .or_insert_with(|| world_matrix_of(world, object))
        else {
            return true;
        };
        for vertex in triangle {
            let at = matrix.transform_point3(vertex.as_dvec3());
            for &index in indices {
                match &mut bounds[index] {
                    Some(b) => b.grow(at),
                    slot => *slot = Some(Bounds::point(at)),
                }
            }
        }
        true
    });
    for (index, &entity) in entities.iter().enumerate() {
        if bounds[index].is_some() {
            continue;
        }
        // No content: the positions of the object and what is beneath it.
        let mut positions = std::iter::once(entity)
            .chain(descendants_of(world, entity))
            .filter_map(|e| world_transform_of(world, e))
            .map(|t| DVec3::from_array(t.position));
        if let Some(first) = positions.next() {
            let mut b = Bounds::point(first);
            positions.for_each(|p| b.grow(p));
            bounds[index] = Some(b);
        }
    }
    bounds
}

/// The selected `ids` that exist, with their entities.
fn resolve(world: &World, ids: &[String]) -> Vec<(String, Entity)> {
    ids.iter()
        .filter_map(|id| Some((id.clone(), world.entity_for(id)?)))
        .collect()
}

/// The pivot offset that puts `entity`'s origin at world point `point`.
fn offset_at(world: &World, entity: Entity, point: DVec3) -> Option<[f32; 3]> {
    Some(offset_for_point(
        &world_transform_of(world, entity)?,
        point.to_array(),
    ))
}

/// The pivot offsets that put each of `ids`' origins at its own `anchor`.
pub fn object_origins(world: &World, ids: &[String], anchor: ObjectOrigin) -> Vec<(String, [f32; 3])> {
    let objects = resolve(world, ids);
    let entities: Vec<Entity> = objects.iter().map(|(_, e)| *e).collect();
    let bounds = if matches!(anchor, ObjectOrigin::Transform | ObjectOrigin::World) {
        Vec::new()
    } else {
        world_bounds(world, &entities)
    };
    objects
        .into_iter()
        .enumerate()
        .filter_map(|(index, (id, entity))| {
            let offset = match anchor {
                ObjectOrigin::Transform => [0.0; 3],
                ObjectOrigin::World => offset_at(world, entity, DVec3::ZERO)?,
                ObjectOrigin::BoundsCenter => offset_at(world, entity, bounds[index]?.center())?,
                ObjectOrigin::BoundsBottom => offset_at(world, entity, bounds[index]?.bottom())?,
                ObjectOrigin::BoundsTop => offset_at(world, entity, bounds[index]?.top())?,
            };
            Some((id, offset))
        })
        .collect()
}

/// The pivot offsets that put every one of `ids`' origins at the one point
/// `anchor` names for the group. `primary` is the last picked object.
pub fn group_origins(
    world: &World,
    ids: &[String],
    primary: Option<&str>,
    anchor: GroupOrigin,
) -> Vec<(String, [f32; 3])> {
    let objects = resolve(world, ids);
    let entities: Vec<Entity> = objects.iter().map(|(_, e)| *e).collect();
    let point = match anchor {
        GroupOrigin::World => Some(DVec3::ZERO),
        GroupOrigin::Primary => primary
            .and_then(|id| world.entity_for(id))
            .and_then(|e| world_transform_of(world, e))
            .map(|t| DVec3::from_array(t.position)),
        _ => {
            let bounds: Vec<Bounds> = world_bounds(world, &entities).into_iter().flatten().collect();
            let whole = bounds.iter().copied().reduce(Bounds::union);
            match (anchor, whole) {
                (GroupOrigin::MedianCenter, _) if !bounds.is_empty() => {
                    Some(bounds.iter().map(|b| b.center()).sum::<DVec3>() / bounds.len() as f64)
                }
                (GroupOrigin::BoundsCenter, Some(b)) => Some(b.center()),
                (GroupOrigin::BoundsBottom, Some(b)) => Some(b.bottom()),
                (GroupOrigin::BoundsTop, Some(b)) => Some(b.top()),
                _ => None,
            }
        }
    };
    let Some(point) = point else {
        return Vec::new();
    };
    objects
        .into_iter()
        .filter_map(|(id, entity)| Some((id, offset_at(world, entity, point)?)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_backend::scene::pivots::pivot_point;
    use engine_backend::scene::{SpawnObject, Transform};

    fn spawn(world: &mut World, id: &str, x: f64, parent: Option<Entity>) -> Entity {
        world
            .spawn_object(
                SpawnObject::new(id)
                    .with_id(id)
                    .with_parent(parent)
                    .with_transform(Transform {
                        position: [x, 0.0, 0.0],
                        ..Default::default()
                    }),
            )
            .unwrap()
    }

    #[test]
    fn a_groups_bounds_center_sits_between_its_children() {
        let mut world = World::new();
        let parent = spawn(&mut world, "parent", 0.0, None);
        spawn(&mut world, "a", 0.0, Some(parent));
        spawn(&mut world, "b", 4.0, Some(parent));
        let entries = object_origins(&world, &["parent".into()], ObjectOrigin::BoundsCenter);
        let (_, offset) = &entries[0];
        let t = world_transform_of(&world, parent).unwrap();
        assert!((pivot_point(&t, *offset)[0] - 2.0).abs() < 1e-5);
    }

    #[test]
    fn a_shared_group_origin_gives_every_member_the_same_pivot() {
        let mut world = World::new();
        let a = spawn(&mut world, "a", 0.0, None);
        let b = spawn(&mut world, "b", 6.0, None);
        let ids: Vec<String> = vec!["a".into(), "b".into()];
        let entries = group_origins(&world, &ids, Some("b"), GroupOrigin::MedianCenter);
        assert_eq!(entries.len(), 2);
        for (entity, (_, offset)) in [a, b].into_iter().zip(&entries) {
            let t = world_transform_of(&world, entity).unwrap();
            assert!((pivot_point(&t, *offset)[0] - 3.0).abs() < 1e-5);
        }
        let primary = group_origins(&world, &ids, Some("b"), GroupOrigin::Primary);
        let t = world_transform_of(&world, a).unwrap();
        assert!((pivot_point(&t, primary[0].1)[0] - 6.0).abs() < 1e-5);
    }
}
