//! Features: procedural content as part of the terrain (`docs/features.md`).
//!
//! A planet's recipe holds a [`FeatureSet`]: stamps (voxel models: trees,
//! rocks, buildings) and spawns (rules of where they grow). Placements are a
//! function of the world's seed and the terrain alone (the generator's
//! heights and ground materials at base columns), evaluated wherever a
//! cell is asked for, at any level: nothing is loaded or stored. The GPU
//! evaluates the same functions bit for bit (`chunk_generate.wgsl`).
//!
//! A spawn's candidates lie one on each `spacing` x `spacing` square of
//! base columns of a face, jittered by hash in the square's middle half; a
//! candidate grows where its hash passes the spawn's density, its noise
//! field passes its threshold, its ground's material (the terrain's rules
//! at its top cell on level ground), height, climate (the terrain's
//! temperature and moisture there: forests follow the rain, tree lines the
//! cold) and relief (across its footprint's corners) suit it, and its
//! footprint (its stamp's reach each way) lies on the face: a raw
//! candidate. It is placed unless its
//! footprint meets an earlier spawn's raw candidate (exclusion reads raw
//! candidates, never placements, so it does not chain from spawn to spawn).
//! Spawns go from the widest spacing to the narrowest (structures before
//! trees before rocks). Its stamp, turn and mirror are hashed too; its
//! anchor is the layer over its ground, less the spawn's sink.
//!
//! A spawn's own placements overlap where their stamps reach past their
//! squares (a forest's crowns interlock). A base cell is what the last
//! placement holding it (spawns in order, a spawn's by row then column)
//! makes of it: air or a material. A coarse cell (cells narrower than the
//! spacing) is what the last placement whose stamp holds its centre makes
//! of it (`stamp::coarse_of`: its parts smooth, a sparse one solid where it
//! keeps half its cells), else the terrain's.
//!
//! A spawn draws as its stamps at its near levels: level 0, and those whose
//! cells are at most half its spacing and an eighth of its footprint (its
//! stamps eight cells wide or more: legible). Each footprint of a level keeps the placements
//! of the spawns up to its last near one (those the near ones' exclusion
//! reads) on the GPU (`tile_features_main`), square by square over the
//! footprint and a margin ([`CompiledFeatures::layout`]), copied from
//! candidate blocks the GPU evaluates once each (`chunks::candidates`).
//!
//! Past them a spawn with a far form (a forest: `far_material`) draws as
//! canopy over the area it grows in, as a mountain is drawn by its
//! envelope: a level column whose ground its rules accept (its field at the
//! column's middle, or past the field's scale the field's majority; its
//! ground material and heights) holds, with the spawn's coverage for odds
//! (by hash: the share of ground its stamps cover, `1 - e^-(density area /
//! spacing^2)`), the canopy's layers from its ground up (`far_height`), a
//! cell solid where they fill half of it. Sparse woods are scattered
//! columns, dense forests a canopy, each as much ground as the trees cover.
//! Cells taller than twice the canopy keep only its colour: the GPU draws a
//! covered column's top in the far material, so a forest shows to the
//! horizon. Near spawns' cells stand over the far forms
//! ([`CompiledFeatures::far_cover`]).

use crate::grid::Grid;
use crate::noise::{hash3, noise, scale, ONE};
use crate::stamp::{cell_of, coarse_of, CompiledPart, Stamp, MAX_STAMP_REACH, UNIT};
use crate::terrain::{top_cells, Column, TerrainField};
use serde::{Deserialize, Serialize};

/// Most stamps a set holds, and most spawns (the GPU's tables).
pub const MAX_STAMPS: usize = 256;
pub const MAX_SPAWNS: usize = 32;
/// Widest spacing (placements pack their anchor's offset in a square in 10
/// bits), and the levels it makes near (`(2 << level) <= spacing` at most).
pub const MAX_SPACING: u32 = 1024;
pub const NEAR_LEVELS: u32 = 10;
/// Most placement squares a footprint keeps (8 bytes each).
pub const MAX_FOOTPRINT_SQUARES: usize = 16384;
/// Squares along a side of a candidate block (`chunks::candidates`: a
/// spawn's raw candidates are evaluated on the GPU by blocks of
/// `BLOCK_SIDE` x `BLOCK_SIDE` squares, each once while kept).
pub const BLOCK_SIDE: i32 = 8;

/// A world's features: stamps and the spawns that place them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FeatureSet {
    pub stamps: Vec<Stamp>,
    pub spawns: Vec<Spawn>,
}

/// Where a spawn's stamps grow (see the module doc).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Spawn {
    pub name: String,
    /// Its stamps (indices into [`FeatureSet::stamps`]), one per placement
    /// by hash.
    pub stamps: Vec<u32>,
    /// Base columns a side of the square each candidate lies in.
    pub spacing: u32,
    /// Share of candidates that grow (before the field).
    pub density: f32,
    /// The noise field gating it: its scale (m; 0: none) and the threshold
    /// it must pass (-1 to 1).
    pub field_scale_m: f64,
    pub field_threshold: f32,
    /// Ground surface materials it grows on (empty: any).
    pub surfaces: Vec<u32>,
    /// Ground heights over the datum it grows between (m).
    pub heights_m: [f64; 2],
    /// How far its heights wander (m) and over what distance (m; 0: they
    /// do not): a tree line follows noise as the terrain's snowline does,
    /// not a contour. The climate is read at the wandered height too.
    pub height_wander_m: [f64; 2],
    /// The climate it grows in ([`TerrainField::climate`] at its ground):
    /// annual mean temperature (°C) and moisture (0 dry to 1 wet).
    pub temperature_c: [f64; 2],
    pub moisture: [f64; 2],
    /// Most layers between the grounds at its anchor and its footprint's
    /// corners (its stamp's reach each way, at least a column).
    pub max_relief: u32,
    /// Layers its anchor lies under the layer over its ground.
    pub sink: i32,
    /// Its far form's material (0: none: past its near levels it is not
    /// drawn), and its canopy's height over the ground (layers; 0: its
    /// stamps' mean height).
    pub far_material: u32,
    pub far_height: u32,
}

impl FeatureSet {
    pub fn is_empty(&self) -> bool {
        self.spawns.is_empty() && self.stamps.is_empty()
    }
}

impl Default for Spawn {
    fn default() -> Self {
        Self {
            name: String::new(),
            stamps: Vec::new(),
            spacing: 8,
            density: 0.5,
            field_scale_m: 0.0,
            field_threshold: -1.0,
            surfaces: Vec::new(),
            heights_m: [-1e9, 1e9],
            height_wander_m: [0.0, 0.0],
            temperature_c: [-1e3, 1e3],
            moisture: [0.0, 1.0],
            max_relief: u32::MAX,
            sink: 0,
            far_material: 0,
            far_height: 0,
        }
    }
}

/// A stamp as evaluation reads it: its parts compiled (`stamp::CompiledPart`)
/// and the box of cells they may hold.
#[derive(Clone, Debug)]
pub struct CompiledStamp {
    /// Its box: lowest offset and size.
    pub lo: [i32; 3],
    pub size: [i32; 3],
    pub parts: Vec<CompiledPart>,
    /// Most base columns it reaches from its anchor in x or z (any turn).
    pub reach: i32,
    /// Columns its solid parts cover, kept share of them counted.
    pub area: u32,
}

