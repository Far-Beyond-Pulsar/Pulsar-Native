//! Stamps: voxel models (a tree, a house, a well) built from shapes around
//! an anchor: boxes and tapered capsules (a ball is a capsule of no length)
//! of a material or of air, the later ones over the earlier. Features place
//! them procedurally (`crate::features`, turned and mirrored by each
//! placement); an edit places one as brushes ([`Stamp::brushes`]).
//!
//! Offsets are `[x, y, z]` in cells with +y up the planet's radial axis and
//! `y = 0` the layer the anchor is in; x runs along the face's i axis and z
//! along its j axis. A shape holds the cells whose centres it holds: cell
//! `c` spans `[c, c + 1)`, its centre `c + 0.5`.
//!
//! A shape is a handful of numbers at any size, so a 10 cm tree of a
//! million cells is a few dozen shapes. Evaluation is integer arithmetic in
//! sixteenths of a cell ([`CompiledPart`]), bit for bit on the CPU and the
//! GPU (`chunk_generate.wgsl`): a base cell takes the last shape holding its
//! centre; a coarse cell the last holding its box's centre, smooth (no
//! rough edge) and solid where a sparse shape keeps at least half its
//! cells.
//!
//! Placing one as edits is a few hundred brushes, not one a cell: vertical
//! runs of a material (a trunk, a wall) become one box brush
//! ([`run_brushes`]), and every brush is history the planet keeps and
//! renderers follow.

use crate::edits::{Brush, BrushOp, BrushShape};
use crate::grid::{Cell, Grid};
use crate::noise::hash3;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

/// Sixteenths of a cell: the unit shapes are evaluated in.
pub const UNIT: i32 = 16;
/// Most cells a stamp spans each way from its anchor: evaluation's integers
/// stay in range (`CompiledPart::holds`).
pub const MAX_STAMP_REACH: i32 = 512;

/// A region of cells (offsets in cells; positions may be fractional).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Shape {
    /// The cells of the box between two corner cells (inclusive).
    Box { lo: [i32; 3], hi: [i32; 3] },
    /// Points within a radius of the segment from `a` to `b`, the radius
    /// going from `radius[0]` at `a` to `radius[1]` at `b` (a branch, a
    /// trunk; a ball with `a == b`).
    Capsule {
        a: [f32; 3],
        b: [f32; 3],
        radius: [f32; 2],
    },
    /// The same between the planes through `a` and `b` square to the
    /// segment: flat ends (a cone's tier, a column).
    Cone {
        a: [f32; 3],
        b: [f32; 3],
        radius: [f32; 2],
    },
}

/// A shape of a stamp: a material (`None`: air, clearing what stands
/// there), a share of its cells left out by hash (`sparse`, 0 to 1:
/// foliage), an edge broken cell by cell by hash (`rough`: up to this many
/// cells off its radius), and an outline broken into lumps (`lump`: up to
/// this many cells off its radius, constant over blocks of `lump_size`
/// cells, a power of two: a leaf mass's knobbly silhouette, a boulder's).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Part {
    pub shape: Shape,
    pub material: Option<u32>,
    #[serde(default)]
    pub sparse: f32,
    #[serde(default)]
    pub rough: f32,
    #[serde(default)]
    pub lump: f32,
    #[serde(default)]
    pub lump_size: u32,
}

impl Part {
    pub fn solid(shape: Shape, material: u32) -> Self {
        Self {
            shape,
            material: Some(material),
            sparse: 0.0,
            rough: 0.0,
            lump: 0.0,
            lump_size: 1,
        }
    }

    pub fn air(shape: Shape) -> Self {
        Self {
            shape,
            material: None,
            sparse: 0.0,
            rough: 0.0,
            lump: 0.0,
            lump_size: 1,
        }
    }
}

/// Shapes of material ids or air around an anchor, the later over the
/// earlier. Serialized as its parts in order.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Stamp {
    parts: Vec<Part>,
}

impl Stamp {
    pub fn new() -> Self {
        Self::default()
    }

    /// `part` over what the stamp holds.
    pub fn add(&mut self, part: Part) {
        self.parts.push(part);
    }

    /// Material `material` at cell `at` (over what was there).
    pub fn set(&mut self, at: [i32; 3], material: u32) {
        self.fill(at, at, material);
    }

    /// Air at cell `at`: whatever the ground or a neighbour put there goes.
    pub fn clear(&mut self, at: [i32; 3]) {
        self.hollow(at, at);
    }

