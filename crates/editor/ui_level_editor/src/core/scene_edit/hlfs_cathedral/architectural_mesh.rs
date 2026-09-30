//! Shared procedural architecture primitives, batched by material.
use super::demo_data::box_mesh;
use glam::Vec3;
use helio::PackedVertex;

#[derive(Default)]
pub(crate) struct Mesh {
    pub(crate) vertices: Vec<PackedVertex>,
    pub(crate) indices: Vec<u32>,
}
impl Mesh {
    /// Dominant-plane mapping in metres for flat architectural stone faces.
    /// Rebuild the tangent basis to agree with the projected UV axes.
    pub(crate) fn world_space_uv(&mut self, tile_metres: f32) {
        assert!(tile_metres > 0.0 && tile_metres.is_finite());
        for vertex in &mut self.vertices {
            let decode = |shift| ((vertex.normal >> shift) as u8 as i8) as f32 / 127.0;
            let n = Vec3::new(decode(0), decode(8), decode(16)).normalize();
            let a = n.abs();
            let (u, v) = if a.y >= a.x && a.y >= a.z {
                (Vec3::X, -Vec3::Z)
            } else if a.x >= a.z {
                (Vec3::Z, Vec3::Y)
            } else {
                (Vec3::X, Vec3::Y)
            };
            let p = Vec3::from_array(vertex.position);
            let axis = v.cross(n).normalize();
            let tangent = axis * axis.dot(u).signum();
            let sign = n.cross(tangent).dot(v).signum();
            let packed = PackedVertex::from_components(
                vertex.position,
                n.to_array(),
                [p.dot(u) / tile_metres, p.dot(v) / tile_metres],
                tangent.to_array(),
                sign,
            );
            vertex.tex_coords0 = packed.tex_coords0;
            vertex.tangent = packed.tangent;
            vertex.bitangent_sign = packed.bitangent_sign;
        }
    }
    pub(crate) fn triangle(&mut self, a: Vec3, b: Vec3, c: Vec3) {
        let normal = (b - a).cross(c - a).normalize();
        let tangent = (b - a).normalize();
        let base = self.vertices.len() as u32;
        for (p, uv) in [(a, [0., 0.]), (b, [1., 0.]), (c, [0., 1.])] {
            self.vertices.push(PackedVertex::from_components(
                p.to_array(),
                normal.to_array(),
                uv,
                tangent.to_array(),
                1.,
            ));
        }
        self.indices.extend_from_slice(&[base, base + 1, base + 2]);
    }
    pub(crate) fn quad(&mut self, a: Vec3, b: Vec3, c: Vec3, d: Vec3) {
        // Both triangles share one UV rectangle. Calling triangle twice maps
        // the whole texture onto each half and disagrees along the diagonal.
        // Preserve per-triangle face normals for non-planar architectural quads.
        for (points, uvs, tangent) in [
            (
                [a, b, c],
                [[0., 0.], [1., 0.], [1., 1.]],
                (b - a).normalize(),
            ),
            (
                [a, c, d],
                [[0., 0.], [1., 1.], [0., 1.]],
                (c - d).normalize(),
            ),
        ] {
            let normal = (points[1] - points[0])
                .cross(points[2] - points[0])
                .normalize();
            let base = self.vertices.len() as u32;
            for (point, uv) in points.into_iter().zip(uvs) {
                self.vertices.push(PackedVertex::from_components(
                    point.to_array(),
                    normal.to_array(),
                    uv,
                    tangent.to_array(),
                    1.0,
                ));
            }
            self.indices.extend_from_slice(&[base, base + 1, base + 2]);
        }
    }
    pub(crate) fn block(&mut self, center: [f32; 3], half: [f32; 3]) {
        let mesh = box_mesh(center, half);
        let base = self.vertices.len() as u32;
        self.vertices.extend(mesh.vertices);
        self.indices
            .extend(mesh.indices.into_iter().map(|i| i + base));
    }
    pub(crate) fn rod(&mut self, a: Vec3, b: Vec3, radius: f32, sides: usize) {
        let axis = (b - a).normalize();
        let helper = if axis.y.abs() < 0.9 { Vec3::Y } else { Vec3::X };
        let u = axis.cross(helper).normalize() * radius;
        let v = axis.cross(u).normalize() * radius;
        for i in 0..sides {
            let t = i as f32 * std::f32::consts::TAU / sides as f32;
            let t1 = (i + 1) as f32 * std::f32::consts::TAU / sides as f32;
            let p = u * t.cos() + v * t.sin();
            let q = u * t1.cos() + v * t1.sin();
            self.quad(a + p, a + q, b + q, b + p);
            self.triangle(a, a + q, a + p);
            self.triangle(b, b + p, b + q);
        }
    }
    /// Smooth cylindrical sides with a duplicated UV seam and flat end caps.
    pub(crate) fn smooth_rod(&mut self, a: Vec3, b: Vec3, radius: f32, sides: usize) {
        assert!(sides >= 3 && radius > 0.0 && a.distance_squared(b) > 0.0);
        let axis = (b - a).normalize();
        let helper = if axis.y.abs() < 0.9 { Vec3::Y } else { Vec3::X };
        let u = axis.cross(helper).normalize();
        let v = axis.cross(u).normalize();
        let base = self.vertices.len() as u32;
        for i in 0..=sides {
            let uv_x = i as f32 / sides as f32;
            // Make the seam geometrically identical instead of relying on sin(TAU).
            let angle = (i % sides) as f32 * std::f32::consts::TAU / sides as f32;
            let radial = u * angle.cos() + v * angle.sin();
            let tangent = -u * angle.sin() + v * angle.cos();
            for (center, uv_y) in [(a, 0.0), (b, 1.0)] {
                self.vertices.push(PackedVertex::from_components(
                    (center + radial * radius).to_array(),
                    radial.to_array(),
                    [uv_x, uv_y],
                    tangent.to_array(),
                    1.0,
                ));
            }
        }
        for i in 0..sides {
            let first = base + (2 * i) as u32;
            self.indices.extend_from_slice(&[
                first,
                first + 2,
                first + 3,
                first,
                first + 3,
                first + 1,
            ]);
            let angle = i as f32 * std::f32::consts::TAU / sides as f32;
            let next = ((i + 1) % sides) as f32 * std::f32::consts::TAU / sides as f32;
            let p = (u * angle.cos() + v * angle.sin()) * radius;
            let q = (u * next.cos() + v * next.sin()) * radius;
            for (center, normal, offsets, sign) in [
                (a, -axis, [Vec3::ZERO, q, p], -1.0),
                (b, axis, [Vec3::ZERO, p, q], 1.0),
            ] {
                let cap_base = self.vertices.len() as u32;
                for offset in offsets {
                    self.vertices.push(PackedVertex::from_components(
                        (center + offset).to_array(),
                        normal.to_array(),
                        [
                            0.5 + 0.5 * offset.dot(u) / radius,
                            0.5 + sign * 0.5 * offset.dot(v) / radius,
                        ],
                        u.to_array(),
                        1.0,
                    ));
                }
                self.indices
                    .extend_from_slice(&[cap_base, cap_base + 1, cap_base + 2]);
            }
        }
    }
    pub(crate) fn ring(&mut self, center: Vec3, u: Vec3, v: Vec3, radius: f32, thickness: f32) {
        for i in 0..48 {
            let t = i as f32 * std::f32::consts::TAU / 48.;
            let t1 = (i + 1) as f32 * std::f32::consts::TAU / 48.;
            self.rod(
                center + (u * t.cos() + v * t.sin()) * radius,
                center + (u * t1.cos() + v * t1.sin()) * radius,
                thickness,
                8,
            );
        }
    }
    pub(crate) fn arch(&mut self, a: Vec3, b: Vec3, rise: f32, radius: f32) {
        // Two curved halves meet at a pointed crown.
        let mid = (a + b) * 0.5 + Vec3::Y * rise;
        for (start, end) in [(a, mid), (b, mid)] {
            let mut previous = start;
            for i in 1..=24 {
                let t = i as f32 / 24.;
                let mut p = start.lerp(end, t);
                p.y += rise * 0.24 * (t * std::f32::consts::PI).sin();
                self.rod(previous, p, radius, 10);
                previous = p;
            }
        }
    }

