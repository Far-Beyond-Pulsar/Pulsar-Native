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
                // Continuous solid tube, with the same triangles used for picking.
                for k in 0..8 {
                    let vertex = |a: f32, b: f32| {
                        let radial = u * a.cos() + v * a.sin();
                        radial * (0.85 + 0.024 * b.cos()) + axis * (0.024 * b.sin())
                    };
                    let b = k as f32 * std::f32::consts::TAU / 8.0;
                    let c = (k + 1) as f32 * std::f32::consts::TAU / 8.0;
                    quad(
                        &mut triangles,
                        vertex(t, b),
                        vertex(s, b),
                        vertex(s, c),
                        vertex(t, c),
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
