//! CPU references of the shader math, for tests and for tools that want to
//! reproduce the resolve (e.g. a debug picker). Mirrors `vgs_bary` and
//! `vgs_pixel_ndc` in `shaders/vg_common.wgsl` exactly.

use glam::{Mat4, Vec2, Vec3, Vec4};

/// NDC xy of a pixel position (pixels, y down) in a `width x height` target.
pub fn pixel_ndc(pixel: Vec2, size: Vec2) -> Vec2 {
    Vec2::new(pixel.x / size.x * 2.0 - 1.0, 1.0 - pixel.y / size.y * 2.0)
}

/// Exact perspective-correct barycentrics (weights of vertices 0, 1, 2; they
/// sum to 1) of the pixel at `ndc` in the triangle with clip-space vertices
/// `c`. Works for triangles straddling the near plane: it solves
/// `sum(b_i * (c_i.xy - ndc * c_i.w)) = 0`, so the homogeneous weights are the
/// perspective-correct ones without any per-vertex divide.
pub fn bary_from_clip(c: [Vec4; 3], ndc: Vec2) -> [f32; 3] {
    let e = [
        c[0].truncate().truncate() - ndc * c[0].w,
        c[1].truncate().truncate() - ndc * c[1].w,
        c[2].truncate().truncate() - ndc * c[2].w,
    ];
    let b = [
        e[1].x * e[2].y - e[2].x * e[1].y,
        e[2].x * e[0].y - e[0].x * e[2].y,
        e[0].x * e[1].y - e[1].x * e[0].y,
    ];
    let s = b[0] + b[1] + b[2];
    if s.abs() < 1e-30 {
        return [1.0, 0.0, 0.0];
    }
    [b[0] / s, b[1] / s, b[2] / s]
}

/// Clip-space position under the engine's `proj * (view * world)` order.
pub fn clip(view: Mat4, proj: Mat4, world: Vec3) -> Vec4 {
    proj * (view * world.extend(1.0))
}

/// Depth (reversed-Z) at barycentrics `b` of clip-space vertices `c`.
pub fn depth_at(c: [Vec4; 3], b: [f32; 3]) -> f32 {
    let p = c[0] * b[0] + c[1] * b[1] + c[2] * b[2];
    p.z / p.w
}

/// Barycentrics of the point where the ray `origin + t * dir` meets the plane
/// of the world-space triangle `v`, by area ratios. Independent of the
/// projection, so it checks [`bary_from_clip`] against geometry rather than
/// against itself. `None` if the ray is parallel to the plane.
pub fn bary_from_ray(v: [Vec3; 3], origin: Vec3, dir: Vec3) -> Option<[f32; 3]> {
    let n = (v[1] - v[0]).cross(v[2] - v[0]);
    let denom = n.dot(dir);
    if denom.abs() < 1e-12 {
        return None;
    }
    let t = n.dot(v[0] - origin) / denom;
    let p = origin + dir * t;
    let area = n.length_squared();
    let b1 = n.dot((p - v[0]).cross(v[2] - v[0])) / area;
    let b2 = n.dot((v[1] - v[0]).cross(p - v[0])) / area;
    Some([1.0 - b1 - b2, b1, b2])
}

/// The unified payload word `G` for barycentrics `b` (vertex 0 implied).
pub fn payload_bary(b: [f32; 3]) -> u32 {
    helio_vg_format::bary_pack(b[1], b[2])
}

/// Barycentrics recovered from a payload word `G`.
pub fn payload_unbary(word: u32) -> [f32; 3] {
    let (u, v) = helio_vg_format::bary_unpack(word);
    [1.0 - u - v, u, v]
}
