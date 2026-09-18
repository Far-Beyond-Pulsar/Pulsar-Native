//! Foliage scatter: one brush stamp → the instances every enabled foliage
//! member should place inside the brush disc.
//!
//! Pure math over [`FoliageSetLibrary`] — no terrain, scene, or GPUI access —
//! so it is deterministic and unit-testable. Ground projection (putting each
//! instance on the actual surface) and turning specs into scene content are
//! separate steps (`super::author`), which is what lets the authoring backend
//! change without touching placement rules.

use crate::level_editor::state::foliage_sets::{FoliageSetLibrary, MemberId, SetId};

/// Upper bound on instances one stamp may produce across all members.
///
/// Each instance is a ground-projection ray plus a scene entity, so an
/// unbounded density × radius product (a 64 m brush at density 500 would ask
/// for ~640 000) must not be honored literally. Members are budgeted in order,
/// so a low-numbered dense member can starve later ones — an accepted
/// trade-off until instancing makes per-stamp cost negligible.
pub const MAX_INSTANCES_PER_STAMP: usize = 256;

/// One instance to place, in the brush's local tangent plane.
#[derive(Clone, Debug, PartialEq)]
pub struct InstanceSpec {
    pub set: SetId,
    pub set_name: String,
    pub member: MemberId,
    pub mesh: String,
    /// Offset from the brush centre in the tangent plane, meters
    /// (`[along tangent, along bitangent]`).
    pub offset_m: [f32; 2],
    pub scale: f32,
    /// Yaw about the surface normal, radians.
    pub yaw_rad: f32,
    pub align_to_normal: bool,
    /// Offset along the surface normal, meters.
    pub ground_offset_m: f32,
}

/// Small deterministic RNG (splitmix64): stamps must be reproducible from a
/// seed so a redo-style replay places the same instances.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, 1)`.
    fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// Instances a stamp should place: for every paintable member (enabled member
/// of an enabled set, mesh chosen), `density / 100 m² × disc area ×
/// brush_density` instances, with the fractional part resolved
/// stochastically so low densities still paint over many stamps.
pub fn scatter(
    library: &FoliageSetLibrary,
    radius_m: f32,
    brush_density: f32,
    seed: u64,
) -> Vec<InstanceSpec> {
    let radius = radius_m.max(0.0);
    let area_m2 = std::f32::consts::PI * radius * radius;
    let mut rng = Rng(seed);
    let mut out = Vec::new();

    for (set, member) in library.paintable_members() {
        let placement = &member.placement;
        let expected = placement.density / 100.0 * area_m2 * brush_density.clamp(0.0, 1.0);
        let mut count = expected.floor() as usize;
        if rng.unit() < expected.fract() {
            count += 1;
        }

        for _ in 0..count {
            if out.len() >= MAX_INSTANCES_PER_STAMP {
                return out;
            }
            let r = radius * rng.unit().sqrt();
            let theta = std::f32::consts::TAU * rng.unit();
            let scale = placement.scale_min
                + (placement.scale_max - placement.scale_min) * rng.unit();
            let yaw_rad = if placement.random_yaw {
                std::f32::consts::TAU * rng.unit()
            } else {
                0.0
            };
            out.push(InstanceSpec {
                set: set.id,
                set_name: set.name.clone(),
                member: member.id,
                mesh: member.mesh.clone(),
                offset_m: [r * theta.cos(), r * theta.sin()],
                scale,
                yaw_rad,
                align_to_normal: placement.align_to_normal,
                ground_offset_m: placement.ground_offset_m,
            });
        }
    }
    out
}

