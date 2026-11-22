//! Pure geometry for the gizmo: rays against planes and boxes, screen-space
//! triangle and ring solves, Euler rotation conversions and axis colours.
use super::View;
use crate::scene::Transform;
use glam::{EulerRot, Mat3, Quat, Vec2, Vec3};

pub(super) fn projected_ring_angle(
    view: View,
    pivot: Vec3,
    basis: Mat3,
    axis: usize,
    length: f32,
    cursor: Vec2,
    previous: f32,
) -> Option<f32> {
    let point = |angle: f32| {
        view.project(
            pivot
                + length
                    * 0.85
                    * (basis.col((axis + 1) % 3) * angle.cos()
                        + basis.col((axis + 2) % 3) * angle.sin()),
        )
    };
    let mut angle = previous;
    // Follow the local projected arc rather than intersecting a nearly parallel plane.
    // Keeping the previous angle as the seed preserves the branch across full turns.
    for _ in 0..16 {
        let p = point(angle)?;
        let tangent = (point(angle + 0.001)? - point(angle - 0.001)?) / 0.002;
        if tangent.length_squared() < 4.0 {
            break;
        }
        let mut step = ((cursor - p).dot(tangent) / tangent.length_squared()).clamp(-0.25, 0.25);
        let error = cursor.distance_squared(p);
        // Damping prevents oscillation when the pointer strays far outside the ring.
        while step.abs() > 0.00001 && cursor.distance_squared(point(angle + step)?) > error {
            step *= 0.5;
        }
        angle += step;
        if step.abs() < 0.00001 {
            break;
        }
    }
    Some(angle)
}
fn segment_distance(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-10)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}
pub(super) fn projected_ring_initial_angle(
    view: View,
    pivot: Vec3,
    basis: Mat3,
    axis: usize,
    length: f32,
    cursor: Vec2,
    signs: Vec3,
) -> f32 {
    let center = view.project(pivot).unwrap_or(cursor);
    let u = (axis + 1) % 3;
    let v = (axis + 2) % 3;
    let mut best = (f32::INFINITY, 0.0);
    for sample in 0..1024 {
        let angle = sample as f32 * std::f32::consts::TAU / 1024.0;
        if angle.cos() * signs[u] < -0.0001 || angle.sin() * signs[v] < -0.0001 {
            continue;
        }
        let point = view
            .project(
                pivot + length * 0.85 * (basis.col(u) * angle.cos() + basis.col(v) * angle.sin()),
            )
            .unwrap_or(center);
        let distance = point.distance_squared(cursor);
        if distance < best.0 {
            best = (distance, angle);
        }
    }
    best.1
}
pub(super) fn triangle_distance(p: Vec2, a: Vec2, b: Vec2, c: Vec2) -> f32 {
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
pub(super) fn rotation_axis_color(axis: usize) -> [f32; 4] {
    match axis {
        0 => [0.95, 0.12, 0.09, 1.0],
        1 => [0.2, 0.85, 0.12, 1.0],
        _ => [0.12, 0.4, 1.0, 1.0],
    }
}
pub(super) fn rotation_axis_highlight_color(axis: usize) -> [f32; 4] {
    match axis {
        0 => [1.0, 0.62, 0.58, 1.0],
        1 => [0.62, 1.0, 0.58, 1.0],
        _ => [0.62, 0.78, 1.0, 1.0],
    }
}
/// Distance along the unit `axis` through `pivot` of the point on that line
/// nearest the mouse ray. The ray is intersected with the plane that contains
/// the axis and faces the camera most, then projected onto the axis. This is
/// the standard editor gizmo solve: the grabbed point stays under the cursor
/// exactly, in perspective, at any distance. `None` when the axis points
/// nearly down the view direction, where the answer is ill-conditioned.
pub(super) fn axis_param(
    o: Vec3,
    d: Vec3,
    pivot: Vec3,
    axis: Vec3,
    view_forward: Vec3,
) -> Option<f32> {
    let normal = view_forward - axis * axis.dot(view_forward);
    if normal.length_squared() < 0.0025 {
        return None;
    }
    let hit = ray_plane_intersection(o, d, pivot, normal.normalize())?;
    Some((hit - pivot).dot(axis))
}
pub(super) fn ray_plane_intersection(o: Vec3, d: Vec3, p: Vec3, n: Vec3) -> Option<Vec3> {
    let denominator = d.dot(n);
    if denominator.abs() < 1e-5 {
        return None;
    }
    let t = (p - o).dot(n) / denominator;
    (t >= 0.0 && t.is_finite()).then_some(o + d * t)
}
pub(super) fn rotation_quat(transform: Transform) -> Quat {
    Quat::from_euler(
        EulerRot::YXZ,
        transform.rotation[1].to_radians(),
        transform.rotation[0].to_radians(),
        transform.rotation[2].to_radians(),
    )
}

pub(super) fn rotation_matrix(transform: Transform) -> Mat3 {
    Mat3::from_quat(rotation_quat(transform))
}

pub(super) fn ray_aabb(origin: Vec3, direction: Vec3, min: Vec3, max: Vec3) -> Option<f32> {
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