impl CompiledStamp {
    fn new(stamp: &Stamp) -> Result<Self, String> {
        let parts = stamp.compile()?;
        let (lo, hi) = stamp.bounds().unwrap_or(([0; 3], [-1; 3]));
        let size = [hi[0] - lo[0] + 1, hi[1] - lo[1] + 1, hi[2] - lo[2] + 1];
        let reach = [lo[0], lo[2], hi[0], hi[2]]
            .into_iter()
            .map(i32::abs)
            .max()
            .unwrap_or(0);
        // The columns under its solid parts (their outlines from above).
        let mut covered = 0.0;
        for z in lo[2]..=hi[2] {
            for x in lo[0]..=hi[0] {
                let c = [x * UNIT + UNIT / 2, z * UNIT + UNIT / 2];
                let keep = parts
                    .iter()
                    .filter(|p| {
                        p.material != 0
                            && p.lo[0] <= x
                            && x <= p.hi[0]
                            && p.lo[2] <= z
                            && z <= p.hi[2]
                            && from_above(p, c)
                    })
                    .map(|p| f64::from(p.keep) / 256.0)
                    .fold(0.0, f64::max);
                covered += keep;
            }
        }
        Ok(Self {
            lo,
            size,
            parts,
            reach,
            area: covered.round() as u32,
        })
    }

    /// What it holds at offset `p` (its own frame): `None` nothing,
    /// `Some(None)` air.
    pub fn at(&self, p: [i32; 3]) -> Option<Option<u32>> {
        cell_of(&self.parts, p)
    }

    /// What it makes of the coarse cell over offsets `[lo, hi)` of its own
    /// frame (`stamp::coarse_of`).
    pub fn coarse(&self, lo: [i32; 3], hi: [i32; 3]) -> Option<Option<u32>> {
        coarse_of(&self.parts, lo, hi)
    }
}

/// Whether a part seen from above covers column centre `c` (x, z in
/// sixteenths of a cell).
fn from_above(p: &CompiledPart, c: [i32; 2]) -> bool {
    if !p.capsule {
        return true;
    }
    let (a, b) = (
        [p.a[0] as f64, p.a[2] as f64],
        [p.b[0] as f64, p.b[2] as f64],
    );
    let d = [b[0] - a[0], b[1] - a[1]];
    let w = [c[0] as f64 - a[0], c[1] as f64 - a[1]];
    let length = d[0] * d[0] + d[1] * d[1];
    let t = if length > 0.0 {
        ((w[0] * d[0] + w[1] * d[1]) / length).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let e = [w[0] - d[0] * t, w[1] - d[1] * t];
    let r = f64::from(p.radius[0].max(p.radius[1]));
    e[0] * e[0] + e[1] * e[1] <= r * r
}

/// A spawn as evaluation reads it.
#[derive(Clone, Debug)]
pub struct CompiledSpawn {
    pub stamps: Vec<u32>,
    pub spacing: i32,
    /// Its stamps' most reach (base columns from the anchor, any turn).
    pub reach: i32,
    /// Columns its anchors keep from their squares' sides (a quarter of the
    /// spacing): neighbours' anchors lie at least twice that apart.
    pub inset: i32,
    /// Q16 share of candidates.
    pub density: u32,
    /// The field's noise shift (0: none), threshold (Q16) and seed.
    pub field_shift: u32,
    pub field_threshold: i32,
    pub seed: u32,
    /// Surface materials as a bit mask (0: any).
    pub surfaces: u64,
    /// Ground top layers it grows between, and how many layers they wander
    /// by noise of what shift (0: none; `CompiledSpawn::wander`).
    pub tops: [i32; 2],
    pub wander: i32,
    pub wander_shift: u32,
    /// Its climate: temperature range (1/128 °C), moisture range packed
    /// (Q16, low and high halves); `None`: any.
    pub climate: Option<[i32; 3]>,
    pub max_relief: i32,
    pub sink: i32,
    /// Its far form: material (0: none) and canopy height (layers), and the
    /// first level whose cells span its field's scale (the field's
    /// majority stands for it from there).
    pub far_material: u32,
    pub far_height: i32,
    pub field_level: u32,
    /// The share of ground its stamps cover (Q16): the odds of a far
    /// column holding its canopy.
    pub cover: u32,
}

impl CompiledSpawn {
    /// Whether ground at layer `ground` of base column `(i, j)` of `face`
    /// lies within its heights and climate, as they wander there
    /// (`spawn_grows_at` in chunk_generate.wgsl).
    pub fn grows_at(
        &self,
        grid: &Grid,
        field: &dyn TerrainField,
        face: u8,
        i: i32,
        j: i32,
        ground: i32,
    ) -> bool {
        let p = grid.domain_point(face, i, j, 0);
        let wander = if self.wander_shift > 0 {
            scale(
                noise(p, self.wander_shift, self.seed ^ 0x7a3d_e1f1),
                self.wander,
            )
        } else {
            0
        };
        let ground = ground.wrapping_add(wander);
        if ground < self.tops[0] || ground > self.tops[1] {
            return false;
        }
        let Some([lo, hi, wet]) = self.climate else {
            return true;
        };
        let (t, m) = field.climate(p, ground.wrapping_mul(grid.layer_mm() as i32));
        t >= lo && t <= hi && m >= wet & 0xffff && m <= (wet >> 16) & 0xffff
    }
}

/// A world's features compiled for evaluation (CPU and GPU).
#[derive(Clone, Debug, Default)]
pub struct CompiledFeatures {
    pub stamps: Vec<CompiledStamp>,
    pub spawns: Vec<CompiledSpawn>,
    /// Most layers a placement's stamp reaches over and under its ground.
    pub above: i32,
    pub below: i32,
    /// Each near level's placement squares ([`Self::layout`]), the most
    /// any footprint keeps, and the most candidate blocks they span.
    pub levels: Vec<LevelLayout>,
    pub stride: usize,
    pub blocks: usize,
}

/// A level's near spawns (a prefix of the set) and, for each, where its
/// squares lie in a footprint's placements; the squares a footprint keeps,
/// and the most candidate blocks (`BLOCK_SIDE` squares a side) they span.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LevelLayout {
    pub spawns: Vec<SquareGrid>,
    pub squares: u32,
    pub blocks: usize,
}

/// A spawn's squares in a footprint's placements: from `offset`, `side` x
/// `side` of them from the square holding the footprint's first column
/// less `margin` columns (each way).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SquareGrid {
    pub offset: u32,
    pub side: u32,
    pub margin: i32,
}

/// A grown candidate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub spawn: u32,
    pub stamp: u32,
    pub face: u8,
    /// Anchor: base column and layer.
    pub i: i32,
    pub j: i32,
    pub k: i32,
    /// Quarter turns about the vertical (after the mirror), and mirror in x.
    pub turn: u32,
    pub mirror: bool,
}