    /// One connected tube along a planar pointed arch. Adjacent rings share
    /// vertices; there are no overlapping cylinder end caps at every segment.
    pub(crate) fn smooth_arch(&mut self, a: Vec3, b: Vec3, rise: f32, radius: f32) {
        assert!(radius.is_finite() && radius > 0.0 && rise.is_finite() && rise > 0.0);
        assert!((b - a).cross(Vec3::Y).length_squared() > 0.0);
        let mid = (a + b) * 0.5 + Vec3::Y * rise;
        let curve = |start: Vec3, t: f32| {
            let mut p = start.lerp(mid, t);
            p.y += rise * 0.24 * (t * std::f32::consts::PI).sin();
            p
        };
        let mut points: Vec<Vec3> = (0..=24).map(|i| curve(a, i as f32 / 24.)).collect();
        points[24] = mid;
        points.extend((0..24).rev().map(|i| curve(b, i as f32 / 24.)));
        let plane_normal = (b - a).cross(Vec3::Y).normalize();
        let sides = 16usize;
        let base = self.vertices.len() as u32;
        let mut distance = 0.0;
        for (i, &point) in points.iter().enumerate() {
            if i > 0 {
                distance += point.distance(points[i - 1]);
            }
            let direction =
                (points[(i + 1).min(points.len() - 1)] - points[i.saturating_sub(1)]).normalize();
            let u = plane_normal;
            let v = direction.cross(u).normalize();
            for side in 0..=sides {
                let angle = (side % sides) as f32 * std::f32::consts::TAU / sides as f32;
                let normal = u * angle.cos() + v * angle.sin();
                let tangent = -u * angle.sin() + v * angle.cos();
                self.vertices.push(PackedVertex::from_components(
                    (point + normal * radius).to_array(),
                    normal.to_array(),
                    [side as f32 / sides as f32, distance],
                    tangent.to_array(),
                    1.0,
                ));
            }
        }
        for ring in 0..points.len() - 1 {
            for side in 0..sides {
                let p = base + (ring * (sides + 1) + side) as u32;
                let q = p + (sides + 1) as u32;
                self.indices
                    .extend_from_slice(&[p, p + 1, q + 1, p, q + 1, q]);
            }
        }
        // Flat caps only at the two springing points; vertices are separate so
        // their normals do not smooth across the end of the tube.
        for (ring, reverse) in [(0usize, true), (points.len() - 1, false)] {
            for side in 0..sides {
                let p = Vec3::from_array(
                    self.vertices[base as usize + ring * (sides + 1) + side].position,
                );
                let q = Vec3::from_array(
                    self.vertices[base as usize + ring * (sides + 1) + side + 1].position,
                );
                if reverse {
                    self.triangle(points[ring], q, p);
                } else {
                    self.triangle(points[ring], p, q);
                }
            }
        }
    }
}
