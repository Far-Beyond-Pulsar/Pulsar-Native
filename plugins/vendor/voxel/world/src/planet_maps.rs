//! Planet maps: what the terrain knows of its planet as a whole
//! (`docs/planet-maps.md`).
//!
//! A world's stack may bake, once from its seed and layers, maps over the
//! whole planet (a cube map of six gnomonic faces; one square on a plane):
//!
//! - **Drainage.** The terrain's coarse heights, filled from the sea and
//!   from dry basins (priority flood), each texel draining to its steepest
//!   lower neighbour; discharge is the rain gathered upstream. Every texel
//!   whose basin reaches a river's size, and every lake, holds a
//!   **river segment**: from its node (a jittered point of the texel) to
//!   its downstream texel's, with the river's bed at both ends, its
//!   channel, floodplain and valley walls. A `Rivers` layer carves them
//!   ([`carve`]): the ground is cut down to a valley around each segment,
//!   a channel in its floor, its walls rising until they meet the terrain.
//! - **Climate.** Sea-level temperature from latitude, and moisture from
//!   latitude's rain belts, the way to the sea upwind and across the
//!   coast, rain shadows behind high ground and the rivers. Height lowers
//!   the temperature at sampling ([`climate`]): materials and features read
//!   both (snow, dry lands, forests, tree lines).
//!
//! The maps are integer data: the GPU reads the same words (the program's
//! storage table, `terrain_tables`) and samples them with the same integer
//! arithmetic (`shaders/planet_maps.wgsl`), so the field stays bit for bit
//! the CPU's. Positions are looked up by domain point (gnomonic: one
//! division per lookup, a Q30 reciprocal), never by grid column, so every
//! level and every voxel size samples the same maps, and seams between
//! faces agree: river segments are 3D, and each face's maps carry a ring of
//! the neighbouring faces' texels.
//!
//! Baking uses only IEEE basic operations and square roots (no
//! transcendental functions), in a fixed order, so every machine bakes the
//! same maps from the same world.
use crate::grid::{face_of, Grid, DOMAIN_UNIT, FACE_BASIS, PLANE_FACE};
use crate::landform::{self, recip_q30, smooth_min, LandformConstants};
use crate::noise::{
    hash3, mul_shr, mul_shr_signed, noise, noise_fine, scale, unit_q30, FINE_ONE, Q30,
};
use glam::{DVec3, IVec3};
use rayon::prelude::*;
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::sync::{Arc, Mutex, OnceLock, Weak};

/// Rows of `LandformConstants::maps` (`TerrainConstants::maps` in WGSL):
///
/// | row | x | y | z | w |
/// |---|---|---|---|---|
/// | 0 | flags | river texels a side `n` | stride `n + 4` | climate texels a side |
/// | 1 | index at (words) | climate at | records at | climate stride |
/// | 2 | plane half width (domain units; 0 on spheres) | unmapped temperature | unmapped moisture | lapse rate |
/// | 3 | snow temperature | snowline mm per temperature unit | valley rounding (mm) | seed |
/// | 4 | erosion at | drainage at | sediment rounding (mm) | a texel's stream reach (domain units) |
///
/// Temperatures are in 1/128 °C, moisture in Q16 (0 dry, 65535 wet), the
/// lapse rate in 1/128 °C per kilometre. The drainage grid has the river
/// index's layout (`n` texels a side and its ring), the erosion grid is
/// of vertices (`n + 1` a side).
pub const MAP_HEADER: usize = 5;
pub const FLAG_CLIMATE: i32 = 1;
pub const FLAG_RIVERS: i32 = 2;
/// The erosion grid ([`erosion`]) holds the worn terrain.
pub const FLAG_EROSION: i32 = 4;
/// The drainage grid ([`drainage`]) holds every texel's discharge.
pub const FLAG_DRAINAGE: i32 = 8;
/// The field carves streams (`crate::streams`) from the program's regions,
/// `flags >> 8 & 3` levels below the texels.
pub const FLAG_STREAMS: i32 = 16;
/// Temperature units per °C.
pub const DEGREE: i32 = 128;
/// Moisture of wet ground.
pub const WET: i32 = 65_535;
/// Ghost texels around each face of the river grid (the 5x5 window of a
/// lookup) and of the climate grid (bilinear).
const GHOST: i32 = 2;
const CLIMATE_GHOST: i32 = 1;
/// Words of a river segment record.
pub const RECORD_WORDS: usize = 8;
/// A record's flag: its texel is a lake.
const RECORD_LAKE: u32 = 1;
/// Most map texels along a face side.
pub const MAX_TEXELS: i32 = 1024;
const MIN_TEXELS: i32 = 32;
/// Most map texels of a world (all faces): larger worlds get larger
/// texels, so a bake costs the same from the Moon's size up (finer
/// drainage is refined per region, not baked).
pub const MAX_MAP_TEXELS: i32 = 6 * 512 * 512;
/// Least depth (mm) of a filled basin that makes a lake.
const LAKE_DEPTH: i32 = 30_000;
/// Erosion grid units (mm): cuts in half metres, fill levels in metres
/// (16-bit each).
const CUT_UNIT: i32 = 500;
const FILL_UNIT: i32 = 1000;
/// Rounding (mm) where sediment meets the ground it buries.
const SEDIMENT_ROUNDING: i32 = 30_000;
/// Drainage grid discharge units: `log2(1 + km²)` in 1/2048.
pub const DISCHARGE_ONE: i32 = 2048;
/// Steps of the bake's erosion.
const ERODE_STEPS: usize = 8;
/// Domain units per metre.
const DU: f64 = 1.0 / DOMAIN_UNIT;

/// How a world's climate is made (`crate::layers::Climate`, compiled).
#[derive(Clone, Debug, PartialEq)]
pub struct ClimateSpec {
    pub maps: bool,
    pub pole: DVec3,
    pub latitude_deg: f64,
    pub equator_c: f64,
    pub pole_c: f64,
    pub lapse_c_per_km: f64,
    pub snow_c: f64,
    pub wetness: f64,
    pub continentality_km: f64,
    pub rain_shadow_m: f64,
    pub ocean: bool,
    pub texel_km: f64,
}

/// A `Rivers` layer's parameters (`crate::layers::LayerKind::Rivers`).
#[derive(Clone, Debug, PartialEq)]
pub struct RiverSpec {
    /// Wet drainage area a river begins at (km²).
    pub source_km2: f64,
    /// Valley incision under the filled terrain at a source (m); it grows
    /// with the fourth root of the discharge.
    pub incision_m: f64,
    /// Floodplain width at a source (m), growing with the square root of
    /// the discharge; the channel is `channel_ratio` of it.
    pub floodplain_m: f64,
    pub channel_ratio: f64,
    /// Channel depth at a source (m), growing with the fourth root.
    pub depth_m: f64,
    /// Valley wall slope (rise over run).
    pub wall: f64,
    /// Shoulder rounding between the valleys and the terrain (m).
    pub rounding_m: f64,
    /// How much the drainage wears the terrain ([`erode`]; 0: not at all).
    pub erosion: f64,
    /// Levels of streams under the maps' texels (`crate::streams`; 0:
    /// rivers only).
    pub streams: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MapSpec {
    pub climate: ClimateSpec,
    pub rivers: Option<RiverSpec>,
    pub seed: u32,
}

impl MapSpec {
    /// Whether the world bakes maps.
    pub fn baked(&self) -> bool {
        self.climate.maps || self.rivers.is_some()
    }
}

/// What a bake made (reports, tests, the `planet_map` example).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MapStats {
    pub texels: usize,
    pub ocean: usize,
    pub rivers: usize,
    pub lakes: usize,
    pub sinks: usize,
    /// Segments whose walls had to steepen past the most a record holds:
    /// their valleys may end in a step at their reach.
    pub clipped: usize,
    /// Largest discharge, in multiples of a source's.
    pub largest: f64,
    pub bake_ms: f64,
    /// Of which: coarse heights, climate, erosion, drainage, river
    /// segments and the table.
    pub phase_ms: [f64; 5],
    pub table_bytes: usize,
    /// Erosion: the deepest cut, the mean change over land and the most
    /// sediment (m), and the land texels sediment raised.
    pub worn_m: [f64; 3],
    pub sediment: usize,
    /// Whether these maps were loaded from the disk cache (the bake's
    /// times are the original bake's).
    pub loaded: bool,
}

/// A world's baked maps: the header rows and the GPU table.
#[derive(Debug)]
pub struct PlanetMaps {
    pub header: [[i32; 4]; MAP_HEADER],
    pub tables: Vec<u32>,
    pub stats: MapStats,
    /// The most the rivers and erosion move a height (mm: the deepest cut
    /// under the terrain's lowest ground, incision and channel, the worn
    /// terrain's cuts and sediment) and the steepest they slope (height
    /// units per domain unit, Q8: walls, beds and channels, cuts and fill
    /// levels together).
    valleys: [i32; 2],
    /// The texel grid, every texel's drainage (`streams` refine it) and
    /// whether the world has a sea.
    grid: CubeGrid,
    drains: Vec<Drain>,
    ocean: bool,
    /// The rivers the maps were baked with (the streams refine them).
    rivers: Option<RiverSpec>,
}

/// A texel's drainage: its downstream texel (`u32::MAX`: an outlet), its
/// bed at its node (mm), the wet area it drains (km²) and its `DRAINS_*`
/// flags.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Drain {
    pub down: u32,
    pub bed: i32,
    pub discharge: f32,
    pub flags: u8,
}

impl PlanetMaps {
    pub(crate) fn cube(&self) -> &CubeGrid {
        &self.grid
    }
    pub(crate) fn drains(&self) -> &[Drain] {
        &self.drains
    }
    /// Whether the world has a sea (land under it is sea floor).
    pub fn ocean(&self) -> bool {
        self.ocean
    }
    /// The rivers the maps were baked with.
    pub fn rivers(&self) -> Option<&RiverSpec> {
        self.rivers.as_ref()
    }

    /// See [`Self::valleys`]: the Rivers layer's bounds.
    pub fn valley_bounds(&self) -> (i32, i32) {
        (self.valleys[0], self.valleys[1])
    }

    /// Every river segment's node (domain point), floodplain width (m) and
    /// whether its texel is a lake (reports, tests).
    pub fn river_nodes(&self) -> Vec<(IVec3, f64, bool)> {
        self.tables[self.header[1][2] as usize..]
            .chunks_exact(RECORD_WORDS)
            .map(|r| {
                (
                    IVec3::new(r[0] as i32, r[1] as i32, r[2] as i32),
                    f64::from((r[3] >> 10) & 2047) * 16.0,
                    r[3] & RECORD_LAKE != 0,
                )
            })
            .collect()
    }
}

/// The header and table a lookup reads (a world without maps has a header
/// of uniform climate and an empty table).
#[derive(Clone, Copy)]
pub struct MapView<'a> {
    pub header: &'a [[i32; 4]; MAP_HEADER],
    pub tables: &'a [u32],
    /// The refined drainage (`None`: rivers only).
    pub streams: Option<&'a crate::streams::Streams>,
}

/// The uniform climate rows of a world without maps (and the climate rows
/// every world has).
pub fn climate_header(c: &ClimateSpec) -> [[i32; 4]; MAP_HEADER] {
    let lat = c.latitude_deg.clamp(-90.0, 90.0) * std::f64::consts::PI / 180.0;
    let s = sin_poly(lat).abs();
    let t = temperature(c, s);
    let m = (c.wetness * precipitation(s)).clamp(0.0, 1.0);
    // At least 1/8 °C per km when it cools at all: snowlines stay within
    // 32-bit heights.
    let lapse = if c.lapse_c_per_km > 0.0 {
        ((c.lapse_c_per_km * f64::from(DEGREE)).round() as i32).clamp(16, 1 << 16)
    } else {
        0
    };
    let mut h = [[0; 4]; MAP_HEADER];
    h[2] = [
        0,
        (t * f64::from(DEGREE)).round() as i32,
        (m * f64::from(WET)).round() as i32,
        lapse,
    ];
    h[3] = [
        (c.snow_c * f64::from(DEGREE)).round() as i32,
        if lapse > 0 { 1_000_000 / lapse } else { 0 },
        0,
        0,
    ];
    h
}

/// `sin x` for `|x| <= pi/2` by its Taylor series (basic operations only,
/// so every machine agrees).
fn sin_poly(x: f64) -> f64 {
    let x2 = x * x;
    let mut term = x;
    let mut sum = x;
    for n in 1..10 {
        term *= -x2 / f64::from((2 * n) * (2 * n + 1));
        sum += term;
    }
    sum
}

fn cos_poly(x: f64) -> f64 {
    (1.0 - sin_poly(x) * sin_poly(x)).max(0.0).sqrt()
}

