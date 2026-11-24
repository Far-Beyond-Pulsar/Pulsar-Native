//! Transform handles with shared solid geometry for drawing and picking.
mod drag_update;
mod gizmo_draw;
mod math;
mod picking;
mod registrations;
#[cfg(test)]
mod tests;

pub use registrations::{content_triangles, ContentGeometryRegistration, ViewIconRegistration};

use super::gizmo_geometry::{shapes, Handle};
use crate::scene::hierarchy_transform::world_transform_of;
use crate::scene::pivots::{pivot_offset_of, pivot_point};
use crate::scene::{GizmoType, SceneWorldExt, Transform, Visibility};
use glam::{DVec3, Mat3, Mat4, Vec2, Vec3};
use math::{
    axis_param, projected_ring_initial_angle, ray_plane_intersection, rotation_matrix,
    triangle_distance,
};
use pulsar_scenedb::{Entity, World};
use std::sync::atomic::{AtomicU32, Ordering};

static LOCATION_SNAP: AtomicU32 = AtomicU32::new(1.0f32.to_bits());
static ROTATION_SNAP: AtomicU32 = AtomicU32::new(15.0f32.to_bits());
static SCALE_SNAP: AtomicU32 = AtomicU32::new(0.1f32.to_bits());

pub(super) fn set_snap_settings(location: f32, rotation: f32, scale: f32) {
    if location.is_finite() && location > 0.0 {
        LOCATION_SNAP.store(location.to_bits(), Ordering::Relaxed);
    }
    if rotation.is_finite() && rotation > 0.0 {
        ROTATION_SNAP.store(rotation.to_bits(), Ordering::Relaxed);
    }
    if scale.is_finite() && scale > 0.0 {
        SCALE_SNAP.store(scale.to_bits(), Ordering::Relaxed);
    }
}
const HANDLE_PIXELS: f32 = 112.0;
const PICK_MARGIN: f32 = 7.0;
#[derive(Clone, Copy, Debug)]
struct View {
    /// The world point (f64) the view's coordinates are taken from: the
    /// eye. Positions here are f32 offsets from it, exact near the eye
    /// however far it is from the world's origin (on a planet an f32 world
    /// position steps by half a metre).
    origin: DVec3,
    /// The eye in those coordinates.
    position: Vec3,
    forward: Vec3,
    matrix: Mat4,
    size: Vec2,
    far: f32,
    /// Half the visible world height of an orthographic view; `0` for a
    /// perspective one.
    ortho_half_height: f32,
}
impl Default for View {
    fn default() -> Self {
        Self {
            origin: DVec3::ZERO,
            position: Vec3::ZERO,
            forward: -Vec3::Z,
            matrix: Mat4::IDENTITY,
            size: Vec2::new(1600.0, 900.0),
            far: 10_000.0,
            ortho_half_height: 0.0,
        }
    }
}
impl View {
    /// World position `p` in the view's coordinates.
    fn relative(self, p: [f64; 3]) -> Vec3 {
        (DVec3::from_array(p) - self.origin).as_vec3()
    }
    /// The world position of `v` (view coordinates).
    fn world(self, v: Vec3) -> [f64; 3] {
        (self.origin + v.as_dvec3()).to_array()
    }
    /// `v` as a debug line vertex: world space, in f64 (Helio draws debug
    /// geometry relative to its frame's origin).
    fn vertex(self, v: Vec3) -> DVec3 {
        self.origin + v.as_dvec3()
    }
    fn project(self, point: Vec3) -> Option<Vec2> {
        let p = self.matrix * point.extend(1.0);
        if p.w <= 0.001 || !p.is_finite() {
            return None;
        }
        Some(Vec2::new(p.x / p.w * 0.5 + 0.5, 0.5 - p.y / p.w * 0.5) * self.size)
    }
    fn cursor(self, o: Vec3, d: Vec3) -> Option<Vec2> {
        self.project(o + d * 100.0)
    }
    fn length(self, p: Vec3) -> Option<f32> {
        let depth = (p - self.position).dot(self.forward);
        // Orthographic: the same world height at every depth.
        let world_height = if self.ortho_half_height > 0.0 {
            2.0 * self.ortho_half_height
        } else {
            2.0 * depth * std::f32::consts::FRAC_PI_8.tan()
        };
        (depth > 0.1 && depth < self.far).then_some(world_height * HANDLE_PIXELS / self.size.y)
    }
}
/// One object a gizmo drag moves.
#[derive(Clone, Copy, Debug)]
struct DragTarget {
    entity: Entity,
    /// Its transform in the world when the drag began.
    initial: Transform,
    /// Its own (parent-relative) transform then, which undo restores.
    initial_local: Transform,
    /// Its pivot, in its local space (`pivots`): what it turns and scales
    /// about.
    pivot_offset: [f32; 3],
}