    /// Fills the box between cells `a` and `b` (inclusive) with `material`.
    pub fn fill(&mut self, a: [i32; 3], b: [i32; 3], material: u32) {
        self.add(Part::solid(
            Shape::Box {
                lo: min3(a, b),
                hi: max3(a, b),
            },
            material,
        ));
    }

    /// Clears the box between cells `a` and `b` (inclusive).
    pub fn hollow(&mut self, a: [i32; 3], b: [i32; 3]) {
        self.add(Part::air(Shape::Box {
            lo: min3(a, b),
            hi: max3(a, b),
        }));
    }

    /// The four walls of the box between `a` and `b` (its outer cells on
    /// the x and z sides), of `material`.
    pub fn walls(&mut self, a: [i32; 3], b: [i32; 3], material: u32) {
        let (lo, hi) = (min3(a, b), max3(a, b));
        self.fill(lo, [hi[0], hi[1], lo[2]], material);
        self.fill([lo[0], lo[1], hi[2]], hi, material);
        self.fill(lo, [lo[0], hi[1], hi[2]], material);
        self.fill([hi[0], lo[1], lo[2]], hi, material);
    }

    /// A ball of `material` of `radius` cells around point `centre`.
    pub fn ball(&mut self, centre: [f32; 3], radius: f32, material: u32) {
        self.add(Part::solid(
            Shape::Capsule {
                a: centre,
                b: centre,
                radius: [radius; 2],
            },
            material,
        ));
    }

    /// A tapered capsule of `material` from point `a` (radius `radius[0]`)
    /// to `b` (`radius[1]`).
    pub fn capsule(&mut self, a: [f32; 3], b: [f32; 3], radius: [f32; 2], material: u32) {
        self.add(Part::solid(Shape::Capsule { a, b, radius }, material));
    }

    /// A flat-ended cone (frustum) of `material` from point `a` (radius
    /// `radius[0]`) to `b` (`radius[1]`).
    pub fn cone(&mut self, a: [f32; 3], b: [f32; 3], radius: [f32; 2], material: u32) {
        self.add(Part::solid(Shape::Cone { a, b, radius }, material));
    }

    /// Every part of `other` over this stamp's, offset by `at` cells.
    pub fn place(&mut self, other: &Stamp, at: [i32; 3]) {
        let f = at.map(|v| v as f32);
        self.parts.extend(other.parts.iter().map(|p| {
            let shape = match p.shape {
                Shape::Box { lo, hi } => Shape::Box {
                    lo: add3(lo, at),
                    hi: add3(hi, at),
                },
                Shape::Capsule { a, b, radius } => Shape::Capsule {
                    a: addf(a, f),
                    b: addf(b, f),
                    radius,
                },
                Shape::Cone { a, b, radius } => Shape::Cone {
                    a: addf(a, f),
                    b: addf(b, f),
                    radius,
                },
            };
            Part { shape, ..*p }
        }));
    }

    pub fn parts(&self) -> &[Part] {
        &self.parts
    }

    pub fn is_empty(&self) -> bool {
        self.parts.is_empty()
    }

    /// The parts compiled for evaluation.
    pub fn compile(&self) -> Result<Vec<CompiledPart>, String> {
        self.parts
            .iter()
            .enumerate()
            .map(|(n, p)| CompiledPart::new(p, n as u32))
            .collect()
    }

    /// The smallest and largest cells any part may hold (`None` when
    /// empty).
    pub fn bounds(&self) -> Option<([i32; 3], [i32; 3])> {
        let parts = self.compile().ok()?;
        let mut boxes = parts.iter().map(|p| (p.lo, p.hi));
        let first = boxes.next()?;
        Some(boxes.fold(first, |(lo, hi), (a, b)| (min3(lo, a), max3(hi, b))))
    }

    /// What the stamp holds at cell `at`: `None` nothing, `Some(None)` air.
    pub fn get(&self, at: [i32; 3]) -> Option<Option<u32>> {
        cell_of(&self.compile().ok()?, at)
    }

    /// The cells the stamp holds (air or a material), in no order.
    pub fn cells(&self) -> Vec<([i32; 3], Option<u32>)> {
        let Ok(parts) = self.compile() else {
            return Vec::new();
        };
        let Some((lo, hi)) = self.bounds() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for y in lo[1]..=hi[1] {
            for z in lo[2]..=hi[2] {
                for x in lo[0]..=hi[0] {
                    if let Some(v) = cell_of(&parts, [x, y, z]) {
                        out.push(([x, y, z], v));
                    }
                }
            }
        }
        out
    }

