//! Natural features grown as stamps of shapes (`crate::stamp`): trees of
//! several species, bushes, desert shrubs, cacti, boulders and impact
//! craters, for a world's feature set (`crate::features`). Trunks and
//! branches are tapered capsules, crowns sparse leaf balls with broken edges
//! (light and sky show through), conifers stacked cones, craters an air bowl
//! inside a lumpy rim.
//!
//! Sizes are in metres and every generator takes the world's voxel size: a
//! stamp's offsets are cells, its anchor the first layer over the ground
//! with the trunk on the anchor column's centre. Materials are the
//! caller's (built-in ids or a world's own); a stamp is a pure function of
//! its seed, on any platform.

mod rng;

pub use rng::Rng;

use crate::stamp::{Part, Shape, Stamp};
use glam::DVec3;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Species {
    #[default]
    Oak,
    DarkOak,
    Birch,
    Spruce,
    Pine,
    Jungle,
}

/// Shapes in metres from the anchor column's centre at ground level, into
/// a stamp in cells of `voxel` metres.
struct Builder {
    stamp: Stamp,
    voxel: f64,
}

impl Builder {
    fn new(voxel: f64) -> Self {
        Self {
            stamp: Stamp::new(),
            voxel,
        }
    }

    /// A point (m) as the stamp's cells: the anchor column's centre is
    /// `(0.5, 0, 0.5)` cells, the ground's top at `y = 0`.
    fn at(&self, p: DVec3) -> [f32; 3] {
        [
            (p.x / self.voxel + 0.5) as f32,
            (p.y / self.voxel) as f32,
            (p.z / self.voxel + 0.5) as f32,
        ]
    }

    fn cells(&self, m: f64) -> f32 {
        (m / self.voxel) as f32
    }

    /// A limb from `a` to `b` (m), its radius tapering from `ra` to `rb`.
    fn limb(&mut self, a: DVec3, b: DVec3, ra: f64, rb: f64, material: u32) {
        let (a, b, r) = (self.at(a), self.at(b), [self.cells(ra), self.cells(rb)]);
        self.stamp.capsule(a, b, r, material);
    }

    /// Foliage: a cone or ball (a limb of leaves) a share of whose cells
    /// is left out, its edge broken by `rough` metres and its outline into
    /// lumps up to `lump` metres deep, 40 cm across.
    fn foliage(
        &mut self,
        a: DVec3,
        b: DVec3,
        ra: f64,
        rb: f64,
        material: u32,
        sparse: f32,
        rough: f64,
        lump: f64,
    ) {
        let shape = Shape::Capsule {
            a: self.at(a),
            b: self.at(b),
            radius: [self.cells(ra), self.cells(rb)],
        };
        let part = Part {
            sparse,
            rough: self.cells(rough),
            lump: self.cells(lump),
            lump_size: self.cells(0.4).round().max(1.0) as u32,
            ..Part::solid(shape, material)
        };
        self.stamp.add(part);
    }

    /// A flat-ended cone of foliage: a conifer's tier.
    fn tier(
        &mut self,
        a: DVec3,
        b: DVec3,
        ra: f64,
        rb: f64,
        material: u32,
        sparse: f32,
        rough: f64,
    ) {
        let shape = Shape::Cone {
            a: self.at(a),
            b: self.at(b),
            radius: [self.cells(ra), self.cells(rb)],
        };
        let part = Part {
            sparse,
            rough: self.cells(rough),
            ..Part::solid(shape, material)
        };
        self.stamp.add(part);
    }

    /// A leaf mass: a lumpy ball of leaves.
    fn crown(&mut self, c: DVec3, r: f64, material: u32, sparse: f32) {
        self.foliage(c, c, r, r, material, sparse, r * 0.2, r * 0.4);
    }
}

/// A unit vector `azimuth` round the vertical and `elevation` above the
/// horizontal (radians).
fn direction(azimuth: f64, elevation: f64) -> DVec3 {
    DVec3::new(
        azimuth.cos() * elevation.cos(),
        elevation.sin(),
        azimuth.sin() * elevation.cos(),
    )
}

