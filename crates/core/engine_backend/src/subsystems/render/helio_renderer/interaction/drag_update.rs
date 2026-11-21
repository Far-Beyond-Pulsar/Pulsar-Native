//! Following the cursor while a gizmo handle is held: solving the dragged
//! pose, snapping it, and carrying it to every object of a group.
use super::math::{
    axis_param, projected_ring_angle, ray_plane_intersection, rotation_matrix, rotation_quat,
};
use super::{
    DragState, Handle, SceneInteraction, HANDLE_PIXELS, LOCATION_SNAP, ROTATION_SNAP, SCALE_SNAP,
};
use crate::scene::hierarchy_transform::{set_world_transform, world_transform_of};
use crate::scene::{GizmoType, SceneWorldExt, Transform, Visibility};
use glam::{DVec3, EulerRot, Mat3, Quat, Vec2, Vec3};
use pulsar_scenedb::{Entity, World};
use std::sync::atomic::Ordering;

/// The cursor ray and the gizmo frame a drag is solved in (view coordinates).
struct DragRay {
    o: Vec3,
    d: Vec3,
    cursor: Vec2,
    pivot: Vec3,
    basis: Mat3,
}

/// The rotation drag angle rounded to the rotation snap step, in radians.
pub(super) fn snapped_rotation_angle(angle: f32) -> f32 {
    let snap_step = f32::from_bits(ROTATION_SNAP.load(Ordering::Relaxed)).to_radians();
    (angle / snap_step).round() * snap_step
}

fn euler_degrees(q: Quat) -> [f32; 3] {
    let (y, x, z) = q.to_euler(EulerRot::YXZ);
    [x.to_degrees(), y.to_degrees(), z.to_degrees()]
}

/// World-space travel along the grabbed axis, from the ray/axis solve.
/// `None` when the axis is edge-on to the view, so the drag holds its last
/// value instead of jumping.
fn axis_travel(drag: &DragState, ray: &DragRay) -> Option<f32> {
    match drag.handle {
        Handle::Axis(i) if drag.mode != GizmoType::Rotate => {
            let t = axis_param(ray.o, ray.d, ray.pivot, ray.basis.col(i), drag.view.forward)?;
            Some(t - drag.axis_start)
        }
        _ => Some(0.0),
    }
}

fn translated_position(drag: &DragState, ray: &DragRay, axis_travel: f32) -> Option<[f64; 3]> {
    let movement = match drag.handle {
        Handle::Axis(i) => ray.basis.col(i) * axis_travel,
        _ => ray_plane_intersection(ray.o, ray.d, ray.pivot, drag.plane_normal)? - drag.plane_start,
    };
    Some(drag.view.world(ray.pivot + movement))
}

fn scaled(drag: &DragState, delta: Vec2, axis_travel: f32) -> [f32; 3] {
    let factor = (1.0
        + if drag.handle == Handle::Center {
            (delta.x - delta.y) / HANDLE_PIXELS
        } else {
            axis_travel / drag.length
        })
    .max(0.001);
    let mut scale = drag.initial.scale;
    for i in 0..3 {
        if drag.handle == Handle::Center || drag.handle == Handle::Axis(i) {
            scale[i] = (drag.initial.scale[i] * factor).max(0.001);
        }
    }
    scale
}

/// The rotation (degrees) after the ring angle under the cursor, advancing
/// the drag's accumulated angle.
fn rotated(drag: &mut DragState, ray: &DragRay) -> Option<[f32; 3]> {
    let Handle::Axis(i) = drag.handle else {
        return None;
    };
    // Resolve every drag against the same projected ring that is
    // visible on screen. A ray-plane angle becomes ill-conditioned
    // as the ring tilts toward edge-on, which is where the old
    // path caused the actual transform to jump while the ring
    // indicator kept following the cursor.
    let angle = projected_ring_angle(
        drag.view,
        ray.pivot,
        ray.basis,
        i,
        drag.length,
        ray.cursor,
        drag.previous_angle,
    )?;
    let angle_delta = (angle - drag.previous_angle + std::f32::consts::PI)
        .rem_euclid(std::f32::consts::TAU)
        - std::f32::consts::PI;
    drag.angle += angle_delta;
    drag.previous_angle = angle;
    let snapped_angle = snapped_rotation_angle(drag.angle);
    // The gizmo ring is parameterized in the object's initial
    // local basis, so compose the delta about that same local axis.
    let local_axis = [Vec3::X, Vec3::Y, Vec3::Z][i];
    let initial_rotation = Quat::from_mat3(&ray.basis);
    Some(euler_degrees(
        initial_rotation * Quat::from_axis_angle(local_axis, snapped_angle),
    ))
}