/// Piecewise linear table lookup at `x`.
fn table(points: &[(f64, f64)], x: f64) -> f64 {
    let x = x.clamp(points[0].0, points[points.len() - 1].0);
    for w in points.windows(2) {
        let ((x0, y0), (x1, y1)) = (w[0], w[1]);
        if x <= x1 {
            return y0 + (y1 - y0) * (x - x0) / (x1 - x0).max(1e-12);
        }
    }
    points[points.len() - 1].1
}

/// Sea-level annual mean temperature (°C) at `|sin latitude|` `s` (Earth's
/// zonal means, between the equator's and the poles').
fn temperature(c: &ClimateSpec, s: f64) -> f64 {
    const WARMTH: [(f64, f64); 6] = [
        (0.0, 1.0),
        (0.5, 0.865),
        (0.707, 0.71),
        (0.866, 0.52),
        (0.966, 0.25),
        (1.0, 0.0),
    ];
    c.pole_c + (c.equator_c - c.pole_c) * table(&WARMTH, s)
}

/// The rain belts (0..1) at `|sin latitude|` `s`: the equator's, the dry
/// subtropics, the mid-latitudes' storm tracks, the dry poles.
fn precipitation(s: f64) -> f64 {
    const RAIN: [(f64, f64); 9] = [
        (0.0, 1.0),
        (0.17, 0.95),
        (0.34, 0.5),
        (0.5, 0.32),
        (0.64, 0.62),
        (0.77, 0.8),
        (0.87, 0.65),
        (0.97, 0.35),
        (1.0, 0.25),
    ];
    table(&RAIN, s)
}

/// The prevailing zonal wind at `|sin latitude|` `s`: -1 easterlies (trades
/// and polar), +1 westerlies.
fn zonal_wind(s: f64) -> f64 {
    const WIND: [(f64, f64); 6] = [
        (0.0, -1.0),
        (0.4, -1.0),
        (0.6, 1.0),
        (0.8, 1.0),
        (0.92, -1.0),
        (1.0, -1.0),
    ];
    table(&WIND, s)
}

/// The texel grid of a map: six gnomonic cube faces of `n` texels a side,
/// or one square over a plane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CubeGrid {
    pub(crate) n: i32,
    /// Plane half width (domain units); 0 on spheres.
    half: f64,
    /// Sphere radius (m) and domain radius (domain units).
    radius_m: f64,
    radius_du: f64,
}

impl CubeGrid {
    pub(crate) fn is_plane(&self) -> bool {
        self.half > 0.0
    }
    /// Domain units per metre (a sphere's domain radius is its grid's, not
    /// exactly the planet's radius over `DOMAIN_UNIT`).
    pub(crate) fn du_per_m(&self) -> f64 {
        if self.half > 0.0 {
            DU
        } else {
            self.radius_du / self.radius_m
        }
    }
    /// A texel's side at face centres (domain units).
    pub(crate) fn texel_du(&self) -> f64 {
        if self.half > 0.0 {
            2.0 * self.half / f64::from(self.n)
        } else {
            2.0 * self.radius_du / f64::from(self.n)
        }
    }
    fn slots(&self) -> usize {
        if self.half > 0.0 {
            1
        } else {
            6
        }
    }
    fn texels(&self) -> usize {
        self.slots() * (self.n * self.n) as usize
    }
    pub(crate) fn id(&self, slot: usize, x: i32, y: i32) -> usize {
        slot * (self.n * self.n) as usize + (y * self.n + x) as usize
    }
    pub(crate) fn at(&self, id: usize) -> (usize, i32, i32) {
        let per = (self.n * self.n) as usize;
        let r = (id % per) as i32;
        (id / per, r % self.n, r / self.n)
    }
    fn basis(&self, slot: usize) -> [DVec3; 3] {
        let face = if self.half > 0.0 {
            PLANE_FACE
        } else {
            slot as u8
        };
        FACE_BASIS[face as usize].map(|v| IVec3::from_array(v).as_dvec3())
    }
    /// Face coordinates `(u, v)` in `[-1, 1]` of continuous texel coordinates.
    fn uv(&self, x: f64, y: f64) -> (f64, f64) {
        let n = f64::from(self.n);
        (2.0 * x / n - 1.0, 2.0 * y / n - 1.0)
    }
    /// Unit direction (sphere) or domain position (plane) at continuous
    /// texel coordinates, which may lie past the face (its tangent plane).
    pub(crate) fn local(&self, slot: usize, x: f64, y: f64) -> DVec3 {
        let [n, a, b] = self.basis(slot);
        let (u, v) = self.uv(x, y);
        if self.half > 0.0 {
            (a * u + b * v) * self.half
        } else {
            (n + a * u + b * v).normalize()
        }
    }
    /// Position in metres (distances between texels).
    pub(crate) fn metres(&self, slot: usize, x: f64, y: f64) -> DVec3 {
        let p = self.local(slot, x, y);
        if self.half > 0.0 {
            p * DOMAIN_UNIT
        } else {
            p * self.radius_m
        }
    }
    /// Domain point (domain units, rounded).
    pub(crate) fn point(&self, slot: usize, x: f64, y: f64) -> IVec3 {
        let p = self.local(slot, x, y);
        let p = if self.half > 0.0 {
            p
        } else {
            p * self.radius_du
        };
        IVec3::new(p.x.round() as i32, p.y.round() as i32, p.z.round() as i32)
    }
    /// The slot and continuous texel coordinates of a direction (sphere) or
    /// domain position (plane), clamped to the face.
    pub(crate) fn locate(&self, p: DVec3) -> (usize, f64, f64) {
        let slot = if self.half > 0.0 {
            0
        } else {
            face_of(p) as usize
        };
        let [n, a, b] = self.basis(slot);
        let (u, v) = if self.half > 0.0 {
            (p.dot(a) / self.half, p.dot(b) / self.half)
        } else {
            let pn = p.dot(n);
            (p.dot(a) / pn, p.dot(b) / pn)
        };
        let s = f64::from(self.n);
        let c = |u: f64| ((u.clamp(-1.0, 1.0) + 1.0) * 0.5 * s).clamp(0.0, s - 1e-9);
        (slot, c(u), c(v))
    }
    /// The real texel holding texel `(x, y)` of `slot`, which may lie in the
    /// ring past the face: on a sphere the neighbouring face's texel under
    /// its centre, on a plane the nearest edge texel (`None` past a plane's
    /// edge when `edge` is false).
    fn real(&self, slot: usize, x: i32, y: i32, edge: bool) -> Option<usize> {
        if (0..self.n).contains(&x) && (0..self.n).contains(&y) {
            return Some(self.id(slot, x, y));
        }
        if self.half > 0.0 {
            return edge.then(|| self.id(slot, x.clamp(0, self.n - 1), y.clamp(0, self.n - 1)));
        }
        let (s, fx, fy) = self.locate(self.local(slot, f64::from(x) + 0.5, f64::from(y) + 0.5));
        Some(self.id(s, fx as i32, fy as i32))
    }
    /// A texel's eight neighbours (fewer at cube corners and plane edges)
    /// into `out`; returns how many.
    fn neighbours(&self, id: usize, out: &mut [usize; 8]) -> usize {
        let (slot, x, y) = self.at(id);
        let mut count = 0;
        for (dx, dy) in [
            (-1, -1),
            (0, -1),
            (1, -1),
            (-1, 0),
            (1, 0),
            (-1, 1),
            (0, 1),
            (1, 1),
        ] {
            if let Some(n) = self.real(slot, x + dx, y + dy, false) {
                if n != id && !out[..count].contains(&n) {
                    out[count] = n;
                    count += 1;
                }
            }
        }
        count
    }
    /// Whether a texel lies on a plane's edge.
    fn on_edge(&self, id: usize) -> bool {
        let (_, x, y) = self.at(id);
        self.half > 0.0 && (x == 0 || y == 0 || x == self.n - 1 || y == self.n - 1)
    }
    /// Area (km²) of a texel.
    pub(crate) fn area_km2(&self, id: usize) -> f64 {
        let (slot, x, y) = self.at(id);
        let (x, y) = (f64::from(x), f64::from(y));
        let a = self.metres(slot, x + 1.0, y + 0.5) - self.metres(slot, x, y + 0.5);
        let b = self.metres(slot, x + 0.5, y + 1.0) - self.metres(slot, x + 0.5, y);
        a.cross(b).length() * 1e-6
    }
    /// The shorter of a texel's two sides (m).
    pub(crate) fn side_m(&self, id: usize) -> f64 {
        let (slot, x, y) = self.at(id);
        let (x, y) = (f64::from(x), f64::from(y));
        let a = (self.metres(slot, x + 1.0, y + 0.5) - self.metres(slot, x, y + 0.5)).length();
        let b = (self.metres(slot, x + 0.5, y + 1.0) - self.metres(slot, x + 0.5, y)).length();
        a.min(b)
    }
}

/// The grid of a world's maps with `texel_km` texels (at face centres).
fn cube_grid(grid: &Grid, texel_km: f64, divisor: i32) -> CubeGrid {
    let texel_m = (texel_km * 1000.0).max(1.0);
    let slots = if grid.is_plane() { 1.0 } else { 6.0 };
    let most = (f64::from(MAX_MAP_TEXELS) / slots).sqrt();
    let even = |n: f64| ((n.min(most) / 2.0).round() as i32 * 2).clamp(MIN_TEXELS, MAX_TEXELS);
    if grid.is_plane() {
        let last = grid.cells() - 1;
        let half = [
            grid.domain_point(PLANE_FACE, 0, 0, 0),
            grid.domain_point(PLANE_FACE, last, last, 0),
        ]
        .iter()
        .flat_map(|p| [p.x.unsigned_abs(), p.z.unsigned_abs()])
        .max()
        .unwrap_or(1) as f64
            + 16.0;
        let n = even(2.0 * half * DOMAIN_UNIT / texel_m) / divisor;
        CubeGrid {
            n,
            half,
            radius_m: 0.0,
            radius_du: 0.0,
        }
    } else {
        let n = even(2.0 * grid.radius() / texel_m) / divisor;
        CubeGrid {
            n,
            half: 0.0,
            radius_m: grid.radius(),
            radius_du: f64::from(grid.sphere_constants()[2]),
        }
    }
}

/// Map lookup coordinates of domain point `p`: its slot (face) and
/// continuous texel coordinates on the river grid of `n` texels (Q16).
/// Integer arithmetic only (`map_coords` in WGSL): the face is the largest
/// component's, `u = (a . p) / (n . p)` through a Q30 reciprocal.
pub fn coords(header: &[[i32; 4]; MAP_HEADER], p: IVec3) -> (u32, i32, i32) {
    let a = p.abs();
    let face = if a.x >= a.y && a.x >= a.z {
        u32::from(p.x < 0)
    } else if a.y >= a.z {
        2 + u32::from(p.y < 0)
    } else {
        4 + u32::from(p.z < 0)
    };
    coords_on(header, p, face)
}

/// The face whose normal is `normal` (a signed axis; `map_face_of_normal`
/// in WGSL).
pub fn face_of_normal(normal: IVec3) -> u32 {
    if normal.x != 0 {
        u32::from(normal.x < 0)
    } else if normal.y != 0 {
        2 + u32::from(normal.y < 0)
    } else {
        4 + u32::from(normal.z < 0)
    }
}

/// [`coords`] of `p` on face `face` (a sphere's; a plane's is its one
/// square), which may lie past the face's edges (its tangent plane).
pub fn coords_on(header: &[[i32; 4]; MAP_HEADER], p: IVec3, face: u32) -> (u32, i32, i32) {
    let half = header[2][0];
    let (slot, pn, pa, pb) = if half > 0 {
        (0u32, half, p.x, p.z.wrapping_neg())
    } else {
        let [n, u, v] = FACE_BASIS[face as usize].map(IVec3::from_array);
        (face, n.dot(p), u.dot(p), v.dot(p))
    };
    let pn = pn.max(1) as u32;
    let bits = 32 - pn.leading_zeros();
    let norm = |x: u32| {
        let x = x.min(pn);
        if bits > 30 {
            x >> (bits - 30)
        } else {
            x << (30 - bits)
        }
    };
    let r = recip_q30(norm(pn));
    let n = header[0][1] as u32;
    let texel = |c: i32| {
        let u = mul_shr(norm(c.unsigned_abs()), r, 30).min(Q30) as i32;
        let u = if c < 0 { -u } else { u };
        mul_shr(u.wrapping_add(Q30 as i32) as u32, n << 15, 30) as i32
    };
    (slot, texel(pa), texel(pb))
}

