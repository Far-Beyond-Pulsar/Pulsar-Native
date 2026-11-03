//! The terrain's volumetric shapes (`docs/volumes.md`): cave networks
//! (chambers, tunnels and entrances placed per region, carved as capsules)
//! and cliff shapes (ledges, undercuts and overhangs on steep ground, the
//! surface displaced along its normal). Engine code over any generator's
//! field: it reads the field's height and surface word. Every operation is
//! integer, mirrored bit for bit by `shaders/volumes.wgsl`.
//!
//! Coordinates are a face's cells in quarters (`Q2`): horizontal base
//! cells, vertical base layers. A column's cells share its horizontal
//! position, and networks never cross a face's edge.
use crate::grid::{Grid, DOMAIN_UNIT};
use crate::landform::noise_quantile;
use crate::noise::{hash3, noise, scale, ONE};
use bytemuck::{Pod, Zeroable};
use glam::IVec3;
use serde::{Deserialize, Serialize};

/// Cave networks: per region, a hub, chambers joined to it, links to the
/// neighbouring regions' hubs and an entrance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Caves {
    pub enabled: bool,
    /// Region size (rounded to a power of two cells, 2^10 to 2^13).
    pub spacing_m: f64,
    /// Share of regions with a network.
    pub share: f64,
    /// Depth of hubs and chambers under the reference ground.
    pub depth_min_m: f64,
    pub depth_max_m: f64,
    /// Cells deeper than this under their column's surface are never
    /// carved (the classification bound).
    pub max_depth_m: f64,
    /// Rock kept over caves: they narrow to nothing over it, except along
    /// entrances.
    pub cover_m: f64,
    /// Share of hubs that are chambers, and of each of a region's two
    /// further chambers.
    pub hub_chambers: f64,
    pub chambers: f64,
    /// Chamber horizontal radius range, and height over width.
    pub chamber_min_m: f64,
    pub chamber_max_m: f64,
    pub chamber_height: f64,
    /// Tunnel radius range, and height over width.
    pub tunnel_min_m: f64,
    pub tunnel_max_m: f64,
    pub tunnel_height: f64,
    /// Share of a vertical radius under the axis kept as floor (flat,
    /// walkable floors): 1 none.
    pub chamber_floor: f64,
    pub tunnel_floor: f64,
    /// Share of neighbouring active regions whose hubs are linked.
    pub links: f64,
    /// Share of networks with an entrance, its end this far over the
    /// reference ground.
    pub entrances: f64,
    pub entrance_lift_m: f64,
    /// Vertical meander of a tunnel's bend.
    pub bend_m: f64,
    /// Wall roughness (share of the radius) and its wavelength.
    pub roughness: f64,
    pub roughness_m: f64,
}

impl Default for Caves {
    fn default() -> Self {
        Self {
            enabled: true,
            spacing_m: 200.0,
            share: 0.55,
            depth_min_m: 25.0,
            depth_max_m: 110.0,
            max_depth_m: 320.0,
            cover_m: 6.0,
            hub_chambers: 0.5,
            chambers: 0.5,
            chamber_min_m: 8.0,
            chamber_max_m: 22.0,
            chamber_height: 0.45,
            tunnel_min_m: 1.8,
            tunnel_max_m: 3.6,
            tunnel_height: 0.8,
            chamber_floor: 0.5,
            tunnel_floor: 0.55,
            links: 0.7,
            entrances: 0.35,
            entrance_lift_m: 20.0,
            bend_m: 8.0,
            roughness: 0.25,
            roughness_m: 3.0,
        }
    }
}

impl Caves {
    /// Lava tubes: long, wide, shallow passages with flat floors, linked
    /// from hub to hub, skylights for entrances, no chambers.
    pub fn lava_tubes() -> Self {
        Self {
            spacing_m: 400.0,
            share: 0.6,
            depth_min_m: 15.0,
            depth_max_m: 40.0,
            max_depth_m: 200.0,
            cover_m: 8.0,
            hub_chambers: 0.0,
            chambers: 0.0,
            tunnel_min_m: 4.0,
            tunnel_max_m: 9.0,
            tunnel_height: 0.6,
            tunnel_floor: 0.4,
            links: 0.9,
            entrances: 0.6,
            bend_m: 6.0,
            roughness: 0.15,
            roughness_m: 5.0,
            ..Self::default()
        }
    }
}

/// Cliff shapes: ledges, undercuts and overhangs on steep ground.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Cliffs {
    pub enabled: bool,
    /// Largest displacement along the surface's normal.
    pub depth_m: f64,
    /// Ledge spacing (the strata's period).
    pub strata_m: f64,
    /// Slope (rise over run) over which shapes fade in.
    pub slope_min: f64,
    pub slope_max: f64,
    /// Share of the displacement from 3D noise (the rest from strata).
    pub noise: f64,
    /// Size and rough share of the regions with cliff shapes.
    pub region_km: f64,
    pub share: f64,
}

impl Default for Cliffs {
    fn default() -> Self {
        Self {
            enabled: true,
            depth_m: 4.0,
            strata_m: 3.0,
            slope_min: 0.9,
            slope_max: 1.8,
            noise: 0.4,
            region_km: 3.0,
            share: 0.5,
        }
    }
}

/// A terrain's volumetric shapes (`TerrainLayers::volumes`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Volumes {
    pub caves: Caves,
    pub cliffs: Cliffs,
}

impl Volumes {
    /// None: a pure heightfield.
    pub fn none() -> Self {
        Self {
            caves: Caves {
                enabled: false,
                ..Caves::default()
            },
            cliffs: Cliffs {
                enabled: false,
                ..Cliffs::default()
            },
        }
    }

