//! Transform handles with shared solid geometry for drawing and picking.
use super::gizmo_geometry::{meshes, Handle};
use crate::scene::{GizmoType, ObjectType, SceneWorldExt, StableId, Transform, Visibility};
use glam::{EulerRot, Mat3, Mat4, Quat, Vec2, Vec3};
use helio::Renderer;
use pulsar_scenedb::{Entity, World};
const HANDLE_PIXELS: f32 = 112.0;
const PICK_MARGIN: f32 = 7.0;
#[derive(Clone, Copy, Debug)]
struct View {
    position: Vec3,
    forward: Vec3,
    matrix: Mat4,
    size: Vec2,
}
impl Default for View {
    fn default() -> Self {
        Self {
            position: Vec3::ZERO,
            forward: -Vec3::Z,
            matrix: Mat4::IDENTITY,
            size: Vec2::new(1600.0, 900.0),
        }
    }
}
impl View {
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
        (depth > 0.1).then_some(
            2.0 * depth * std::f32::consts::FRAC_PI_8.tan() * HANDLE_PIXELS / self.size.y,
        )
    }
}
#[derive(Clone, Copy, Debug)]
struct DragState {
    entity: Entity,
    handle: Handle,
    mode: GizmoType,
    initial: Transform,
    view: View,
    length: f32,
    start: Vec2,
    screen_axis: Vec2,
    plane_normal: Vec3,
    plane_start: Vec3,
    previous_angle: f32,
    angle: f32,
}
#[derive(Debug)]
pub struct SceneInteraction {
    mode: GizmoType,
    hovered: Option<Handle>,
    drag: Option<DragState>,
    view: View,
}
impl Default for SceneInteraction {
    fn default() -> Self {
        Self {
            mode: GizmoType::None,
            hovered: None,
            drag: None,
            view: View::default(),
        }
    }
}
impl SceneInteraction {
    pub fn mode(&self) -> GizmoType {
        self.mode
    }
    pub fn set_mode(&mut self, mode: GizmoType) {
        if self.mode != mode {
            self.cancel_drag();
            self.mode = mode;
        }
    }
    pub fn set_view(&mut self, position: Vec3, forward: Vec3, matrix: Mat4, size: Vec2) {
        self.view = View {
            position,
            forward,
            matrix,
            size: size.max(Vec2::ONE),
        };
    }
    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }
    pub fn cancel_drag(&mut self) {
        self.drag = None;
        self.hovered = None;
    }
    pub fn pick(&self, world: &World, origin: Vec3, direction: Vec3) -> Option<String> {
        let direction = direction.normalize_or_zero();
        if direction == Vec3::ZERO {
            return None;
        }

        world
            .query::<&StableId>()
            .filter_map(|(entity, id)| {
                let visibility = world.get::<Visibility>(entity)?;
                let object_type = *world.get::<ObjectType>(entity)?;
                if !visibility.visible || !is_pickable(object_type) {
                    return None;
                }
                let transform = *world.get::<Transform>(entity)?;
                let (min, max) = world_bounds(&object_type, transform);
                ray_aabb(origin, direction, min, max).map(|distance| (distance, id.0.clone()))
            })
            .min_by(|(left, left_id), (right, right_id)| {
                left.partial_cmp(right)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| left_id.cmp(right_id))
            })
            .map(|(_, stable_id)| stable_id)
    }

    fn selected(&self, world: &World) -> Option<(Entity, Transform)> {
        let entity = world.selected_entity()?;
        let v = world.get::<Visibility>(entity)?;
        if !v.visible || v.locked || self.mode == GizmoType::None {
            return None;
        }
        Some((entity, *world.get::<Transform>(entity)?))
    }
    fn hit(&self, t: Transform, cursor: Vec2) -> Option<Handle> {
        let pivot = Vec3::from_array(t.position);
        let length = self.view.length(pivot)?;
        let basis = rotation_matrix(t);
        let mut best: Option<(f32, f32, Handle)> = None;
        for mesh in meshes(self.mode) {
            for tri in &mesh.triangles {
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
                    best = Some((distance, depth, mesh.handle));
                }
            }
        }
        best.map(|(_, _, h)| h)
    }
    pub fn try_start_drag(&mut self, world: &World, o: Vec3, d: Vec3, _camera: Vec3) -> bool {
        let Some((entity, initial)) = self.selected(world) else {
            return false;
        };
        let Some(start) = self.view.cursor(o, d) else {
            return false;
        };
        let Some(handle) = self.hit(initial, start) else {
            return false;
        };
        let pivot = Vec3::from_array(initial.position);
        let length = self.view.length(pivot).unwrap();
        let basis = rotation_matrix(initial);
        let center = self.view.project(pivot).unwrap();
        let axis = match handle {
            Handle::Axis(i) | Handle::Plane(i) => basis.col(i),
            Handle::Center => self.view.forward,
        };
        let screen_axis = self.view.project(pivot + axis * length).unwrap_or(center) - center;
        let plane_normal = if matches!(handle, Handle::Plane(_)) {
            axis
        } else {
            self.view.forward
        };
        let plane_start = ray_plane_intersection(o, d, pivot, plane_normal).unwrap_or(pivot);
        let previous_angle = if let Handle::Axis(i) = handle {
            rotation_angle(o, d, pivot, basis, i).unwrap_or(0.0)
        } else {
            0.0
        };
        self.drag = Some(DragState {
            entity,
            handle,
            mode: self.mode,
            initial,
            view: self.view,
            length,
            start,
            screen_axis,
            plane_normal,
            plane_start,
            previous_angle,
            angle: 0.0,
        });
        self.hovered = Some(handle);
        true
    }
    pub fn update_drag(&mut self, world: &mut World, o: Vec3, d: Vec3, _camera: Vec3) {
        let Some(mut drag) = self.drag else {
            return;
        };
        if world.selected_entity() != Some(drag.entity)
            || !world
                .get::<Visibility>(drag.entity)
                .is_some_and(|v| v.visible && !v.locked)
        {
            self.cancel_drag();
            return;
        }
        let Some(current) = world.get::<Transform>(drag.entity).copied() else {
            self.cancel_drag();
            return;
        };
        let Some(cursor) = drag.view.cursor(o, d) else {
            return;
        };
        let delta = cursor - drag.start;
        let pivot = Vec3::from_array(drag.initial.position);
        let basis = rotation_matrix(drag.initial);
        let amount = if drag.screen_axis.length_squared() > 16.0 {
            delta.dot(drag.screen_axis) / drag.screen_axis.length_squared()
        } else {
            -delta.y / HANDLE_PIXELS
        };
        let mut next = current;
        match drag.mode {
            GizmoType::Translate => {
                let movement = match drag.handle {
                    Handle::Axis(i) => basis.col(i) * amount * drag.length,
                    _ => {
                        let Some(p) = ray_plane_intersection(o, d, pivot, drag.plane_normal) else {
                            return;
                        };
                        p - drag.plane_start
                    }
                };
                next.position = (pivot + movement).to_array();
            }
            GizmoType::Scale => {
                let factor = (1.0
                    + if drag.handle == Handle::Center {
                        (delta.x - delta.y) / HANDLE_PIXELS
                    } else {
                        amount
                    })
                .max(0.001);
                next.scale = drag.initial.scale;
                for i in 0..3 {
                    if drag.handle == Handle::Center || drag.handle == Handle::Axis(i) {
                        next.scale[i] = (drag.initial.scale[i] * factor).max(0.001);
                    }
                }
            }
            GizmoType::Rotate => {
                let Handle::Axis(i) = drag.handle else {
                    return;
                };
                let axis = basis.col(i);
                if axis.dot(drag.view.forward).abs() > 0.15 {
                    let Some(angle) = rotation_angle(o, d, pivot, basis, i) else {
                        return;
                    };
                    let step = (angle - drag.previous_angle + std::f32::consts::PI)
                        .rem_euclid(std::f32::consts::TAU)
                        - std::f32::consts::PI;
                    drag.angle += step;
                    drag.previous_angle = angle;
                } else {
                    // Edge-on rings have an ill-conditioned ray/plane intersection.
                    // Use a fixed screen tangent for the duration of the gesture.
                    let radial = (drag.plane_start - pivot).normalize_or_zero();
                    let tangent = axis.cross(radial).normalize_or_zero();
                    let center = drag.view.project(pivot).unwrap();
                    let projected = drag
                        .view
                        .project(pivot + tangent * drag.length)
                        .unwrap_or(center)
                        - center;
                    let tangent = projected.try_normalize().unwrap_or(Vec2::X);
                    drag.angle = delta.dot(tangent) / HANDLE_PIXELS;
                }
                let q = Quat::from_axis_angle(axis, drag.angle) * Quat::from_mat3(&basis);
                let (y, x, z) = q.to_euler(EulerRot::YXZ);
                next.rotation = [x.to_degrees(), y.to_degrees(), z.to_degrees()];
            }
            GizmoType::None => return,
        }
        self.drag = Some(drag);
        self.hovered = Some(drag.handle);
        if next != current {
            if let Some(mut t) = world.get_mut::<Transform>(drag.entity) {
                *t = next;
            }
        }
    }
    pub fn update_hover(&mut self, world: &World, o: Vec3, d: Vec3, _camera: Vec3) -> bool {
        let previous = self.hovered;
        if let Some(drag) = self.drag {
            self.hovered = Some(drag.handle);
            return previous != self.hovered;
        }
        self.hovered = self
            .selected(world)
            .and_then(|(_, t)| self.view.cursor(o, d).and_then(|p| self.hit(t, p)));
        previous != self.hovered
    }
    pub fn draw_gizmo(&self, renderer: &mut Renderer, world: &World, _camera: Vec3) {
        let Some((_, t)) = self.selected(world) else {
            return;
        };
        let pivot = Vec3::from_array(t.position);
        let Some(length) = self.view.length(pivot) else {
            return;
        };
        let basis = rotation_matrix(
            self.drag
                .filter(|d| d.mode == GizmoType::Rotate)
                .map(|d| d.initial)
                .unwrap_or(t),
        );
        let active = self.drag.map(|d| d.handle).or(self.hovered);
        // Submit the complete widget under one lock and upload generation.
        renderer.debug_batch(|batch| {
            for mesh in meshes(self.mode) {
                let base = if active == Some(mesh.handle) {
                    [1.0, 0.8, 0.12, 1.0]
                } else {
                    match mesh.handle {
                        Handle::Axis(0) | Handle::Plane(0) => [0.95, 0.12, 0.09, 1.0],
                        Handle::Axis(1) | Handle::Plane(1) => [0.2, 0.85, 0.12, 1.0],
                        Handle::Axis(_) | Handle::Plane(_) => [0.12, 0.4, 1.0, 1.0],
                        Handle::Center => [0.88, 0.9, 0.94, 1.0],
                    }
                };
                for tri in &mesh.triangles {
                    let [a, b, c] = tri.map(|p| pivot + basis * p * length);
                    let n = (b - a).cross(c - a).normalize_or_zero();
                    let shade = 0.65 + 0.35 * n.dot(Vec3::new(0.3, 0.8, 0.5).normalize()).abs();
                    batch.tri(
                        a.to_array(),
                        b.to_array(),
                        c.to_array(),
                        [base[0] * shade, base[1] * shade, base[2] * shade, base[3]],
                    );
                }
            }
        });
    }
}
fn segment_distance(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-10)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}
fn rotation_angle(o: Vec3, d: Vec3, p: Vec3, basis: Mat3, i: usize) -> Option<f32> {
    let point = ray_plane_intersection(o, d, p, basis.col(i))? - p;
    if point.length_squared() < 1e-10 {
        return None;
    }
    Some(
        point
            .dot(basis.col((i + 2) % 3))
            .atan2(point.dot(basis.col((i + 1) % 3))),
    )
}
fn triangle_distance(p: Vec2, a: Vec2, b: Vec2, c: Vec2) -> f32 {
    let area = (b - a).perp_dot(c - a);
    if area.abs() > 0.001 {
        if (b - a).perp_dot(p - a) / area >= 0.0
            && (c - b).perp_dot(p - b) / area >= 0.0
            && (a - c).perp_dot(p - c) / area >= 0.0
        {
            return 0.0;
        }
    }
    segment_distance(p, a, b)
        .min(segment_distance(p, b, c))
        .min(segment_distance(p, c, a))
}
fn ray_plane_intersection(o: Vec3, d: Vec3, p: Vec3, n: Vec3) -> Option<Vec3> {
    let denominator = d.dot(n);
    if denominator.abs() < 1e-5 {
        return None;
    }
    let t = (p - o).dot(n) / denominator;
    (t >= 0.0 && t.is_finite()).then_some(o + d * t)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{MeshType, SpawnObject};

    fn setup(mode: GizmoType) -> (World, SceneInteraction, Entity) {
        let mut world = World::new();
        spawn_cube(&mut world, "selected", -10.0);
        let entity = world.query::<&StableId>().next().unwrap().0;
        world.select(Some(entity));
        let mut interaction = SceneInteraction::default();
        interaction.set_mode(mode);
        interaction.set_view(
            Vec3::ZERO,
            -Vec3::Z,
            Mat4::perspective_rh(std::f32::consts::FRAC_PI_4, 1.0, 0.1, 10000.0),
            Vec2::splat(900.0),
        );
        (world, interaction, entity)
    }

    #[test]
    fn press_captures_without_hover_and_drag_continues_off_handle() {
        let (mut world, mut interaction, entity) = setup(GizmoType::Translate);
        let pivot = Vec3::new(0.0, 0.0, -10.0);
        let length = interaction.view.length(pivot).unwrap();
        let start = pivot + Vec3::X * length * 0.7;
        assert!(interaction.try_start_drag(&world, Vec3::ZERO, start.normalize(), Vec3::ZERO));
        assert_eq!(interaction.drag.unwrap().handle, Handle::Axis(0));
        let away = pivot + Vec3::new(length * 4.0, length * 2.0, 0.0);
        interaction.update_hover(&world, Vec3::ZERO, away.normalize(), Vec3::ZERO);
        assert_eq!(interaction.hovered, Some(Handle::Axis(0)));
        interaction.update_drag(&mut world, Vec3::ZERO, away.normalize(), Vec3::ZERO);
        assert!(world.get::<Transform>(entity).unwrap().position[0] > length * 3.0);
        interaction.cancel_drag();
        let final_transform = *world.get::<Transform>(entity).unwrap();
        interaction.update_drag(&mut world, Vec3::ZERO, start.normalize(), Vec3::ZERO);
        assert_eq!(*world.get::<Transform>(entity).unwrap(), final_transform);
    }

    #[test]
    fn ring_is_pickable_between_axes() {
        let (world, interaction, entity) = setup(GizmoType::Rotate);
        let t = *world.get::<Transform>(entity).unwrap();
        let pivot = Vec3::from_array(t.position);
        let length = interaction.view.length(pivot).unwrap();
        let point = pivot + Vec3::new(0.6, 0.6, 0.0) * length;
        assert_eq!(
            interaction.hit(t, interaction.view.project(point).unwrap()),
            Some(Handle::Axis(2))
        );
    }

    #[test]
    fn handle_size_is_constant_with_distance() {
        let (_, interaction, _) = setup(GizmoType::Translate);
        for depth in [1.0, 10.0, 1000.0] {
            let pivot = Vec3::new(0.0, 0.0, -depth);
            let end = pivot + Vec3::X * interaction.view.length(pivot).unwrap();
            let size = interaction
                .view
                .project(end)
                .unwrap()
                .distance(interaction.view.project(pivot).unwrap());
            assert!((size - HANDLE_PIXELS).abs() < 0.01);
        }
    }

    #[test]
    fn hover_margin_is_in_pixels_and_empty_space_does_not_capture() {
        let (_, interaction, _) = setup(GizmoType::Translate);
        for depth in [2.0, 40.0, 1000.0] {
            let t = Transform {
                position: [0.0, 0.0, -depth],
                ..Default::default()
            };
            let pivot = Vec3::from_array(t.position);
            let length = interaction.view.length(pivot).unwrap();
            let shaft = interaction
                .view
                .project(pivot + Vec3::X * length * 0.65)
                .unwrap();
            assert_eq!(
                interaction.hit(t, shaft + Vec2::Y * 6.0),
                Some(Handle::Axis(0))
            );
            assert_eq!(interaction.hit(t, shaft + Vec2::Y * 25.0), None);
        }
    }

    #[test]
    fn plane_and_center_handles_capture_and_locked_objects_do_not() {
        let (mut world, mut interaction, entity) = setup(GizmoType::Translate);
        let t = *world.get::<Transform>(entity).unwrap();
        let pivot = Vec3::from_array(t.position);
        let length = interaction.view.length(pivot).unwrap();
        let plane = pivot + Vec3::new(0.33, 0.33, 0.0) * length;
        assert!(interaction.try_start_drag(&world, Vec3::ZERO, plane.normalize(), Vec3::ZERO));
        assert_eq!(interaction.drag.unwrap().handle, Handle::Plane(2));
        interaction.cancel_drag();
        assert!(interaction.try_start_drag(&world, Vec3::ZERO, pivot.normalize(), Vec3::ZERO));
        assert_eq!(interaction.drag.unwrap().handle, Handle::Center);
        interaction.cancel_drag();
        world.get_mut::<Visibility>(entity).unwrap().locked = true;
        assert!(!interaction.try_start_drag(&world, Vec3::ZERO, plane.normalize(), Vec3::ZERO));
    }

    #[test]
    fn rotation_follows_ring_plane_and_preserves_other_fields() {
        let (mut world, mut interaction, entity) = setup(GizmoType::Rotate);
        let pivot = Vec3::new(0.0, 0.0, -10.0);
        let length = interaction.view.length(pivot).unwrap();
        let start = pivot + Vec3::new(0.6, 0.6, 0.0) * length;
        assert!(interaction.try_start_drag(&world, Vec3::ZERO, start.normalize(), Vec3::ZERO));
        let end = pivot + Vec3::new(-0.6, 0.6, 0.0) * length;
        interaction.update_drag(&mut world, Vec3::ZERO, end.normalize(), Vec3::ZERO);
        let t = world.get::<Transform>(entity).unwrap();
        assert!((t.rotation[2] - 90.0).abs() < 0.01);
        assert_eq!(t.position, pivot.to_array());
        assert_eq!(t.scale, [1.0; 3]);
    }

    fn spawn_cube(world: &mut World, id: &str, z: f32) {
        world
            .spawn_object(
                SpawnObject::new(id)
                    .with_id(id)
                    .with_object_type(ObjectType::Mesh(MeshType::Cube))
                    .with_transform(Transform {
                        position: [0.0, 0.0, z],
                        ..Default::default()
                    }),
            )
            .unwrap();
    }

    #[test]
    fn nearest_visible_mesh_is_picked_from_world() {
        let mut world = World::new();
        spawn_cube(&mut world, "far", -10.0);
        spawn_cube(&mut world, "near", -3.0);

        let interaction = SceneInteraction::default();
        assert_eq!(
            interaction.pick(&world, Vec3::ZERO, -Vec3::Z),
            Some("near".into())
        );
    }

    #[test]
    fn hidden_mesh_is_not_pickable() {
        let mut world = World::new();
        world
            .spawn_object(
                SpawnObject::new("Hidden")
                    .with_id("hidden")
                    .with_object_type(ObjectType::Mesh(MeshType::Cube))
                    .with_visibility(Visibility {
                        visible: false,
                        locked: false,
                    }),
            )
            .unwrap();
        assert_eq!(
            SceneInteraction::default().pick(&world, Vec3::ZERO, -Vec3::Z),
            None
        );
    }
}