    /// The brushes that place the stamp with its origin at cell `(i, j, k)`
    /// of `face`: its air first (so it clears what stands there), then its
    /// materials, vertical runs as one brush each.
    pub fn brushes(&self, grid: &Grid, face: u8, i: i32, j: i32, k: i32) -> Vec<Brush> {
        let at = |p: [i32; 3]| Cell::new(face, i + p[0], j + p[2], k + p[1]);
        let (air, solid): (Vec<_>, Vec<_>) = self
            .cells()
            .into_iter()
            .map(|(p, m)| (at(p), m))
            .partition(|(_, m)| m.is_none());
        let mut out = run_brushes(grid, air);
        out.extend(run_brushes(grid, solid));
        out
    }
}

/// What `parts` (in order) make of base cell `at`: the last holding it.
pub fn cell_of(parts: &[CompiledPart], at: [i32; 3]) -> Option<Option<u32>> {
    let centre = at.map(|v| v * UNIT + UNIT / 2);
    let mut out = None;
    for p in parts {
        if p.holds(centre, Some(at)) {
            out = Some(p.material());
        }
    }
    out
}

/// What `parts` make of the coarse cell over base cells `[lo, hi)` (its
/// box in the stamp's frame): the last holding its centre, smooth, solid
/// where a sparse part keeps half its cells.
pub fn coarse_of(parts: &[CompiledPart], lo: [i32; 3], hi: [i32; 3]) -> Option<Option<u32>> {
    let centre = [0, 1, 2].map(|a| (lo[a] + hi[a]) * (UNIT / 2));
    let mut out = None;
    for p in parts {
        if p.holds(centre, None) {
            out = Some(p.material());
        }
    }
    out
}

/// A part as evaluation reads it (`chunk_generate.wgsl` the same, 20 words
/// a part): its kind (a box; a capsule; flat ended) and material code (0
/// air, else the material), the cells it may hold (inclusive), the
/// segment's ends and radii (sixteenths of a cell; a box's corners as `a`
/// and `b`), its edge's roughness (sixteenths) and the share of its cells
/// kept (of 256), its seed, and its lumps' depth (sixteenths) and size
/// (log2 cells).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompiledPart {
    pub capsule: bool,
    pub flat: bool,
    pub material: u32,
    pub lo: [i32; 3],
    pub hi: [i32; 3],
    pub a: [i32; 3],
    pub b: [i32; 3],
    pub radius: [i32; 2],
    pub rough: i32,
    pub keep: u32,
    pub seed: u32,
    pub lump: i32,
    pub lump_shift: u32,
}