/// A trunk `height` metres to its fork, of `radius` at the ground, gently
/// curved, with roots flaring into the ground: the fork's point.
fn trunk(b: &mut Builder, rng: &mut Rng, height: f64, radius: f64, wood: u32) -> DVec3 {
    let lean = direction(rng.uniform(0.0, std::f64::consts::TAU), 0.0);
    let mid = DVec3::Y * height * 0.5 + lean * rng.uniform(0.0, 0.25);
    let fork = DVec3::Y * height + lean * rng.uniform(0.1, 0.5);
    b.limb(DVec3::Y * -0.5, mid, radius, radius * 0.85, wood);
    b.limb(mid, fork, radius * 0.85, radius * 0.72, wood);
    for n in 0..rng.int(3, 5) {
        let out = direction(f64::from(n) * 1.7 + rng.uniform(0.0, 0.6), 0.0);
        b.limb(
            DVec3::Y * radius * 1.4,
            out * radius * rng.uniform(2.2, 3.2) - DVec3::Y * 0.25,
            radius * 0.55,
            radius * 0.2,
            wood,
        );
    }
    fork
}

/// How a species branches: its wood and leaves, its leaf masses' share
/// left out and radius range (m), how far a child limb turns from its
/// parent (radians) and how much it is drawn upward.
struct Limbs {
    wood: u32,
    leaves: u32,
    sparse: f32,
    cluster: [f64; 2],
    spread: f64,
    rise: f64,
}

impl Limbs {
    /// A limb from `from` along `dir` of `length` and base `radius`, then
    /// two or three children to `depth` more levels, each shorter and
    /// thinner; a leaf mass at every twig's end.
    fn grow(
        &self,
        b: &mut Builder,
        rng: &mut Rng,
        from: DVec3,
        dir: DVec3,
        length: f64,
        radius: f64,
        depth: u32,
    ) {
        let end = from + dir * length;
        b.limb(from, end, radius, radius * 0.7, self.wood);
        if depth == 0 {
            b.crown(
                end,
                rng.uniform(self.cluster[0], self.cluster[1]),
                self.leaves,
                self.sparse,
            );
            return;
        }
        let side = dir.any_orthonormal_vector();
        for n in 0..rng.int(2, 3) {
            // Turned about the limb, then pulled outward and up.
            let around =
                glam::DQuat::from_axis_angle(dir, f64::from(n) * 2.4 + rng.uniform(0.0, 1.0));
            let tilt =
                glam::DQuat::from_axis_angle(around * side, self.spread * rng.uniform(0.6, 1.2));
            let outward = DVec3::new(end.x, 0.0, end.z).normalize_or_zero();
            let child = (tilt * dir + outward * 0.25 + DVec3::Y * self.rise * 0.5).normalize();
            let next = length * rng.uniform(0.7, 0.85);
            self.grow(b, rng, end, child, next, radius * 0.62, depth - 1);
        }
    }

    /// `count` leaf masses filling the crown round `centre`: a flattened
    /// ellipsoid `[radius, half height]` (m), closing the gaps between the
    /// twigs' masses.
    fn fill(&self, b: &mut Builder, rng: &mut Rng, centre: DVec3, extent: [f64; 2], count: u32) {
        for _ in 0..count {
            let at = centre
                + direction(rng.uniform(0.0, std::f64::consts::TAU), 0.0)
                    * extent[0]
                    * rng.uniform(0.0, 0.8)
                + DVec3::Y * extent[1] * rng.uniform(-0.6, 0.8);
            b.crown(
                at,
                rng.uniform(self.cluster[0], self.cluster[1]) * 1.25,
                self.leaves,
                self.sparse,
            );
        }
    }
}