    /// Errors in the settings.
    pub fn validate(&self) -> Result<(), String> {
        let c = &self.caves;
        let k = &self.cliffs;
        let values = [
            c.spacing_m,
            c.share,
            c.depth_min_m,
            c.depth_max_m,
            c.max_depth_m,
            c.cover_m,
            c.hub_chambers,
            c.chambers,
            c.chamber_min_m,
            c.chamber_max_m,
            c.chamber_height,
            c.tunnel_min_m,
            c.tunnel_max_m,
            c.tunnel_height,
            c.chamber_floor,
            c.tunnel_floor,
            c.links,
            c.entrances,
            c.entrance_lift_m,
            c.bend_m,
            c.roughness,
            c.roughness_m,
            k.depth_m,
            k.strata_m,
            k.slope_min,
            k.slope_max,
            k.noise,
            k.region_km,
            k.share,
        ];
        if values.iter().any(|v| !v.is_finite() || *v < 0.0) {
            return Err("cave and cliff settings must be finite and non-negative".into());
        }
        if c.spacing_m <= 0.0 || c.roughness_m <= 0.0 || k.region_km <= 0.0 {
            return Err("cave spacing, roughness and cliff region sizes must be positive".into());
        }
        if c.depth_min_m > c.depth_max_m
            || c.chamber_min_m > c.chamber_max_m
            || c.tunnel_min_m > c.tunnel_max_m
        {
            return Err("cave ranges must be ordered (low, high)".into());
        }
        if c.chamber_height <= 0.0 || c.tunnel_height <= 0.0 {
            return Err("cave heights must be positive".into());
        }
        Ok(())
    }
}

/// Primitives a footprint keeps (`tile_caves` slot: a count word, then
/// this many of [`PRIMITIVE_WORDS`]).
pub const CAVE_SLOT: usize = 48;
pub const PRIMITIVE_WORDS: usize = 8;
/// Words of a footprint's slot.
pub const CAVE_SLOT_WORDS: usize = PRIMITIVE_WORDS * (CAVE_SLOT + 1);
/// Most primitives one region's network has: a hub chamber, two chambers,
/// their two tunnels, two links and an entrance (tunnels of two segments).
pub const REGION_PRIMITIVES: usize = 13;
/// Sites a region's network reads the ground at: hub, chambers 1 and 2,
/// the entrance's end, the `+i` and `+j` neighbours' hubs.
pub const SITES: usize = 6;
/// Cliff shapes are resolved below this level (the material slope's exact
/// levels, `Planet::material_slope`).
pub const CLIFF_LEVELS: u32 = 4;

const SEED_CAVES: u32 = 0x5C4F_E0A1;
const SEED_ROUGH: u32 = 0x2D97_1E35;
const SEED_CLIFF: u32 = 0x7A1B_C9F3;
const SEED_CLIFF_REGION: u32 = 0x41E8_06D7;
const SEED_STRATA: u32 = 0x9B3C_5A2F;

/// Hash slots of a region (`hash3(ri, rj, face * 128 + slot, seed)`).
const H_ACTIVE: i32 = 0;
const H_DEPTH: i32 = 1;
const H_HUB_CHAMBER: i32 = 2;
const H_LINK: i32 = 3; // + axis
const H_ENTRANCE: i32 = 5;
const H_SITE: i32 = 6; // + 2 site (x, y)
const H_CHAMBER: i32 = 14; // + chamber
const H_RADIUS: i32 = 16; // + site
const H_EDGE: i32 = 22; // + 3 edge (radius, bend xy, bend z)
const H_CHAMBER_DEPTH: i32 = 40; // + chamber
const H_ENTRANCE_END: i32 = 44;

/// Primitive flags.
pub const CHAMBER: u32 = 1;
pub const ENTRANCE: u32 = 2;

/// Compiled constants (`volumes` uniform, binding 18).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct VolumeConstants {
    /// flags (1 caves, 2 cliffs), region shift R, reference level, seed.
    pub head: [i32; 4],
    /// share, hub chamber share, chamber share, link share (Q16).
    pub shares: [i32; 4],
    /// entrance share (Q16), depth low and high (Q2 layers), entrance lift (Q2).
    pub depths: [i32; 4],
    /// tunnel radius low and high, chamber radius low and high (Q2).
    pub radii: [i32; 4],
    /// tunnel and chamber shape (height Q8 | floor Q8 << 16), roughness
    /// (Q8), roughness lattice shift (Q2 units).
    pub shapes: [i32; 4],
    /// cover, max depth (Q2 layers), layer (mm), bend (Q2 layers).
    pub bounds: [i32; 4],
    /// cliff amplitude (mm), slope low and high (eighths), region shift.
    pub cliff: [i32; 4],
    /// cliff region threshold (Q16), noise shift (domain units), strata
    /// period (mm), strata wander shift.
    pub cliff_shape: [i32; 4],
    /// noise and strata weights (Q8; noise scaled by 4/3), the face's
    /// cells, 0.
    pub cliff_mix: [i32; 4],
}

/// A capsule of a network (`a == b`: a chamber): absolute Q2 face
/// coordinates, horizontal and vertical radius (Q2), the floor (Q8 of the
/// vertical radius under the axis kept solid), flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Primitive {
    pub a: [i64; 3],
    pub b: [i64; 3],
    pub r: i32,
    pub rv: i32,
    pub floor: i32,
    pub flags: u32,
}