impl CompiledPart {
    fn new(part: &Part, index: u32) -> Result<Self, String> {
        let material = match part.material {
            None => 0,
            Some(m) if (1..128).contains(&m) => m,
            // Leaves draw materials in 7 bits; 0 is the terrain's rule.
            Some(m) => return Err(format!("a stamp's material {m} is not 1 to 127")),
        };
        let units = |v: f32| (v * UNIT as f32).round() as i32;
        let keep = ((1.0 - part.sparse.clamp(0.0, 1.0)) * 256.0).round() as u32;
        let rough = units(part.rough.max(0.0));
        let seed = hash3(index as i32, 0x57a, 0x3e7, 0x9e37_79b9);
        let lump = units(part.lump.max(0.0));
        let lump_shift = part
            .lump_size
            .max(1)
            .next_power_of_two()
            .trailing_zeros()
            .min(8);
        let p = match part.shape {
            Shape::Box { lo, hi } => {
                let (lo, hi) = (min3(lo, hi), max3(lo, hi));
                Self {
                    capsule: false,
                    flat: false,
                    material,
                    lo,
                    hi,
                    a: lo.map(|v| v * UNIT),
                    b: hi.map(|v| (v + 1) * UNIT),
                    radius: [0; 2],
                    rough: 0,
                    keep,
                    seed,
                    lump: 0,
                    lump_shift: 0,
                }
            }
            Shape::Capsule { a, b, radius } | Shape::Cone { a, b, radius } => {
                let flat = matches!(part.shape, Shape::Cone { .. });
                let (a, b) = (a.map(units), b.map(units));
                let radius = [units(radius[0].max(0.0)), units(radius[1].max(0.0))];
                // Each end's reach along each axis: the larger radius (a
                // capsule's balls), or for flat ends the end's disk square
                // to the segment, `r sqrt(1 - u_k^2)` along axis k for the
                // segment's direction u, plus what `holds` rounds (its
                // closest point is found to 1/256 of the segment), never
                // past the radius: a vertical tier's box ends at its
                // planes.
                let r = radius[0].max(radius[1]);
                let d = sub3(b, a);
                let length = (dot3(d, d) as f64).sqrt();
                let reach = |end: i32, k: usize| {
                    if flat && length > 0.0 {
                        let disk = f64::from(end)
                            * (1.0 - (f64::from(d[k]) / length).powi(2)).max(0.0).sqrt();
                        r.min((disk + length / 256.0).ceil() as i32 + 2)
                    } else {
                        r
                    }
                };
                // Cells whose centre may lie within the radius.
                let lo = [0, 1, 2].map(|k| {
                    ((a[k] - reach(radius[0], k)).min(b[k] - reach(radius[1], k)) - UNIT / 2)
                        .div_euclid(UNIT)
                });
                let hi = [0, 1, 2].map(|k| {
                    ((a[k] + reach(radius[0], k)).max(b[k] + reach(radius[1], k)) - UNIT / 2)
                        .div_euclid(UNIT)
                        + 1
                });
                Self {
                    capsule: true,
                    flat,
                    material,
                    lo,
                    hi,
                    a,
                    b,
                    radius,
                    rough,
                    keep,
                    seed,
                    lump,
                    lump_shift,
                }
            }
        };
        if p.lo.iter().chain(&p.hi).any(|v| v.abs() > MAX_STAMP_REACH) {
            return Err(format!(
                "a stamp's part reaches past {MAX_STAMP_REACH} cells from its anchor"
            ));
        }
        Ok(p)
    }

    /// Its material: `None` air.
    pub fn material(&self) -> Option<u32> {
        (self.material != 0).then_some(self.material)
    }

    /// Whether it holds point `p` (sixteenths of a cell). A base cell
    /// (`cell`) is left out by hash where the part is sparse, and its edge
    /// broken by hash where it is rough; a coarse cell (`None`) is smooth,
    /// held where the part keeps half its cells. Nothing outside its cell
    /// box: there the distance test would overflow (and the box is what
    /// classification bounds a stamp by, so the GPU never asks).
    pub fn holds(&self, p: [i32; 3], cell: Option<[i32; 3]>) -> bool {
        if (0..3).any(|k| p[k] < self.lo[k] * UNIT || p[k] >= (self.hi[k] + 1) * UNIT) {
            return false;
        }
        let h = cell.map_or(0, |c| hash3(c[0], c[1], c[2], self.seed));
        match cell {
            Some(_) => {
                if (h & 255) >= self.keep {
                    return false;
                }
            }
            None => {
                if self.keep < 128 {
                    return false;
                }
            }
        }
        if !self.capsule {
            return (0..3).all(|k| p[k] >= self.a[k] && p[k] < self.b[k]);
        }
        // The closest point of the segment, at t (of 256) along it.
        let d = sub3(self.b, self.a);
        let w = sub3(p, self.a);
        let length = dot3(d, d);
        let along_d = dot3(w, d);
        if self.flat && (along_d < 0 || along_d > length) {
            return false;
        }
        let t = along(along_d, length);
        let e = sub3(w, d.map(|v| v * t / 256));
        let mut r = self.radius[0] + (self.radius[1] - self.radius[0]) * t / 256;
        if cell.is_some() {
            r -= self.rough * (((h >> 8) & 255) as i32) / 256;
        }
        if self.lump > 0 {
            // Constant over blocks of 2^lump_shift cells (the point's cell
            // block, at any level).
            let s = 4 + self.lump_shift;
            let l = hash3(p[0] >> s, p[1] >> s, p[2] >> s, self.seed ^ 0x6c75_6d70);
            r -= self.lump * ((l & 255) as i32) / 256;
        }
        r > 0 && dot3(e, e) <= r * r
    }