/// The dragged pose before snapping, or `None` to leave the drag unchanged.
fn solve_drag(drag: &mut DragState, current: Transform, ray: &DragRay) -> Option<Transform> {
    let delta = ray.cursor - drag.start;
    let axis_travel = axis_travel(drag, ray)?;
    let mut next = current;
    match drag.mode {
        GizmoType::Translate => next.position = translated_position(drag, ray, axis_travel)?,
        GizmoType::Scale => next.scale = scaled(drag, delta, axis_travel),
        GizmoType::Rotate => next.rotation = rotated(drag, ray)?,
        GizmoType::None => return None,
    }
    Some(next)
}

fn snap(mode: GizmoType, next: &mut Transform) {
    match mode {
        GizmoType::Translate => {
            // Snapped in world space, in f64.
            let step = f64::from(f32::from_bits(LOCATION_SNAP.load(Ordering::Relaxed)));
            for value in &mut next.position {
                *value = (*value / step).round() * step;
            }
        }
        GizmoType::Scale => {
            let step = f32::from_bits(SCALE_SNAP.load(Ordering::Relaxed));
            for value in &mut next.scale {
                *value = ((*value / step).round() * step).max(0.001);
            }
        }
        GizmoType::Rotate | GizmoType::None => {}
    }
}

impl SceneInteraction {
    /// Follow the ray from `o` (world) along `d` with the handle held.
    pub fn update_drag(&mut self, world: &mut World, o: DVec3, d: Vec3) {
        let Some(mut drag) = self.drag else {
            return;
        };
        // In the coordinates of the view the drag began in.
        let o = drag.view.relative(o.to_array());
        if !self.drag_targets_still_movable(world, &drag) {
            self.cancel_drag();
            return;
        }
        let Some(current) = self.drag_current_pose(world, &drag) else {
            self.cancel_drag();
            return;
        };
        let Some(cursor) = drag.view.cursor(o, d) else {
            return;
        };
        let ray = DragRay {
            o,
            d,
            cursor,
            pivot: drag.view.relative(drag.initial.position),
            basis: rotation_matrix(drag.initial),
        };
        let Some(mut next) = solve_drag(&mut drag, current, &ray) else {
            return;
        };
        snap(drag.mode, &mut next);
        self.drag = Some(drag);
        self.hovered = Some(drag.handle);
        if !drag.group {
            if next != current {
                set_world_transform(world, self.drag_targets[0].entity, next);
            }
            return;
        }
        self.apply_group_drag(world, &drag, next);
    }

    /// A selection change, or a hidden or locked target, ends the drag.
    fn drag_targets_still_movable(&self, world: &World, drag: &DragState) -> bool {
        let movable = |entity: Entity| {
            world
                .get::<Visibility>(entity)
                .is_some_and(|v| v.visible && !v.locked)
        };
        world.selected_entity() == drag.primary
            && self
                .drag_targets
                .iter()
                .all(|target| movable(target.entity))
    }

    /// The pose the handles move: one object's live transform, or the
    /// group's pivot as the drag began (each frame is applied from the
    /// starting poses, so a group never accumulates rounding).
    fn drag_current_pose(&self, world: &World, drag: &DragState) -> Option<Transform> {
        if drag.group {
            Some(drag.initial)
        } else {
            world_transform_of(world, self.drag_targets[0].entity)
        }
    }

    /// The pivot's change, carried to every object about the pivot.
    fn apply_group_drag(&self, world: &mut World, drag: &DragState, next: Transform) {
        let pivot_start = DVec3::from_array(drag.initial.position);
        let travel = DVec3::from_array(next.position) - pivot_start;
        let turn = rotation_quat(next);
        let stretch = Vec3::from_array(next.scale);
        for target in &self.drag_targets {
            let start = target.initial;
            let offset = (DVec3::from_array(start.position) - pivot_start).as_vec3();
            let mut moved = start;
            match drag.mode {
                GizmoType::Translate => {
                    moved.position = (DVec3::from_array(start.position) + travel).to_array();
                }
                GizmoType::Rotate => {
                    moved.position = (pivot_start + (turn * offset).as_dvec3()).to_array();
                    moved.rotation = euler_degrees(turn * rotation_quat(start));
                }
                GizmoType::Scale => {
                    moved.position = (pivot_start + (offset * stretch).as_dvec3()).to_array();
                    for axis in 0..3 {
                        moved.scale[axis] = (start.scale[axis] * stretch[axis]).max(0.001);
                    }
                }
                GizmoType::None => {}
            }
            if world_transform_of(world, target.entity) != Some(moved) {
                set_world_transform(world, target.entity, moved);
            }
        }
    }
}