impl Primitive {
    /// Vertical squash: the radius over the vertical radius (Q8).
    pub fn inv_v(&self) -> i32 {
        (self.r << 8) / self.rv.max(1)
    }
    /// Its reach past its axis, horizontally and vertically (Q2): its
    /// radii and the walls' roughness (conservative).
    pub fn reach(&self, roughness: i32) -> (i32, i32) {
        (
            self.r + ((self.r * roughness * 3) >> 9) + 2,
            self.rv + ((self.rv * roughness * 3) >> 9) + (self.rv >> 5) + 2,
        )
    }
    /// Whether its horizontal box (reach included) holds `(x, y)`.
    pub fn covers(&self, x: i64, y: i64, roughness: i32) -> bool {
        let (h, _) = self.reach(roughness);
        let h = i64::from(h);
        x >= self.a[0].min(self.b[0]) - h
            && x <= self.a[0].max(self.b[0]) + h
            && y >= self.a[1].min(self.b[1]) - h
            && y <= self.a[1].max(self.b[1]) + h
    }
    /// Lowest and highest Q2 layer it may carve.
    pub fn z_range(&self, roughness: i32) -> (i64, i64) {
        let (_, v) = self.reach(roughness);
        let v = i64::from(v);
        (self.a[2].min(self.b[2]) - v, self.a[2].max(self.b[2]) + v)
    }
    /// The GPU's words (`tile_caves`), relative to Q2 horizontal origin
    /// `(ox, oy)`.
    pub fn words(&self, ox: i64, oy: i64) -> [i32; PRIMITIVE_WORDS] {
        [
            (self.a[0] - ox) as i32,
            (self.a[1] - oy) as i32,
            self.a[2] as i32,
            (self.b[0] - ox) as i32,
            (self.b[1] - oy) as i32,
            self.b[2] as i32,
            self.r | (self.rv << 16),
            self.inv_v() | (self.floor << 16) | ((self.flags as i32) << 28),
        ]
    }
}

/// Where a region's network reads the ground.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Site {
    /// Base cell of the face.
    pub x: i32,
    pub y: i32,
    /// Whether the network reads it.
    pub used: bool,
}

/// A column's cliff shape at a level (`VolumeLayer::cliff`): its rise
/// (mm, the band's half height), its band in level cells, whether the
/// noise and the strata are resolved, the strata's phase.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cliff {
    pub rise: i32,
    pub cells: i32,
    pub noise: bool,
    pub strata: bool,
    pub wander: i32,
}

/// `x / span` in Q16 for `x` clamped to `[0, span]`, exact to the bit on
/// the GPU (`volume_ramp`): 32-bit, `span` cut to 15 bits.
pub(crate) fn ramp_q16(x: i32, span: i32) -> i32 {
    if span <= 0 {
        return if x >= 0 { ONE } else { 0 };
    }
    let x = x.clamp(0, span);
    let s = (32 - span.leading_zeros() as i32 - 15).max(0) as u32;
    ((x >> s) << 16) / (span >> s).max(1)
}

/// The compiled volume layer of one world grid.
#[derive(Clone, Debug, PartialEq)]
pub struct VolumeLayer {
    k: VolumeConstants,
}

impl VolumeLayer {
    pub fn new(volumes: &Volumes, grid: &Grid, seed: u32) -> Self {
        let c = &volumes.caves;
        let k = &volumes.cliffs;
        let voxel = grid.voxel_size();
        let layer = grid.layer_mm() as i32;
        let q2 = |m: f64| ((m / voxel * 4.0).round().clamp(0.0, f64::from(1 << 24))) as i32;
        let q16 = |share: f64| (share.clamp(0.0, 1.0) * f64::from(ONE)).round() as i32;
        let q8 = |share: f64| (share.clamp(0.0, 4.0) * 256.0).round() as i32;
        let mm = |m: f64| (m.max(0.0) * 1000.0).round().min(f64::from(1 << 28)) as i32;
        let domain_shift = |m: f64| ((m / DOMAIN_UNIT).log2().round().clamp(1.0, 30.0)) as i32;
        let region = ((c.spacing_m / voxel).log2().round().clamp(10.0, 13.0)) as i32;
        let s = 1i32 << region;
        // Radii within a region's bounds (`docs/volumes.md`, owners): S/16
        // cells for tunnels, S/8 for chambers (Q2).
        let tunnel_max = s / 4;
        let chamber_max = s / 2;
        let radius = |m: f64, max: i32| q2(m).clamp(4, max);
        let mut flags = 0;
        if c.enabled && c.share > 0.0 {
            flags |= 1;
        }
        let amplitude = mm(k.depth_m).min(64 * layer << 3);
        if k.enabled && amplitude > 0 && k.share > 0.0 {
            flags |= 2;
        }
        let shape = |height: f64, floor: f64| {
            q8(height.clamp(0.1, 1.0)) | (q8(floor.clamp(0.0, 1.0)) << 16)
        };
        let rough_shift = ((c.roughness_m / voxel * 4.0)
            .log2()
            .round()
            .clamp(2.0, 20.0)) as i32;
        let eighths = |slope: f64| (slope * 8.0).round().clamp(0.0, 4096.0) as i32;
        let wn = (k.noise.clamp(0.0, 1.0) * 256.0 * 4.0 / 3.0).round() as i32;
        let ws = ((1.0 - k.noise.clamp(0.0, 1.0)) * 256.0).round() as i32;
        let constants = VolumeConstants {
            head: [flags, region, region - 3, seed as i32],
            shares: [
                q16(c.share),
                q16(c.hub_chambers),
                q16(c.chambers),
                q16(c.links),
            ],
            depths: [
                q16(c.entrances),
                q2(c.depth_min_m),
                q2(c.depth_max_m),
                q2(c.entrance_lift_m),
            ],
            radii: [
                radius(c.tunnel_min_m, tunnel_max),
                radius(c.tunnel_max_m, tunnel_max),
                radius(c.chamber_min_m, chamber_max),
                radius(c.chamber_max_m, chamber_max),
            ],
            shapes: [
                shape(c.tunnel_height, c.tunnel_floor),
                shape(c.chamber_height, c.chamber_floor),
                q8(c.roughness.clamp(0.0, 0.5)),
                rough_shift,
            ],
            bounds: [
                q2(c.cover_m),
                q2(c.max_depth_m).max(4),
                layer,
                q2(c.bend_m).min(s),
            ],
            cliff: [
                amplitude,
                eighths(k.slope_min),
                eighths(k.slope_max).max(eighths(k.slope_min) + 1),
                domain_shift(k.region_km * 1000.0),
            ],
            cliff_shape: [
                (noise_quantile(1.0 - k.share.clamp(0.0, 1.0)) * f64::from(ONE)).round() as i32,
                domain_shift(4.0 * k.depth_m.max(0.1)),
                mm(k.strata_m).max(layer),
                domain_shift(16.0 * k.strata_m.max(0.1)),
            ],
            cliff_mix: [wn, ws, grid.cells(), 0],
        };
        Self { k: constants }
    }

