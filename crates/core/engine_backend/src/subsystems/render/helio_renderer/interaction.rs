//! SceneDB-owned viewport interaction.
//!
//! This module deliberately contains no renderer scene mirror.  Picking reads
//! the current [`WorldSceneStore`] snapshot and gizmo drags write transforms
//! back to that same store.  The only state retained between pointer events is
//! the mathematical state of an in-progress drag; it is revalidated against
//! the World before every write.

use glam::{EulerRot, Mat3, Quat, Vec3};
use helio::Renderer;
use pulsar_scenedb::Entity;

use crate::scene::{GizmoAxis, GizmoType, ObjectType, Transform, WorldSceneStore};

const AXIS_HIT_RADIUS: f32 = 0.16;
const GIZMO_LENGTH: f32 = 1.25;
const MIN_SCALE: f32 = 0.001;

#[derive(Clone, Copy, Debug)]
struct DragState {
    entity: Entity,
    axis: GizmoAxis,
    mode: GizmoType,
    initial: Transform,
    start_axis_parameter: f32,
    start_rotation_vector: Vec3,
}

/// Persistent interaction state that is not world state.
#[derive(Debug)]
pub struct SceneInteraction {
    mode: GizmoType,
    hovered_axis: Option<GizmoAxis>,
    drag: Option<DragState>,
}

impl Default for SceneInteraction {
    fn default() -> Self {
        Self {
            mode: GizmoType::None,
            hovered_axis: None,
            drag: None,
        }
    }
}

impl SceneInteraction {
    pub fn mode(&self) -> GizmoType {
        self.mode
    }

    pub fn set_mode(&mut self, mode: GizmoType) {
        self.mode = mode;
        self.hovered_axis = None;
        self.drag = None;
    }

    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    pub fn cancel_drag(&mut self) {
        self.drag = None;
        self.hovered_axis = None;
    }