/// An orthonormal `(tangent, bitangent)` pair spanning the plane
/// perpendicular to `normal`.
pub fn tangent_basis(normal: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    // Cross with whichever world axis is least parallel to the normal.
    let helper = if normal[1].abs() < 0.9 {
        [0.0, 1.0, 0.0]
    } else {
        [1.0, 0.0, 0.0]
    };
    let tangent = normalize(cross(helper, normal));
    let bitangent = normalize(cross(normal, tangent));
    (tangent, bitangent)
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len <= f32::EPSILON {
        return [1.0, 0.0, 0.0];
    }
    [v[0] / len, v[1] / len, v[2] / len]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library(density: f32) -> FoliageSetLibrary {
        let mut lib = FoliageSetLibrary::default();
        let set = lib.add_set();
        let member = lib.add_member(set, "meshes/tree.mesh".into()).unwrap();
        lib.member_mut(set, member).unwrap().placement.density = density;
        lib
    }

    #[test]
    fn same_seed_gives_the_same_instances() {
        let lib = library(10.0);
        assert_eq!(scatter(&lib, 8.0, 1.0, 7), scatter(&lib, 8.0, 1.0, 7));
    }

    #[test]
    fn count_follows_density_and_area() {
        // 10 per 100 m² over a 10 m radius disc (~314 m²) ≈ 31.
        let n = scatter(&library(10.0), 10.0, 1.0, 1).len();
        assert!((30..=32).contains(&n), "got {n}");
    }

    #[test]
    fn brush_density_scales_the_count() {
        let lib = library(10.0);
        let full = scatter(&lib, 10.0, 1.0, 1).len();
        let half = scatter(&lib, 10.0, 0.5, 1).len();
        assert!(half < full);
        assert_eq!(scatter(&lib, 10.0, 0.0, 1).len(), 0);
    }

    #[test]
    fn instances_stay_inside_the_brush_disc() {
        for spec in scatter(&library(50.0), 6.0, 1.0, 3) {
            let d = (spec.offset_m[0].powi(2) + spec.offset_m[1].powi(2)).sqrt();
            assert!(d <= 6.0 + 1e-4, "{d}");
        }
    }

    #[test]
    fn scale_stays_within_the_members_range() {
        let mut lib = library(50.0);
        let (set, member) = {
            let s = &lib.sets[0];
            (s.id, s.members[0].id)
        };
        {
            let p = &mut lib.member_mut(set, member).unwrap().placement;
            p.scale_min = 0.5;
            p.scale_max = 2.0;
        }
        for spec in scatter(&lib, 8.0, 1.0, 9) {
            assert!((0.5..=2.0).contains(&spec.scale), "{}", spec.scale);
        }
    }

    #[test]
    fn a_stamp_is_capped() {
        let n = scatter(&library(500.0), 64.0, 1.0, 5).len();
        assert_eq!(n, MAX_INSTANCES_PER_STAMP);
    }

    #[test]
    fn disabled_members_and_sets_place_nothing() {
        let mut lib = library(50.0);
        let set = lib.sets[0].id;
        lib.set_mut(set).unwrap().enabled = false;
        assert!(scatter(&lib, 8.0, 1.0, 1).is_empty());
    }

    #[test]
    fn a_member_without_a_mesh_places_nothing() {
        let mut lib = FoliageSetLibrary::default();
        let set = lib.add_set();
        lib.add_member(set, String::new());
        assert!(scatter(&lib, 8.0, 1.0, 1).is_empty());
    }

    #[test]
    fn every_enabled_member_contributes() {
        let mut lib = library(20.0);
        let set = lib.sets[0].id;
        let second = lib.add_member(set, "meshes/bush.mesh".into()).unwrap();
        lib.member_mut(set, second).unwrap().placement.density = 20.0;
        let specs = scatter(&lib, 6.0, 1.0, 2);
        assert!(specs.iter().any(|s| s.mesh.contains("tree")));
        assert!(specs.iter().any(|s| s.mesh.contains("bush")));
    }

    #[test]
    fn tangent_basis_is_orthonormal_to_the_normal() {
        for n in [[0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.3, 0.8, -0.5]] {
            let n = normalize(n);
            let (t, b) = tangent_basis(n);
            let dot = |a: [f32; 3], c: [f32; 3]| a[0] * c[0] + a[1] * c[1] + a[2] * c[2];
            assert!(dot(t, n).abs() < 1e-5);
            assert!(dot(b, n).abs() < 1e-5);
            assert!(dot(t, b).abs() < 1e-5);
            assert!((dot(t, t) - 1.0).abs() < 1e-5);
        }
    }
}