impl Placement {
    /// World offset `(dx, dy, dz)` from the anchor in the stamp's frame.
    pub fn local(&self, d: [i32; 3]) -> [i32; 3] {
        let [mut x, y, mut z] = d;
        for _ in 0..self.turn {
            (x, z) = (z, -x);
        }
        if self.mirror {
            x = -x;
        }
        [x, y, z]
    }
    /// Base columns its stamp's footprint covers (`[i0, i1)`, `[j0, j1)`:
    /// its reach each way).
    pub fn footprint(&self, features: &CompiledFeatures) -> ([i32; 2], [i32; 2]) {
        let r = features.stamps[self.stamp as usize].reach;
        ([self.i - r, self.i + r + 1], [self.j - r, self.j + r + 1])
    }
    /// A world box of offsets `[lo, hi)` in the stamp's frame.
    fn local_box(&self, lo: [i32; 3], hi: [i32; 3]) -> ([i32; 3], [i32; 3]) {
        let a = self.local(lo);
        let b = self.local([hi[0] - 1, hi[1] - 1, hi[2] - 1]);
        (
            [a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2])],
            [a[0].max(b[0]) + 1, a[1].max(b[1]) + 1, a[2].max(b[2]) + 1],
        )
    }
}

impl CompiledFeatures {
    /// `set` compiled for a world of seed `seed`.
    pub fn new(set: &FeatureSet, grid: &Grid, seed: u64) -> Result<Self, String> {
        if set.stamps.len() > MAX_STAMPS || set.spawns.len() > MAX_SPAWNS {
            return Err(format!(
                "at most {MAX_STAMPS} stamps and {MAX_SPAWNS} spawns"
            ));
        }
        let stamps: Vec<CompiledStamp> = set
            .stamps
            .iter()
            .map(CompiledStamp::new)
            .collect::<Result<_, _>>()?;
        let voxel = grid.voxel_size();
        let mut spawns = Vec::new();
        let (mut above, mut below) = (0, 0);
        for (n, s) in set.spawns.iter().enumerate() {
            if s.stamps.is_empty() || s.stamps.iter().any(|&i| i as usize >= stamps.len()) {
                return Err(format!("spawn {} names no stamp of the set", s.name));
            }
            if !(1..=MAX_SPACING).contains(&s.spacing) {
                return Err(format!(
                    "spawn {} spacing {} is not 1 to {MAX_SPACING} columns",
                    s.name, s.spacing
                ));
            }
            if n > 0 && s.spacing > set.spawns[n - 1].spacing {
                return Err(format!("spawn {} is wider than the spawn before it: spawns go from the widest spacing to the narrowest", s.name));
            }
            if s.stamps.len() > 128 {
                return Err(format!("spawn {} has more than 128 stamps", s.name));
            }
            let mut surfaces = 0u64;
            for &m in &s.surfaces {
                if m >= 64 {
                    return Err(format!(
                        "spawn {} surface material {m} is not under 64",
                        s.name
                    ));
                }
                surfaces |= 1 << m;
            }
            let mut reach = 0;
            let mut heights = 0;
            let mut area = 0.0;
            for &i in &s.stamps {
                let st = &stamps[i as usize];
                reach = reach.max(st.reach);
                area += f64::from(st.area) / s.stamps.len() as f64;
                above = above.max(st.lo[1] + st.size[1] - s.sink);
                below = below.max(s.sink - st.lo[1]);
                heights += st.lo[1] + st.size[1] - s.sink;
            }
            if s.far_material >= 128 {
                return Err(format!(
                    "spawn {} far material {} is not under 128",
                    s.name, s.far_material
                ));
            }
            let far_height = if s.far_height > 0 {
                s.far_height.min(2 * MAX_STAMP_REACH as u32) as i32
            } else {
                (heights as f64 / s.stamps.len() as f64).round().max(1.0) as i32
            };
            if s.far_material != 0 {
                above = above.max(far_height);
            }
            // The field's scale as the layer stack's noise takes it.
            let field_shift = if s.field_scale_m > 0.0 {
                (s.field_scale_m / crate::grid::DOMAIN_UNIT)
                    .log2()
                    .round()
                    .clamp(1.0, 30.0) as u32
            } else {
                0
            };
            let layer = |m: f64| (m / voxel).floor().clamp(-2.0e9, 2.0e9) as i32;
            let degrees = |c: f64| {
                (c * f64::from(crate::planet_maps::DEGREE))
                    .clamp(-1.0e9, 1.0e9)
                    .round() as i32
            };
            let wet =
                |m: f64| (m.clamp(0.0, 1.0) * f64::from(crate::planet_maps::WET)).round() as i32;
            let climate = [
                degrees(s.temperature_c[0]),
                degrees(s.temperature_c[1]),
                wet(s.moisture[0]) | wet(s.moisture[1]) << 16,
            ];
            let wander_shift = if s.height_wander_m[0] > 0.0 && s.height_wander_m[1] > 0.0 {
                (s.height_wander_m[1] / crate::grid::DOMAIN_UNIT)
                    .log2()
                    .round()
                    .clamp(1.0, 30.0) as u32
            } else {
                0
            };
            spawns.push(CompiledSpawn {
                stamps: s.stamps.clone(),
                spacing: s.spacing as i32,
                reach,
                inset: s.spacing as i32 / 4,
                density: (f64::from(s.density.clamp(0.0, 1.0)) * 65_536.0).round() as u32,
                field_shift,
                field_threshold: (f64::from(s.field_threshold.clamp(-1.0, 1.0)) * f64::from(ONE))
                    .round() as i32,
                seed: (hash3(n as i32, (seed >> 32) as i32, seed as i32, 0x5eed_f00d)) | 1,
                surfaces,
                tops: [layer(s.heights_m[0]) + 1, layer(s.heights_m[1]) + 1],
                wander: if wander_shift > 0 {
                    layer(s.height_wander_m[0]).min(1 << 26)
                } else {
                    0
                },
                wander_shift,
                climate: (s.temperature_c[0] > -273.0
                    || s.temperature_c[1] < 1e3
                    || s.moisture[0] > 0.0
                    || s.moisture[1] < 1.0)
                    .then_some(climate),
                max_relief: s.max_relief.min(i32::MAX as u32) as i32,
                sink: s.sink,
                far_material: s.far_material,
                far_height,
                field_level: if s.field_scale_m > 0.0 {
                    (s.field_scale_m / voxel).log2().ceil().clamp(0.0, 31.0) as u32
                } else {
                    0
                },
                // Overlapping crowns: the union of placements at this density.
                cover: ((1.0
                    - (-f64::from(s.density.clamp(0.0, 1.0)) * area
                        / f64::from(s.spacing * s.spacing))
                    .exp())
                    * 65_536.0)
                    .round() as u32,
            });
        }
        let mut features = Self {
            stamps,
            spawns,
            above: above.max(0),
            below: below.max(0),
            levels: Vec::new(),
            stride: 0,
            blocks: 0,
        };
        features.levels = (0..NEAR_LEVELS)
            .map(|level| features.layout(level))
            .collect();
        features.stride = features
            .levels
            .iter()
            .map(|l| l.squares as usize)
            .max()
            .unwrap_or(0);
        features.blocks = features.levels.iter().map(|l| l.blocks).max().unwrap_or(0);
        if features.stride > MAX_FOOTPRINT_SQUARES {
            return Err(format!("the spawns' placements take {} squares a footprint, more than {MAX_FOOTPRINT_SQUARES}: fewer spawns of narrow spacing", features.stride));
        }
        Ok(features)
    }