/// The tree of `species` grown from `seed`, of `wood` and `leaves`, in
/// cells of `voxel` metres.
pub fn tree(species: Species, wood: u32, leaves: u32, seed: u64, voxel: f64) -> Stamp {
    // Seeds of nearby variants differ in a few bits: spread them.
    let mut rng =
        Rng::new(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15).rotate_left(23) ^ 0x5851_f42d_4c95_7f2d);
    let mut b = Builder::new(voxel);
    match species {
        Species::Oak | Species::DarkOak => {
            let dark = species == Species::DarkOak;
            let height = if dark {
                rng.uniform(3.0, 4.0)
            } else {
                rng.uniform(3.2, 4.6)
            };
            let radius = if dark {
                rng.uniform(0.45, 0.6)
            } else {
                rng.uniform(0.38, 0.5)
            };
            let fork = trunk(&mut b, &mut rng, height, radius, wood);
            let limbs = Limbs {
                wood,
                leaves,
                sparse: if dark { 0.18 } else { 0.24 },
                cluster: if dark { [1.5, 2.1] } else { [1.3, 1.9] },
                spread: 0.55,
                rise: 0.35,
            };
            // Three or four main limbs out of the fork, each branching on.
            let main = rng.int(3, 4);
            let turn = rng.uniform(0.0, std::f64::consts::TAU);
            for n in 0..main {
                let azimuth = turn
                    + (f64::from(n) + rng.uniform(-0.25, 0.25)) * std::f64::consts::TAU
                        / f64::from(main);
                let dir = direction(azimuth, rng.uniform(0.45, 0.8));
                let length = rng.uniform(2.4, 3.0) * if dark { 1.1 } else { 1.0 };
                limbs.grow(&mut b, &mut rng, fork, dir, length, radius * 0.62, 2);
            }
            // The crown's top and its body, filled between the limbs' ends.
            limbs.fill(&mut b, &mut rng, fork + DVec3::Y * 2.8, [3.6, 2.2], 5);
        }
        Species::Birch => {
            let height = rng.uniform(4.5, 6.0);
            let fork = trunk(&mut b, &mut rng, height, 0.17, wood);
            let limbs = Limbs {
                wood,
                leaves,
                sparse: 0.32,
                cluster: [0.8, 1.2],
                spread: 0.35,
                rise: 0.75,
            };
            let leader = direction(rng.uniform(0.0, std::f64::consts::TAU), 1.35);
            let length = rng.uniform(2.4, 3.2);
            limbs.grow(&mut b, &mut rng, fork, leader, length, 0.12, 2);
            for n in 0..rng.int(2, 3) {
                let from = fork + DVec3::Y * rng.uniform(-1.2, 0.4);
                let dir = direction(
                    f64::from(n) * 2.3 + rng.uniform(0.0, 0.8),
                    rng.uniform(0.6, 0.9),
                );
                let length = rng.uniform(1.6, 2.2);
                limbs.grow(&mut b, &mut rng, from, dir, length, 0.08, 2);
            }
            limbs.fill(&mut b, &mut rng, fork + DVec3::Y * 2.6, [1.8, 2.4], 3);
        }
        Species::Spruce | Species::Pine => {
            let pine = species == Species::Pine;
            let height = if pine {
                rng.uniform(12.0, 18.0)
            } else {
                rng.uniform(9.0, 14.0)
            };
            let top = DVec3::Y * height;
            b.limb(
                DVec3::Y * -0.4,
                top,
                if pine { 0.3 } else { 0.26 },
                0.04,
                wood,
            );
            if pine {
                // A bare trunk under a crown of flat clusters.
                for _ in 0..rng.int(4, 6) {
                    let c = DVec3::Y * height * rng.uniform(0.72, 0.95)
                        + direction(rng.uniform(0.0, std::f64::consts::TAU), 0.0)
                            * rng.uniform(0.5, 1.6);
                    b.limb(DVec3::Y * c.y * 0.92, c, 0.08, 0.04, wood);
                    b.tier(
                        c - DVec3::Y * 0.3,
                        c + DVec3::Y * 0.5,
                        rng.uniform(1.2, 1.7),
                        0.6,
                        leaves,
                        0.25,
                        0.3,
                    );
                }
            } else {
                // Stacked cones, wide at the bottom.
                let tiers = rng.int(4, 6);
                let base = height * 0.18;
                for n in 0..tiers {
                    let t = f64::from(n) / f64::from(tiers);
                    let y0 = base + (height - base) * t * 0.92;
                    let reach = (2.4 - 1.9 * t) * height / 11.0;
                    b.tier(
                        DVec3::Y * y0,
                        DVec3::Y * (y0 + (height - base) / f64::from(tiers) * 1.6),
                        reach,
                        0.15,
                        leaves,
                        0.22,
                        0.3,
                    );
                }
            }
        }
        Species::Jungle => {
            let height = rng.uniform(15.0, 19.0);
            let radius = rng.uniform(0.5, 0.65);
            let top = DVec3::Y * height
                + direction(rng.uniform(0.0, std::f64::consts::TAU), 0.0) * rng.uniform(0.0, 0.8);
            b.limb(DVec3::Y * -0.5, top, radius, radius * 0.5, wood);
            // Buttress roots.
            for n in 0..rng.int(4, 6) {
                let out = direction(f64::from(n) * 1.3 + rng.uniform(0.0, 0.5), 0.0);
                b.limb(
                    DVec3::Y * rng.uniform(1.2, 2.2) + out * radius * 0.5,
                    out * rng.uniform(1.3, 2.0) - DVec3::Y * 0.3,
                    radius * 0.45,
                    0.12,
                    wood,
                );
            }
            // Spreading limbs and a broad, layered canopy.
            let limbs = Limbs {
                wood,
                leaves,
                sparse: 0.22,
                cluster: [1.6, 2.4],
                spread: 0.6,
                rise: 0.25,
            };
            for n in 0..rng.int(4, 5) {
                let from = top - DVec3::Y * rng.uniform(1.0, 3.5);
                let dir = direction(
                    f64::from(n) * 1.4 + rng.uniform(0.0, 0.6),
                    rng.uniform(0.25, 0.5),
                );
                let length = rng.uniform(2.8, 3.6);
                limbs.grow(&mut b, &mut rng, from, dir, length, radius * 0.4, 2);
            }
            limbs.fill(&mut b, &mut rng, top + DVec3::Y * 0.8, [5.0, 2.0], 6);
            // Vines' worth of leaves hanging down the trunk.
            for _ in 0..3 {
                let at = DVec3::Y * height * rng.uniform(0.3, 0.8)
                    + direction(rng.uniform(0.0, std::f64::consts::TAU), 0.0) * radius;
                let down = at - DVec3::Y * rng.uniform(1.5, 3.0);
                b.foliage(at, down, 0.35, 0.2, leaves, 0.45, 0.15, 0.0);
            }
        }
    }
    b.stamp
}