    /// The part as `chunk_generate.wgsl` reads it.
    pub fn gpu_words(&self) -> [u32; 20] {
        let i = |v: i32| v as u32;
        [
            u32::from(self.capsule) | (u32::from(self.flat) << 1) | (self.material << 8),
            i(self.lo[0]),
            i(self.lo[1]),
            i(self.lo[2]),
            i(self.hi[0]),
            i(self.hi[1]),
            i(self.hi[2]),
            i(self.a[0]),
            i(self.a[1]),
            i(self.a[2]),
            i(self.b[0]),
            i(self.b[1]),
            i(self.b[2]),
            i(self.radius[0]) | (i(self.radius[1]) << 16),
            i(self.rough) | (self.keep << 16),
            self.seed,
            i(self.lump),
            self.lump_shift,
            0,
            0,
        ]
    }
}

/// How far along a segment of squared length `length` its closest point to
/// a point lies (of 256), its offset's dot product with the segment being
/// `dot`: both shifted by the same bits so the product stays in range.
fn along(dot: i32, length: i32) -> i32 {
    if length <= 0 {
        return 0;
    }
    let dot = dot.clamp(0, length);
    let shift = (32 - length.leading_zeros() as i32 - 22).max(0);
    ((dot >> shift) * 256) / (length >> shift).max(1)
}

/// The brush that makes `cell` of `material`, or air (`None`): a cube of the
/// cell exactly.
pub fn cell_brush(grid: &Grid, cell: Cell, material: Option<u32>) -> Brush {
    let (op, material) = material.map_or((BrushOp::Remove, 0), |m| (BrushOp::Add, m));
    Brush {
        center: grid.cell_center(cell).to_array(),
        radius: grid.voxel_size() * 0.5,
        shape: BrushShape::Cube,
        op,
        material,
        height: 0.0,
    }
}

/// A flat-topped box brush over `columns` x `columns` columns (odd, centred
/// on column `(i, j)` of `face`), `layers` tall from layer `k`, of
/// `material` or air: one brush fills or clears what would take thousands
/// of cells (a building's footprint).
pub fn column_box(
    grid: &Grid,
    face: u8,
    i: i32,
    j: i32,
    k: i32,
    columns: i32,
    layers: i32,
    material: Option<u32>,
) -> Brush {
    let voxel = grid.voxel_size();
    let centre = grid.position(
        face,
        [
            f64::from(i) + 0.5,
            f64::from(j) + 0.5,
            f64::from(k) + f64::from(layers) * 0.5,
        ],
    );
    let (op, material) = material.map_or((BrushOp::Remove, 0), |m| (BrushOp::Add, m));
    Brush {
        center: centre.to_array(),
        radius: f64::from(columns) * voxel * 0.5,
        shape: BrushShape::Cube,
        op,
        material,
        height: f64::from(layers) * voxel * 0.5,
    }
}

/// Brushes for `cells`, a vertical run of one material (or of air) in a
/// column made one box brush. Later cells of the list win where it names a
/// cell twice.
pub fn run_brushes(grid: &Grid, cells: Vec<(Cell, Option<u32>)>) -> Vec<Brush> {
    let mut unique: FxHashMap<Cell, Option<u32>> = FxHashMap::default();
    for (cell, m) in cells {
        unique.insert(cell, m);
    }
    let mut cells: Vec<(Cell, Option<u32>)> = unique.into_iter().collect();
    cells.sort_by_key(|(c, m)| (c.face, c.i, c.j, *m, c.k));
    let mut out = Vec::new();
    let mut run: Option<(Cell, Option<u32>, i32)> = None;
    let flush = |run: Option<(Cell, Option<u32>, i32)>, out: &mut Vec<Brush>| {
        if let Some((start, m, length)) = run {
            out.push(if length == 1 {
                cell_brush(grid, start, m)
            } else {
                column_box(grid, start.face, start.i, start.j, start.k, 1, length, m)
            });
        }
    };
    for (cell, m) in cells {
        run = match run {
            Some((start, rm, length))
                if rm == m
                    && start.face == cell.face
                    && start.i == cell.i
                    && start.j == cell.j
                    && start.k + length == cell.k =>
            {
                Some((start, rm, length + 1))
            }
            other => {
                flush(other, &mut out);
                Some((cell, m, 1))
            }
        };
    }
    flush(run, &mut out);
    out
}

fn min3(a: [i32; 3], b: [i32; 3]) -> [i32; 3] {
    [a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2])]
}

fn max3(a: [i32; 3], b: [i32; 3]) -> [i32; 3] {
    [a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2])]
}