    pub fn is_empty(&self) -> bool {
        self.spawns.is_empty()
    }

    /// Whether spawn `s` draws as its stamps at `level`: at level 0, and
    /// where its spacing is two of its cells or more and its footprint
    /// eight (its shapes stay legible; past that its far form, if any).
    pub fn drawn(&self, s: usize, level: u32) -> bool {
        let sp = &self.spawns[s];
        level == 0
            || (8i64 << level) <= (4 * i64::from(sp.spacing)).min(i64::from(2 * sp.reach + 1))
    }

    /// Spawns whose placements a footprint of `level` keeps: up to the last
    /// drawn one (the drawn ones exclude the earlier ones' footprints).
    pub fn near(&self, level: u32) -> usize {
        (0..self.spawns.len())
            .rev()
            .find(|&s| self.drawn(s, level))
            .map_or(0, |s| s + 1)
    }

    /// The far form over level column `(i, j)` of `face` (`level` > 0),
    /// whose ground (first air layer, base layers) and material are
    /// `ground`: the first spawn not drawn at `level` whose far form holds
    /// the column (its coverage by hash; its field at the column's middle
    /// base column, or from its field level the field's majority, a
    /// threshold under zero; its heights and ground materials) gives its
    /// material and canopy, layers `[ground, ground + height)`.
    pub fn far_cover(
        &self,
        grid: &Grid,
        field: &dyn TerrainField,
        face: u8,
        level: u32,
        i: i32,
        j: i32,
        (ground, surface): (i32, i32),
    ) -> Option<(u32, i32, i32)> {
        if level == 0 {
            return None;
        }
        for (s, sp) in self.spawns.iter().enumerate() {
            let mid = 1 << (level - 1);
            if self.drawn(s, level)
                || sp.far_material == 0
                || !sp.grows_at(
                    grid,
                    field,
                    face,
                    (i << level) + mid,
                    (j << level) + mid,
                    ground,
                )
            {
                continue;
            }
            // The column holds canopy with the spawn's coverage for odds.
            if hash3(
                i,
                j,
                (level << 3) as i32 | i32::from(face),
                sp.seed ^ 0xfa7f_0e55,
            ) >> 16
                >= sp.cover
            {
                continue;
            }
            if sp.surfaces != 0 && (surface >= 64 || sp.surfaces & (1 << surface) == 0) {
                continue;
            }
            if sp.field_shift > 0 {
                let pass = if level >= sp.field_level {
                    sp.field_threshold < 0
                } else {
                    noise(
                        grid.domain_point(face, (i << level) + mid, (j << level) + mid, 0),
                        sp.field_shift,
                        sp.seed ^ 0x632b_e5ab,
                    ) > sp.field_threshold
                };
                if !pass {
                    continue;
                }
            }
            return Some((sp.far_material, ground, ground + sp.far_height));
        }
        None
    }

    /// Cell `k` of `level` under a far form's canopy `(material, lo, hi)`:
    /// its material where the canopy's layers fill half of it.
    pub fn far_cell(
        level: u32,
        k: i32,
        (material, lo, hi): (u32, i32, i32),
    ) -> Option<Option<u32>> {
        let a = (i64::from(k) << level).max(i64::from(lo));
        let b = ((i64::from(k) + 1) << level).min(i64::from(hi));
        (2 * (b - a) >= 1i64 << level).then_some(Some(material))
    }

    /// Where a footprint of `level` keeps its near spawns' candidates: each
    /// spawn's squares whose candidates may reach the footprint's columns
    /// (its reach past them) or meet those of a later spawn that may (their
    /// exclusion: a later candidate reaching the footprint stands a spacing
    /// and its reach twice past it, and this spawn's reach past that), one
    /// after another.
    fn layout(&self, level: u32) -> LevelLayout {
        let near = self.near(level);
        let width = 64i64 << level;
        let mut spawns = vec![SquareGrid::default(); near];
        let mut margins = vec![0i64; near];
        for s in (0..near).rev() {
            let (spacing, reach) = (
                i64::from(self.spawns[s].spacing),
                i64::from(self.spawns[s].reach),
            );
            margins[s] = (s + 1..near)
                .map(|t| {
                    i64::from(self.spawns[t].spacing - self.spawns[t].inset)
                        + 2 * i64::from(self.spawns[t].reach)
                        + reach
                })
                .fold(reach, i64::max);
            spawns[s] = SquareGrid {
                offset: 0,
                side: ((width + 2 * margins[s] - 1) / spacing + 2) as u32,
                margin: margins[s] as i32,
            };
        }
        let mut offset = 0;
        for g in &mut spawns {
            g.offset = offset;
            offset += g.side * g.side;
        }
        // `side` squares from anywhere span this many blocks a side.
        let blocks = spawns
            .iter()
            .map(|g| (g.side as usize).div_ceil(BLOCK_SIDE as usize) + 1)
            .map(|b| b * b)
            .sum();
        LevelLayout {
            spawns,
            squares: offset,
            blocks,
        }
    }

    /// Spawn `s`'s squares in a footprint of `level` at columns `first`
    /// (its first column, each way): the first square, and how many a side
    /// (as `tile_features_main` of chunk_generate.wgsl lays them out).
    pub fn squares(&self, level: u32, s: usize, first: [i32; 2]) -> ([i32; 2], i32) {
        let g = &self.levels[level as usize].spawns[s];
        let sp = self.spawns[s].spacing;
        (first.map(|f| (f - g.margin).div_euclid(sp)), g.side as i32)
    }