/// A cactus of `material` grown from `seed`: a column and up to two arms.
pub fn cactus(material: u32, seed: u64, voxel: f64) -> Stamp {
    let mut rng = Rng::new(seed);
    let mut b = Builder::new(voxel);
    let height = rng.uniform(1.4, 3.2);
    b.limb(DVec3::Y * -0.3, DVec3::Y * height, 0.2, 0.17, material);
    for n in 0..rng.int(0, 2) {
        let out = direction(
            f64::from(n) * std::f64::consts::PI + rng.uniform(-0.4, 0.4),
            0.0,
        );
        let from = DVec3::Y * height * rng.uniform(0.4, 0.7);
        let elbow = from + out * rng.uniform(0.4, 0.6);
        b.limb(from, elbow, 0.13, 0.13, material);
        b.limb(
            elbow,
            elbow + DVec3::Y * rng.uniform(0.5, 1.0),
            0.13,
            0.11,
            material,
        );
    }
    b.stamp
}

/// A bush of `leaves` grown from `seed`: a few low clusters.
pub fn bush(leaves: u32, seed: u64, voxel: f64) -> Stamp {
    let mut rng = Rng::new(seed);
    let mut b = Builder::new(voxel);
    for _ in 0..rng.int(2, 4) {
        let c = DVec3::new(
            rng.uniform(-0.5, 0.5),
            rng.uniform(0.2, 0.5),
            rng.uniform(-0.5, 0.5),
        );
        b.crown(c, rng.uniform(0.45, 0.8), leaves, 0.3);
    }
    b.stamp
}

/// A dry-land shrub of `wood` and `leaves` grown from `seed`: thin stems
/// fanning low out of one root, wider than tall, airy flat tufts of leaves
/// along them down to the ground (sagebrush, saltbush), knee to waist high.
pub fn shrub(wood: u32, leaves: u32, seed: u64, voxel: f64) -> Stamp {
    let mut rng = Rng::new(seed ^ 0x5bd1_e995);
    let mut b = Builder::new(voxel);
    let height = rng.uniform(0.4, 1.0);
    let width = height * rng.uniform(1.4, 2.2);
    let turn = rng.uniform(0.0, std::f64::consts::TAU);
    let stems = rng.int(6, 9);
    for n in 0..stems {
        let azimuth =
            turn + f64::from(n) * std::f64::consts::TAU / f64::from(stems) + rng.uniform(-0.3, 0.3);
        let out = direction(azimuth, 0.0);
        let tip = out * width * rng.uniform(0.3, 0.5) + DVec3::Y * height * rng.uniform(0.55, 0.9);
        b.limb(DVec3::Y * -0.1, tip, 0.03, 0.012, wood);
        let r = width * rng.uniform(0.14, 0.2);
        b.foliage(tip, tip * 0.55, r, r * 0.8, leaves, 0.55, r * 0.25, r * 0.3);
    }
    b.stamp
}