    pub fn constants(&self) -> VolumeConstants {
        self.k
    }
    pub fn caves(&self) -> bool {
        self.k.head[0] & 1 != 0
    }
    pub fn cliffs(&self) -> bool {
        self.k.head[0] & 2 != 0
    }
    /// Region size: `2^region_shift` base cells.
    pub fn region_shift(&self) -> u32 {
        self.k.head[1] as u32
    }
    /// The level whose column heights are the networks' reference ground.
    pub fn reference_level(&self) -> u32 {
        self.k.head[2] as u32
    }
    pub fn roughness(&self) -> i32 {
        self.k.shapes[2]
    }
    /// Largest depth (mm) under the surface and height over it at which
    /// any level's cells may differ from the heightfield.
    pub fn bounds(&self) -> (i32, i32) {
        let layer = self.k.bounds[2];
        let mut depth = 0;
        let mut height = 0;
        if self.caves() {
            depth = self.k.bounds[1] / 4 * layer + 2 * layer;
        }
        if self.cliffs() {
            let rise = self.k.cliff[0] * 4 + 2 * layer;
            depth = depth.max(rise);
            height = rise;
        }
        (depth, height)
    }
    /// Level cells a finer column's highest solid cell may rise over its
    /// heightfield top, seen from `level` (`TerrainField::bound_margins`).
    pub fn rise_cells(&self, level: u32) -> i32 {
        if !self.cliffs() {
            return 0;
        }
        self.bounds().1 / (self.k.bounds[2] << level.min(24)).max(1) + 2
    }
    /// Level cells under a column's top caves may carve at `level`.
    pub fn max_depth_cells(&self, level: u32) -> i32 {
        (self.k.bounds[1] >> level) / 4 + 2
    }
    /// The deepest level at which tunnels (`chamber` false) or chambers
    /// are resolved: a footprint at most a region (tunnels) or two
    /// (chambers) across keeps its primitives within `CAVE_SLOT`.
    pub fn deepest_level(&self, chamber: bool) -> i32 {
        self.k.head[1] - if chamber { 5 } else { 6 }
    }
    /// Whether `p` is resolved at `level`: its smaller radius at least a
    /// cell.
    pub fn resolved(&self, p: &Primitive, level: u32) -> bool {
        p.r.min(p.rv) >= 4 << level && level as i32 <= self.deepest_level(p.flags & CHAMBER != 0)
    }

    fn hash(&self, face: u8, ri: i32, rj: i32, slot: i32) -> u32 {
        hash3(
            ri,
            rj,
            i32::from(face) * 128 + slot,
            (self.k.head[3] as u32) ^ SEED_CAVES,
        )
    }
    /// `lo + (hi - lo) h` for the top 16 bits `h` of a hash.
    fn pick(lo: i32, hi: i32, h: u32) -> i32 {
        lo + scale((h >> 16) as i32, hi - lo)
    }
    /// Whether region `(ri, rj)` lies whole on the face.
    fn inside(&self, ri: i32, rj: i32) -> bool {
        let n = self.k.cliff_mix[2] >> self.region_shift();
        ri >= 0 && rj >= 0 && ri < n && rj < n
    }
    /// Whether the region may have a network (its hash; the gate at its
    /// hub decides).
    fn may_be_active(&self, face: u8, ri: i32, rj: i32) -> bool {
        self.inside(ri, rj) && (self.hash(face, ri, rj, H_ACTIVE) >> 16) < self.k.shares[0] as u32
    }
    /// Whether a region the hash allows is active under gate `gate` (Q8)
    /// at its hub.
    fn gate_passes(&self, face: u8, ri: i32, rj: i32, gate: i32) -> bool {
        ((self.hash(face, ri, rj, H_ACTIVE) >> 16) as i32)
            < (self.k.shares[0] * gate.clamp(0, 256)) >> 8
    }
    /// A node in the region's middle half.
    fn node(&self, face: u8, ri: i32, rj: i32, n: i32) -> (i32, i32) {
        let s = 1 << self.region_shift();
        let at = |slot: i32, r: i32| {
            r * s
                + s / 4
                + (((s / 2) * (self.hash(face, ri, rj, H_SITE + 2 * n + slot) >> 16) as i32) >> 16)
        };
        (at(0, ri), at(1, rj))
    }
    /// Hub depth (Q2 layers) of a region.
    fn hub_depth(&self, face: u8, ri: i32, rj: i32) -> i32 {
        Self::pick(
            self.k.depths[1],
            self.k.depths[2],
            self.hash(face, ri, rj, H_DEPTH),
        )
    }
    /// The entrance's end: a quarter region from the hub at most, in the
    /// middle half.
    fn entrance_end(&self, face: u8, ri: i32, rj: i32) -> (i32, i32) {
        let s = 1 << self.region_shift();
        let (hx, hy) = self.node(face, ri, rj, 0);
        let h = self.hash(face, ri, rj, H_ENTRANCE_END);
        let dx = (((s / 2) * ((h >> 16) as i32)) >> 16) - s / 4;
        let dy = (((s / 2) * ((h & 0xffff) as i32)) >> 16) - s / 4;
        (
            (hx + dx).clamp(ri * s + s / 4, ri * s + 3 * s / 4),
            (hy + dy).clamp(rj * s + s / 4, rj * s + 3 * s / 4),
        )
    }