    /// Pick the nearest visible mesh/light by testing conservative world-space
    /// bounds derived from the authoritative SceneDB transform and type.
    pub fn pick(&self, store: &WorldSceneStore, origin: Vec3, direction: Vec3) -> Option<String> {
        let direction = direction.normalize_or_zero();
        if direction == Vec3::ZERO {
            return None;
        }

        store
            .get_all_snapshots()
            .into_iter()
            .filter(|snapshot| snapshot.visibility.visible && is_pickable(snapshot.object_type))
            .filter_map(|snapshot| {
                let (min, max) = world_bounds(&snapshot.object_type, snapshot.transform);
                ray_aabb(origin, direction, min, max).map(|distance| (distance, snapshot.stable_id))
            })
            .min_by(|(left, left_id), (right, right_id)| {
                left.partial_cmp(right)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| left_id.cmp(right_id))
            })
            .map(|(_, stable_id)| stable_id)
    }

    /// Begin a gizmo drag using only the selected SceneDB entity's transform.
    pub fn try_start_drag(
        &mut self,
        store: &WorldSceneStore,
        origin: Vec3,
        direction: Vec3,
        camera_position: Vec3,
    ) -> bool {
        let Some(entity) = store.get_selected_entity() else {
            return false;
        };
        let Some(initial) = store.transform(entity) else {
            return false;
        };
        let Some(visibility) = store.visibility(entity) else {
            return false;
        };
        if !visibility.visible || visibility.locked || self.mode == GizmoType::None {
            return false;
        }

        let pivot = Vec3::from_array(initial.position);
        let scale = gizmo_scale(camera_position, pivot);
        let Some((axis, axis_parameter, rotation_vector)) =
            self.hit_gizmo(origin, direction, pivot, scale, camera_position)
        else {
            return false;
        };

        self.hovered_axis = Some(axis);
        self.drag = Some(DragState {
            entity,
            axis,
            mode: self.mode,
            initial,
            start_axis_parameter: axis_parameter,
            start_rotation_vector: rotation_vector,
        });
        true
    }

    /// Update the active drag and write the resulting transform directly to
    /// SceneDB.  A deleted entity or a newly-hidden/locked entity cancels the
    /// operation without touching any stale renderer state.
    pub fn update_drag(
        &mut self,
        store: &mut WorldSceneStore,
        origin: Vec3,
        direction: Vec3,
        camera_position: Vec3,
    ) {
        let Some(drag) = self.drag else {
            return;
        };
        let Some(current) = store.transform(drag.entity) else {
            self.cancel_drag();
            return;
        };
        let Some(visibility) = store.visibility(drag.entity) else {
            self.cancel_drag();
            return;
        };
        if !visibility.visible || visibility.locked {
            self.cancel_drag();
            return;
        }

        let pivot = Vec3::from_array(drag.initial.position);
        let scale = gizmo_scale(camera_position, pivot);
        let Some((_, parameter, rotation_vector)) =
            self.hit_gizmo_axis(origin, direction, pivot, scale, drag.axis, camera_position)
        else {
            return;
        };

        let mut next = drag.initial;
        match drag.mode {
            GizmoType::Translate => {
                let delta = axis_vector(drag.axis) * (parameter - drag.start_axis_parameter);
                next.position = (pivot + delta).to_array();
            }
            GizmoType::Scale => {
                let amount = (parameter - drag.start_axis_parameter) / scale;
                let axis_index = axis_index(drag.axis);
                next.scale[axis_index] =
                    (drag.initial.scale[axis_index] * (1.0 + amount)).max(MIN_SCALE);
            }
            GizmoType::Rotate => {
                let Some(start) = drag.start_rotation_vector.try_normalize() else {
                    return;
                };
                let Some(current) = rotation_vector.try_normalize() else {
                    return;
                };
                let axis = axis_vector(drag.axis);
                let angle = start.dot(current).clamp(-1.0, 1.0).acos()
                    * start.cross(current).dot(axis).signum();
                let initial_rotation = Quat::from_euler(
                    EulerRot::YXZ,
                    drag.initial.rotation[1].to_radians(),
                    drag.initial.rotation[0].to_radians(),
                    drag.initial.rotation[2].to_radians(),
                );
                let rotated = Quat::from_axis_angle(axis, angle) * initial_rotation;
                let (yaw, pitch, roll) = rotated.to_euler(EulerRot::YXZ);
                next.rotation = [pitch.to_degrees(), yaw.to_degrees(), roll.to_degrees()];
            }
            GizmoType::None => return,
        }

        // Use the current World transform only to avoid overwriting unrelated
        // component edits made between pointer events.  The drag owns the
        // selected transform fields, while every write still goes through the
        // normal SceneDB dirty/mirror path.
        if current != next {
            store.set_transform(drag.entity, next);
        }
    }

    pub fn update_hover(
        &mut self,
        store: &WorldSceneStore,
        origin: Vec3,
        direction: Vec3,
        camera_position: Vec3,
    ) {
        let Some(entity) = store.get_selected_entity() else {
            self.hovered_axis = None;
            return;
        };
        let Some(transform) = store.transform(entity) else {
            self.hovered_axis = None;
            return;
        };
        let pivot = Vec3::from_array(transform.position);
        self.hovered_axis = self
            .hit_gizmo(
                origin,
                direction,
                pivot,
                gizmo_scale(camera_position, pivot),
                camera_position,
            )
            .map(|(axis, _, _)| axis);
    }

    /// Draw the selected gizmo from the current World transform. Helio is used
    /// only as a transient debug-line sink; it is not queried for scene state.
    pub fn draw_gizmo(
        &self,
        renderer: &mut Renderer,
        store: &WorldSceneStore,
        camera_position: Vec3,
    ) {
        let Some(entity) = store.get_selected_entity() else {
            return;
        };
        let Some(transform) = store.transform(entity) else {
            return;
        };
        let Some(visibility) = store.visibility(entity) else {
            return;
        };
        if !visibility.visible || self.mode == GizmoType::None {
            return;
        }

        let pivot = Vec3::from_array(transform.position);
        let length = gizmo_scale(camera_position, pivot);
        let rotation = rotation_matrix(transform);
        for axis in [GizmoAxis::X, GizmoAxis::Y, GizmoAxis::Z] {
            let direction = rotation * axis_vector(axis);
            let color = if self.hovered_axis == Some(axis) {
                [1.0, 1.0, 0.2, 1.0]
            } else {
                axis_color(axis)
            };
            renderer.debug_line(
                pivot.to_array(),
                (pivot + direction * length).to_array(),
                color,
            );
        }

        if self.mode == GizmoType::Rotate {
            for axis in [GizmoAxis::X, GizmoAxis::Y, GizmoAxis::Z] {
                let normal = rotation * axis_vector(axis);
                renderer.debug_torus(
                    pivot.to_array(),
                    normal.to_array(),
                    length * 0.85,
                    length * 0.025,
                    axis_color(axis),
                    32,
                    6,
                );
            }
        }
    }

    fn hit_gizmo(
        &self,
        origin: Vec3,
        direction: Vec3,
        pivot: Vec3,
        scale: f32,
        camera_position: Vec3,
    ) -> Option<(GizmoAxis, f32, Vec3)> {
        [GizmoAxis::X, GizmoAxis::Y, GizmoAxis::Z]
            .into_iter()
            .filter_map(|axis| {
                self.hit_gizmo_axis(origin, direction, pivot, scale, axis, camera_position)
                    .map(|(_, parameter, rotation)| (axis, parameter, rotation))
            })
            .min_by(|(_, left, _), (_, right, _)| {
                left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal)
            })
    }

    fn hit_gizmo_axis(
        &self,
        origin: Vec3,
        direction: Vec3,
        pivot: Vec3,
        scale: f32,
        axis: GizmoAxis,
        camera_position: Vec3,
    ) -> Option<(GizmoAxis, f32, Vec3)> {
        let direction = direction.normalize_or_zero();
        let axis_direction = axis_vector(axis);
        let (ray_t, axis_t) = closest_ray_line(origin, direction, pivot, axis_direction)?;
        let point_on_ray = origin + direction * ray_t;
        let point_on_axis = pivot + axis_direction * axis_t;
        let tolerance = (camera_position - pivot).length() * 0.015 + AXIS_HIT_RADIUS * scale;
        if ray_t < 0.0
            || !(0.0..=GIZMO_LENGTH * scale).contains(&axis_t)
            || point_on_ray.distance(point_on_axis) > tolerance
        {
            return None;
        }

        let plane_normal = direction
            .cross(axis_direction)
            .cross(axis_direction)
            .normalize_or_zero();
        let rotation_vector = ray_plane_intersection(origin, direction, pivot, plane_normal)
            .map(|point| point - pivot)
            .unwrap_or(Vec3::ZERO);
        Some((axis, axis_t, rotation_vector))
    }
}