/// A boulder of `material` grown from `seed`: rough lumps half sunk, 0.5
/// to 1.3 m.
pub fn boulder(material: u32, seed: u64, voxel: f64) -> Stamp {
    boulder_sized(material, seed, voxel, [0.5, 1.3])
}

/// A boulder of `material` grown from `seed`, `size` metres (a range): one
/// to three rough lumps, half sunk.
pub fn boulder_sized(material: u32, seed: u64, voxel: f64, size: [f64; 2]) -> Stamp {
    let mut rng = Rng::new(seed);
    let mut b = Builder::new(voxel);
    let size = rng.uniform(size[0], size[1]);
    for _ in 0..rng.int(1, 3) {
        let c = DVec3::new(
            rng.uniform(-0.4, 0.4),
            rng.uniform(-0.3, 0.1) * size,
            rng.uniform(-0.4, 0.4),
        ) * size;
        let r = size * rng.uniform(0.6, 1.0);
        b.foliage(c, c, r, r, material, 0.0, r * 0.1, r * 0.3);
    }
    b.stamp
}

/// A simple impact crater `diameter` metres across (a range) grown from
/// `seed`: a bowl about a fifth as deep as it is wide, cut into the ground
/// inside a raised, lumpy rim of `rim` (the ground's own material, or fresh
/// ejecta), `blocks` lumps of `rocks` thrown out over it.
pub fn crater(
    rim: u32,
    rocks: u32,
    blocks: u32,
    seed: u64,
    voxel: f64,
    diameter: [f64; 2],
) -> Stamp {
    let mut rng = Rng::new(seed ^ 0x2f69_3a1d);
    let mut b = Builder::new(voxel);
    let radius = rng.uniform(diameter[0], diameter[1]) * 0.5;
    let depth = radius * rng.uniform(0.34, 0.44);
    // The rim: a ring of lumpy balls round the bowl's edge, its crest a
    // tenth of the radius over the ground, sloping out over half the radius.
    let crest = radius * rng.uniform(0.08, 0.12);
    let ring = (radius * 0.5).max(voxel * 2.0);
    let segments = 16;
    let turn = rng.uniform(0.0, std::f64::consts::TAU);
    for n in 0..segments {
        let a = turn + f64::from(n) * std::f64::consts::TAU / f64::from(segments);
        let c = direction(a, 0.0) * radius * rng.uniform(1.0, 1.08) + DVec3::Y * (crest - ring);
        b.foliage(c, c, ring, ring, rim, 0.0, ring * 0.08, ring * 0.25);
    }
    // The bowl: air in the sphere through the ground's line at `radius` and
    // `depth` under the ground, up to just over the crest, as tiers (a
    // whole sphere would reach far over the ground: the stamp's box is
    // what classification bounds it by).
    let sphere = (radius * radius + depth * depth) / (2.0 * depth);
    let profile = |y: f64| {
        (sphere * sphere - (y - (sphere - depth)).powi(2))
            .max(0.0)
            .sqrt()
    };
    let heights = [
        -depth,
        -0.8 * depth,
        -0.55 * depth,
        -0.3 * depth,
        0.0,
        crest + voxel,
    ];
    for pair in heights.windows(2) {
        let (y0, y1) = (pair[0], pair[1]);
        let shape = Shape::Cone {
            a: b.at(DVec3::Y * y0),
            b: b.at(DVec3::Y * y1),
            radius: [b.cells(profile(y0)), b.cells(profile(y1))],
        };
        b.stamp.add(Part {
            rough: b.cells(radius * 0.02),
            ..Part::air(shape)
        });
    }
    // Blocks thrown out over the rim.
    for _ in 0..blocks {
        let a = rng.uniform(0.0, std::f64::consts::TAU);
        let r = radius * rng.uniform(0.9, 1.5);
        let s = radius * rng.uniform(0.03, 0.07);
        let c = direction(a, 0.0) * r + DVec3::Y * (crest * (1.5 - r / radius).max(0.0) - s * 0.3);
        b.foliage(c, c, s, s, rocks, 0.0, s * 0.1, s * 0.3);
    }
    b.stamp
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::material;

    const VOXEL: f64 = 0.1;

    /// Solid cells' box of a stamp: lowest and highest offsets, in metres.
    fn extent(s: &Stamp) -> (DVec3, DVec3) {
        let cells: Vec<[i32; 3]> = s
            .cells()
            .into_iter()
            .filter(|(_, m)| m.is_some())
            .map(|(p, _)| p)
            .collect();
        let lo = cells.iter().fold([i32::MAX; 3], |a, p| {
            [a[0].min(p[0]), a[1].min(p[1]), a[2].min(p[2])]
        });
        let hi = cells.iter().fold([i32::MIN; 3], |a, p| {
            [a[0].max(p[0]), a[1].max(p[1]), a[2].max(p[2])]
        });
        let m = |c: [i32; 3]| DVec3::new(f64::from(c[0]), f64::from(c[1]), f64::from(c[2])) * VOXEL;
        (m(lo), m(hi))
    }

    /// Every species grows a tree of its size, its trunk standing on the
    /// anchor column, within a stamp's reach; a seed grows the same tree.
    #[test]
    fn every_species_grows_a_plausible_tree() {
        let (wood, leaves) = (material::WOOD, material::LEAVES);
        for (species, height) in [
            (Species::Oak, 7.0..13.0),
            (Species::DarkOak, 6.0..12.0),
            (Species::Birch, 8.0..16.0),
            (Species::Spruce, 9.0..18.0),
            (Species::Pine, 12.0..21.0),
            (Species::Jungle, 15.0..25.0),
        ] {
            for seed in 0..3 {
                let s = tree(species, wood, leaves, seed, VOXEL);
                let (lo, hi) = extent(&s);
                assert!(height.contains(&hi.y), "{species:?} {seed}: {}", hi.y);
                assert!(
                    hi.x - lo.x > 1.0 && hi.x - lo.x < 24.0,
                    "{species:?} {seed}: width {}",
                    hi.x - lo.x
                );
                assert_eq!(
                    s.get([0, 1, 0]),
                    Some(Some(wood)),
                    "{species:?} {seed}: trunk"
                );
                s.compile().expect("within a stamp's reach");
                assert_eq!(s, tree(species, wood, leaves, seed, VOXEL));
            }
            assert_ne!(
                tree(species, wood, leaves, 1, VOXEL),
                tree(species, wood, leaves, 2, VOXEL)
            );
        }
    }

    #[test]
    fn small_growth_stands_on_its_anchor() {
        for seed in 0..4 {
            assert_eq!(
                cactus(material::LEAVES, seed, VOXEL).get([0, 2, 0]),
                Some(Some(material::LEAVES))
            );
            assert!(!bush(material::LEAVES, seed, VOXEL).cells().is_empty());
            assert!(boulder(material::STONE, seed, VOXEL)
                .get([0, -1, 0])
                .is_some());
            let s = shrub(material::WOOD, material::LEAVES, seed, VOXEL);
            let (lo, hi) = extent(&s);
            assert!((0.3..1.4).contains(&hi.y), "shrub {seed}: {}", hi.y);
            assert!(hi.x - lo.x > hi.y, "shrub {seed}: wider than tall");
        }
    }

    /// A crater is a bowl of air under the ground inside a rim over it,
    /// within a stamp's reach at its largest.
    #[test]
    fn craters_are_bowls_inside_rims() {
        for seed in 0..4 {
            let s = crater(
                material::REGOLITH,
                material::BOULDER,
                8,
                seed,
                VOXEL,
                [36.0, 36.0],
            );
            s.compile().expect("within a stamp's reach");
            // Air at the bowl's middle, well under the ground.
            assert_eq!(s.get([0, -40, 0]), Some(None), "{seed}: bowl");
            assert_eq!(s.get([0, 10, 0]), Some(None), "{seed}: open over the bowl");
            // Rim over the ground somewhere on the circle of the radius.
            let rim = (0..64).any(|n| {
                let a = f64::from(n) / 64.0 * std::f64::consts::TAU;
                (180..240).step_by(4).any(|r| {
                    (1..15).any(|y| {
                        s.get([
                            (a.cos() * f64::from(r)) as i32,
                            y,
                            (a.sin() * f64::from(r)) as i32,
                        ]) == Some(Some(material::REGOLITH))
                    })
                })
            });
            assert!(rim, "{seed}: rim");
            let (lo, hi) = s.bounds().unwrap();
            // Its box ends a little over the rim (no air sphere over it).
            assert!(hi[1] < 30 && lo[1] > -180, "{seed}: box {lo:?} {hi:?}");
        }
    }
}