/// What the gizmo acts on: the selected objects that can move, and the pose
/// its handles sit on.
struct GizmoTargets {
    /// Visible, unlocked selected objects, less any inside another of them
    /// (moving the outer one carries them). Never empty.
    objects: Vec<(Entity, Transform)>,
    /// One object: its own world transform, so the handles follow its axes,
    /// placed at its pivot. Several: the average of their pivots on world
    /// axes at unit scale, as Unreal and Blender place a multi-object gizmo
    /// by default.
    pivot: Transform,
}

#[derive(Clone, Copy, Debug)]
struct DragState {
    /// The primary selection when the drag began: a selection change ends it.
    primary: Option<Entity>,
    handle: Handle,
    mode: GizmoType,
    /// The gizmo's pose in the world when the drag began: what the handles
    /// are laid out on ([`GizmoTargets::pivot`]).
    initial: Transform,
    /// Whether it moves several objects about a shared pivot
    /// ([`SceneInteraction::drag_targets`]) rather than one object's own pose.
    group: bool,
    view: View,
    length: f32,
    start: Vec2,
    plane_normal: Vec3,
    plane_start: Vec3,
    /// Where on the axis line the cursor grabbed it (axis handles only).
    axis_start: f32,
    previous_angle: f32,
    angle: f32,
}
#[derive(Debug)]
pub struct SceneInteraction {
    mode: GizmoType,
    hovered: Option<Handle>,
    drag: Option<DragState>,
    /// The objects the drag in progress moves (empty without one).
    drag_targets: Vec<DragTarget>,
    view: View,
}
impl Default for SceneInteraction {
    fn default() -> Self {
        Self {
            mode: GizmoType::None,
            hovered: None,
            drag: None,
            drag_targets: Vec::new(),
            view: View::default(),
        }
    }
}
impl SceneInteraction {
    pub fn set_mode(&mut self, mode: GizmoType) {
        if self.mode != mode {
            self.cancel_drag();
            self.mode = mode;
        }
    }
    /// The view from `eye` (world, f64) along `forward`; `matrix` is the
    /// view-projection with the eye at the origin.
    pub fn set_view(&mut self, eye: DVec3, forward: Vec3, matrix: Mat4, size: Vec2, far: f32) {
        self.view = View {
            origin: eye,
            position: Vec3::ZERO,
            forward,
            matrix,
            size: size.max(Vec2::ONE),
            far,
            ortho_half_height: 0.0,
        };
    }
    /// Mark the view set by [`Self::set_view`] as orthographic, `half_height`
    /// world units from the centre to the top edge (`None`: perspective).
    pub fn set_orthographic(&mut self, half_height: Option<f32>) {
        self.view.ortho_half_height = half_height.filter(|h| *h > 0.0).unwrap_or(0.0);
    }
    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }
    pub fn cancel_drag(&mut self) {
        self.drag = None;
        self.drag_targets.clear();
        self.hovered = None;
    }
    /// End the drag in progress, if any: each object it moved with the
    /// (parent-relative) transform it started from, for one undo step.
    pub fn finish_drag(&mut self) -> Option<Vec<(Entity, Transform)>> {
        self.drag.take()?;
        self.hovered = None;
        Some(
            std::mem::take(&mut self.drag_targets)
                .into_iter()
                .map(|target| (target.entity, target.initial_local))
                .collect(),
        )
    }

    fn gizmo_targets(&self, world: &World) -> Option<GizmoTargets> {
        if self.mode == GizmoType::None {
            return None;
        }
        let selection = world.selection();
        let movable = |entity: Entity| {
            world
                .get::<Visibility>(entity)
                .is_some_and(|v| v.visible && !v.locked)
        };
        // A selected object inside another selected one moves with it.
        let inside_selected = |entity: Entity| {
            let mut at = world.parent_of(entity);
            while let Some(parent) = at {
                if selection.contains(&parent) {
                    return true;
                }
                at = world.parent_of(parent);
            }
            false
        };
        let objects: Vec<(Entity, Transform)> = selection
            .iter()
            .copied()
            .filter(|&entity| movable(entity) && !inside_selected(entity))
            .filter_map(|entity| Some((entity, world_transform_of(world, entity)?)))
            .collect();
        let pivot_of = |entity: Entity, transform: &Transform| {
            pivot_point(transform, pivot_offset_of(world, entity))
        };
        let pivot = match objects.as_slice() {
            [] => return None,
            [(entity, transform)] => Transform {
                position: pivot_of(*entity, transform),
                ..*transform
            },
            many => {
                let sum = many.iter().fold(DVec3::ZERO, |sum, (entity, t)| {
                    sum + DVec3::from_array(pivot_of(*entity, t))
                });
                Transform {
                    position: (sum / many.len() as f64).to_array(),
                    rotation: [0.0; 3],
                    scale: [1.0; 3],
                }
            }
        };
        Some(GizmoTargets { objects, pivot })
    }
    /// The primary selection and the pose the gizmo is drawn on.
    fn selected(&self, world: &World) -> Option<(Entity, Transform)> {
        let targets = self.gizmo_targets(world)?;
        Some((targets.objects[0].0, targets.pivot))
    }
    fn handle_visible(&self, handle: Handle, pivot: Vec3, basis: Mat3, length: f32) -> bool {
        let Some(center) = self.view.project(pivot) else {
            return false;
        };
        let projected = |i| {
            self.view
                .project(pivot + basis.col(i) * length)
                .map(|p| p - center)
        };
        match handle {
            Handle::Axis(i) if self.mode != GizmoType::Rotate => {
                projected(i).is_some_and(|v| v.length() > 12.0)
            }
            Handle::Plane(i) => match (projected((i + 1) % 3), projected((i + 2) % 3)) {
                (Some(a), Some(b)) => a.perp_dot(b).abs() * 0.04 > 16.0,
                _ => false,
            },
            _ => true,
        }
    }
    fn triangle_visible(&self, handle: Handle, tri: &[Vec3; 3], pivot: Vec3, basis: Mat3) -> bool {
        if self.mode != GizmoType::Rotate {
            return true;
        }
        let Handle::Axis(i) = handle else {
            return true;
        };
        let signs = self.quadrant_signs(pivot, basis);
        let center = (tri[0] + tri[1] + tri[2]) / 3.0;
        // Display only the camera-facing quadrant; picking uses the identical mask.
        [(i + 1) % 3, (i + 2) % 3]
            .into_iter()
            .all(|j| center[j] * signs[j] >= -0.0001)
    }
    fn quadrant_signs(&self, pivot: Vec3, basis: Mat3) -> Vec3 {
        let toward_camera = basis.transpose() * (self.view.position - pivot);
        // Independent axis signs select an octant, never a continuously turning billboard.
        Vec3::from_array(
            toward_camera
                .to_array()
                .map(|v| if v < -0.0001 { -1.0 } else { 1.0 }),
        )
    }
    fn hit(&self, t: Transform, cursor: Vec2) -> Option<Handle> {
        let pivot = self.view.relative(t.position);
        let length = self.view.length(pivot)?;
        let basis = rotation_matrix(t);
        let mut best: Option<(f32, f32, Handle)> = None;
        for shape in shapes(self.mode) {
            if !self.handle_visible(shape.handle, pivot, basis, length) {
                continue;
            }
            for tri in &shape.triangles {
                if !self.triangle_visible(shape.handle, tri, pivot, basis) {
                    continue;
                }
                let points = tri.map(|p| pivot + basis * p * length);
                let [Some(a), Some(b), Some(c)] = points.map(|p| self.view.project(p)) else {
                    continue;
                };
                let distance = triangle_distance(cursor, a, b, c);
                if distance > PICK_MARGIN {
                    continue;
                }
                let depth = points
                    .iter()
                    .map(|p| (*p - self.view.position).dot(self.view.forward))
                    .sum::<f32>()
                    / 3.0;
                if best.is_none_or(|(d, z, _)| {
                    distance < d - 0.01 || ((distance - d).abs() < 0.01 && depth < z)
                }) {
                    best = Some((distance, depth, shape.handle));
                }
            }
        }
        best.map(|(_, _, h)| h)
    }
    /// Grab a gizmo handle under the ray from `o` (world) along `d`.
    pub fn try_start_drag(&mut self, world: &World, o: DVec3, d: Vec3) -> bool {
        let Some(targets) = self.gizmo_targets(world) else {
            return false;
        };
        let initial = targets.pivot;
        let o = self.view.relative(o.to_array());
        let pivot = self.view.relative(initial.position);
        let (Some(start), Some(length)) = (self.view.cursor(o, d), self.view.length(pivot)) else {
            return false;
        };
        let Some(handle) = self.hit(initial, start) else {
            return false;
        };
        let basis = rotation_matrix(initial);
        let axis = match handle {
            Handle::Axis(i) | Handle::Plane(i) => basis.col(i),
            Handle::Center => self.view.forward,
        };
        let plane_normal = if matches!(handle, Handle::Plane(_)) {
            axis
        } else {
            self.view.forward
        };
        let plane_start = ray_plane_intersection(o, d, pivot, plane_normal).unwrap_or(pivot);
        let axis_start = match handle {
            Handle::Axis(_) => axis_param(o, d, pivot, axis, self.view.forward).unwrap_or(0.0),
            _ => 0.0,
        };
        let previous_angle = if let Handle::Axis(i) = handle {
            projected_ring_initial_angle(
                self.view,
                pivot,
                basis,
                i,
                length,
                start,
                self.quadrant_signs(pivot, basis),
            )
        } else {
            0.0
        };
        let group = targets.objects.len() > 1;
        self.drag_targets = targets
            .objects
            .iter()
            .map(|&(entity, world_pose)| DragTarget {
                entity,
                initial: world_pose,
                initial_local: world
                    .get::<Transform>(entity)
                    .copied()
                    .unwrap_or(world_pose),
                pivot_offset: pivot_offset_of(world, entity),
            })
            .collect();
        self.drag = Some(DragState {
            primary: world.selected_entity(),
            handle,
            mode: self.mode,
            initial,
            group,
            view: self.view,
            length,
            start,
            plane_normal,
            plane_start,
            axis_start,
            previous_angle,
            angle: 0.0,
        });
        self.hovered = Some(handle);
        true
    }
    /// Highlight the handle under the ray from `o` (world) along `d`.
    pub fn update_hover(&mut self, world: &World, o: DVec3, d: Vec3) -> bool {
        let previous = self.hovered;
        let o = self.view.relative(o.to_array());
        if let Some(drag) = self.drag {
            self.hovered = Some(drag.handle);
            return previous != self.hovered;
        }
        self.hovered = self
            .selected(world)
            .and_then(|(_, t)| self.view.cursor(o, d).and_then(|p| self.hit(t, p)));
        previous != self.hovered
    }
}
