//! Shared, immutable handle meshes for drawing and screen-space picking.
use crate::scene::GizmoType;
use glam::Vec3;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Handle {
    Axis(usize),
    Plane(usize),
    Center,
}
pub(super) struct Mesh {
    pub handle: Handle,
    pub triangles: Vec<[Vec3; 3]>,
}
/// Non-interactive quarter-disc grid, bounded by the inner edge of the arc.
pub(super) fn rotation_grid(axis: usize, signs: Vec3) -> Vec<[Vec3; 2]> {
    let u = Vec3::AXES[(axis + 1) % 3] * signs[(axis + 1) % 3];
    let v = Vec3::AXES[(axis + 2) % 3] * signs[(axis + 2) % 3];
    let radius = 0.81_f32;
    let mut lines = Vec::with_capacity(18);
    for step in 0..9 {
        let offset = radius * step as f32 / 9.0;
        let extent = (radius * radius - offset * offset).sqrt();
        lines.push([u * offset, u * offset + v * extent]);
        lines.push([v * offset, v * offset + u * extent]);
    }
    lines
}
fn quad(tris: &mut Vec<[Vec3; 3]>, a: Vec3, b: Vec3, c: Vec3, d: Vec3) {
    tris.extend([[a, b, c], [a, c, d]]);
}
fn tube(tris: &mut Vec<[Vec3; 3]>, a: Vec3, b: Vec3, ra: f32, rb: f32) {
    let n = (b - a).normalize();
    let u = n.any_orthonormal_vector();
    let v = n.cross(u);
    for i in 0..16 {
        let angle = |j: usize| j as f32 * std::f32::consts::TAU / 16.0;
        let radial = |t: f32| u * t.cos() + v * t.sin();
        let p = radial(angle(i));
        let q = radial(angle(i + 1));
        quad(tris, a + p * ra, a + q * ra, b + q * rb, b + p * rb);
        tris.extend([[a, a + q * ra, a + p * ra], [b, b + p * rb, b + q * rb]]);
    }
}
fn cube(tris: &mut Vec<[Vec3; 3]>, c: Vec3, r: f32) {
    for n in [Vec3::X, Vec3::Y, Vec3::Z] {
        let u = n.any_orthonormal_vector() * r;
        let v = n.cross(u);
        for sign in [-1.0, 1.0] {
            let p = c + n * r * sign;
            quad(tris, p - u - v, p + u - v, p + u + v, p - u + v);
        }
    }
}
fn build(mode: GizmoType) -> Vec<Mesh> {
    let axes = [Vec3::X, Vec3::Y, Vec3::Z];
    let mut meshes = Vec::new();
    for i in 0..3 {
        let mut triangles = Vec::new();
        let axis = axes[i];
        if mode == GizmoType::Rotate {
            let u = axes[(i + 1) % 3];
            let v = axes[(i + 2) % 3];
            for j in 0..96 {
                let t = j as f32 * std::f32::consts::TAU / 96.0;
                let s = (j + 1) as f32 * std::f32::consts::TAU / 96.0;
                // Broad annular ribbon with a thin solid edge, not a wire tube.
                let vertex = |a: f32, radius: f32, height: f32| {
                    (u * a.cos() + v * a.sin()) * radius + axis * height
                };
                for height in [-0.006, 0.006] {
                    quad(
                        &mut triangles,
                        vertex(t, 0.81, height),
                        vertex(s, 0.81, height),
                        vertex(s, 0.89, height),
                        vertex(t, 0.89, height),
                    );
                }
                for radius in [0.81, 0.89] {
                    quad(
                        &mut triangles,
                        vertex(t, radius, -0.006),
                        vertex(s, radius, -0.006),
                        vertex(s, radius, 0.006),
                        vertex(t, radius, 0.006),
                    );
                }
            }
        } else {
            tube(&mut triangles, axis * 0.12, axis * 0.82, 0.022, 0.022);
            if mode == GizmoType::Translate {
                tube(&mut triangles, axis * 0.78, axis, 0.075, 0.0);
            } else {
                cube(&mut triangles, axis * 0.94, 0.065);
            }
        }
        meshes.push(Mesh {
            handle: Handle::Axis(i),
            triangles,
        });
    }
    if mode == GizmoType::Translate {
        for i in 0..3 {
            let u = axes[(i + 1) % 3];
            let v = axes[(i + 2) % 3];
            let mut triangles = Vec::new();
            quad(
                &mut triangles,
                (u + v) * 0.23,
                u * 0.43 + v * 0.23,
                (u + v) * 0.43,
                u * 0.23 + v * 0.43,
            );
            meshes.push(Mesh {
                handle: Handle::Plane(i),
                triangles,
            });
        }
    }
    if mode != GizmoType::Rotate {
        let mut triangles = Vec::new();
        cube(&mut triangles, Vec3::ZERO, 0.055);
        meshes.push(Mesh {
            handle: Handle::Center,
            triangles,
        });
    }
    meshes
}
pub(super) fn meshes(mode: GizmoType) -> &'static [Mesh] {
    static MOVE: OnceLock<Vec<Mesh>> = OnceLock::new();
    static ROTATE: OnceLock<Vec<Mesh>> = OnceLock::new();
    static SCALE: OnceLock<Vec<Mesh>> = OnceLock::new();
    match mode {
        GizmoType::Translate => MOVE.get_or_init(|| build(mode)),
        GizmoType::Rotate => ROTATE.get_or_init(|| build(mode)),
        GizmoType::Scale => SCALE.get_or_init(|| build(mode)),
        GizmoType::None => &[],
    }
}