/// Climate at domain point `p` and height `height` (mm): temperature
/// (1/128 °C, the sea level's less the lapse rate) and moisture (Q16),
/// bilinear between climate texels (`terrain_climate` in WGSL).
pub fn climate(m: MapView, p: IVec3, height: i32) -> (i32, i32) {
    let h = m.header;
    let fall = height.div_euclid(1000).wrapping_mul(h[2][3]) / 1000;
    if h[0][0] & FLAG_CLIMATE == 0 {
        return (h[2][1].wrapping_sub(fall), h[2][2]);
    }
    let (slot, xq, yq) = coords(h, p);
    let (nc, sc) = (h[0][3], h[1][3]);
    let axis = |q: i32| {
        let f = (q >> 1) - 32_768;
        let i = (f >> 16).clamp(-1, nc - 1);
        (i, ((f - (i << 16)).clamp(0, 65_536)) >> 8)
    };
    let ((ix, wx), (iy, wy)) = (axis(xq), axis(yq));
    let at = h[1][1] as usize + slot as usize * (sc * sc) as usize;
    let word = |x: i32, y: i32| m.tables[at + ((y + 1) * sc + x + 1) as usize];
    let lerp = |a: i32, b: i32, w: i32| a + (((b - a) * w) >> 8);
    let channel = |f: &dyn Fn(u32) -> i32| {
        let a = lerp(f(word(ix, iy)), f(word(ix + 1, iy)), wx);
        let b = lerp(f(word(ix, iy + 1)), f(word(ix + 1, iy + 1)), wx);
        lerp(a, b, wy)
    };
    let t = channel(&|w| w as u16 as i16 as i32);
    let wet = channel(&|w| (w >> 16) as i32);
    (t.wrapping_sub(fall), wet)
}

/// A vertex grid (`n + 1` vertices a face side, starting at `at`) at
/// domain point `p`, each word's two signed halves in `units` (mm),
/// bilinear. Faces share their edges' vertices (equal values: the bake
/// gives each shared vertex one), so it is continuous across face edges.
fn vertex_pair(m: MapView, p: IVec3, at: i32, units: [i32; 2]) -> [i32; 2] {
    let h = m.header;
    let (slot, xq, yq) = coords(h, p);
    let (n, v) = (h[0][1], h[0][1] + 1);
    let axis = |q: i32| {
        let i = (q >> 16).clamp(0, n - 1);
        (i, (q - (i << 16)).clamp(0, 65_536))
    };
    let ((ix, wx), (iy, wy)) = (axis(xq), axis(yq));
    let base = at as usize + slot as usize * (v * v) as usize + (iy * v + ix) as usize;
    let words = [
        m.tables[base],
        m.tables[base + 1],
        m.tables[base + v as usize],
        m.tables[base + v as usize + 1],
    ];
    let mut out = [0i32; 2];
    for (half, unit) in units.iter().enumerate() {
        let [a, b, c, d] = words.map(|w| (w >> (16 * half)) as u16 as i16 as i32 * unit);
        let top = a + mul_shr_signed(b - a, wx, 16);
        let bottom = c + mul_shr_signed(d - c, wx, 16);
        out[half] = top + mul_shr_signed(bottom - top, wy, 16);
    }
    out
}

/// The worn terrain at domain point `p` (mm): how far the drainage cut the
/// terrain down, and the level its sediment fills up to (`i32::MIN`:
/// none); the field's ground is `max(ground + cut, fill)`, rounded
/// (`terrain_erosion` in WGSL). Bilinear between the vertices of the map's
/// texels: continuous everywhere, across face edges too.
pub fn erosion(m: MapView, p: IVec3) -> (i32, i32) {
    if m.header[0][0] & FLAG_EROSION == 0 {
        return (0, i32::MIN);
    }
    let [cut, fill] = vertex_pair(m, p, m.header[4][0], [CUT_UNIT, FILL_UNIT]);
    (cut.min(0), fill)
}

/// The drainage at domain point `p` (`terrain_drainage` in WGSL): the wet
/// area draining through it (`log2(1 + km²)` in [`DISCHARGE_ONE`] units,
/// bilinear between texels), its texel's flags (`DRAINS_*`), and the
/// nearest river or lake within its valley's reach: the distance to its
/// axis (domain units, `i32::MAX`: none) and its bed there (mm).
pub fn drainage(m: MapView, p: IVec3) -> [i32; 4] {
    let h = m.header;
    if h[0][0] & FLAG_DRAINAGE == 0 {
        return [0, 0, i32::MAX, 0];
    }
    let (slot, xq, yq) = coords(h, p);
    let (n, s) = (h[0][1], h[0][2]);
    let axis = |q: i32| {
        let f = q - 32_768;
        let i = (f >> 16).clamp(-1, n - 1);
        (i, ((f - (i << 16)).clamp(0, 65_536)) >> 8)
    };
    let ((ix, wx), (iy, wy)) = (axis(xq), axis(yq));
    let at = h[4][1] as usize + slot as usize * (s * s) as usize;
    let word = |x: i32, y: i32| m.tables[at + ((y + GHOST) * s + x + GHOST) as usize];
    let log = |x: i32, y: i32| (word(x, y) & 0xffff) as i32;
    let lerp = |a: i32, b: i32, w: i32| a + (((b - a) * w) >> 8);
    let a = lerp(log(ix, iy), log(ix + 1, iy), wx);
    let b = lerp(log(ix, iy + 1), log(ix + 1, iy + 1), wx);
    let flags = (word((xq >> 16).min(n - 1), (yq >> 16).min(n - 1)) >> 16) as i32;
    let (dist, bed) = nearest_river(m, p).unwrap_or((i32::MAX, 0));
    [lerp(a, b, wy), flags, dist, bed]
}

/// Drainage flags ([`drainage`]'s second word).
pub const DRAINS_LAKE: i32 = 1;
pub const DRAINS_OCEAN: i32 = 2;
pub const DRAINS_RIVER: i32 = 4;
pub const DRAINS_SINK: i32 = 8;

/// The valley wall's rise (mm) `x` domain units past a floodplain's edge,
/// at `wall` (Q6) rise over run: its foot curves over half the
/// floodplain's width (at most 409 m), so the slope does not jump.
pub(crate) fn wall_rise(x: i32, floodplain: i32, wall: i32) -> i32 {
    let w = (floodplain / 2).clamp(1, 32_767);
    if x < w {
        mul_shr_signed(mul_shr_signed(x, wall * 25, 7), (x << 15) / w, 16)
    } else {
        mul_shr_signed(x - w / 2, wall * 25, 7)
    }
}

/// Meanders: noise fields bend a river, of wavelengths `2^MEANDER_SHIFTS`
/// domain units (13 km, 3.3 km and 820 m), each by at most `MEANDER_MAX`
/// (840 m, 410 m and 100 m; gradients stay low enough that courses never
/// fold). The first swings the whole valley by a third of its floodplain's
/// half-width; the others swing the channel within the floodplain by up to
/// one and a half of its half-width, at a wavelength near a dozen of its
/// widths (large channels the second field, small the third, blended
/// between 32 and 96 m half-widths).
const MEANDER_SHIFTS: [u32; 3] = [20, 18, 16];
const MEANDER_MAX: [i32; 3] = [67_200, 32_768, 8_192];
const MEANDER_SEEDS: [u32; 9] = [
    0x2f6b_1c43,
    0x7a1d_93e5,
    0x51c8_e0a7,
    0x4d9e_83b1,
    0x16c2_a5f7,
    0x63f0_4e29,
    0x0b4e_7f29,
    0x6c93_2d51,
    0x3e58_a6bd,
];

/// The bend fields at `p` (Q16 noise), each a vector.
pub(crate) fn bends(p: IVec3, seed: u32) -> [IVec3; 3] {
    let field = |o: usize| {
        IVec3::new(
            noise(p, MEANDER_SHIFTS[o], seed ^ MEANDER_SEEDS[3 * o]),
            noise(p, MEANDER_SHIFTS[o], seed ^ MEANDER_SEEDS[3 * o + 1]),
            noise(p, MEANDER_SHIFTS[o], seed ^ MEANDER_SEEDS[3 * o + 2]),
        )
    };
    [field(0), field(1), field(2)]
}

/// A segment's meander amplitudes (domain units) from its floodplain and
/// channel half-widths (domain units): the valley's swing, then the
/// channel's on the large and the small field. The channel stays on the
/// floodplain: its swing is at most 9/16 of the room there (a swing's
/// length is at most sqrt(3) times its largest component).
pub(crate) fn meander_amplitudes(floodplain: i32, channel: i32) -> [i32; 3] {
    let room = (floodplain - channel).max(0) * 9 / 16;
    let w = ((channel - 2_560) * 256 / 5_120).clamp(0, 256);
    let swing = (channel * 3 / 2).min(room);
    [
        (floodplain / 3).min(MEANDER_MAX[0]),
        ((swing * w) >> 8).min(MEANDER_MAX[1]),
        ((swing * (256 - w)) >> 8).min(MEANDER_MAX[2]),
    ]
}

/// `p` swung by field `bend` scaled to `a` (per component).
pub(crate) fn swing(p: IVec3, bend: IVec3, a: i32) -> IVec3 {
    p + IVec3::new(scale(bend.x, a), scale(bend.y, a), scale(bend.z, a))
}

/// The distance from `p` to the segment from `node` along unit `e` (Q30)
/// for `length` domain units, and the position of its foot along it.
pub(crate) fn axis(p: IVec3, node: IVec3, e: IVec3, length: i32) -> (i32, i32) {
    let d = p - node;
    let along =
        mul_shr_signed(d.x, e.x, 30) + mul_shr_signed(d.y, e.y, 30) + mul_shr_signed(d.z, e.z, 30);
    let a = along.clamp(0, length.max(0));
    let w = d - IVec3::new(
        mul_shr_signed(a, e.x, 30),
        mul_shr_signed(a, e.y, 30),
        mul_shr_signed(a, e.z, 30),
    );
    let n = unit_q30(w);
    let dist = (mul_shr_signed(w.x, n.x, 30)
        + mul_shr_signed(w.y, n.y, 30)
        + mul_shr_signed(w.z, n.z, 30))
    .max(0);
    (dist, a)
}

/// One river segment at `p`: its carving surface (the bed, interpolated
/// along the segment, plus the valley walls past the floodplain, less the
/// channel where the level's cells resolve it), whether `p` lies in the
/// channel there, its distance to the (meandering) axis and the bed.
struct SegmentAt {
    z: i32,
    inside: bool,
    dist: i32,
    bed: i32,
}

fn segment(r: &[u32], p: IVec3, bends: [IVec3; 3], cell: i32) -> SegmentAt {
    let node = IVec3::new(r[0] as i32, r[1] as i32, r[2] as i32);
    let geometry = r[3];
    let wall = ((geometry >> 2) & 255) as i32;
    let floodplain = ((geometry >> 10) & 2047) as i32 * 640;
    let channel = ((geometry >> 21) & 2047) as i32 * 40;
    let e = IVec3::new(
        (r[4] as u16 as i16 as i32) << 15,
        ((r[4] >> 16) as u16 as i16 as i32) << 15,
        (r[5] as u16 as i16 as i32) << 15,
    );
    let length = ((r[5] >> 16) as i32) << 6;
    // Meanders (`MEANDER_SHIFTS`): the valley swings, its channel swings on
    // its floodplain.
    let m = meander_amplitudes(floodplain, channel);
    let valley = swing(p, bends[0], m[0]);
    let (dist, a) = axis(valley, node, e, length);
    let t = if length > 0 {
        ((a << 8) / (length >> 8).max(1)).min(65_536)
    } else {
        0
    };
    let bed = (r[6] as i32) - mul_shr_signed((r[7] & 0x3f_ffff) as i32, t, 16);
    let mut z = bed + wall_rise((dist - floodplain).max(0), floodplain, wall);
    let mut inside = false;
    let flow = if dist < floodplain && channel > 0 {
        axis(
            swing(swing(valley, bends[1], m[1]), bends[2], m[2]),
            node,
            e,
            length,
        )
        .0
    } else {
        i32::MAX
    };
    if flow < channel {
        let q = (flow << 14) / channel;
        let depth = (r[7] >> 22) as i32 * 100;
        let cut = depth - mul_shr_signed(depth, (q * q) >> 14, 14);
        // The channel shows where it spans four of the level's cells, and
        // fades out by two: coarser levels keep its floodplain.
        let resolved = (((channel << 7) / cell) - 256).clamp(0, 256);
        z -= (cut * resolved) >> 8;
        inside = resolved > 0;
    }
    SegmentAt {
        z,
        inside,
        dist,
        bed,
    }
}