    /// The set as `chunk_generate.wgsl` reads it (`feature_*`): a header
    /// (spawn count, where spawns, stamps and levels start, the footprint
    /// stride), 24 words a spawn, 16 a stamp (its box's lowest offset,
    /// reach, size, part count, where its parts start), `4 (1 + MAX_SPAWNS)`
    /// a near level (the spawns whose placements it keeps, the mask of those
    /// it draws, its squares, then each spawn's offset, side, margin), then
    /// spawns' stamp lists and stamps' parts (`CompiledPart::gpu_words`).
    pub fn gpu_words(&self) -> Vec<u32> {
        const HEADER: usize = 8;
        let spawns_at = HEADER;
        let stamps_at = spawns_at + 24 * self.spawns.len();
        let levels_at = stamps_at + 16 * self.stamps.len();
        let mut data = levels_at + 4 * (1 + MAX_SPAWNS) * NEAR_LEVELS as usize;
        let mut w = vec![0u32; data];
        w[..5].copy_from_slice(&[
            self.spawns.len() as u32,
            spawns_at as u32,
            stamps_at as u32,
            levels_at as u32,
            self.stride as u32,
        ]);
        let mut tail: Vec<u32> = Vec::new();
        let put = |tail: &mut Vec<u32>, words: &[u32]| {
            let at = data + tail.len();
            tail.extend_from_slice(words);
            at as u32
        };
        for (n, sp) in self.spawns.iter().enumerate() {
            let list = put(&mut tail, &sp.stamps);
            let r = &mut w[spawns_at + 24 * n..spawns_at + 24 * n + 24];
            r.copy_from_slice(&[
                sp.spacing as u32,
                sp.reach as u32,
                sp.density,
                sp.field_shift,
                sp.field_threshold as u32,
                sp.seed,
                sp.surfaces as u32,
                (sp.surfaces >> 32) as u32,
                sp.tops[0] as u32,
                sp.tops[1] as u32,
                sp.max_relief as u32,
                sp.sink as u32,
                list,
                sp.stamps.len() as u32,
                sp.far_material,
                sp.far_height as u32,
                sp.field_level,
                sp.inset as u32,
                sp.cover,
                sp.wander as u32,
                sp.wander_shift,
                // The climate (`spawn_grows_at`): any, or its ranges.
                sp.climate.map_or(i32::MIN, |c| c[0]) as u32,
                sp.climate.map_or(i32::MAX, |c| c[1]) as u32,
                sp.climate.map_or(0xffff_0000, |c| c[2] as u32),
            ]);
        }
        for (n, st) in self.stamps.iter().enumerate() {
            let parts: Vec<u32> = st.parts.iter().flat_map(|p| p.gpu_words()).collect();
            let parts = put(&mut tail, &parts);
            let r = &mut w[stamps_at + 16 * n..stamps_at + 16 * n + 16];
            r[..9].copy_from_slice(&[
                st.lo[0] as u32,
                st.lo[1] as u32,
                st.lo[2] as u32,
                st.reach as u32,
                st.size[0] as u32,
                st.size[1] as u32,
                st.size[2] as u32,
                st.parts.len() as u32,
                parts,
            ]);
        }
        for (level, l) in self.levels.iter().enumerate() {
            let at = levels_at + 4 * (1 + MAX_SPAWNS) * level;
            w[at] = l.spawns.len() as u32;
            w[at + 1] = (0..l.spawns.len())
                .filter(|&s| self.drawn(s, level as u32))
                .fold(0, |m, s| m | 1 << s);
            w[at + 2] = l.squares;
            for (s, g) in l.spawns.iter().enumerate() {
                w[at + 4 * (1 + s)..at + 4 * (1 + s) + 3].copy_from_slice(&[
                    g.offset,
                    g.side,
                    g.margin as u32,
                ]);
            }
        }
        data += tail.len();
        w.extend_from_slice(&tail);
        debug_assert_eq!(w.len(), data);
        w
    }

    /// Spawn `s`'s raw candidate on square `(gx, gy)` of `face`: where its
    /// own rules let it grow (exclusion aside).
    pub fn candidate(
        &self,
        s: usize,
        grid: &Grid,
        field: &dyn TerrainField,
        top: &dyn Fn(u8, i32, i32) -> (i32, i32),
        face: u8,
        gx: i32,
        gy: i32,
    ) -> Option<Placement> {
        let sp = &self.spawns[s];
        let h = hash3(gx, gy, i32::from(face), sp.seed);
        if (h >> 16) >= sp.density {
            return None;
        }
        // The anchor in the square's middle.
        let slack = (sp.spacing - 2 * sp.inset) as u32;
        let jitter =
            |salt: u32| sp.inset + (hash3(gx, gy, i32::from(face), sp.seed ^ salt) % slack) as i32;
        let (i, j) = (
            gx * sp.spacing + jitter(0x9e37_79b9),
            gy * sp.spacing + jitter(0x85eb_ca6b),
        );
        // Its footprint on the face.
        let n = grid.cells();
        if i < sp.reach || j < sp.reach || i >= n - sp.reach || j >= n - sp.reach {
            return None;
        }
        if sp.field_shift > 0
            && noise(
                grid.domain_point(face, i, j, 0),
                sp.field_shift,
                sp.seed ^ 0x632b_e5ab,
            ) <= sp.field_threshold
        {
            return None;
        }
        let (ground, surface) = top(face, i, j);
        if !sp.grows_at(grid, field, face, i, j, ground) {
            return None;
        }
        if sp.surfaces != 0 && (surface >= 64 || sp.surfaces & (1 << surface) == 0) {
            return None;
        }
        let v = hash3(gx, gy, i32::from(face), sp.seed ^ 0xc2b2_ae35);
        let stamp = sp.stamps[(v % sp.stamps.len() as u32) as usize];
        let placement = Placement {
            spawn: s as u32,
            stamp,
            face,
            i,
            j,
            k: ground - sp.sink,
            turn: (v >> 8) & 3,
            mirror: (v >> 10) & 1 == 1,
        };
        if sp.max_relief < i32::MAX {
            let r = self.stamps[stamp as usize].reach.max(1);
            let (mut lo, mut hi) = (ground, ground);
            for (a, b) in [(-r, -r), (r, -r), (-r, r), (r, r)] {
                let g = top(face, i + a, j + b).0;
                lo = lo.min(g);
                hi = hi.max(g);
            }
            if hi - lo > sp.max_relief {
                return None;
            }
        }
        Some(placement)
    }
}

/// Squares decided so far, by spawn, face and square: the raw candidate and
/// whether it is placed.
pub type PlacementCache =
    rustc_hash::FxHashMap<(u32, u8, i32, i32), (Option<Placement>, Option<bool>)>;

/// Feature evaluation over a planet's terrain (the CPU's; the GPU lists a
/// footprint's placements).
pub struct FeatureEval<'a> {
    pub features: &'a CompiledFeatures,
    pub grid: &'a Grid,
    pub field: &'a dyn TerrainField,
    /// The ground top layer and surface material of a base column.
    pub top: &'a dyn Fn(u8, i32, i32) -> (i32, i32),
    cache: &'a mut PlacementCache,
}

impl<'a> FeatureEval<'a> {
    pub fn new(
        features: &'a CompiledFeatures,
        grid: &'a Grid,
        field: &'a dyn TerrainField,
        top: &'a dyn Fn(u8, i32, i32) -> (i32, i32),
        cache: &'a mut PlacementCache,
    ) -> Self {
        Self {
            features,
            grid,
            field,
            top,
            cache,
        }
    }

    /// Spawn `s`'s raw candidate on square `(gx, gy)` of `face`, if any.
    fn raw(&mut self, s: usize, face: u8, gx: i32, gy: i32) -> Option<Placement> {
        let key = (s as u32, face, gx, gy);
        if let Some(&(p, _)) = self.cache.get(&key) {
            return p;
        }
        let p = self
            .features
            .candidate(s, self.grid, self.field, self.top, face, gx, gy);
        self.cache.insert(key, (p, None));
        p
    }

