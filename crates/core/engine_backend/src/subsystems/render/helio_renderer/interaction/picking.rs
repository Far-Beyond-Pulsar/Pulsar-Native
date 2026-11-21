//! Picking in the viewport: the object under a ray, the objects inside a
//! screen box, and the surface point a placement follows.
use super::math::{ray_aabb, ray_plane_intersection, rotation_matrix};
use super::registrations::{view_icon, ContentGeometryRegistration};
use super::{SceneInteraction, View};
use crate::scene::hierarchy_transform::world_transform_of;
use crate::scene::{SceneWorldExt, StableId, Transform, Visibility};
use glam::{DVec3, Vec2, Vec3};
use pulsar_scenedb::{Entity, World};

impl SceneInteraction {
    /// The objects whose whole click bounds lie inside the screen rectangle
    /// between `a` and `b` (normalized viewport coordinates, either corner
    /// first), in front of the camera: a mesh by the box around what it draws
    /// projected to the screen, an object with an editor icon by its icon.
    /// Objects with neither (empties, folders) are selected in the
    /// hierarchy, as with a click ([`Self::pick`]).
    ///
    /// One pass over the objects on release, the same cost as a click pick;
    /// nothing runs while the box is being dragged.
    pub fn box_pick(&self, world: &World, a: Vec2, b: Vec2) -> Vec<Entity> {
        let min = a.min(b) * self.view.size;
        let max = a.max(b) * self.view.size;
        let inside = |p: Vec2| p.x >= min.x && p.y >= min.y && p.x <= max.x && p.y <= max.y;
        let right = self.view.matrix.row(0).truncate().normalize_or_zero();
        let up = self.view.matrix.row(1).truncate().normalize_or_zero();
        let mut picked: Vec<(String, Entity)> = world
            .query::<&StableId>()
            .filter_map(|(entity, id)| {
                if !world.get::<Visibility>(entity)?.visible {
                    return None;
                }
                let transform = world_transform_of(world, entity)?;
                let corners: Vec<Vec3> = if let Some(size) = view_icon(world, entity) {
                    let center = self.view.relative(transform.position);
                    let depth = (center - self.view.position).dot(self.view.forward);
                    if depth <= 0.001 || depth >= self.view.far {
                        return None;
                    }
                    let half = size * (0.5 * depth);
                    [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)]
                        .into_iter()
                        .map(|(x, y)| center + right * (half.x * x) + up * (half.y * y))
                        .collect()
                } else {
                    let (lo, hi) = object_bounds(world, entity, transform, self.view)?;
                    (0..8)
                        .map(|i| {
                            Vec3::new(
                                if i & 1 == 0 { lo.x } else { hi.x },
                                if i & 2 == 0 { lo.y } else { hi.y },
                                if i & 4 == 0 { lo.z } else { hi.z },
                            )
                        })
                        .collect()
                };
                corners
                    .into_iter()
                    .all(|corner| self.view.project(corner).is_some_and(inside))
                    .then(|| (id.0.clone(), entity))
            })
            .collect();
        // A stable order (by id), whatever order the world stores them in.
        picked.sort_by(|left, right| left.0.cmp(&right.0));
        picked.into_iter().map(|(_, entity)| entity).collect()
    }
    /// The object a ray from `origin` (world) along `direction` hits first.
    pub fn pick(&self, world: &World, origin: DVec3, direction: Vec3) -> Option<String> {
        let direction = direction.normalize_or_zero();
        if direction == Vec3::ZERO {
            return None;
        }
        let origin = self.view.relative(origin.to_array());

        // Of objects hit at the same distance (a duplicate starts on its
        // source), the selected one stays picked; then the lowest id.
        let selected = world.selected_entity();
        world
            .query::<&StableId>()
            .filter_map(|(entity, id)| {
                let visibility = world.get::<Visibility>(entity)?;
                if !visibility.visible {
                    return None;
                }
                let transform = world_transform_of(world, entity)?;
                // An object with an editor icon (a light's billboard) picks
                // by its icon, not by a box around nothing.
                if let Some(size) = view_icon(world, entity) {
                    let distance = icon_hit(self.view, transform, size, origin, direction)?;
                    return Some((distance, Some(entity) != selected, id.0.clone()));
                }
                // Otherwise by what it draws: an object without content (an
                // empty, a folder, a mesh not loaded yet) is picked in the
                // hierarchy.
                let (min, max) = object_bounds(world, entity, transform, self.view)?;
                let broad_phase = ray_aabb(origin, direction, min, max)?;
                let (has_geometry, triangle_hit) =
                    content_hit_distance(world, entity, transform, self.view, origin, direction);
                let distance = if has_geometry {
                    triangle_hit?
                } else {
                    broad_phase
                };
                Some((distance, Some(entity) != selected, id.0.clone()))
            })
            .min_by(
                |(left, left_other, left_id), (right, right_other, right_id)| {
                    left.partial_cmp(right)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| left_other.cmp(right_other))
                        .then_with(|| left_id.cmp(right_id))
                },
            )
            .map(|(_, _, stable_id)| stable_id)
    }

    /// Surface point beneath a viewport ray, ignoring the object currently
    /// being placed (and its descendants) so a live placement cannot hit
    /// itself while following the cursor.
    pub fn placement_point(
        &self,
        world: &World,
        origin: DVec3,
        direction: Vec3,
        ignore_id: Option<&str>,
    ) -> Option<DVec3> {
        let direction = direction.normalize_or_zero();
        if direction == Vec3::ZERO {
            return None;
        }
        let relative_origin = self.view.relative(origin.to_array());
        let nearest = world
            .query::<&StableId>()
            .filter_map(|(entity, _id)| {
                if ignore_id.is_some_and(|ignored| {
                    let mut current = Some(entity);
                    while let Some(candidate) = current {
                        if world.stable_id_of(candidate) == Some(ignored) {
                            return true;
                        }
                        current = world.parent_of(candidate);
                    }
                    false
                }) {
                    return None;
                }
                let visibility = world.get::<Visibility>(entity)?;
                if !visibility.visible {
                    return None;
                }
                let transform = world_transform_of(world, entity)?;
                let (min, max) = object_bounds(world, entity, transform, self.view)?;
                let broad_phase = ray_aabb(relative_origin, direction, min, max)?;
                let (has_geometry, triangle_hit) = content_hit_distance(
                    world,
                    entity,
                    transform,
                    self.view,
                    relative_origin,
                    direction,
                );
                if has_geometry {
                    triangle_hit
                } else {
                    Some(broad_phase)
                }
            })
            .filter(|distance| distance.is_finite() && *distance >= 0.0)
            .min_by(f32::total_cmp)?;
        Some(origin + direction.as_dvec3() * nearest as f64)
    }
}