/// Height `h` (mm) of the column at domain point `p` (reference level
/// `level`) carved by the rivers: the smooth minimum of the terrain and
/// every segment's surface within reach (`map_carve` in WGSL); and whether
/// the column lies in a channel whose bed is its ground.
pub fn carve(m: MapView, p: IVec3, level: u32, h: i32) -> (i32, bool) {
    let header = m.header;
    if header[0][0] & FLAG_RIVERS == 0 {
        return (h, false);
    }
    let (slot, xq, yq) = coords(header, p);
    let (n, s) = (header[0][1], header[0][2]);
    let (ix, iy) = ((xq >> 16).min(n - 1), (yq >> 16).min(n - 1));
    let index = header[1][0] as usize + slot as usize * (s * s) as usize;
    let cell = 8i32 << level.min(24);
    let mut best = i32::MAX;
    let mut inside = false;
    let mut bent: Option<[IVec3; 3]> = None;
    for dy in -GHOST..=GHOST {
        for dx in -GHOST..=GHOST {
            let (tx, ty) = (ix + dx, iy + dy);
            let word = m.tables[index + ((ty + GHOST) * s + tx + GHOST) as usize];
            if word == 0 {
                continue;
            }
            let reach = ((word >> 24) as i32) << 12;
            if (xq - ((tx << 16) + 32_768)).abs() > reach
                || (yq - ((ty << 16) + 32_768)).abs() > reach
            {
                continue;
            }
            let at = header[1][2] as usize + ((word & 0xff_ffff) - 1) as usize * RECORD_WORDS;
            let b = *bent.get_or_insert_with(|| bends(p, header[3][3] as u32));
            let s = segment(&m.tables[at..at + RECORD_WORDS], p, b, cell);
            best = best.min(s.z);
            inside |= s.inside && s.z <= h;
        }
    }
    if best == i32::MAX {
        return (h, false);
    }
    (smooth_min(h, best, header[3][2]), inside)
}

/// The river or lake segment nearest domain point `p` among those whose
/// valleys reach it: its distance to the axis (domain units) and its bed
/// there (mm).
fn nearest_river(m: MapView, p: IVec3) -> Option<(i32, i32)> {
    let header = m.header;
    if header[0][0] & FLAG_RIVERS == 0 {
        return None;
    }
    let (slot, xq, yq) = coords(header, p);
    let (n, s) = (header[0][1], header[0][2]);
    let (ix, iy) = ((xq >> 16).min(n - 1), (yq >> 16).min(n - 1));
    let index = header[1][0] as usize + slot as usize * (s * s) as usize;
    let mut best: Option<(i32, i32)> = None;
    let mut bent: Option<[IVec3; 3]> = None;
    for dy in -GHOST..=GHOST {
        for dx in -GHOST..=GHOST {
            let (tx, ty) = (ix + dx, iy + dy);
            let word = m.tables[index + ((ty + GHOST) * s + tx + GHOST) as usize];
            if word == 0 {
                continue;
            }
            let reach = ((word >> 24) as i32) << 12;
            if (xq - ((tx << 16) + 32_768)).abs() > reach
                || (yq - ((ty << 16) + 32_768)).abs() > reach
            {
                continue;
            }
            let at = header[1][2] as usize + ((word & 0xff_ffff) - 1) as usize * RECORD_WORDS;
            let b = *bent.get_or_insert_with(|| bends(p, header[3][3] as u32));
            let seg = segment(&m.tables[at..at + RECORD_WORDS], p, b, 1 << 24);
            if best.map_or(true, |(d, _)| seg.dist < d) {
                best = Some((seg.dist, seg.bed));
            }
        }
    }
    best
}

/// Process-wide bakes, by world: planets of the same world share them.
fn cache() -> &'static Mutex<Vec<(u64, Weak<PlanetMaps>)>> {
    static CACHE: OnceLock<Mutex<Vec<(u64, Weak<PlanetMaps>)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Vec::new()))
}

/// The maps of `spec` over the terrain `k` (its rivers not yet carved) on
/// `grid`, baked once per process and world, and kept on disk
/// ([`set_disk_cache`]): later runs load them.
pub fn maps(spec: &MapSpec, k: &LandformConstants, grid: &Grid) -> Arc<PlanetMaps> {
    let key = fingerprint(spec, k, grid);
    let mut cache = cache().lock().unwrap_or_else(|e| e.into_inner());
    cache.retain(|(_, w)| w.strong_count() > 0);
    if let Some(maps) = cache
        .iter()
        .find(|(k, _)| *k == key)
        .and_then(|(_, w)| w.upgrade())
    {
        return maps;
    }
    let path = disk_dir().map(|dir| dir.join(format!("{key:016x}.maps")));
    let loaded = path
        .as_ref()
        .and_then(|p| load(p, key))
        .map(|m| PlanetMaps {
            rivers: spec.rivers.clone(),
            ..m
        });
    let maps = Arc::new(loaded.unwrap_or_else(|| {
        let maps = bake(spec, k, grid);
        if let Some(p) = &path {
            // A failed write only costs the next run a bake.
            let _ = store(p, key, &maps);
        }
        maps
    }));
    cache.push((key, Arc::downgrade(&maps)));
    maps
}

/// What a world's maps depend on: its spec, terrain and grid, and the
/// code that bakes them (a changed bake never loads an old file).
pub fn fingerprint(spec: &MapSpec, k: &LandformConstants, grid: &Grid) -> u64 {
    const SOURCES: [&str; 4] = [
        include_str!("planet_maps.rs"),
        include_str!("landform.rs"),
        include_str!("noise.rs"),
        include_str!("grid.rs"),
    ];
    let mut bytes = format!("{spec:?}").into_bytes();
    bytes.extend_from_slice(bytemuck::bytes_of(k));
    bytes.extend_from_slice(format!("{grid:?}").as_bytes());
    for source in SOURCES {
        bytes.extend_from_slice(&crate::record::fnv64(source.as_bytes()).to_le_bytes());
    }
    crate::record::fnv64(&bytes)
}

/// Where bakes are kept: `HELIO_PLANET_MAPS_CACHE` (a directory; empty
/// turns the disk cache off), else the user's cache directory.
fn disk_dir() -> Option<std::path::PathBuf> {
    let configured = DISK
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    if let Some(dir) = configured {
        return dir;
    }
    if let Some(dir) = std::env::var_os("HELIO_PLANET_MAPS_CACHE") {
        return (!dir.is_empty()).then(|| dir.into());
    }
    let env = |key: &str| {
        std::env::var_os(key)
            .filter(|v| !v.is_empty())
            .map(std::path::PathBuf::from)
    };
    let base = if cfg!(windows) {
        env("LOCALAPPDATA")
    } else if cfg!(target_os = "macos") {
        env("HOME").map(|h| h.join("Library/Caches"))
    } else {
        env("XDG_CACHE_HOME").or_else(|| env("HOME").map(|h| h.join(".cache")))
    };
    base.map(|b| b.join("helio").join("planet-maps"))
}

static DISK: OnceLock<Mutex<Option<Option<std::path::PathBuf>>>> = OnceLock::new();

/// Keep bakes in `dir` (`None`: never on disk), over the environment's
/// and the default.
pub fn set_disk_cache(dir: Option<std::path::PathBuf>) {
    *DISK
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(dir);
}

const FILE_MAGIC: &[u8; 8] = b"HPMAPS03";

/// A bake as a file: magic, key, header, bounds, stats, table, checksum.
fn encode(key: u64, maps: &PlanetMaps) -> Vec<u8> {
    let mut out = Vec::with_capacity(maps.tables.len() * 4 + 256);
    out.extend_from_slice(FILE_MAGIC);
    out.extend_from_slice(&key.to_le_bytes());
    out.extend_from_slice(bytemuck::cast_slice(&maps.header));
    out.extend_from_slice(bytemuck::cast_slice(&maps.valleys));
    let s = &maps.stats;
    for v in [
        s.texels,
        s.ocean,
        s.rivers,
        s.lakes,
        s.sinks,
        s.clipped,
        s.table_bytes,
        s.sediment,
    ] {
        out.extend_from_slice(&(v as u64).to_le_bytes());
    }
    for v in [s.largest, s.bake_ms]
        .iter()
        .chain(&s.phase_ms)
        .chain(&s.worn_m)
    {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&(maps.tables.len() as u64).to_le_bytes());
    out.extend_from_slice(bytemuck::cast_slice(&maps.tables));
    let g = &maps.grid;
    out.extend_from_slice(&(g.n as u64).to_le_bytes());
    for v in [g.half, g.radius_m, g.radius_du] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.push(u8::from(maps.ocean));
    out.extend_from_slice(&(maps.drains.len() as u64).to_le_bytes());
    for d in &maps.drains {
        out.extend_from_slice(&d.down.to_le_bytes());
        out.extend_from_slice(&d.bed.to_le_bytes());
        out.extend_from_slice(&d.discharge.to_le_bytes());
        out.push(d.flags);
    }
    let sum = crate::record::fnv64(&out);
    out.extend_from_slice(&sum.to_le_bytes());
    out
}

fn decode(bytes: &[u8], key: u64) -> Option<PlanetMaps> {
    let (body, sum) = bytes.split_at_checked(bytes.len().checked_sub(8)?)?;
    if crate::record::fnv64(body).to_le_bytes() != sum || !body.starts_with(FILE_MAGIC) {
        return None;
    }
    let mut at = FILE_MAGIC.len();
    let mut take = |n: usize| -> Option<&[u8]> {
        let s = body.get(at..at + n)?;
        at += n;
        Some(s)
    };
    let word = |s: &[u8]| u64::from_le_bytes(s.try_into().unwrap());
    if word(take(8)?) != key {
        return None;
    }
    let mut header = [[0i32; 4]; MAP_HEADER];
    bytemuck::cast_slice_mut::<_, u8>(&mut header).copy_from_slice(take(MAP_HEADER * 16)?);
    let mut valleys = [0i32; 2];
    bytemuck::cast_slice_mut::<_, u8>(&mut valleys).copy_from_slice(take(8)?);
    let mut counts = [0usize; 8];
    for c in &mut counts {
        *c = word(take(8)?) as usize;
    }
    let mut reals = [0f64; 10];
    for r in &mut reals {
        *r = f64::from_le_bytes(take(8)?.try_into().unwrap());
    }
    let words = word(take(8)?) as usize;
    let mut tables = vec![0u32; words];
    bytemuck::cast_slice_mut::<_, u8>(&mut tables).copy_from_slice(take(words.checked_mul(4)?)?);
    let [texels, ocean, rivers, lakes, sinks, clipped, table_bytes, sediment] = counts;
    let n = word(take(8)?) as i32;
    let mut reals3 = [0f64; 3];
    for r in &mut reals3 {
        *r = f64::from_le_bytes(take(8)?.try_into().unwrap());
    }
    let sea = take(1)?[0] != 0;
    let count = word(take(8)?) as usize;
    let mut drains = Vec::with_capacity(count.min(1 << 24));
    for _ in 0..count {
        let d = take(13)?;
        drains.push(Drain {
            down: u32::from_le_bytes(d[0..4].try_into().unwrap()),
            bed: i32::from_le_bytes(d[4..8].try_into().unwrap()),
            discharge: f32::from_le_bytes(d[8..12].try_into().unwrap()),
            flags: d[12],
        });
    }
    Some(PlanetMaps {
        header,
        tables,
        stats: MapStats {
            texels,
            ocean,
            rivers,
            lakes,
            sinks,
            clipped,
            largest: reals[0],
            bake_ms: reals[1],
            phase_ms: [reals[2], reals[3], reals[4], reals[5], reals[6]],
            table_bytes,
            worn_m: [reals[7], reals[8], reals[9]],
            sediment,
            loaded: true,
        },
        valleys,
        grid: CubeGrid {
            n,
            half: reals3[0],
            radius_m: reals3[1],
            radius_du: reals3[2],
        },
        drains,
        ocean: sea,
        rivers: None,
    })
}

fn load(path: &std::path::Path, key: u64) -> Option<PlanetMaps> {
    decode(&std::fs::read(path).ok()?, key)
}

fn store(path: &std::path::Path, key: u64, maps: &PlanetMaps) -> std::io::Result<()> {
    let dir = path.parent().expect("a cache file's directory");
    std::fs::create_dir_all(dir)?;
    // Written whole, then renamed: readers never see half a file.
    let partial = path.with_extension(format!("{}.partial", std::process::id()));
    std::fs::write(&partial, encode(key, maps))?;
    std::fs::rename(&partial, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&partial);
    })?;
    // The newest few only: every changed world or bake writes a new file.
    let mut files: Vec<(std::time::SystemTime, std::path::PathBuf)> = std::fs::read_dir(dir)?
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "maps"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, old) in files.into_iter().skip(DISK_KEEP) {
        let _ = std::fs::remove_file(old);
    }
    Ok(())
}

/// Bakes the disk cache keeps (the most recently written).
const DISK_KEEP: usize = 8;