    /// Where region `(ri, rj)` of `face` reads the ground ([`SITES`]).
    pub fn sites(&self, face: u8, ri: i32, rj: i32) -> [Site; SITES] {
        let none = Site {
            x: 0,
            y: 0,
            used: false,
        };
        let mut sites = [none; SITES];
        if !self.caves() || !self.may_be_active(face, ri, rj) {
            return sites;
        }
        let site = |(x, y): (i32, i32), used: bool| Site { x, y, used };
        let k = &self.k.shares;
        sites[0] = site(self.node(face, ri, rj, 0), true);
        for m in 1..3 {
            let used = (self.hash(face, ri, rj, H_CHAMBER + m - 1) >> 16) < k[2] as u32;
            sites[m as usize] = site(self.node(face, ri, rj, m), used);
        }
        sites[3] = site(
            self.entrance_end(face, ri, rj),
            (self.hash(face, ri, rj, H_ENTRANCE) >> 16) < self.k.depths[0] as u32,
        );
        for (axis, (ni, nj)) in [(ri + 1, rj), (ri, rj + 1)].into_iter().enumerate() {
            let used = self.may_be_active(face, ni, nj)
                && (self.hash(face, ri, rj, H_LINK + axis as i32) >> 16) < k[3] as u32;
            sites[4 + axis] = site(self.node(face, ni, nj, 0), used);
        }
        sites
    }

    /// Region `(ri, rj)`'s network: `ground[s]` is the reference ground
    /// (height mm, surface word) and the gate (Q8) at site `s` of
    /// [`Self::sites`] (read only where used).
    pub fn network(
        &self,
        face: u8,
        ri: i32,
        rj: i32,
        ground: &[(i32, i32); SITES],
    ) -> Vec<Primitive> {
        let sites = self.sites(face, ri, rj);
        let mut out = Vec::with_capacity(REGION_PRIMITIVES);
        if !sites[0].used || !self.gate_passes(face, ri, rj, ground[0].1) {
            return out;
        }
        let layer = self.k.bounds[2];
        let z = |height: i32, depth: i32| {
            (i64::from(height) * 4).div_euclid(i64::from(layer)) - i64::from(depth)
        };
        let at = |s: &Site, z: i64| [4 * i64::from(s.x) + 2, 4 * i64::from(s.y) + 2, z];
        let hub_depth = self.hub_depth(face, ri, rj);
        let hub = at(&sites[0], z(ground[0].0, hub_depth));
        let shape = |w: i32| (w & 0xffff, w >> 16);
        let (tunnel_h, tunnel_floor) = shape(self.k.shapes[0]);
        let (chamber_h, chamber_floor) = shape(self.k.shapes[1]);
        let chamber = |c: [i64; 3], slot: i32| {
            let r = Self::pick(
                self.k.radii[2],
                self.k.radii[3],
                self.hash(face, ri, rj, H_RADIUS + slot),
            );
            Primitive {
                a: c,
                b: c,
                r,
                rv: ((r * chamber_h) >> 8).max(4),
                floor: chamber_floor,
                flags: CHAMBER,
            }
        };
        let s = 1i64 << self.region_shift();
        let bend = i64::from(self.k.bounds[3]);
        let edge = |out: &mut Vec<Primitive>, a: [i64; 3], b: [i64; 3], e: i32, flags: u32| {
            let h = |n: i32| self.hash(face, ri, rj, H_EDGE + 3 * e + n);
            let r = Self::pick(self.k.radii[0], self.k.radii[1], h(0));
            // The bend: the midpoint moved up to S/16 across and `bend`
            // up or down (Q2).
            let off = |v: u32, span: i64| ((2 * span * i64::from(v & 0xffff)) >> 16) - span;
            let m = [
                (a[0] + b[0]).div_euclid(2) + off(h(1), s / 4),
                (a[1] + b[1]).div_euclid(2) + off(h(1) >> 16, s / 4),
                (a[2] + b[2]).div_euclid(2) + off(h(2), bend),
            ];
            for (p, q) in [(a, m), (m, b)] {
                out.push(Primitive {
                    a: p,
                    b: q,
                    r,
                    rv: ((r * tunnel_h) >> 8).max(4),
                    floor: tunnel_floor,
                    flags,
                });
            }
        };
        if (self.hash(face, ri, rj, H_HUB_CHAMBER) >> 16) < self.k.shares[1] as u32 {
            out.push(chamber(hub, 0));
        }
        let spread = (self.k.depths[2] - self.k.depths[1]) / 4;
        for m in 1..3usize {
            if !sites[m].used {
                continue;
            }
            let h = self.hash(face, ri, rj, H_CHAMBER_DEPTH + m as i32);
            let depth = (hub_depth + Self::pick(-spread, spread, h))
                .clamp(self.k.depths[1], self.k.depths[2]);
            let c = at(&sites[m], z(ground[m].0, depth));
            out.push(chamber(c, m as i32));
            edge(&mut out, hub, c, m as i32 - 1, 0);
        }
        for axis in 0..2usize {
            let (ni, nj) = if axis == 0 {
                (ri + 1, rj)
            } else {
                (ri, rj + 1)
            };
            if !sites[4 + axis].used || !self.gate_passes(face, ni, nj, ground[4 + axis].1) {
                continue;
            }
            let other = at(
                &sites[4 + axis],
                z(ground[4 + axis].0, self.hub_depth(face, ni, nj)),
            );
            edge(&mut out, hub, other, 2 + axis as i32, 0);
        }
        if sites[3].used {
            let lift = i64::from(self.k.depths[3]);
            let end = at(&sites[3], z(ground[3].0, 0) + lift);
            edge(&mut out, hub, end, 4, ENTRANCE);
        }
        out
    }