fn add3(a: [i32; 3], b: [i32; 3]) -> [i32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn addf(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub3(a: [i32; 3], b: [i32; 3]) -> [i32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot3(a: [i32; 3], b: [i32; 3]) -> i32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Planet, PlanetRecipe};

    /// A part's box holds every cell it holds: flat-ended cones of any
    /// tilt, taper and length, rough and lumpy, whose boxes end near their
    /// disks, against every cell within their radius of the segment.
    #[test]
    fn a_cone_box_holds_its_cells() {
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 11) as f32 / (1u64 << 53) as f32
        };
        for n in 0..300 {
            let a = [
                next() * 40.0 - 20.0,
                next() * 40.0 - 20.0,
                next() * 40.0 - 20.0,
            ];
            let tilt = if n % 3 == 0 {
                [0.0, 1.0, 0.0]
            } else {
                [next() - 0.5, next() - 0.5, next() - 0.5]
            };
            let length = next() * 30.0;
            let b = [
                a[0] + tilt[0] * length,
                a[1] + tilt[1] * length,
                a[2] + tilt[2] * length,
            ];
            let radius = [next() * 12.0, next() * 12.0];
            let part = Part {
                rough: next() * 2.0,
                lump: next() * 2.0,
                lump_size: 2,
                ..Part::solid(Shape::Cone { a, b, radius }, 5)
            };
            let compiled = CompiledPart::new(&part, n).unwrap();
            let r = radius[0].max(radius[1]).ceil() as i32 + 1;
            for x in (a[0].min(b[0]) as i32 - r)..=(a[0].max(b[0]) as i32 + r) {
                for y in (a[1].min(b[1]) as i32 - r)..=(a[1].max(b[1]) as i32 + r) {
                    for z in (a[2].min(b[2]) as i32 - r)..=(a[2].max(b[2]) as i32 + r) {
                        if cell_of(&[compiled], [x, y, z]).is_some() {
                            let inside = (0..3).all(|k| {
                                compiled.lo[k] <= [x, y, z][k] && [x, y, z][k] <= compiled.hi[k]
                            });
                            assert!(
                                inside,
                                "cone {n} {a:?} {b:?} {radius:?}: {:?} outside {:?} {:?}",
                                [x, y, z],
                                compiled.lo,
                                compiled.hi
                            );
                        }
                    }
                }
            }
        }
    }

    /// A capsule holds nothing far outside its box, where its distance test
    /// would overflow (a Moon boulder once held cells 290 m over its anchor).
    #[test]
    fn a_capsule_holds_nothing_far_outside_its_box() {
        let part = Part::solid(
            Shape::Capsule {
                a: [0.0, 0.0, 0.0],
                b: [0.0, 4.0, 0.0],
                radius: [6.0, 3.0],
            },
            19,
        );
        let compiled = CompiledPart::new(&part, 9).unwrap();
        assert!(cell_of(&[compiled], [0, 2, 0]).is_some());
        for y in [2902, 3000, -2902, 100_000] {
            for (x, z) in [(31, 30), (0, 0), (-500, 500)] {
                assert_eq!(cell_of(&[compiled], [x, y, z]), None, "{:?}", [x, y, z]);
            }
        }
    }

    /// A placed stamp reads back cell for cell: its materials where it set
    /// them, air where it cleared, in a handful of run brushes.
    #[test]
    fn a_placed_stamp_reads_back_cell_for_cell() {
        let mut planet = Planet::new(PlanetRecipe::default()).unwrap();
        let grid = *planet.grid();
        let (face, i, j) = (2u8, grid.cells() / 2, grid.cells() / 2);
        let k = planet.column_top(face, i, j, 0) + 1;
        let mut house = Stamp::new();
        house.fill([-3, -1, -2], [3, -1, 2], 7);
        house.walls([-3, 0, -2], [3, 3, 2], 12);
        house.hollow([0, 0, -2], [0, 1, -2]);
        house.fill([-2, 0, -1], [2, 3, 1], 7);
        house.hollow([-2, 0, -1], [2, 3, 1]);
        house.ball([0.5, 6.5, 0.5], 2.0, 9);
        let cells = house.cells();
        let brushes = house.brushes(&grid, face, i, j, k);
        assert!(
            brushes.len() * 2 < cells.len(),
            "{} brushes for {} cells",
            brushes.len(),
            cells.len()
        );
        for brush in brushes {
            planet.apply(brush).unwrap();
        }
        for (p, m) in cells {
            let cell = Cell::new(face, i + p[0], j + p[2], k + p[1]);
            assert_eq!(planet.solid(cell), m.is_some(), "{p:?}");
            if let Some(m) = m {
                assert_eq!(planet.material(cell), m, "{p:?}");
            }
        }
    }

    /// Shapes hold the cells whose centres they hold: a ball of radius 2
    /// about a cell's centre holds the 33 cells within 2 of it, a capsule's
    /// radius tapers along it, and a sparse part keeps about its share.
    #[test]
    fn shapes_hold_the_cells_whose_centres_they_hold() {
        let mut ball = Stamp::new();
        ball.ball([0.5, 0.5, 0.5], 2.0, 5);
        let expected = (-2..=2)
            .flat_map(|x| (-2..=2).flat_map(move |y| (-2..=2).map(move |z| x * x + y * y + z * z)))
            .filter(|&d| d <= 4)
            .count();
        assert_eq!(ball.cells().len(), expected);
        let mut cone = Stamp::new();
        cone.capsule([0.5, 0.0, 0.5], [0.5, 20.0, 0.5], [3.0, 0.5], 5);
        let width = |y: i32| (-4..=4).filter(|&x| cone.get([x, y, 0]).is_some()).count();
        assert!(
            width(1) > width(10) && width(10) > width(19),
            "{} {} {}",
            width(1),
            width(10),
            width(19)
        );
        assert!(cone.get([0, -2, 0]).is_some(), "a capsule's end is round");
        let mut flat = Stamp::new();
        flat.cone([0.5, 0.0, 0.5], [0.5, 20.0, 0.5], [3.0, 0.5], 5);
        assert_eq!(
            (
                flat.get([0, -1, 0]),
                flat.get([0, 0, 0]),
                flat.get([2, 0, 0])
            ),
            (None, Some(Some(5)), Some(Some(5))),
            "a cone's end is flat"
        );
        let mut leaves = Stamp::new();
        leaves.add(Part {
            sparse: 0.25,
            ..Part::solid(
                Shape::Capsule {
                    a: [0.0; 3],
                    b: [0.0; 3],
                    radius: [8.0; 2],
                },
                5,
            )
        });
        let kept = leaves.cells().len() as f64;
        let mut whole = Stamp::new();
        whole.ball([0.0; 3], 8.0, 5);
        let share = kept / whole.cells().len() as f64;
        assert!((share - 0.75).abs() < 0.05, "{share}");
        // Lumps break the outline in blocks: fewer cells than the ball,
        // more than the ball shrunk by the whole lump, the same through
        // each block of cells.
        let mut lumpy = Stamp::new();
        lumpy.add(Part {
            lump: 3.0,
            lump_size: 4,
            ..Part::solid(
                Shape::Capsule {
                    a: [0.0; 3],
                    b: [0.0; 3],
                    radius: [8.0; 2],
                },
                5,
            )
        });
        let mut small = Stamp::new();
        small.ball([0.0; 3], 5.0, 5);
        let n = lumpy.cells().len();
        assert!(n < whole.cells().len() && n > small.cells().len(), "{n}");
    }

    /// A coarse cell takes the last part holding its box's centre: a box
    /// of eight cells is one cell of level 1 where it lines up, and air
    /// holding the centre clears it.
    #[test]
    fn coarse_cells_read_their_centre() {
        let mut s = Stamp::new();
        s.fill([0, 0, 0], [1, 1, 1], 4);
        let parts = s.compile().unwrap();
        assert_eq!(coarse_of(&parts, [0, 0, 0], [2, 2, 2]), Some(Some(4)));
        assert_eq!(coarse_of(&parts, [2, 0, 0], [4, 2, 2]), None);
        s.hollow([0, 0, 0], [0, 0, 0]);
        assert_eq!(
            coarse_of(&s.compile().unwrap(), [0, 0, 0], [2, 2, 2]),
            Some(Some(4)),
            "cell (0, 0, 0) ends at the centre"
        );
        s.hollow([1, 1, 1], [1, 1, 1]);
        assert_eq!(
            coarse_of(&s.compile().unwrap(), [0, 0, 0], [2, 2, 2]),
            Some(None),
            "cell (1, 1, 1) starts at it"
        );
    }
}