/// Bake the maps (see the module doc).
pub fn bake(spec: &MapSpec, k: &LandformConstants, grid: &Grid) -> PlanetMaps {
    let started = std::time::Instant::now();
    let c = &spec.climate;
    let g = cube_grid(grid, c.texel_km, 1);
    let cg = CubeGrid { n: g.n / 2, ..g };
    let seed = spec.seed;

    // Coarse heights: the mean (and highest) of 2x2 samples a texel, at a
    // level resolving wavelengths of half a texel.
    let texel_du = if g.half > 0.0 {
        2.0 * g.half / f64::from(g.n)
    } else {
        2.0 * g.radius_du / f64::from(g.n)
    };
    let level = (64 - ((texel_du / 2.0) as u64).max(1).leading_zeros()).saturating_sub(6);
    let field = landform::SingleRate::new(k, level);
    let heights: Vec<(i32, i32)> = (0..g.texels())
        .into_par_iter()
        .map(|id| {
            let (slot, x, y) = g.at(id);
            let (mut sum, mut top) = (0i64, i32::MIN);
            for (sx, sy) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
                let p = g.point(slot, f64::from(x) + sx, f64::from(y) + sy);
                let h = field.height(k, p);
                sum += i64::from(h);
                top = top.max(h);
            }
            ((sum / 4) as i32, top)
        })
        .collect();
    let mut phases = [started.elapsed().as_secs_f64() * 1e3; 5];
    let raw: Vec<i32> = heights.iter().map(|h| h.0).collect();
    let ocean: Vec<bool> = raw.iter().map(|&h| c.ocean && h < 0).collect();
    let around = |id: usize| {
        let mut out = [0usize; 8];
        let n = g.neighbours(id, &mut out);
        (out, n)
    };

    // Climate, on a grid of half the resolution.
    let child = |id: usize| {
        let (slot, x, y) = cg.at(id);
        [(0, 0), (1, 0), (0, 1), (1, 1)].map(|(dx, dy)| g.id(slot, 2 * x + dx, 2 * y + dy))
    };
    let c_height: Vec<f64> = (0..cg.texels())
        .map(|id| child(id).iter().map(|&t| f64::from(raw[t])).sum::<f64>() / 4000.0)
        .collect();
    let c_ocean: Vec<bool> = (0..cg.texels())
        .map(|id| child(id).iter().filter(|&&t| ocean[t]).count() >= 2)
        .collect();
    let pole = c.pole.normalize_or(DVec3::Y);
    let (lat0, north) = (
        c.latitude_deg.clamp(-90.0, 90.0) * std::f64::consts::PI / 180.0,
        -DVec3::Z,
    );
    // Sine of the latitude and the eastward direction at a point.
    let frame = |p: DVec3| -> (f64, DVec3) {
        if g.half > 0.0 {
            let metres = p.dot(north) * DOMAIN_UNIT;
            let s = (sin_poly(lat0) + metres / 6_371_000.0 * cos_poly(lat0)).clamp(-1.0, 1.0);
            (s, DVec3::X)
        } else {
            let east = pole.cross(p);
            let east = if east.length_squared() < 1e-12 {
                p.any_orthonormal_vector()
            } else {
                east.normalize()
            };
            (p.dot(pole).clamp(-1.0, 1.0), east)
        }
    };
    let step_m = cg.side_m(cg.id(0, cg.n / 2, cg.n / 2)).max(1.0);
    let reach_m = (c.continentality_km * 3_000.0).max(step_m);
    // Distance to the sea any way (m).
    let coast = distances(&cg, &c_ocean);
    let moisture_shift = 64
        - ((c.continentality_km * 500.0 * DU) as u64)
            .max(2)
            .leading_zeros()
        - 1;
    let c_climate: Vec<(f64, f64)> = (0..cg.texels())
        .into_par_iter()
        .map(|id| {
            let (slot, x, y) = cg.at(id);
            let (fx, fy) = (f64::from(x) + 0.5, f64::from(y) + 0.5);
            let here = cg.local(slot, fx, fy);
            let (s, _) = frame(here);
            let point = cg.point(slot, fx, fy);
            let wander = |salt: u32| {
                f64::from(noise_fine(point, moisture_shift, seed ^ salt)) / f64::from(FINE_ONE)
            };
            let t = temperature(c, s.abs()) + 1.5 * wander(0x2c1b_3c6d);
            let base = precipitation(s.abs());
            if c_ocean[id] {
                return (t, base);
            }
            // Upwind: march against the prevailing wind to the sea.
            let zonal = zonal_wind(s.abs());
            let (mut d_up, mut barrier) = (reach_m, 0.0f64);
            let mut p = here;
            let mut walked = 0.0;
            while walked < reach_m {
                let (_, e) = frame(p);
                let back = -e * zonal.signum();
                p = if g.half > 0.0 {
                    p + back * (step_m * DU)
                } else {
                    (p + back * (step_m / g.radius_m)).normalize()
                };
                walked += step_m;
                let (ts, tx, ty) = cg.locate(p);
                let t_id = cg.id(ts, tx as i32, ty as i32);
                if c_ocean[t_id] {
                    d_up = walked;
                    break;
                }
                barrier = barrier.max(c_height[t_id]);
            }
            let local = c_height[id].max(0.0);
            let l = c.continentality_km.max(1.0) * 1000.0;
            let decay = |d: f64| 1.0 / (1.0 + (d / l) * (d / l));
            let lift = (barrier - local).max(0.0) / c.rain_shadow_m.max(1.0);
            let shadow = 1.0 / (1.0 + lift * lift);
            let up = 0.25 + 0.75 * decay(d_up) * shadow;
            let any = 0.25 + 0.75 * decay(coast[id]);
            let wind = zonal.abs();
            let wet = base * (wind * up + (1.0 - wind) * any.max(up));
            (t, (wet * (1.0 + 0.3 * wander(0x51ed_270b))).clamp(0.0, 1.0))
        })
        .collect();
    // Smoothed: neighbouring texels' upwind marches find their sea apart.
    let mut moist: Vec<f64> = c_climate.iter().map(|c| c.1).collect();
    for _ in 0..3 {
        moist = (0..cg.texels())
            .into_par_iter()
            .map(|id| {
                let mut out = [0usize; 8];
                let count = cg.neighbours(id, &mut out);
                (moist[id] + out[..count].iter().map(|&n| moist[n]).sum::<f64>())
                    / (1 + count) as f64
            })
            .collect();
    }
    let wet_at = |id: usize| {
        let (slot, x, y) = g.at(id);
        (moist[cg.id(slot, x / 2, y / 2)] * c.wetness).clamp(0.0, 1.0)
    };

    phases[1] = started.elapsed().as_secs_f64() * 1e3;
    let mut stats = MapStats {
        texels: g.texels(),
        ocean: ocean.iter().filter(|&&o| o).count(),
        ..MapStats::default()
    };
    let wet: Vec<f64> = (0..g.texels()).map(wet_at).collect();
    let centres: Vec<DVec3> = (0..g.texels())
        .map(|id| {
            let (slot, x, y) = g.at(id);
            g.metres(slot, f64::from(x) + 0.5, f64::from(y) + 0.5)
        })
        .collect();
    let areas: Vec<f64> = (0..g.texels()).map(|id| g.area_km2(id)).collect();

    // Erosion: the drainage wears the coarse heights (`erode`); the
    // drainage below is the worn terrain's.
    // It runs on the climate's grid (half the resolution: a quarter of
    // the texels), and its change is interpolated back.
    let strength = spec.rivers.as_ref().map_or(0.0, |r| r.erosion);
    let worn = if strength > 0.0 {
        let raw_c: Vec<i32> = (0..cg.texels())
            .map(|id| (child(id).iter().map(|&t| i64::from(raw[t])).sum::<i64>() / 4) as i32)
            .collect();
        let wet_c: Vec<f64> = moist
            .iter()
            .map(|m| (m * c.wetness).clamp(0.0, 1.0))
            .collect();
        let centres_c: Vec<DVec3> = (0..cg.texels())
            .map(|id| {
                let (slot, x, y) = cg.at(id);
                cg.metres(slot, f64::from(x) + 0.5, f64::from(y) + 0.5)
            })
            .collect();
        let areas_c: Vec<f64> = (0..cg.texels()).map(|id| cg.area_km2(id)).collect();
        let eroded = erode(
            &cg, &raw_c, &c_ocean, &wet_c, &centres_c, &areas_c, strength,
        );
        let delta: Vec<f64> = eroded
            .iter()
            .zip(&raw_c)
            .map(|(e, r)| f64::from(e - r))
            .collect();
        (0..g.texels())
            .into_par_iter()
            .map(|id| {
                let (slot, x, y) = g.at(id);
                let axis = |v: i32| {
                    let u = f64::from(v) * 0.5 - 0.25;
                    let i = (u.floor() as i32).clamp(0, cg.n - 2);
                    (i, (u - f64::from(i)).clamp(0.0, 1.0))
                };
                let ((i, fx), (j, fy)) = (axis(x), axis(y));
                let d = |a: i32, b: i32| delta[cg.id(slot, a, b)];
                let top = d(i, j) + (d(i + 1, j) - d(i, j)) * fx;
                let bottom = d(i, j + 1) + (d(i + 1, j + 1) - d(i, j + 1)) * fx;
                raw[id].saturating_add((top + (bottom - top) * fy).round() as i32)
            })
            .collect()
    } else {
        raw.clone()
    };
    let raw0 = raw;
    let raw = worn;
    phases[2] = started.elapsed().as_secs_f64() * 1e3;
    // The terrain's relief finer than the maps resolve, over any texel.
    let fine = fine_relief(k, level);

    // Drainage: filled from the sea, the plane's edges and dry basins.
    let Routing {
        filled,
        down,
        order,
        seed_set,
    } = route(&g, &Neighbours::new(&g), &raw, &ocean, &wet, &centres);
    stats.sinks = (0..g.texels())
        .filter(|&id| seed_set[id] && !ocean[id] && !g.on_edge(id))
        .count();
    let mut discharge: Vec<f64> = (0..g.texels()).map(|id| areas[id] * wet[id]).collect();
    for &id in &order {
        if let Some(d) = down[id] {
            discharge[d] += discharge[id];
        }
    }

    // The worn terrain as the field applies it (`erosion`, quantized as
    // the table keeps it): cut where the drainage wore it down, and where
    // sediment settled a fill level the ground under it rises to;
    // elsewhere a level far under any ground, so sediment's margins ramp
    // down over a texel (fans and aprons at the foot of the ranges).
    //
    // Sediment `d` thick on average fills the hollows first: over ground
    // spread evenly within the unresolved relief `R` of the texel's mean,
    // it rises to `R` under the mean plus `2 sqrt(d R)` (all of it buried
    // past `d = R`).
    let relief = fine * 1000.0;
    let under = relief as i32 + 50_000;
    let worn: Vec<(i32, i32)> = (0..g.texels())
        .map(|id| {
            let delta = raw[id] - raw0[id];
            let d = f64::from(delta);
            let fill = if delta <= 500 {
                raw[id].saturating_sub(under)
            } else if d < relief {
                raw0[id] + (2.0 * (d * relief).sqrt() - relief).round() as i32
            } else {
                raw[id]
            };
            (
                (delta.min(0) / CUT_UNIT).max(-16_383) * CUT_UNIT,
                (fill / FILL_UNIT).clamp(-16_383, 16_383) * FILL_UNIT,
            )
        })
        .collect();
    if strength > 0.0 {
        let land = (0..g.texels()).filter(|&id| !ocean[id]);
        stats.worn_m = [
            f64::from(worn.iter().map(|w| w.0).min().unwrap_or(0)) / 1000.0,
            land.clone()
                .map(|id| f64::from(raw[id] - raw0[id]))
                .sum::<f64>()
                / 1000.0
                / land.clone().count().max(1) as f64,
            f64::from(land.clone().map(|id| raw[id] - raw0[id]).max().unwrap_or(0)) / 1000.0,
        ];
        stats.sediment = land.filter(|&id| raw[id] - raw0[id] > 500).count();
    }
    // The worn terrain's bounds: how far it moves a height (the deepest
    // cut, the most sediment over the lowest ground under it) and how
    // steeply cuts and fill levels slope (a B-spline's gradient stays under
    // its steepest step between texels).
    let (mut moved, mut steepest) = (0.0f64, 0.0f64);
    if strength > 0.0 {
        let texel_du = (0..g.texels())
            .map(|id| g.side_m(id))
            .fold(f64::INFINITY, f64::min)
            * DU;
        let near = |id: usize| {
            let (n, count) = around(id);
            std::iter::once(id).chain(n.into_iter().take(count))
        };
        for id in 0..g.texels() {
            let (cut, fill) = worn[id];
            let fill_near = near(id).map(|t| worn[t].1).max().unwrap_or(fill);
            let lowest = f64::from(raw0[id] + cut) - fine * 1000.0;
            moved = moved
                .max(f64::from(-cut))
                .max(f64::from(fill_near) - lowest);
            let step = |f: fn(&(i32, i32)) -> i32| {
                near(id)
                    .map(|t| (f(&worn[t]) - f(&worn[id])).abs())
                    .max()
                    .unwrap_or(0)
            };
            steepest = steepest.max(f64::from(step(|w| w.0) + step(|w| w.1)) / texel_du);
        }
        moved += f64::from(SEDIMENT_ROUNDING);
    }

    phases[3] = started.elapsed().as_secs_f64() * 1e3;
    // River segments.
    let rivers = spec.rivers.as_ref();
    let source = rivers.map_or(f64::INFINITY, |r| r.source_km2.max(1.0));
    let lake: Vec<bool> = (0..g.texels())
        .map(|id| !seed_set[id] && filled[id] - raw[id] >= LAKE_DEPTH)
        .collect();
    let river = |id: usize| rivers.is_some() && !ocean[id] && (discharge[id] >= source || lake[id]);
    // Incision grows with the discharge's fourth root, under a source as
    // well (the streams' stems).
    let incision = |id: usize| match rivers {
        Some(r) => r.incision_m * (discharge[id] / source).max(1e-6).sqrt().sqrt(),
        None => 0.0,
    };
    // Base levels: rivers cut no lower than where they end (two metres
    // over the sea's surface, a dry basin's floor).
    let mut base = vec![0i32; g.texels()];
    for &id in order.iter().rev() {
        base[id] = match down[id] {
            Some(d) => base[d],
            // A river's mouth stays a little over the sea.
            None if ocean[id] => 2_000,
            None => raw[id],
        };
    }
    let mut bed: Vec<i32> = (0..g.texels())
        .map(|id| {
            if ocean[id] {
                0
            } else if lake[id] {
                filled[id]
            } else {
                (filled[id] - (incision(id) * 1000.0).round() as i32).max(base[id])
            }
        })
        .collect();
    for &id in order.iter().rev() {
        if let Some(d) = down[id] {
            bed[id] = bed[id].max(bed[d]);
        }
    }
    let node = |id: usize| -> (DVec3, IVec3) {
        let (slot, x, y) = g.at(id);
        let (fx, fy) = texel_jitter(seed, slot, x, y);
        (g.metres(slot, fx, fy), texel_node(&g, seed, slot, x, y))
    };
    let rounding = rivers.map_or(0.0, |r| r.rounding_m.max(0.0));
    let mut records: Vec<u32> = Vec::new();
    let mut index = vec![0u32; g.texels()];
    let (mut cut, mut slope) = (0.0f64, 0.0f64);
    for id in 0..g.texels() {
        if !river(id) {
            continue;
        }
        let r = rivers.expect("rivers");
        let q = (discharge[id] / source).max(1.0);
        let (at_m, at) = node(id);
        let (to_m, to, bed_b) = match down[id] {
            Some(d) => {
                let (m, p) = node(d);
                (m, p, bed[d])
            }
            None => (at_m, at, bed[id]),
        };
        let span = (to - at).as_dvec3();
        let length = span.length();
        let e = if length > 0.0 {
            span / length
        } else {
            DVec3::ZERO
        };
        let side = g.side_m(id);
        let limit = 0.8 * side;
        let mut floodplain = (r.floodplain_m * q.sqrt()).min(limit * 0.5);
        if lake[id] {
            floodplain = floodplain.max(0.35 * side).min(limit * 0.5);
        }
        let channel = (floodplain * r.channel_ratio).min(1_000.0);
        let depth = (r.depth_m * q.sqrt().sqrt()).min(100.0);
        // The highest ground the valley may meet: around both ends.
        let top = [id]
            .iter()
            .chain(down[id].iter())
            .flat_map(|&t| {
                let (n, count) = around(t);
                std::iter::once(t).chain(n.into_iter().take(count))
            })
            .map(|t| heights[t].1.max(worn[t].1))
            .max()
            .unwrap_or(raw[id]);
        let rise = f64::from(top - bed_b.min(bed[id])) / 1000.0 + fine + rounding;
        // The walls' foot curves over half the floodplain (`wall_rise`).
        let foot = (floodplain / 2.0).min(32_767.0 * DOMAIN_UNIT) / 2.0;
        let mut wall = r.wall.max(0.01);
        // Meanders move the valley by up to a third of the floodplain.
        let bent = floodplain / 3.0 + 60.0;
        if floodplain + foot + bent + rise / wall > limit {
            wall = rise / (limit - floodplain - foot - bent).max(1.0);
        }
        let wall_q6 = (wall * 64.0).ceil() as u32;
        if wall_q6 > 255 {
            stats.clipped += 1;
        }
        let [m0, ..] = meander_amplitudes(
            ((floodplain / 8.0).round().min(2047.0) * 640.0) as i32,
            ((channel * 2.0).round().min(2047.0) * 40.0) as i32,
        );
        // The valley's swing (its length: up to sqrt(3) a component's);
        // the channel's stays on the floodplain.
        let bend = 1.75 * f64::from(m0) * DOMAIN_UNIT;
        let reach = floodplain + foot + bend + rise / (f64::from(wall_q6.min(255)) / 64.0);
        let cull =
            ((((to_m - at_m).length() + reach + 0.4 * side) / side + 0.5) * 16.0).ceil() as u32;
        let q15 = |v: f64| ((v * 32_767.0).round() as i32 as u16) as u32;
        records.extend_from_slice(&[
            at.x as u32,
            at.y as u32,
            at.z as u32,
            u32::from(lake[id]) * RECORD_LAKE
                | wall_q6.min(255) << 2
                | ((floodplain / 8.0).round() as u32).min(2047) << 10
                | ((channel * 2.0).round() as u32).min(2047) << 21,
            q15(e.x) | q15(e.y) << 16,
            q15(e.z) | ((length / 64.0).round() as u32).min(65_535) << 16,
            bed[id] as u32,
            ((bed[id] - bed_b).clamp(0, 0x3f_ffff) as u32)
                | ((depth * 10.0).round() as u32).min(1023) << 22,
        ]);
        let n = records.len() / RECORD_WORDS;
        index[id] = n as u32 | cull.min(255) << 24;
        cut = cut.max(incision(id) + depth);
        let drop = f64::from(bed[id] - bed_b) / length.max(64.0);
        slope = slope.max(
            (f64::from(wall_q6.min(255)) / 64.0 * DOMAIN_UNIT * 1000.0
                + drop
                + depth * 1000.0 / (channel * DU).max(40.0))
                * 1.7,
        );
        stats.rivers += usize::from(!lake[id]);
        stats.lakes += usize::from(lake[id]);
        stats.largest = stats.largest.max(q);
    }

    // Moisture along the rivers (riparian ground), at the climate texels
    // they cross.
    let mut c_wet = moist.clone();
    let boosted = c_wet.len();
    for id in 0..g.texels() {
        if index[id] != 0 && !lake[id] {
            let (slot, x, y) = g.at(id);
            let q = (discharge[id] / source).max(1.0) as u64;
            let boost = (0.04 * f64::from(1 + q.ilog2())).min(0.3);
            let t = cg.id(slot, x / 2, y / 2);
            c_wet[t] = (c_wet[t] + boost).min(1.0);
        }
    }

    // Smoothed like the climate's moisture.
    for _ in 0..2 {
        c_wet = (0..boosted)
            .into_par_iter()
            .map(|id| {
                let mut out = [0usize; 8];
                let count = cg.neighbours(id, &mut out);
                (c_wet[id] + out[..count].iter().map(|&n| c_wet[n]).sum::<f64>())
                    / (1 + count) as f64
            })
            .collect();
    }

    // The table: index (with ghost rings), climate (with its ring), records.
    let s = g.n + 2 * GHOST;
    let sc = cg.n + 2 * CLIMATE_GHOST;
    let pad = |n: usize| n.next_multiple_of(4);
    let index_at = 0usize;
    let grid_words = g.slots() * (s * s) as usize;
    let v = g.n + 1;
    let erosion_at = pad(index_at + grid_words);
    let drainage_at = pad(erosion_at + g.slots() * (v * v) as usize);
    let climate_at = pad(drainage_at + grid_words);
    let records_at = pad(climate_at + g.slots() * (sc * sc) as usize);
    let mut tables = vec![0u32; records_at];
    let drained = |id: usize| {
        let log = ((1.0 + discharge[id]).log2() * f64::from(DISCHARGE_ONE))
            .round()
            .clamp(0.0, 65_535.0) as u32;
        log | u32::from(lake[id]) << 16
            | u32::from(ocean[id]) << 17
            | u32::from(index[id] != 0 && !lake[id]) << 18
            | u32::from(seed_set[id] && !ocean[id]) << 19
    };
    let mut shared = rustc_hash::FxHashMap::<IVec3, u32>::default();
    for slot in 0..g.slots() {
        for y in -GHOST..g.n + GHOST {
            for x in -GHOST..g.n + GHOST {
                if let Some(id) = g.real(slot, x, y, true) {
                    let at = slot * (s * s) as usize + ((y + GHOST) * s + x + GHOST) as usize;
                    tables[index_at + at] = index[id];
                    tables[drainage_at + at] = drained(id);
                }
            }
        }
        // Erosion vertices: each the mean of the four texels around it
        // (past a face's edge, the neighbouring face's), and every vertex
        // faces share one value (the first face's).
        for y in 0..=g.n {
            for x in 0..=g.n {
                let (mut cut, mut fill, mut count) = (0i64, 0i64, 0i64);
                for (dx, dy) in [(-1, -1), (0, -1), (-1, 0), (0, 0)] {
                    if let Some(id) = g.real(slot, x + dx, y + dy, true) {
                        cut += i64::from(worn[id].0);
                        fill += i64::from(worn[id].1);
                        count += 1;
                    }
                }
                let half = |sum: i64, unit: i32| {
                    ((sum as f64 / count.max(1) as f64 / f64::from(unit))
                        .round()
                        .clamp(-16_383.0, 16_383.0) as i16) as u16
                };
                let word = u32::from(half(cut, CUT_UNIT)) | u32::from(half(fill, FILL_UNIT)) << 16;
                let edge = x == 0 || y == 0 || x == g.n || y == g.n;
                let word = if edge && g.half == 0.0 {
                    *shared
                        .entry(g.point(slot, f64::from(x), f64::from(y)))
                        .or_insert(word)
                } else {
                    word
                };
                tables[erosion_at + slot * (v * v) as usize + (y * v + x) as usize] = word;
            }
        }
        for y in -CLIMATE_GHOST..cg.n + CLIMATE_GHOST {
            for x in -CLIMATE_GHOST..cg.n + CLIMATE_GHOST {
                if let Some(id) = cg.real(slot, x, y, true) {
                    let t = (c_climate[id].0 * f64::from(DEGREE))
                        .round()
                        .clamp(-32_768.0, 32_767.0) as i16;
                    let w =
                        ((c_wet[id] * c.wetness).clamp(0.0, 1.0) * f64::from(WET)).round() as u32;
                    tables[climate_at
                        + slot * (sc * sc) as usize
                        + ((y + CLIMATE_GHOST) * sc + x + CLIMATE_GHOST) as usize] =
                        u32::from(t as u16) | w << 16;
                }
            }
        }
    }
    tables.extend_from_slice(&records);
    let mut header = climate_header(c);
    header[0] = [
        if c.maps { FLAG_CLIMATE } else { 0 }
            | if rivers.is_some() { FLAG_RIVERS } else { 0 }
            | if strength > 0.0 { FLAG_EROSION } else { 0 }
            | FLAG_DRAINAGE,
        g.n,
        s,
        cg.n,
    ];
    header[1] = [index_at as i32, climate_at as i32, records_at as i32, sc];
    header[2][0] = g.half.round() as i32;
    header[3][2] = (rounding * 1000.0).round() as i32;
    header[3][3] = seed as i32;
    header[4] = [
        erosion_at as i32,
        drainage_at as i32,
        SEDIMENT_ROUNDING,
        (g.texel_du() * f64::from(crate::streams::REACH_Q8) / 256.0).round() as i32,
    ];
    let drains: Vec<Drain> = (0..g.texels())
        .map(|id| Drain {
            down: down[id].map_or(u32::MAX, |d| d as u32),
            bed: bed[id],
            discharge: discharge[id] as f32,
            flags: (drained(id) >> 16) as u8,
        })
        .collect();
    stats.table_bytes = tables.len() * 4;
    phases[4] = started.elapsed().as_secs_f64() * 1e3;
    stats.phase_ms = [
        phases[0],
        phases[1] - phases[0],
        phases[2] - phases[1],
        phases[3] - phases[2],
        phases[4] - phases[3],
    ];
    stats.bake_ms = started.elapsed().as_secs_f64() * 1e3;
    PlanetMaps {
        header,
        tables,
        stats,
        valleys: [
            ((cut + 1.0) * 1000.0 + moved)
                .ceil()
                .min(f64::from(i32::MAX / 2)) as i32,
            ((slope + steepest) * 256.0)
                .ceil()
                .min(f64::from(i32::MAX / 2)) as i32,
        ],
        grid: g,
        drains,
        ocean: c.ocean,
        rivers: spec.rivers.clone(),
    }
}