    /// Whether level cell `k` (centre `(x, y)`, Q2) of a column of field
    /// height `height` is carved by `p` (`volume_carves` in WGSL).
    pub fn carves(&self, p: &Primitive, x: i64, y: i64, k: i32, level: u32, height: i32) -> bool {
        let rough = self.roughness();
        let zc = (4 * k + 2).wrapping_shl(level);
        let (lo, hi) = p.z_range(rough);
        if !p.covers(x, y, rough) || i64::from(zc) < lo || i64::from(zc) > hi {
            return false;
        }
        let hq = height.wrapping_mul(4).div_euclid(self.k.bounds[2]);
        let depth = hq.wrapping_sub(zc);
        let (cover, max_depth) = (self.k.bounds[0], self.k.bounds[1]);
        if depth > max_depth {
            return false;
        }
        // The radius narrows (Q8) under the cover, except along entrances,
        // and over the deepest cells.
        let mut s = 256;
        if p.flags & ENTRANCE == 0 && cover > 0 {
            let half = (cover / 2).max(1);
            s = ((depth - half).clamp(0, half) << 8) / half;
        }
        let taper = (max_depth / 16).max(4);
        s = s.min(((max_depth - depth).min(taper) << 8) / taper);
        let r_eff = (p.r * s) >> 8;
        if r_eff <= 0 {
            return false;
        }
        let e = [
            (x - p.a[0]) as i32,
            (y - p.a[1]) as i32,
            zc.wrapping_sub(p.a[2] as i32),
        ];
        let d = [
            (p.b[0] - p.a[0]) as i32,
            (p.b[1] - p.a[1]) as i32,
            (p.b[2] - p.a[2]) as i32,
        ];
        let len2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
        let mut t12 = 0;
        if len2 > 0 {
            let t = (e[0] * d[0] + e[1] * d[1] + e[2] * d[2]).clamp(0, len2);
            let sh = (32 - len2.leading_zeros() as i32 - 18).max(0) as u32;
            t12 = (((t >> sh) << 12) / (len2 >> sh).max(1)).min(4096);
        }
        let axis = [(d[0] * t12) >> 12, (d[1] * t12) >> 12, (d[2] * t12) >> 12];
        // The floor: cells under the axis by `floor` of the vertical
        // radius stay solid.
        if p.floor < 256 && e[2] < axis[2] - ((p.rv * p.floor) >> 8) {
            return false;
        }
        let q = [
            e[0] - axis[0],
            e[1] - axis[1],
            ((e[2] - axis[2]) * p.inv_v()) >> 8,
        ];
        let ra = (r_eff * self.k.shapes[2]) >> 8;
        let outer = r_eff + ((ra * 3) >> 1) + 1;
        if q.iter().any(|c| c.abs() >= outer) {
            return false;
        }
        let dist2 = q[0] * q[0] + q[1] * q[1] + q[2] * q[2];
        let inner = r_eff - ra;
        if inner > 0 && dist2 < inner * inner {
            return true;
        }
        if dist2 >= outer * outer {
            return false;
        }
        let n = noise(
            IVec3::new(x as i32, y as i32, zc),
            self.k.shapes[3] as u32,
            (self.k.head[3] as u32) ^ SEED_ROUGH,
        );
        let r2 = r_eff + ((ra * (n >> 4)) >> 11);
        r2 > 0 && dist2 < r2 * r2
    }

    /// The regions whose networks may reach base column `(x, y)`: its own,
    /// `-i` and `-j`.
    pub fn owners(&self, x: i32, y: i32) -> [(i32, i32); 3] {
        let (ri, rj) = (x >> self.region_shift(), y >> self.region_shift());
        [(ri, rj), (ri - 1, rj), (ri, rj - 1)]
    }

    /// The reference-level columns `[i0, i1] x [j0, j1]` whose ground the
    /// networks over base columns `[x0, x1] x [y0, y1]` at `level` read
    /// (their owners' sites, links into the next regions included; none
    /// where caves are not resolved): what generation keeps resident for
    /// them (a field's regions).
    pub fn ground_columns(
        &self,
        level: u32,
        x: [i32; 2],
        y: [i32; 2],
    ) -> Option<([i32; 2], [i32; 2])> {
        if !self.caves() || level as i32 > self.deepest_level(true) {
            return None;
        }
        let (r, reference) = (self.region_shift(), self.reference_level());
        // Regions past the face have no network (`inside`).
        let last = (self.k.cliff_mix[2] >> reference) - 1;
        let span = |a: [i32; 2]| {
            [
                (((a[0] >> r) - 1) << r >> reference).max(0),
                ((((a[1] >> r) + 2) << r) - 1 >> reference).min(last),
            ]
        };
        Some((span(x), span(y)))
    }

    /// The cliff shape of a column at `level` (domain point `p`, material
    /// slope `slope` in eighths).
    pub fn cliff(&self, p: IVec3, slope: i32, level: u32) -> Cliff {
        if !self.cliffs() || level >= CLIFF_LEVELS {
            return Cliff::default();
        }
        let c = &self.k.cliff;
        let gate = ramp_q16(slope - c[1], c[2] - c[1]);
        if gate == 0 {
            return Cliff::default();
        }
        let seed = self.k.head[3] as u32;
        let n = noise(p, c[3] as u32, seed ^ SEED_CLIFF_REGION);
        let region = (n.wrapping_sub(self.k.cliff_shape[0]).wrapping_mul(4)).clamp(0, ONE);
        let a = scale(scale(region, c[0]), gate);
        let layer = self.k.bounds[2];
        let cell = layer << level;
        if a < cell {
            return Cliff::default();
        }
        let s = slope.clamp(0, 1000);
        let nq = ((64 + s * s) as u32).isqrt().min(32) as i32;
        let rise = (a * nq) >> 3;
        let k = &self.k.cliff_shape;
        let period = k[2];
        // Domain units of a level cell (12.5 mm each).
        let cell_du = (cell * 2) / 25;
        Cliff {
            rise,
            cells: rise / cell + 2,
            noise: self.k.cliff_mix[0] > 0 && (1i32 << k[1]) >= 2 * cell_du,
            strata: self.k.cliff_mix[1] > 0 && period >= 2 * cell,
            wander: scale(noise(p, k[3] as u32, seed ^ SEED_STRATA), period),
        }
    }