fn is_pickable(object_type: ObjectType) -> bool {
    matches!(object_type, ObjectType::Mesh(_) | ObjectType::Light(_))
}

fn local_half_extent(object_type: &ObjectType) -> Vec3 {
    match object_type {
        ObjectType::Mesh(crate::scene::MeshType::Plane) => Vec3::new(0.5, 0.02, 0.5),
        ObjectType::Mesh(crate::scene::MeshType::Cylinder) => Vec3::new(0.5, 0.5, 0.5),
        ObjectType::Mesh(crate::scene::MeshType::Sphere) => Vec3::splat(0.5),
        ObjectType::Mesh(_) => Vec3::splat(0.5),
        ObjectType::Light(_) => Vec3::splat(0.25),
        _ => Vec3::ZERO,
    }
}

fn world_bounds(object_type: &ObjectType, transform: Transform) -> (Vec3, Vec3) {
    let half = local_half_extent(object_type);
    let rotation = rotation_matrix(transform);
    let extents = rotation.abs() * (half * Vec3::from_array(transform.scale).abs());
    let center = Vec3::from_array(transform.position);
    (center - extents, center + extents)
}

fn rotation_matrix(transform: Transform) -> Mat3 {
    Mat3::from_quat(Quat::from_euler(
        EulerRot::YXZ,
        transform.rotation[1].to_radians(),
        transform.rotation[0].to_radians(),
        transform.rotation[2].to_radians(),
    ))
}