/// A texel's node: a jittered point of the middle half of the texel
/// (continuous texel coordinates).
fn texel_jitter(seed: u32, slot: usize, x: i32, y: i32) -> (f64, f64) {
    let j = |salt: u32| {
        f64::from(hash3(x, y, slot as i32, seed ^ salt) & 0xffff) / 65_536.0 * 0.5 - 0.25
    };
    (
        f64::from(x) + 0.5 + j(0x68e3_1da4),
        f64::from(y) + 0.5 + j(0x1b87_3593),
    )
}

/// A texel's node as a domain point, on a 16-unit lattice (segments meet
/// exactly): its rivers' and streams' ends.
pub(crate) fn texel_node(g: &CubeGrid, seed: u32, slot: usize, x: i32, y: i32) -> IVec3 {
    let (fx, fy) = texel_jitter(seed, slot, x, y);
    (g.point(slot, fx, fy) >> 4) << 4
}

/// Upper bound (m) on the relief the terrain adds below the maps' level:
/// every octave they do not resolve, at its largest.
fn fine_relief(k: &LandformConstants, level: u32) -> f64 {
    landform::unresolved_relief(k, level) / 1000.0 + 20.0
}

/// Every texel's neighbours (`CubeGrid::neighbours`), listed once.
struct Neighbours {
    list: Vec<[u32; 8]>,
    count: Vec<u8>,
}