    /// Whether level cell `k` of a column with cliff `c` (field height
    /// `height` mm) is solid, its volume point `q`.
    pub fn cliff_solid(&self, c: &Cliff, q: IVec3, height: i32, k: i32, level: u32) -> bool {
        let cell = self.k.bounds[2] << level;
        let z = k.wrapping_mul(cell).wrapping_add(cell / 2);
        let d = height.wrapping_sub(z);
        let mut g = 0;
        if c.noise {
            let n = noise(
                q,
                self.k.cliff_shape[1] as u32,
                (self.k.head[3] as u32) ^ SEED_CLIFF,
            );
            g += (n >> 4) * self.k.cliff_mix[0];
        }
        if c.strata {
            let period = self.k.cliff_shape[2];
            let f = z.wrapping_add(c.wander).rem_euclid(period);
            let tri = 2 * ramp_q16((2 * f - period).abs(), period) - ONE;
            g += (tri >> 4) * self.k.cliff_mix[1];
        }
        let g = (g >> 4).clamp(-ONE, ONE);
        d.wrapping_add((c.rise * (g >> 4)) >> 12) > 0
    }
}

/// A level column's volume (`Planet::volume_column`): what its cells read
/// once.
#[derive(Clone, Debug, Default)]
pub struct VolumeColumn {
    /// Field height (mm) and heightfield top (level cells).
    pub height: i32,
    pub top: i32,
    pub cliff: Cliff,
    /// The primitives resolved at the level over the column.
    pub primitives: Vec<Primitive>,
    /// Q2 centre.
    pub x: i64,
    pub y: i64,
    /// Level cells below and above `top` the volume may change.
    pub below: i32,
    pub above: i32,
}

impl VolumeColumn {
    /// Whether the column is a pure heightfield.
    pub fn is_heightfield(&self) -> bool {
        self.below == 0 && self.above == 0
    }
}