/// Test the ray against the object's content (every [`ContentGeometryRegistration`]).
/// The AABB above is a broad phase only: treating it as the hit made empty
/// space inside concave meshes selectable. The ray goes into the object's
/// local space so the content's test stays well-conditioned under rotation
/// and non-uniform scale. Returns whether the object has content to test,
/// and the nearest hit.
fn content_hit_distance(
    world: &World,
    entity: Entity,
    transform: Transform,
    view: View,
    origin: Vec3,
    direction: Vec3,
) -> (bool, Option<f32>) {
    let rotation = rotation_matrix(transform);
    let scale = Vec3::from_array(transform.scale);
    let degenerate = scale.abs().min_element() <= 1e-8 || !scale.is_finite();
    let object_origin = view.relative(transform.position);
    let inverse = |v: Vec3| (rotation.transpose() * v) / scale;
    let (local_origin, local_direction) = if degenerate {
        (Vec3::ZERO, Vec3::Z)
    } else {
        (inverse(origin - object_origin), inverse(direction))
    };
    let mut has_content = false;
    let mut nearest: Option<f32> = None;
    for registration in pulsar_plugins::registered::<ContentGeometryRegistration>() {
        let Some(hit) = (registration.hit)(world, entity, local_origin, local_direction) else {
            continue;
        };
        has_content = true;
        if degenerate {
            continue;
        }
        // The local direction is the inverse transform of a unit world
        // direction, so a hit parameter is a world distance.
        if let Some(t) = hit.filter(|t| *t >= 0.0 && t.is_finite()) {
            nearest = Some(nearest.map_or(t, |best| best.min(t)));
        }
    }
    (has_content, nearest)
}

/// A ray against an icon of `size` (world units per unit of depth) facing
/// the camera at the object's position. Object scale/rotation do not affect
/// icons. Ray distance lets meshes occlude the icon.
fn icon_hit(
    view: View,
    transform: Transform,
    size: Vec2,
    origin: Vec3,
    direction: Vec3,
) -> Option<f32> {
    let center = view.relative(transform.position);
    let depth = (center - view.position).dot(view.forward);
    if depth <= 0.001 || depth >= view.far {
        return None;
    }
    let hit = ray_plane_intersection(origin, direction, center, view.forward)?;
    // Projection rows are scaled camera right/up in either projection mode.
    let right = view.matrix.row(0).truncate().normalize_or_zero();
    let up = view.matrix.row(1).truncate().normalize_or_zero();
    let half_size = size * (0.5 * depth);
    let offset = hit - center;
    (offset.dot(right).abs() <= half_size.x && offset.dot(up).abs() <= half_size.y)
        .then_some((hit - origin).dot(direction))
}

/// Box of what the object draws, in `view`'s coordinates: its content's
/// local boxes ([`ContentGeometryRegistration::local_bounds`]: its enabled mesh
/// instances' bounding spheres); `None` without any.
fn object_bounds(
    world: &World,
    entity: Entity,
    transform: Transform,
    view: View,
) -> Option<(Vec3, Vec3)> {
    let mut local: Option<(Vec3, Vec3)> = None;
    for (min, max) in pulsar_plugins::registered::<ContentGeometryRegistration>()
        .filter_map(|registration| (registration.local_bounds)(world, entity))
    {
        local = Some(match local {
            Some((lo, hi)) => (lo.min(min), hi.max(max)),
            None => (min, max),
        });
    }
    let (min, max) = local?;
    let rotation = rotation_matrix(transform);
    let scale = Vec3::from_array(transform.scale);
    let center = rotation * ((min + max) * 0.5 * scale) + view.relative(transform.position);
    let extents = rotation.abs() * ((max - min) * 0.5 * scale.abs());
    Some((center - extents, center + extents))
}