impl Neighbours {
    fn new(g: &CubeGrid) -> Self {
        let (list, count) = (0..g.texels())
            .into_par_iter()
            .map(|id| {
                let mut out = [0usize; 8];
                let n = g.neighbours(id, &mut out);
                (out.map(|v| v as u32), n as u8)
            })
            .unzip();
        Self { list, count }
    }
    fn of(&self, id: usize) -> &[u32] {
        &self.list[id][..usize::from(self.count[id])]
    }
}

/// A drainage: heights filled from their outlets, each texel's steepest
/// lower neighbour (`None` at outlets), the texels from the highest filled
/// down, and the outlets.
struct Routing {
    filled: Vec<i32>,
    down: Vec<Option<usize>>,
    order: Vec<usize>,
    seed_set: Vec<bool>,
}

/// The drainage of heights `h` (mm): filled from the sea, the plane's
/// edges and dry basins' floors (priority flood, rising 1 mm a texel), each
/// texel draining to its steepest lower neighbour.
fn route(
    g: &CubeGrid,
    nb: &Neighbours,
    h: &[i32],
    ocean: &[bool],
    wet: &[f64],
    centres: &[DVec3],
) -> Routing {
    let key = |id: usize| (h[id], id);
    let mut seeds: Vec<usize> = (0..g.texels())
        .into_par_iter()
        .filter(|&id| {
            ocean[id]
                || g.on_edge(id)
                || (nb.of(id).iter().all(|&n| key(n as usize) > key(id)) && wet[id] < 0.22)
        })
        .collect();
    if seeds.is_empty() {
        seeds.push((0..g.texels()).min_by_key(|&id| key(id)).unwrap_or(0));
    }
    let mut filled = vec![i32::MIN; g.texels()];
    let mut heap = BinaryHeap::new();
    for &id in &seeds {
        filled[id] = h[id];
        heap.push(Reverse((h[id], id)));
    }
    while let Some(Reverse((level, id))) = heap.pop() {
        for &n in nb.of(id) {
            let n = n as usize;
            if filled[n] == i32::MIN {
                filled[n] = h[n].max(level.saturating_add(1));
                heap.push(Reverse((filled[n], n)));
            }
        }
    }
    let mut seed_set = vec![false; g.texels()];
    for &id in &seeds {
        seed_set[id] = true;
    }
    let down: Vec<Option<usize>> = (0..g.texels())
        .into_par_iter()
        .map(|id| {
            if seed_set[id] {
                return None;
            }
            let mut best: Option<(f64, usize)> = None;
            for &n in nb.of(id) {
                let n = n as usize;
                if filled[n] < filled[id] {
                    let slope = f64::from(filled[id] - filled[n])
                        / (centres[id] - centres[n]).length().max(1.0);
                    if best.map_or(true, |(s, b)| slope > s || (slope == s && n < b)) {
                        best = Some((slope, n));
                    }
                }
            }
            best.map(|b| b.1)
        })
        .collect();
    let mut order: Vec<usize> = (0..g.texels()).collect();
    order.par_sort_unstable_by_key(|&id| Reverse((filled[id], id)));
    Routing {
        filled,
        down,
        order,
        seed_set,
    }
}

/// Coarse erosion (`docs/planet-maps.md`, Erosion): for `ERODE_STEPS`
/// steps, the heights `raw` (mm) are routed and, from the sources down,
/// every texel's stream erodes toward what it can carry (its capacity: the
/// square root of its wet drainage area times its slope) or drops what it
/// cannot: lakes and dry basins fill with sediment into plains, fans
/// spread where the streams leave the ranges, the sea keeps the rest (up
/// to 10 m under its surface); then hillslopes creep toward their
/// neighbours. `strength` scales the capacity and creep (0: none). Returns
/// the worn heights (mm).
fn erode(
    g: &CubeGrid,
    raw: &[i32],
    ocean: &[bool],
    wet: &[f64],
    centres: &[DVec3],
    areas: &[f64],
    strength: f64,
) -> Vec<i32> {
    // Capacity per unit of sqrt(km²) times slope, as a height over the
    // texel (m); the share of a shortfall eroded and of an excess dropped
    // each step; creep toward the neighbours' mean each step.
    const CAPACITY: f64 = 30.0;
    const ERODE: f64 = 0.5;
    const DROP: f64 = 0.15;
    const CREEP: f64 = 0.05;
    let n = g.texels();
    let nb = Neighbours::new(g);
    let mut h: Vec<f64> = raw.iter().map(|&v| f64::from(v) / 1000.0).collect();
    let mm = |h: &[f64]| -> Vec<i32> { h.iter().map(|&v| (v * 1000.0).round() as i32).collect() };
    for _ in 0..ERODE_STEPS {
        let r = route(g, &nb, &mm(&h), ocean, wet, centres);
        let mut area: Vec<f64> = (0..n).map(|id| areas[id] * wet[id]).collect();
        for &id in &r.order {
            if let Some(d) = r.down[id] {
                area[d] += area[id];
            }
        }
        // Sediment arriving at each texel (m km²).
        let mut load = vec![0.0f64; n];
        for &id in &r.order {
            let a = areas[id];
            let mut q = load[id];
            if ocean[id] {
                let room = (-10.0 - h[id]).max(0.0) * a;
                h[id] += (q * 0.5).min(room) / a;
                continue;
            }
            // Lakes keep what reaches them, up to their surface.
            let surface = f64::from(r.filled[id]) / 1000.0;
            if surface > h[id] {
                let d = q.min((surface - h[id]) * a);
                h[id] += d / a;
                q -= d;
            }
            let Some(d) = r.down[id] else {
                // Dry basins keep it; the plane's edges let it go.
                if !g.on_edge(id) {
                    h[id] += q / a;
                }
                continue;
            };
            // Rivers reaching the sea wear down to its surface.
            let base = if ocean[d] { 0.0 } else { h[d] };
            let run = (centres[id] - centres[d]).length().max(1.0);
            let slope = ((h[id] - base) / run).max(0.0);
            let capacity = CAPACITY * strength * area[id].sqrt() * slope * a;
            if q < capacity {
                let room = ((h[id] - base) * 0.5).max(0.0) * a;
                let e = (ERODE * (capacity - q)).min(room);
                h[id] -= e / a;
                q += e;
            } else {
                let dropped = DROP * (q - capacity);
                h[id] += dropped / a;
                q -= dropped;
            }
            load[d] += q;
        }
        let creep = CREEP * strength.min(1.0);
        h = (0..n)
            .into_par_iter()
            .map(|id| {
                if ocean[id] {
                    return h[id];
                }
                let around = nb.of(id);
                let mean =
                    around.iter().map(|&t| h[t as usize]).sum::<f64>() / around.len().max(1) as f64;
                h[id] + creep * (mean - h[id])
            })
            .collect();
    }
    mm(&h)
}