/// Q2 centre of a level column's index `i`.
pub fn q2_center(i: i32, level: u32) -> i64 {
    (4 * i64::from(i) + 2) << level
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::Shape;

    fn layer() -> VolumeLayer {
        let grid = Grid::plane(Shape::InfinitePlane, 0.0, 0.1).unwrap();
        VolumeLayer::new(&Volumes::default(), &grid, 7)
    }

    /// A region's primitives stay inside its box and its `+i`/`+j`
    /// neighbours' (a column needs three regions' networks), and segment
    /// products fit 32 bits.
    #[test]
    fn networks_stay_within_their_owners() {
        let v = layer();
        let s = 1i64 << v.region_shift();
        let mut seen = 0;
        for ri in 100..160 {
            for rj in 100..160 {
                let ground = [(1000, 256); SITES];
                let net = v.network(0, ri, rj, &ground);
                assert!(net.len() <= REGION_PRIMITIVES);
                for p in &net {
                    seen += 1;
                    let (h, _) = p.reach(v.roughness());
                    let h = i64::from(h);
                    for c in [p.a, p.b] {
                        assert!(
                            c[0] - h >= 4 * s * i64::from(ri)
                                && c[0] + h < 4 * s * (i64::from(ri) + 2)
                        );
                        assert!(
                            c[1] - h >= 4 * s * i64::from(rj)
                                && c[1] + h < 4 * s * (i64::from(rj) + 2)
                        );
                    }
                    // Not both +i and +j past the region.
                    let past = |c: [i64; 3]| {
                        (
                            c[0] >= 4 * s * (i64::from(ri) + 1),
                            c[1] >= 4 * s * (i64::from(rj) + 1),
                        )
                    };
                    assert!(!(past(p.a).0 && past(p.b).1) && !(past(p.a).1 && past(p.b).0));
                    let d = (0..3).map(|n| (p.b[n] - p.a[n]).abs()).max().unwrap();
                    assert!(d < 1 << 14, "segment {d}");
                }
            }
        }
        assert!(seen > 10_000, "{seen}");
    }

    /// Flat ground 300 m over the datum under every site (gate open).
    const GROUND: [(i32, i32); SITES] = [(300_000, 256); SITES];

    /// Whether any primitive of the three owners carves level cell `k` of
    /// column `(i, j)` under flat ground.
    fn carved(
        v: &VolumeLayer,
        nets: &dyn Fn(i32, i32) -> Vec<Primitive>,
        i: i32,
        j: i32,
        k: i32,
        level: u32,
    ) -> bool {
        let half = (1 << level) >> 1;
        let (x, y) = (q2_center(i, level), q2_center(j, level));
        v.owners((i << level) + half, (j << level) + half)
            .into_iter()
            .any(|(ri, rj)| {
                nets(ri, rj)
                    .iter()
                    .any(|p| v.resolved(p, level) && v.carves(p, x, y, k, level, GROUND[0].0))
            })
    }

    /// Networks join into systems: hubs linked to their neighbours form
    /// components spanning many regions, and a passage is open all along
    /// its axis (over its floor), chamber to chamber.
    #[test]
    fn networks_connect_across_regions() {
        let v = layer();
        let n = 24usize;
        let mut parent: Vec<usize> = (0..n * n).collect();
        fn find(p: &mut [usize], a: usize) -> usize {
            let mut a = a;
            while p[a] != a {
                p[a] = p[p[a]];
                a = p[a];
            }
            a
        }
        let mut hubs = 0;
        for rj in 0..n as i32 {
            for ri in 0..n as i32 {
                let sites = v.sites(0, 1000 + ri, 1000 + rj);
                if !sites[0].used {
                    continue;
                }
                hubs += 1;
                for (axis, other) in [(4, (ri + 1, rj)), (5, (ri, rj + 1))] {
                    if sites[axis].used && other.0 < n as i32 && other.1 < n as i32 {
                        let a = (rj * n as i32 + ri) as usize;
                        let b = (other.1 * n as i32 + other.0) as usize;
                        let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
                        parent[ra] = rb;
                    }
                }
            }
        }
        let mut sizes = std::collections::HashMap::new();
        for a in 0..n * n {
            *sizes.entry(find(&mut parent, a)).or_insert(0) += 1;
        }
        let largest = sizes.values().copied().max().unwrap();
        assert!(hubs > n * n / 3, "{hubs} hubs");
        assert!(largest >= 20, "largest system {largest} regions");
        // Along every tunnel's axis, half a vertical radius over it, cells
        // are open.
        let nets = |ri: i32, rj: i32| v.network(0, ri, rj, &GROUND);
        let mut checked = 0;
        for ri in 1000..1006 {
            for rj in 1000..1006 {
                for p in nets(ri, rj).iter().filter(|p| p.flags & CHAMBER == 0) {
                    for t in 0..=8i64 {
                        let at = |c: usize| p.a[c] + (p.b[c] - p.a[c]) * t / 8;
                        let k = ((at(2) + i64::from(p.rv) / 2) >> 2) as i32;
                        let (i, j) = ((at(0) >> 2) as i32, (at(1) >> 2) as i32);
                        if p.flags & ENTRANCE != 0 && GROUND[0].0 / 100 - k < 80 {
                            continue;
                        }
                        assert!(
                            carved(&v, &nets, i, j, k, 0),
                            "tunnel {p:?} closed at {t}/8"
                        );
                        checked += 1;
                    }
                }
            }
        }
        assert!(checked > 100, "{checked}");
    }

    /// Coarse levels sample the same caves: the carved volume of a level's
    /// cells is the finer level's, within a few percent, at every level
    /// that resolves the primitives.
    #[test]
    fn carved_volume_agrees_across_levels() {
        let v = layer();
        // The primitives every compared level resolves (smaller ones are
        // omitted where cells outgrow them, as finer octaves are).
        let nets = |ri: i32, rj: i32| {
            let mut n = v.network(0, ri, rj, &GROUND);
            n.retain(|p| v.resolved(p, 5));
            n
        };
        let s = 1 << v.region_shift();
        let (i0, j0) = (1002 * s, 1002 * s);
        let top = GROUND[0].0 / 100;
        let volume = |level: u32| {
            let (n, deep) = (s >> level, 1300 >> level);
            let mut count = 0u64;
            for j in (j0 >> level)..(j0 >> level) + n {
                for i in (i0 >> level)..(i0 >> level) + n {
                    for k in (top >> level) - deep..(top >> level) {
                        count += u64::from(carved(&v, &nets, i, j, k, level));
                    }
                }
            }
            count as f64 * f64::from(1u32 << (3 * level))
        };
        let fine = volume(3);
        assert!(fine > 1.0e5, "{fine}");
        for level in [4, 5] {
            let ratio = volume(level) / fine;
            assert!((ratio - 1.0).abs() < 0.15, "level {level}: {ratio}");
        }
    }

    /// Caves keep their cover over them (except along entrances) and never
    /// reach past the depth limit; chambers have flat floors.
    #[test]
    fn caves_keep_their_cover_floor_and_depth() {
        let v = layer();
        let k = v.constants();
        let (cover, max_depth) = (k.bounds[0] / 4, k.bounds[1] / 4);
        let top = GROUND[0].0 / 100;
        let nets = |ri: i32, rj: i32| v.network(0, ri, rj, &GROUND);
        let mut floors = 0;
        for ri in 1000..1008 {
            for rj in 1000..1008 {
                for p in nets(ri, rj).iter() {
                    let (x, y) = ((p.a[0] >> 2) as i32, (p.a[1] >> 2) as i32);
                    for k in top - max_depth - 20..top + 4 {
                        if !carved(&v, &nets, x, y, k, 0) {
                            continue;
                        }
                        assert!(k >= top - max_depth - 1, "carved past the depth limit");
                        if k >= top - cover / 2 {
                            // Only an entrance opens the cover.
                            let entrance = v.owners(x, y).into_iter().any(|(a, b)| {
                                nets(a, b).iter().any(|q| {
                                    q.flags & ENTRANCE != 0
                                        && v.carves(
                                            q,
                                            q2_center(x, 0),
                                            q2_center(y, 0),
                                            k,
                                            0,
                                            GROUND[0].0,
                                        )
                                })
                            });
                            assert!(entrance, "cover broken at {x} {y} {k}");
                        }
                    }
                    if p.flags & CHAMBER != 0 {
                        // Over the floor plane at the centre: air.
                        let floor = ((p.a[2] - i64::from((p.rv * p.floor) >> 8)) >> 2) as i32;
                        let depth = top - floor;
                        if depth > cover + 2 && depth < max_depth - 40 {
                            assert!(carved(&v, &nets, x, y, floor + 1, 0), "chamber {p:?}");
                            floors += 1;
                        }
                    }
                }
            }
        }
        assert!(floors > 10, "{floors}");
    }

    /// Cliff shapes leave level ground alone and on an even slope keep
    /// every solid cell attached: at each height a column is solid only if
    /// the one uphill of it is (no floating ledges).
    #[test]
    fn cliff_shapes_stay_attached() {
        let grid = Grid::plane(Shape::InfinitePlane, 0.0, 0.1).unwrap();
        let v = VolumeLayer::new(
            &Volumes {
                cliffs: Cliffs {
                    share: 1.0,
                    noise: 0.0,
                    ..Cliffs::default()
                },
                ..Volumes::none()
            },
            &grid,
            7,
        );
        let face = crate::grid::PLANE_FACE;
        let points = grid.volume_points();
        assert_eq!(v.cliff(grid.domain_point(face, 0, 0, 0), 0, 0).rise, 0);
        // A slope of 3 (24 eighths) rising along +i.
        let slope = 24;
        let mut displaced = 0;
        for j in 0..32 {
            for k in 2_950..3_250 {
                let solid: Vec<bool> = (0..64)
                    .map(|i| {
                        let height = 300_000 + 300 * i;
                        let c = v.cliff(grid.domain_point(face, i, j, 0), slope, 0);
                        assert!(c.rise > 0);
                        let top = height / 100;
                        if k < top - c.cells || k >= top + c.cells {
                            return k < top;
                        }
                        let q = points.point(face, i, j, k, 0);
                        let s = v.cliff_solid(&c, q, height, k, 0);
                        displaced += u32::from(s != (k < top));
                        s
                    })
                    .collect();
                for i in 1..64 {
                    assert!(!solid[i - 1] || solid[i], "floating cell at {i} {j} {k}");
                }
            }
        }
        assert!(displaced > 1000, "{displaced}");
    }
}