fn ray_aabb(origin: Vec3, direction: Vec3, min: Vec3, max: Vec3) -> Option<f32> {
    let mut near = 0.0_f32;
    let mut far = f32::INFINITY;
    for axis in 0..3 {
        if direction[axis].abs() < f32::EPSILON {
            if origin[axis] < min[axis] || origin[axis] > max[axis] {
                return None;
            }
            continue;
        }
        let mut t0 = (min[axis] - origin[axis]) / direction[axis];
        let mut t1 = (max[axis] - origin[axis]) / direction[axis];
        if t0 > t1 {
            std::mem::swap(&mut t0, &mut t1);
        }
        near = near.max(t0);
        far = far.min(t1);
        if near > far {
            return None;
        }
    }
    (far >= 0.0).then_some(near.max(0.0))
}

fn closest_ray_line(
    ray_origin: Vec3,
    ray_direction: Vec3,
    line_origin: Vec3,
    line_direction: Vec3,
) -> Option<(f32, f32)> {
    let w0 = ray_origin - line_origin;
    let a = ray_direction.dot(ray_direction);
    let b = ray_direction.dot(line_direction);
    let c = line_direction.dot(line_direction);
    let d = ray_direction.dot(w0);
    let e = line_direction.dot(w0);
    let denominator = a * c - b * b;
    if denominator.abs() < 1e-6 {
        return None;
    }
    Some(((b * e - c * d) / denominator, (a * e - b * d) / denominator))
}

fn ray_plane_intersection(
    origin: Vec3,
    direction: Vec3,
    point: Vec3,
    normal: Vec3,
) -> Option<Vec3> {
    let denominator = direction.dot(normal);
    if denominator.abs() < 1e-6 {
        return None;
    }
    let t = (point - origin).dot(normal) / denominator;
    (t >= 0.0).then_some(origin + direction * t)
}

fn gizmo_scale(camera_position: Vec3, pivot: Vec3) -> f32 {
    ((camera_position - pivot).length() * 0.1).clamp(0.25, 10.0)
}

fn axis_vector(axis: GizmoAxis) -> Vec3 {
    match axis {
        GizmoAxis::X => Vec3::X,
        GizmoAxis::Y => Vec3::Y,
        GizmoAxis::Z => Vec3::Z,
    }
}

fn axis_index(axis: GizmoAxis) -> usize {
    match axis {
        GizmoAxis::X => 0,
        GizmoAxis::Y => 1,
        GizmoAxis::Z => 2,
    }
}

fn axis_color(axis: GizmoAxis) -> [f32; 4] {
    match axis {
        GizmoAxis::X => [0.9, 0.15, 0.15, 1.0],
        GizmoAxis::Y => [0.2, 0.9, 0.2, 1.0],
        GizmoAxis::Z => [0.2, 0.4, 1.0, 1.0],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::WorldSceneStore;

    #[test]
    fn nearest_visible_mesh_is_picked_from_world() {
        let mut store = WorldSceneStore::new();
        let far = store.spawn(Some("far".into()), "Far", None).unwrap();
        store.set_object_type(far, ObjectType::Mesh(crate::scene::MeshType::Cube));
        store.set_transform(
            far,
            Transform {
                position: [0.0, 0.0, -10.0],
                ..Default::default()
            },
        );
        let near = store.spawn(Some("near".into()), "Near", None).unwrap();
        store.set_object_type(near, ObjectType::Mesh(crate::scene::MeshType::Cube));
        store.set_transform(
            near,
            Transform {
                position: [0.0, 0.0, -3.0],
                ..Default::default()
            },
        );

        let interaction = SceneInteraction::default();
        assert_eq!(
            interaction.pick(&store, Vec3::ZERO, -Vec3::Z),
            Some("near".into())
        );
    }

    #[test]
    fn hidden_mesh_is_not_pickable() {
        let mut store = WorldSceneStore::new();
        let entity = store.spawn(Some("hidden".into()), "Hidden", None).unwrap();
        store.set_object_type(entity, ObjectType::Mesh(crate::scene::MeshType::Cube));
        store.set_visibility(
            entity,
            crate::scene::Visibility {
                visible: false,
                locked: false,
            },
        );
        assert_eq!(
            SceneInteraction::default().pick(&store, Vec3::ZERO, -Vec3::Z),
            None
        );
    }
}