    /// Spawn `s`'s placement on square `(gx, gy)` of `face`, if any: its raw
    /// candidate unless an earlier spawn's meets its footprint.
    pub fn placement(&mut self, s: usize, face: u8, gx: i32, gy: i32) -> Option<Placement> {
        let p = self.raw(s, face, gx, gy)?;
        let key = (s as u32, face, gx, gy);
        let placed = match self.cache.get(&key).and_then(|e| e.1) {
            Some(placed) => placed,
            None => {
                let (fi, fj) = p.footprint(self.features);
                let placed = !(0..s).any(|e| self.raw_meets(e, face, fi, fj));
                self.cache.insert(key, (Some(p), Some(placed)));
                placed
            }
        };
        placed.then_some(p)
    }

    /// Whether a raw candidate of spawn `s` has its footprint meet columns
    /// `[i0, i1)` x `[j0, j1)`.
    fn raw_meets(&mut self, s: usize, face: u8, i: [i32; 2], j: [i32; 2]) -> bool {
        let features = self.features;
        self.squares_near(s, i, j)
            .into_iter()
            .filter_map(|(gx, gy)| self.raw(s, face, gx, gy))
            .any(|p| {
                let (fi, fj) = p.footprint(features);
                fi[0] < i[1] && fi[1] > i[0] && fj[0] < j[1] && fj[1] > j[0]
            })
    }

    /// Spawn `s`'s squares that may reach base columns `[i0, i1)` x
    /// `[j0, j1)` (within its reach of them), by row then column.
    fn squares_near(&self, s: usize, i: [i32; 2], j: [i32; 2]) -> Vec<(i32, i32)> {
        let (spacing, r) = (
            self.features.spawns[s].spacing,
            self.features.spawns[s].reach,
        );
        let g = |v: i32| v.div_euclid(spacing);
        (g(j[0] - r)..=g(j[1] - 1 + r))
            .flat_map(|gy| (g(i[0] - r)..=g(i[1] - 1 + r)).map(move |gx| (gx, gy)))
            .collect()
    }

    /// Spawn `s`'s placements of the squares that may reach base columns
    /// `[i0, i1)` x `[j0, j1)`, by row then column.
    pub fn placements_near(
        &mut self,
        s: usize,
        face: u8,
        i: [i32; 2],
        j: [i32; 2],
    ) -> Vec<Placement> {
        self.squares_near(s, i, j)
            .into_iter()
            .filter_map(|(gx, gy)| self.placement(s, face, gx, gy))
            .collect()
    }

    /// What the features make of base cell `(i, j, k)` of `face` (the last
    /// placement holding it): `None` nothing, `Some(None)` air,
    /// `Some(Some(m))` material `m`.
    pub fn cell(&mut self, face: u8, i: i32, j: i32, k: i32) -> Option<Option<u32>> {
        let mut out = None;
        for s in 0..self.features.spawns.len() {
            for p in self.placements_near(s, face, [i, i + 1], [j, j + 1]) {
                if let Some(v) =
                    self.features.stamps[p.stamp as usize].at(p.local([i - p.i, k - p.k, j - p.j]))
                {
                    out = Some(v);
                }
            }
        }
        out
    }

    /// [`Self::cell`] of every base cell of the `n`^3 brick at `origin`, into
    /// `out` (index `((dj * n) + di) * n + dk`, left alone where nothing
    /// is): each spawn's placements near the brick listed once, not per
    /// cell, and written in the same order, so a cell keeps the last
    /// placement holding it.
    pub fn brick(&mut self, face: u8, origin: [i32; 3], n: i32, out: &mut [Option<Option<u32>>]) {
        let [i0, j0, k0] = origin;
        for s in 0..self.features.spawns.len() {
            for p in self.placements_near(s, face, [i0, i0 + n], [j0, j0 + n]) {
                let (fi, fj) = p.footprint(self.features);
                let stamp = &self.features.stamps[p.stamp as usize];
                for j in fj[0].max(j0)..fj[1].min(j0 + n) {
                    for i in fi[0].max(i0)..fi[1].min(i0 + n) {
                        let column = (((j - j0) * n + (i - i0)) * n) as usize;
                        for k in k0..k0 + n {
                            if let Some(v) = stamp.at(p.local([i - p.i, k - p.k, j - p.j])) {
                                out[column + (k - k0) as usize] = Some(v);
                            }
                        }
                    }
                }
            }
        }
    }

    /// What the near spawns make of cell `(i, j, k)` of `level`: the last
    /// placement whose stamp holds its centre (`stamp::coarse_of`).
    pub fn coarse_cell(
        &mut self,
        face: u8,
        level: u32,
        i: i32,
        j: i32,
        k: i32,
    ) -> Option<Option<u32>> {
        if level == 0 {
            return self.cell(face, i, j, k);
        }
        let s0 = 1i32 << level;
        let (bi, bj, bk) = (i * s0, j * s0, k * s0);
        let mut out = None;
        for s in (0..self.features.near(level)).filter(|&s| self.features.drawn(s, level)) {
            for p in self.placements_near(s, face, [bi, bi + s0], [bj, bj + s0]) {
                let (lo, hi) = p.local_box(
                    [bi - p.i, bk - p.k, bj - p.j],
                    [bi + s0 - p.i, bk + s0 - p.k, bj + s0 - p.j],
                );
                if let Some(v) = self.features.stamps[p.stamp as usize].coarse(lo, hi) {
                    out = Some(v);
                }
            }
        }
        out
    }
}

/// The ground's first air layer (base layers) and material of column
/// `(i, j)` of `level` (what candidates and far forms read), from `height`:
/// the terrain's height there. The material is the terrain's rules' at the
/// column's top cell on level ground (slope 0: the same at every level, a
/// column's own inputs).
pub fn ground(
    grid: &Grid,
    field: &dyn TerrainField,
    face: u8,
    i: i32,
    j: i32,
    level: u32,
    height: i32,
) -> (i32, i32) {
    let top = top_cells(grid, height, level) - 1;
    let surface = field.surface(Column { face, i, j, level }, height) & 0xff;
    let material = field.ground_material(
        grid.volume_point(face, i, j, top, level),
        surface,
        height,
        0,
        0,
        top << level,
    ) & crate::terrain::material::ID;
    (top_cells(grid, height, 0), material as i32)
}