/// Distance (m) from every texel to the nearest of `sources` along the
/// texel graph (Dijkstra; unreachable: `f64::INFINITY`).
fn distances(g: &CubeGrid, sources: &[bool]) -> Vec<f64> {
    let mut d = vec![u64::MAX; g.texels()];
    let mut heap = BinaryHeap::new();
    for id in 0..g.texels() {
        if sources[id] {
            d[id] = 0;
            heap.push(Reverse((0u64, id)));
        }
    }
    let centre = |id: usize| {
        let (slot, x, y) = g.at(id);
        g.metres(slot, f64::from(x) + 0.5, f64::from(y) + 0.5)
    };
    while let Some(Reverse((dist, id))) = heap.pop() {
        if dist > d[id] {
            continue;
        }
        let here = centre(id);
        let mut out = [0usize; 8];
        let count = g.neighbours(id, &mut out);
        for &n in &out[..count] {
            let next = dist + (centre(n) - here).length().round() as u64;
            if next < d[n] {
                d[n] = next;
                heap.push(Reverse((next, n)));
            }
        }
    }
    d.iter()
        .map(|&v| {
            if v == u64::MAX {
                f64::INFINITY
            } else {
                v as f64
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layers::TerrainLayers;

    fn earth_maps() -> (Grid, Arc<PlanetMaps>) {
        let grid = Grid::new(6_371_000.0, 0.1).unwrap();
        let field = TerrainLayers::earth().field(&grid, 7).unwrap();
        (grid, field.maps().expect("Earth bakes maps").clone())
    }

    /// Lookups land on the texel whose centre direction they came from,
    /// on every face, and agree with the bake's own (f64) inverse.
    #[test]
    fn lookups_find_their_texels() {
        let grid = Grid::new(6_371_000.0, 0.1).unwrap();
        let g = cube_grid(&grid, 25.0, 1);
        let mut header = [[0; 4]; MAP_HEADER];
        header[0][1] = g.n;
        for slot in 0..6 {
            for (x, y) in [(0, 0), (g.n - 1, 3), (g.n / 2, g.n / 3), (5, g.n - 1)] {
                let p = g.point(slot, f64::from(x) + 0.5, f64::from(y) + 0.5);
                let (s, xq, yq) = coords(&header, p);
                assert_eq!(s as usize, slot);
                assert!(
                    (xq - ((x << 16) + 32_768)).abs() < 64,
                    "{slot} {x} {y}: {xq}"
                );
                assert!(
                    (yq - ((y << 16) + 32_768)).abs() < 64,
                    "{slot} {x} {y}: {yq}"
                );
            }
        }
    }

    /// The same world bakes the same maps; another seed others.
    #[test]
    fn bakes_are_deterministic() {
        let grid = Grid::new(1_000_000.0, 0.1).unwrap();
        let stack = TerrainLayers::earth();
        let (k, _) = stack.compile(&grid, 3).unwrap();
        let spec = stack.map_spec(3).unwrap();
        let a = bake(&spec, &k, &grid);
        let b = bake(&spec, &k, &grid);
        assert_eq!(a.tables, b.tables);
        assert_eq!(a.header, b.header);
        let (k2, _) = stack.compile(&grid, 4).unwrap();
        let c = bake(&stack.map_spec(4).unwrap(), &k2, &grid);
        assert_ne!(a.tables, c.tables);
        eprintln!("{:?}", a.stats);
    }

    /// Earth's drainage wears its ranges down and fills basins with
    /// sediment, and the worn terrain it hands the field has no steps: not
    /// between texels, not across face edges (arcs over the cube's edges
    /// and corners, sampled every ~40 m).
    #[test]
    fn erosion_wears_ranges_and_fills_basins() {
        let (grid, maps) = earth_maps();
        let s = &maps.stats;
        eprintln!("erosion: deepest cut {:.0} m, mean change on land {:.1} m, most sediment {:.0} m, {} sediment texels of {}", s.worn_m[0], s.worn_m[1], s.worn_m[2], s.sediment, s.texels);
        assert!(s.worn_m[0] < -50.0, "{:?}", s.worn_m);
        assert!(s.worn_m[2] > 20.0, "{:?}", s.worn_m);
        assert!(s.sediment > s.texels / 500, "{}", s.sediment);
        let view = MapView {
            header: &maps.header,
            tables: &maps.tables,
            streams: None,
        };
        let r = f64::from(grid.sphere_constants()[2]);
        let point = |d: DVec3| {
            let p = d.normalize() * r;
            IVec3::new(p.x.round() as i32, p.y.round() as i32, p.z.round() as i32)
        };
        // Steps a sample apart: at most the fields' slope over it (B: a
        // bilinear cell's gradient stays under its steepest edge, ~1 m a
        // texel at worst here) plus rounding.
        let step = 40.0 / grid.radius();
        let (mut worst, mut samples) = ([0i32; 2], 0);
        for (from, to) in [
            (DVec3::new(1.0, 0.3, 0.2), DVec3::new(0.3, 1.0, -0.1)),
            (DVec3::new(1.0, 0.9, 0.95), DVec3::new(0.9, 1.0, 1.1)),
            (DVec3::new(-0.2, -1.0, 0.4), DVec3::new(0.1, -0.5, -1.0)),
            (DVec3::new(0.7, 0.2, -1.0), DVec3::new(-0.6, -0.1, -1.0)),
        ] {
            let (a, b) = (from.normalize(), to.normalize());
            let angle = a.angle_between(b);
            let axis = a.cross(b).normalize();
            let mut last: Option<(i32, i32)> = None;
            for t in 0..(angle / step) as usize {
                let d = glam::DQuat::from_axis_angle(axis, step * t as f64) * a;
                let e = erosion(view, point(d));
                if let Some(l) = last {
                    worst[0] = worst[0].max((e.0 - l.0).abs());
                    worst[1] = worst[1].max((e.1 - l.1).abs());
                }
                last = Some(e);
                samples += 1;
            }
        }
        eprintln!(
            "worst steps over {samples} samples 40 m apart: cut {} mm, fill {} mm",
            worst[0], worst[1]
        );
        assert!(worst[0] < 3_000 && worst[1] < 3_000, "{worst:?}");
    }

    /// A bake survives the disk cache's file whole, and a damaged or
    /// foreign file is refused.
    #[test]
    fn cache_files_load_the_bake() {
        let grid = Grid::new(600_000.0, 0.1).unwrap();
        let stack = TerrainLayers::earth();
        let (k, _) = stack.compile(&grid, 5).unwrap();
        let spec = stack.map_spec(5).unwrap();
        let maps = bake(&spec, &k, &grid);
        let key = fingerprint(&spec, &k, &grid);
        let mut bytes = encode(key, &maps);
        let back = decode(&bytes, key).expect("its own file loads");
        assert!(back.stats.loaded);
        assert_eq!(
            (back.header, &back.tables, back.valleys),
            (maps.header, &maps.tables, maps.valleys)
        );
        assert_eq!(
            (back.grid, &back.drains, back.ocean),
            (maps.grid, &maps.drains, maps.ocean)
        );
        assert_eq!(
            MapStats {
                loaded: false,
                ..back.stats
            },
            maps.stats
        );
        assert!(decode(&bytes, key ^ 1).is_none());
        let middle = bytes.len() / 2;
        bytes[middle] ^= 1;
        assert!(decode(&bytes, key).is_none());
        assert!(decode(&bytes[..10], key).is_none());
        assert_ne!(fingerprint(&stack.map_spec(6).unwrap(), &k, &grid), key);
    }

    /// The bake's working vectors a texel at their most, summed: heights,
    /// moisture, texel centres and areas (~70 bytes, climate texels a
    /// quarter as many), an erosion step's heights, routing (fill, flow
    /// directions and order, the heap's frontier), wet area and sediment
    /// (~85), or later the drainage, beds, records' index, drains and the
    /// table (~85).
    const BAKE_BYTES_PER_TEXEL: usize = 160;

    /// Bake time and memory by planet size (`--ignored --nocapture`): the
    /// Moon, Earth and twice Earth.
    #[test]
    #[ignore]
    fn bake_costs() {
        for radius in [1_737_000.0, 6_371_000.0, 12_742_000.0] {
            let grid = Grid::new(radius, 0.1).unwrap();
            let stack = TerrainLayers::earth();
            let (k, _) = stack.compile(&grid, 7).unwrap();
            let maps = bake(&stack.map_spec(7).unwrap(), &k, &grid);
            let s = &maps.stats;
            // What a later run pays instead: the cache file's decode.
            let bytes = encode(1, &maps);
            let loading = std::time::Instant::now();
            assert!(decode(&bytes, 1).is_some());
            eprintln!(
                "  cache file {:.1} MB, decoded in {:.0} ms",
                bytes.len() as f64 / 1e6,
                loading.elapsed().as_secs_f64() * 1e3
            );
            let texel_km = 2.0 * radius / 1000.0 / f64::from(maps.header[0][1]);
            eprintln!(
                "radius {:.0} km: {} texels ({texel_km:.1} km), bake {:.2} s (heights {:.2}, climate {:.2}, erosion {:.2}, drainage {:.2}, rivers and table {:.2}), table {:.1} MB, working set ~{:.0} MB",
                radius / 1000.0,
                s.texels,
                s.bake_ms / 1e3,
                s.phase_ms[0] / 1e3,
                s.phase_ms[1] / 1e3,
                s.phase_ms[2] / 1e3,
                s.phase_ms[3] / 1e3,
                s.phase_ms[4] / 1e3,
                s.table_bytes as f64 / 1e6,
                (s.texels * BAKE_BYTES_PER_TEXEL) as f64 / 1e6,
            );
        }
    }

    /// The base column under domain point `p` of a sphere.
    fn column_at(grid: &Grid, p: IVec3) -> crate::terrain::Column {
        let (cell, _) = grid.locate(p.as_dvec3().normalize() * grid.radius());
        crate::terrain::Column {
            face: cell.face,
            i: cell.i,
            j: cell.j,
            level: 0,
        }
    }

    /// The climate: tropics warmer than poles, heights colder, and both
    /// wet and dry land.
    #[test]
    fn climate_follows_latitude_and_height() {
        use crate::terrain::TerrainField;
        let grid = Grid::new(6_371_000.0, 0.1).unwrap();
        let field = TerrainLayers::earth().field(&grid, 7).unwrap();
        let r = f64::from(grid.sphere_constants()[2]);
        let at = |d: DVec3| {
            let p = d.normalize() * r;
            IVec3::new(p.x.round() as i32, p.y.round() as i32, p.z.round() as i32)
        };
        let pole = DVec3::new(0.0, 1.0, 1.0);
        let equator = DVec3::new(1.0, 0.0, 0.0);
        let (t_pole, _) = field.climate(at(pole), 0);
        let (t_equator, _) = field.climate(at(equator), 0);
        eprintln!(
            "equator {} C, pole {} C",
            t_equator / DEGREE,
            t_pole / DEGREE
        );
        assert!(t_equator > t_pole + 35 * DEGREE);
        let (t_high, _) = field.climate(at(equator), 3_000_000);
        assert!((t_equator - t_high - (6.5 * 3.0 * f64::from(DEGREE)) as i32).abs() < DEGREE);
        let (mut dry, mut wet) = (0, 0);
        let mut rng = 0x2545_F491_4F6C_DD1Du64;
        for _ in 0..20_000 {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            let d = DVec3::new(
                (rng & 0xffff) as f64 - 32_768.0,
                ((rng >> 16) & 0xffff) as f64 - 32_768.0,
                ((rng >> 32) & 0xffff) as f64 - 32_768.0,
            );
            let p = at(d);
            if field.height(column_at(&grid, p)) <= 0 {
                continue;
            }
            let (_, m) = field.climate(p, 0);
            dry += usize::from(m < WET * 3 / 10);
            wet += usize::from(m > WET * 6 / 10);
        }
        eprintln!("land samples: {dry} dry, {wet} wet");
        assert!(dry > 200 && wet > 200, "{dry} dry, {wet} wet");
    }

    /// Rivers cut valleys and channels, without steps: around river nodes,
    /// the carved field is lower than the field without rivers, has
    /// channels, and neighbouring base columns never step by a layer.
    #[test]
    fn rivers_carve_valleys_without_steps() {
        use crate::terrain::{Column, TerrainField};
        let grid = Grid::new(6_371_000.0, 0.1).unwrap();
        let stack = TerrainLayers::earth();
        let field = stack.field(&grid, 7).unwrap();
        let mut dry = stack.clone();
        dry.layers
            .retain(|l| l.kind != crate::layers::LayerKind::Rivers);
        let dry = dry.field(&grid, 7).unwrap();
        let nodes = field.maps().unwrap().river_nodes();
        let (mut cut, mut channels, mut worst, mut samples) = (0i64, 0usize, 0, 0usize);
        for &(p, _, _) in nodes.iter().step_by((nodes.len() / 150).max(1)) {
            let c = column_at(&grid, p);
            let at = |i: i32, j: i32| Column { i, j, ..c };
            cut += i64::from(dry.height(at(c.i, c.j)) - field.height(at(c.i, c.j)));
            // Lines of 600 columns through the node, both ways.
            for (di, dj) in [(1, 0), (0, 1)] {
                let h: Vec<i32> = (-300..300)
                    .map(|t| field.height(at(c.i + di * t, c.j + dj * t)))
                    .collect();
                for t in -300..300 {
                    let col = at(c.i + di * t, c.j + dj * t);
                    channels += usize::from(field.surface(col, 0) == crate::landform::RIVER_BED);
                }
                for w in h.windows(3) {
                    worst = worst.max((w[0] - 2 * w[1] + w[2]).abs());
                    samples += 1;
                }
            }
        }
        let mean_cut = cut as f64 / 150.0 / 1000.0;
        eprintln!("mean cut at nodes {mean_cut:.1} m, {channels} channel columns, worst second difference {worst} mm over {samples}");
        assert!(mean_cut > 5.0, "{mean_cut} m");
        assert!(channels > 1000, "{channels}");
        assert!(worst < 200, "{worst} mm");
    }

    /// Across cube edges the carved field is continuous: marching across
    /// a face edge near a river, level-4 columns never step.
    #[test]
    fn valleys_continue_across_face_edges() {
        use crate::terrain::{Column, TerrainField};
        let grid = Grid::new(6_371_000.0, 0.1).unwrap();
        let field = TerrainLayers::earth().field(&grid, 7).unwrap();
        let maps = field.maps().unwrap();
        let (header, r) = (&maps.header, grid.radius());
        let n = header[0][1];
        let mut marches = 0;
        let mut worst = 0;
        for (p, _, _) in maps.river_nodes() {
            let (slot, x, y) = coords(header, p);
            // Nodes within a texel and a half of an edge.
            let near = |q: i32| q < 3 << 15 || q > (n << 16) - (3 << 15);
            if !near(x) && !near(y) || marches >= 40 {
                continue;
            }
            let [nrm, a, b] = FACE_BASIS[slot as usize].map(|v| IVec3::from_array(v).as_dvec3());
            let towards = if near(x) {
                if x < 1 << 16 {
                    -a
                } else {
                    a
                }
            } else if y < 1 << 16 {
                -b
            } else {
                b
            };
            let start = p.as_dvec3().normalize() * r;
            let _ = nrm;
            let mut last: Option<(u8, i32, i32)> = None;
            let mut heights = Vec::new();
            for step in 0..40_000 {
                let q = (start + towards * (f64::from(step) * 1.5)).normalize() * r;
                let (cell, _) = grid.locate(q);
                let key = (cell.face, cell.i >> 4, cell.j >> 4);
                if last == Some(key) {
                    continue;
                }
                last = Some(key);
                heights.push(field.height(Column {
                    face: key.0,
                    i: key.1,
                    j: key.2,
                    level: 4,
                }));
            }
            for w in heights.windows(3) {
                worst = worst.max((w[0] - 2 * w[1] + w[2]).abs());
            }
            marches += 1;
        }
        eprintln!("{marches} marches across face edges, worst second difference {worst} mm");
        assert!(marches > 5);
        assert!(worst < 8_000, "{worst} mm");
    }

    /// Earth: oceans, a river network with lakes and sinks, climate from
    /// the tropics to the poles; reports the bake.
    #[test]
    fn earth_has_rivers_and_climates() {
        let (_, maps) = earth_maps();
        let s = &maps.stats;
        eprintln!("{s:?}");
        assert!(
            s.ocean * 4 > s.texels && s.ocean * 4 < s.texels * 3,
            "{s:?}"
        );
        assert!(s.rivers > 500, "{s:?}");
        assert!(s.largest > 50.0, "{s:?}");
        assert!(s.clipped * 100 <= s.rivers, "{s:?}");
    }
}