/// A cell's features as a cell of the world.
pub fn apply(v: Option<Option<u32>>, kind: u32, material: u32) -> (u32, u32) {
    match v {
        None => (kind, material),
        Some(None) => (0, 0),
        Some(Some(m)) => (1, m),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edits::{Brush, BrushOp, BrushShape};
    use crate::grid::Cell;
    use crate::{Planet, PlanetRecipe};

    /// Heights that wander let ground past their limit grow in places and
    /// keep ground short of it bare in others, by no more than the wander,
    /// smoothly over its distance; without wander the limit is a contour.
    #[test]
    fn wandering_heights_break_the_contour() {
        let grid = Grid::new(crate::planet::EARTH_RADIUS, 0.1).unwrap();
        let spawn = |wander: [f64; 2]| Spawn {
            stamps: vec![0],
            heights_m: [-1e9, 100.0],
            height_wander_m: wander,
            ..Spawn::default()
        };
        let set = FeatureSet {
            stamps: stamps(),
            spawns: vec![spawn([20.0, 400.0]), spawn([0.0, 0.0])],
        };
        let compiled = CompiledFeatures::new(&set, &grid, 7).unwrap();
        let (wandering, fixed) = (&compiled.spawns[0], &compiled.spawns[1]);
        let field = crate::layers::TerrainLayers::flat()
            .field(&grid, 7)
            .unwrap();
        let field = &field;
        let n = grid.cells() / 2;
        let (mut above, mut below) = (0, 0);
        for step in 0..2000 {
            let (i, j) = (n + step * 37, n - step * 23);
            for (ground_m, past) in [(110.0, true), (90.0, false)] {
                let ground = (ground_m / 0.1) as i32;
                assert_eq!(fixed.grows_at(&grid, field, 2, i, j, ground), !past);
                match (wandering.grows_at(&grid, field, 2, i, j, ground), past) {
                    (true, true) => above += 1,
                    (false, false) => below += 1,
                    _ => {}
                }
            }
            // Never past the wander.
            assert!(
                wandering.grows_at(&grid, field, 2, i, j, (79.0 / 0.1) as i32)
                    && !wandering.grows_at(&grid, field, 2, i, j, (121.0 / 0.1) as i32)
            );
        }
        assert!(
            above > 100 && below > 100,
            "{above} grow past the limit, {below} bare short of it"
        );
    }

    /// A tree (trunk and crown) and a hut (walls, its inside and a door
    /// cleared).
    fn stamps() -> Vec<Stamp> {
        let mut tree = Stamp::new();
        tree.fill([-2, 5, -2], [2, 8, 2], 11);
        tree.fill([0, 0, 0], [0, 7, 0], 10);
        let mut hut = Stamp::new();
        hut.fill([-8, -1, -8], [8, 4, 8], 12);
        hut.hollow([-7, 0, -7], [7, 3, 7]);
        hut.hollow([0, 0, -8], [1, 1, -8]);
        vec![tree, hut]
    }

    fn set() -> FeatureSet {
        FeatureSet {
            stamps: stamps(),
            spawns: vec![
                Spawn {
                    name: "huts".into(),
                    stamps: vec![1],
                    spacing: 48,
                    density: 0.6,
                    max_relief: 3,
                    ..Spawn::default()
                },
                Spawn {
                    name: "trees".into(),
                    stamps: vec![0],
                    spacing: 6,
                    density: 0.7,
                    field_scale_m: 40.0,
                    field_threshold: -0.3,
                    max_relief: 3,
                    ..Spawn::default()
                },
            ],
        }
    }

    fn world(features: FeatureSet) -> Planet {
        Planet::new(PlanetRecipe {
            features,
            ..PlanetRecipe::default()
        })
        .unwrap()
    }

    /// A block of 40 x 40 base columns on face 4.
    fn block(p: &Planet) -> (u8, i32, i32) {
        let n = p.grid().cells();
        (4, n / 2 + 1000, n / 2 - 3000)
    }

    /// Every spawn's placements reaching the block, spawns in order.
    fn placements(p: &Planet, (face, i0, j0): (u8, i32, i32)) -> Vec<Placement> {
        let top = |f: u8, i: i32, j: i32| {
            ground(p.grid(), p.field(), f, i, j, 0, p.column_height(f, i, j, 0))
        };
        let mut cache = PlacementCache::default();
        let mut eval = FeatureEval::new(p.features(), p.grid(), p.field(), &top, &mut cache);
        (0..p.features().spawns.len())
            .flat_map(|s| eval.placements_near(s, face, [i0, i0 + 40], [j0, j0 + 40]))
            .collect()
    }

    /// Features are a function of the world: a world made again from its
    /// recipe read back from JSON places the same, and the features are in
    /// its fingerprint.
    #[test]
    fn features_are_a_function_of_the_world() {
        let a = world(set());
        let recipe: PlanetRecipe = serde_json::from_str(&a.recipe().to_json()).unwrap();
        assert_eq!(recipe.features, set());
        let b = Planet::new(recipe).unwrap();
        assert_eq!(a.recipe().fingerprint(), b.recipe().fingerprint());
        assert_ne!(
            a.recipe().fingerprint(),
            PlanetRecipe::default().fingerprint()
        );
        let (pa, pb) = (placements(&a, block(&a)), placements(&b, block(&b)));
        assert!(
            pa.iter().any(|p| p.spawn == 0) && pa.iter().filter(|p| p.spawn == 1).count() > 10,
            "{pa:?}"
        );
        assert_eq!(pa, pb);
    }

    /// Every placement's stamp reads back cell for cell (the last
    /// placement holding a cell decides it), and cells no stamp holds are
    /// the terrain's.
    #[test]
    fn stamps_stand_where_they_grow() {
        let p = world(set());
        let bare = world(FeatureSet::default());
        let (face, i0, j0) = block(&p);
        let all = placements(&p, (face, i0, j0));
        let mut solid = 0;
        for i in i0..i0 + 40 {
            for j in j0..j0 + 40 {
                let top = p.column_top(face, i, j, 0);
                for k in top - 4..top + 12 {
                    let expected = all
                        .iter()
                        .filter_map(|pl| {
                            p.features().stamps[pl.stamp as usize].at(pl.local([
                                i - pl.i,
                                k - pl.k,
                                j - pl.j,
                            ]))
                        })
                        .last();
                    let cell = Cell::new(face, i, j, k);
                    match expected {
                        Some(Some(m)) => {
                            assert_eq!((p.kind(cell), p.material(cell)), (1, m), "({i}, {j}, {k})");
                            solid += 1;
                        }
                        Some(None) => assert_eq!(p.kind(cell), 0, "({i}, {j}, {k}) cleared"),
                        None => assert_eq!(
                            p.sample_kind(0, face, i, j, k),
                            bare.sample_kind(0, face, i, j, k),
                            "({i}, {j}, {k}) terrain"
                        ),
                    }
                }
            }
        }
        assert!(solid > 300, "{solid} feature cells");
    }

    /// Anchors lie in their squares' middles, and a later spawn's footprint
    /// meets no earlier spawn's (no tree through a hut); a spawn's own
    /// placements may overlap.
    #[test]
    fn later_spawns_keep_out_of_earlier_footprints() {
        let p = world(set());
        let all = placements(&p, block(&p));
        assert!(all.iter().any(|p| p.spawn == 0));
        let mut overlapping = 0;
        for (n, a) in all.iter().enumerate() {
            let sp = &p.features().spawns[a.spawn as usize];
            for v in [a.i, a.j] {
                assert!(
                    (sp.inset..sp.spacing - sp.inset).contains(&v.rem_euclid(sp.spacing)),
                    "{a:?} off its square's middle"
                );
            }
            let (ai, aj) = a.footprint(p.features());
            for b in &all[n + 1..] {
                let (bi, bj) = b.footprint(p.features());
                let meet = ai[0] < bi[1] && bi[0] < ai[1] && aj[0] < bj[1] && bj[0] < aj[1];
                assert!(!meet || a.spawn == b.spawn, "{a:?} meets {b:?}");
                overlapping += usize::from(meet);
            }
        }
        assert!(overlapping > 0, "no crowns interlock");
    }

    /// A coarse cell is what the last near placement whose stamp holds its
    /// centre makes of it, else the terrain's (level 1 draws huts as
    /// stamps, not trees; level 2 neither, their footprints narrower than
    /// eight of its cells).
    #[test]
    fn coarse_cells_read_their_centres() {
        let p = world(set());
        let (face, i0, j0) = block(&p);
        let top = p.column_top(face, i0 + 20, j0 + 20, 0);
        let all = placements(&p, (face, i0, j0));
        assert_eq!((p.features().near(1), p.features().near(2)), (1, 0));
        let level = 1u32;
        let near: Vec<&Placement> = all
            .iter()
            .filter(|pl| p.features().drawn(pl.spawn as usize, level))
            .collect();
        let s = 1 << level;
        let (mut solid, mut air) = (0, 0);
        for ci in (i0 >> level)..((i0 + 40) >> level) {
            for cj in (j0 >> level)..((j0 + 40) >> level) {
                for ck in ((top - 8) >> level)..((top + 16) >> level) {
                    let (bi, bj, bk) = (ci * s, cj * s, ck * s);
                    let expected = near
                        .iter()
                        .filter_map(|pl| {
                            let a = pl.local([bi - pl.i, bk - pl.k, bj - pl.j]);
                            let b =
                                pl.local([bi + s - 1 - pl.i, bk + s - 1 - pl.k, bj + s - 1 - pl.j]);
                            let (lo, hi) = (
                                [0, 1, 2].map(|k| a[k].min(b[k])),
                                [0, 1, 2].map(|k| a[k].max(b[k]) + 1),
                            );
                            p.features().stamps[pl.stamp as usize].coarse(lo, hi)
                        })
                        .last();
                    solid += usize::from(matches!(expected, Some(Some(_))));
                    air += usize::from(expected == Some(None));
                    assert_eq!(
                        p.features_at(level, face, ci, cj, ck),
                        expected,
                        "level {level} ({ci}, {cj}, {ck})"
                    );
                }
            }
        }
        assert!(
            solid > 5 && air > 0,
            "level {level}: {solid} solid, {air} air cells"
        );
    }

    /// Near and far agree in coverage: past the trees' near levels their
    /// far canopy covers about as much ground as their crowns do at level 0
    /// (in a 256-column square), solid while its cells are at most twice
    /// its height.
    #[test]
    fn far_canopies_cover_what_the_crowns_cover() {
        let mut features = set();
        features.spawns[1].far_material = 11;
        features.spawns[1].max_relief = u32::MAX;
        let p = world(features);
        let fc = p.features();
        assert!(fc.drawn(1, 0) && !fc.drawn(1, 1) && fc.spawns[1].far_height == 9);
        let (face, i0, j0) = block(&p);
        let side = 256;
        let top = |f: u8, i: i32, j: i32| {
            ground(p.grid(), p.field(), f, i, j, 0, p.column_height(f, i, j, 0))
        };
        let mut cache = PlacementCache::default();
        let trees = FeatureEval::new(fc, p.grid(), p.field(), &top, &mut cache).placements_near(
            1,
            face,
            [i0, i0 + side],
            [j0, j0 + side],
        );
        let mut crowns = rustc_hash::FxHashSet::default();
        for t in &trees {
            let st = &fc.stamps[t.stamp as usize];
            for (dx, dz) in
                (-st.reach..=st.reach).flat_map(|x| (-st.reach..=st.reach).map(move |z| (x, z)))
            {
                let (i, j) = (t.i + dx, t.j + dz);
                if (i0..i0 + side).contains(&i)
                    && (j0..j0 + side).contains(&j)
                    && (st.lo[1]..st.lo[1] + st.size[1])
                        .any(|y| matches!(st.at(t.local([dx, y, dz])), Some(Some(_))))
                {
                    crowns.insert((i, j));
                }
            }
        }
        let near = crowns.len() as f64 / f64::from(side * side);
        assert!(near > 0.2, "crowns cover {near}");
        for level in 2..6u32 {
            let n = side >> level;
            let (mut covered, mut solid) = (0, 0);
            for (i, j) in
                (0..n).flat_map(|a| (0..n).map(move |b| ((i0 >> level) + a, (j0 >> level) + b)))
            {
                let g = ground(
                    p.grid(),
                    p.field(),
                    face,
                    i,
                    j,
                    level,
                    p.column_height(face, i, j, level),
                );
                let Some(cover) = fc.far_cover(p.grid(), p.field(), face, level, i, j, g) else {
                    continue;
                };
                assert_eq!((cover.0, cover.2 - cover.1), (11, 9));
                covered += 1;
                solid += usize::from(
                    ((cover.1 >> level)..=((cover.2 - 1) >> level))
                        .any(|k| p.features_at(level, face, i, j, k) == Some(Some(11))),
                );
            }
            let far = covered as f64 / f64::from(n * n);
            assert!(
                (far - near).abs() < 0.12,
                "level {level}: canopy covers {far}, crowns {near}"
            );
            // Cells of 0.4 m hold the 0.9 m canopy; cells of 3.2 m none.
            match level {
                2 => assert_eq!(solid, covered),
                5 => assert_eq!(solid, 0),
                _ => {}
            }
        }
    }

    /// Edits apply over features as over the terrain.
    #[test]
    fn edits_apply_over_features() {
        let mut p = world(set());
        let all = placements(&p, block(&p));
        let tree = *all.iter().find(|pl| pl.spawn == 1).expect("a tree");
        let grid = *p.grid();
        let crown = Cell::new(tree.face, tree.i, tree.j, tree.k + 6);
        let trunk = Cell::new(tree.face, tree.i, tree.j, tree.k + 2);
        assert!(p.solid(crown) && p.solid(trunk));
        let brush = |cell: Cell, radius: f64, shape: BrushShape, op: BrushOp| Brush {
            center: grid.cell_center(cell).to_array(),
            radius,
            shape,
            op,
            material: 3,
            height: 0.0,
        };
        p.apply(brush(
            crown,
            grid.voxel_size() * 1.5,
            BrushShape::Sphere,
            BrushOp::Remove,
        ))
        .unwrap();
        assert!(!p.solid(crown), "dug");
        p.apply(brush(
            crown,
            grid.voxel_size() * 0.5,
            BrushShape::Cube,
            BrushOp::Paint,
        ))
        .unwrap();
        assert!(!p.solid(crown), "paint keeps air air");
        p.apply(brush(
            trunk,
            grid.voxel_size() * 0.5,
            BrushShape::Cube,
            BrushOp::Paint,
        ))
        .unwrap();
        assert_eq!(p.material(trunk), 3, "paint recolours a feature's cell");
        p.apply(brush(
            Cell::new(tree.face, tree.i, tree.j, tree.k + 20),
            grid.voxel_size() * 0.5,
            BrushShape::Cube,
            BrushOp::Add,
        ))
        .unwrap();
        assert_eq!(
            p.material(Cell::new(tree.face, tree.i, tree.j, tree.k + 20)),
            3,
            "added over a tree"
        );
    }
}
