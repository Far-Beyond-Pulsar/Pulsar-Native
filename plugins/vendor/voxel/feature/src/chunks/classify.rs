//! What each node of the chunk octree holds (`docs/chunk-octree.md`): from
//! its footprint's measured (or estimated) ranges and the edits over it,
//! a node is Mixed (generated into a chunk), Buried (the field's volume
//! under its surface: rock until a ray reaches it), or uniform Empty or
//! Solid (drawn without data).

use super::CHUNK;
use crate::edit_store::EditStore;
use crate::edits::{center_half, FaceBrush};
use crate::grid::Grid;
use rustc_hash::{FxHashMap, FxHashSet};

/// Cells of its level a node's descendants' surface may lie past its
/// footprint's band (`EditView::classify`).
const DESCENT: i32 = 2;

/// The 64x64 columns of one level over a stack of nodes: the unit of
/// column generation and of classification, in volume `volume` (a world's
/// index of it, `chunks::VolumeId`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Footprint {
    pub volume: u16,
    pub face: u8,
    pub level: u32,
    pub cx: i32,
    pub cy: i32,
}

impl Footprint {
    pub fn node(self, cz: i32) -> NodeKey {
        NodeKey {
            volume: self.volume,
            face: self.face,
            level: self.level,
            cx: self.cx,
            cy: self.cy,
            cz,
        }
    }

    /// Its columns' half-cell bounds along i and j (inclusive).
    pub fn span_half(self) -> ((i64, i64), (i64, i64)) {
        let shift = self.level + 1;
        let span = |c: i32| {
            (
                i64::from(c * CHUNK) << shift,
                (i64::from(c * CHUNK + CHUNK) << shift) - 1,
            )
        };
        (span(self.cx), span(self.cy))
    }
}

/// A node of the octree: 64^3 cells of `level`, layer `cz` over footprint
/// (`face`, `level`, `cx`, `cy`) of volume `volume`. Its key in the chunk
/// table is `(face | level << 8 | volume << 16, cx, cy, cz)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodeKey {
    pub volume: u16,
    pub face: u8,
    pub level: u32,
    pub cx: i32,
    pub cy: i32,
    pub cz: i32,
}

impl NodeKey {
    pub fn footprint(self) -> Footprint {
        Footprint {
            volume: self.volume,
            face: self.face,
            level: self.level,
            cx: self.cx,
            cy: self.cy,
        }
    }

    pub fn table_key(self) -> [i32; 4] {
        [
            i32::from(self.face) | ((self.level as i32) << 8) | (i32::from(self.volume) << 16),
            self.cx,
            self.cy,
            self.cz,
        ]
    }

    pub fn from_table_key(k: [i32; 4]) -> Self {
        Self {
            volume: (k[0] >> 16) as u16,
            face: (k[0] & 0xff) as u8,
            level: ((k[0] >> 8) & 0xff) as u32,
            cx: k[1],
            cy: k[2],
            cz: k[3],
        }
    }

    pub fn parent(self) -> Self {
        Self {
            volume: self.volume,
            face: self.face,
            level: self.level + 1,
            cx: self.cx >> 1,
            cy: self.cy >> 1,
            cz: self.cz >> 1,
        }
    }

    /// The eight nodes under it (none at level 0); those past the face's
    /// edge (`tiles` footprints along it at the finer level) are left out.
    pub fn children(self, tiles: i32) -> impl Iterator<Item = NodeKey> {
        let level = self.level.checked_sub(1);
        (0..8).filter_map(move |n| {
            let level = level?;
            let c = Self {
                volume: self.volume,
                face: self.face,
                level,
                cx: self.cx * 2 + (n & 1),
                cy: self.cy * 2 + ((n >> 1) & 1),
                cz: self.cz * 2 + (n >> 2),
            };
            (c.cx < tiles && c.cy < tiles).then_some(c)
        })
    }

    /// Its layers [lo, hi) in cells of its level.
    pub fn layers(self) -> (i32, i32) {
        (self.cz * CHUNK, self.cz * CHUNK + CHUNK)
    }
}

/// What a node holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A surface may cross it: generated into a chunk.
    Mixed,
    /// The field's volume (caves) under the surface band: drawn as rock
    /// until a ray hits it, then generated.
    Buried,
    Empty,
    Solid,
}

/// Layers (level cells, [lo, hi)) a footprint's field may change: its
/// volume (caves, overhangs) and the band around its surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ranges {
    pub volume: (i32, i32),
    pub surface: (i32, i32),
}

impl Ranges {
    /// A child footprint's ranges from its parent's: the same heights in
    /// cells half as tall, widened by a parent cell (a level stays within
    /// its parent's cell of the parent's surface).
    pub fn child(self) -> Self {
        let half = |(lo, hi): (i32, i32)| (lo.saturating_mul(2) - 2, hi.saturating_mul(2) + 2);
        Self {
            volume: half(self.volume),
            surface: half(self.surface),
        }
    }

    /// The ranges holding all of `parts` (none if none is).
    pub fn hull(parts: &[Option<Ranges>]) -> Option<Self> {
        let join = |a: (i32, i32), b: (i32, i32)| (a.0.min(b.0), a.1.max(b.1));
        parts.iter().flatten().copied().reduce(|a, b| Self {
            volume: join(a.volume, b.volume),
            surface: join(a.surface, b.surface),
        })
    }
}

/// A footprint's nodes by layer: Mixed in `mixed` (chunk layers,
/// inclusive), Buried in `buried`, else uniform ([`EditView::kind`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Class {
    pub mixed: Vec<(i32, i32)>,
    pub buried: (i32, i32),
    /// The field's volume floor (level cells): rock under it, unless an
    /// edit says otherwise.
    volume_floor: i32,
    /// Brushes that add or remove over the footprint (indices in
    /// `EditView::faces`), in order.
    shapes: Vec<u32>,
}

/// The edits as classification reads them: brushes kept analytic,
/// resolved per face, and the baked store. Shared by the render side and
/// the planner's thread.
pub struct EditView {
    pub grid: Grid,
    /// The large layer's faces (the first `large_faces`), then the recent
    /// brushes' (`recent_counts` a brush).
    pub faces: Faces,
    /// Where the large faces are (`LargeIndex`: a seal adds to it); where
    /// the recent ones are.
    pub index: LargeIndex,
    pub recent_index: BrushIndex,
    /// Base layers the adding and removing brushes reach (the planner's
    /// roots cover them), the large layer's kept with it.
    pub reach: Option<(i32, i32)>,
    pub large_reach: Option<(i32, i32)>,
    pub large_faces: usize,
    /// Large brushes the view holds, and the history's hash at the last of
    /// them (`EditLog::prefix_hash`): a later view extends them while the
    /// large layer's first ones keep that hash.
    pub large_brushes: usize,
    pub large_prefix: u64,
    pub recent_counts: Vec<usize>,
    pub baked: EditStore,
    /// Where the baked bricks are (shared while the baked store is
    /// unchanged).
    pub bricks: std::sync::Arc<BrickIndex>,
    /// Bumped whenever the edits change.
    pub version: u64,
    /// Base layers under the surface where any level's cells may differ
    /// from the heightfield (`TerrainField::volume_bounds`): caves a coarse
    /// level cannot show still lie there, so its nodes are Buried, not
    /// Solid, and refine towards them.
    pub volume_depth: i32,
}

/// Where an edit store's bricks are, at every level they show and over
/// them: a node whose descendants hold baked edits is Mixed though its own
/// level's cells, built by majority, may not show them (a 4 m shaft in
/// 6.4 m cells), so the cut refines down to them. Kept in shared runs, the
/// latest smallest, merged like a binary counter: a seal adds a run of its
/// own bricks (it once copied the whole index).
#[derive(Clone, Default)]
pub struct BrickIndex {
    /// The volume whose store it indexes (its keys').
    pub volume: u16,
    runs: Vec<std::sync::Arc<BrickRun>>,
    /// Base layers the bricks span.
    pub layers: Option<(i32, i32)>,
}

/// Bricks of a [`BrickIndex`]: footprints holding bricks of their level,
/// each footprint's layers of nodes holding bricks or over them, and how
/// many bricks it came from.
#[derive(Clone, Default)]
struct BrickRun {
    footprints: FxHashSet<Footprint>,
    nodes: FxHashMap<Footprint, Vec<i32>>,
    bricks: usize,
}

impl BrickIndex {
    pub fn new(grid: &Grid, store: &EditStore, volume: u16) -> Self {
        let mut index = Self {
            volume,
            ..Self::default()
        };
        index.add(grid, store.bricks().map(|(k, _)| k).collect());
        index
    }

    /// This index with the bricks of `columns` added (a seal: bricks are
    /// only added until an unseal restores them, which needs [`Self::new`]).
    pub fn extended(
        &self,
        grid: &Grid,
        store: &EditStore,
        columns: impl Iterator<Item = crate::edit_store::ColumnKey>,
    ) -> Self {
        let mut index = self.clone();
        let keys: Vec<crate::edit_store::BrickKey> = columns
            .filter_map(|c| {
                store.column(&c).map(|column| {
                    column
                        .iter()
                        .map(move |(bk, _)| crate::edit_store::BrickKey {
                            face: c.face,
                            level: c.level,
                            bi: c.ci,
                            bj: c.cj,
                            bk: *bk,
                        })
                        .collect::<Vec<_>>()
                })
            })
            .flatten()
            .collect();
        index.add(grid, keys);
        index
    }

    /// Whether `fp` holds bricks of its level.
    pub fn holds(&self, fp: &Footprint) -> bool {
        self.runs.iter().any(|r| r.footprints.contains(fp))
    }

    /// The layers of `fp`'s nodes holding bricks or over them (a layer may
    /// come more than once).
    pub fn node_layers(&self, fp: &Footprint) -> impl Iterator<Item = i32> + '_ {
        let fp = *fp;
        self.runs
            .iter()
            .flat_map(move |r| r.nodes.get(&fp).into_iter().flatten().copied())
    }

    fn holds_node(&self, node: &NodeKey) -> bool {
        let fp = node.footprint();
        self.runs
            .iter()
            .any(|r| r.nodes.get(&fp).is_some_and(|l| l.contains(&node.cz)))
    }

    fn add(&mut self, grid: &Grid, keys: Vec<crate::edit_store::BrickKey>) {
        if keys.is_empty() {
            return;
        }
        let top = grid.levels() - 1;
        let mut run = BrickRun {
            bricks: keys.len(),
            ..Default::default()
        };
        for k in keys {
            let level = u32::from(k.level);
            run.footprints.insert(Footprint {
                volume: self.volume,
                face: k.face,
                level,
                cx: k.bi >> 3,
                cy: k.bj >> 3,
            });
            let (lo, hi) = ((k.bk * 8) << level, (k.bk * 8 + 8) << level);
            self.layers = Some(
                self.layers
                    .map_or((lo, hi), |(a, b)| (a.min(lo), b.max(hi))),
            );
            // A node marked is marked with its ancestors.
            let mut node = NodeKey {
                volume: self.volume,
                face: k.face,
                level,
                cx: k.bi >> 3,
                cy: k.bj >> 3,
                cz: k.bk >> 3,
            };
            loop {
                if self.holds_node(&node) {
                    break;
                }
                let layers = run.nodes.entry(node.footprint()).or_default();
                if layers.contains(&node.cz) {
                    break;
                }
                layers.push(node.cz);
                if node.level >= top {
                    break;
                }
                node = node.parent();
            }
        }
        self.runs.push(std::sync::Arc::new(run));
        while self.runs.len() >= 2 {
            let n = self.runs.len();
            if self.runs[n - 2].bricks > self.runs[n - 1].bricks {
                break;
            }
            let last = self.runs.pop().expect("two runs");
            let mut merged = (*self.runs.pop().expect("two runs")).clone();
            merged.bricks += last.bricks;
            merged.footprints.extend(last.footprints.iter().copied());
            for (fp, layers) in &last.nodes {
                let into = merged.nodes.entry(*fp).or_default();
                for cz in layers {
                    if !into.contains(cz) {
                        into.push(*cz);
                    }
                }
            }
            self.runs.push(std::sync::Arc::new(merged));
        }
    }
}

/// Faces in shared runs, only the last of which grows: a copy shares every
/// run, and extending it copies at most the last.
const RUN: usize = 4096;

/// An edit view's faces: the large layer's in shared runs (a later view
/// extends them by pointer, so an edit or a seal copies at most a run, not
/// the history), then the recent brushes'.
#[derive(Clone, Default)]
pub struct Faces {
    runs: Vec<std::sync::Arc<Vec<FaceBrush>>>,
    large: usize,
    recent: Vec<FaceBrush>,
}

impl Faces {
    pub fn len(&self) -> usize {
        self.large + self.recent.len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// The large layer's faces.
    pub fn large_len(&self) -> usize {
        self.large
    }
    /// The large layer extended by `faces`.
    pub fn extend_large(&mut self, faces: impl IntoIterator<Item = FaceBrush>) {
        for fb in faces {
            if self.runs.last().is_none_or(|r| r.len() >= RUN) {
                self.runs.push(std::sync::Arc::new(Vec::with_capacity(RUN)));
            }
            std::sync::Arc::make_mut(self.runs.last_mut().expect("pushed")).push(fb);
            self.large += 1;
        }
    }
    /// The recent brushes' faces, replaced.
    pub fn set_recent(&mut self, faces: Vec<FaceBrush>) {
        self.recent = faces;
    }
    pub fn iter(&self) -> impl Iterator<Item = &FaceBrush> + '_ {
        self.runs.iter().flat_map(|r| r.iter()).chain(&self.recent)
    }
    /// Faces from `from` on, as contiguous slices with the index of their
    /// first.
    pub fn slices_from(&self, from: usize) -> Vec<(usize, &[FaceBrush])> {
        let mut out = Vec::new();
        let mut at = 0;
        for run in self
            .runs
            .iter()
            .map(|r| r.as_slice())
            .chain([self.recent.as_slice()])
        {
            let end = at + run.len();
            if end > from && !run.is_empty() {
                let skip = from.saturating_sub(at);
                out.push((at + skip, &run[skip..]));
            }
            at = end;
        }
        out
    }
    /// Faces from `from` on (without walking those before).
    pub fn iter_from(&self, from: usize) -> impl Iterator<Item = &FaceBrush> + '_ {
        self.slices_from(from)
            .into_iter()
            .flat_map(|(_, run)| run.iter())
    }
    /// The faces `[from, to)` in one list.
    pub fn collect(&self, from: usize, to: usize) -> Vec<FaceBrush> {
        self.iter_from(from).take(to - from).copied().collect()
    }
}

impl std::ops::Index<usize> for Faces {
    type Output = FaceBrush;
    fn index(&self, n: usize) -> &FaceBrush {
        if n < self.large {
            &self.runs[n / RUN][n % RUN]
        } else {
            &self.recent[n - self.large]
        }
    }
}

/// Where the large layer's faces are: indices of runs of them, the latest
/// smallest. A seal's faces join as a run of their own and a run no larger
/// than the one after it merges with it (a binary counter), so a face is indexed again O(log n)
/// times over the history, not at every seal (each large brush once
/// rebuilt the whole index).
#[derive(Clone, Default)]
pub struct LargeIndex {
    /// Each run's first face, faces and index.
    runs: Vec<(usize, usize, std::sync::Arc<BrushIndex>)>,
}

impl LargeIndex {
    /// The large faces of `faces` from `from` (the faces indexed so far)
    /// added.
    pub fn extend(&mut self, faces: &Faces, from: usize) {
        let to = faces.large_len();
        if to <= from {
            return;
        }
        self.runs.push((
            from,
            to - from,
            std::sync::Arc::new(BrushIndex::new(&faces.collect(from, to), from as u32, true)),
        ));
        while self.runs.len() >= 2 {
            let (a, b) = (
                &self.runs[self.runs.len() - 2],
                &self.runs[self.runs.len() - 1],
            );
            if a.1 > b.1 {
                break;
            }
            let (first, count) = (a.0, a.1 + b.1);
            self.runs.truncate(self.runs.len() - 2);
            self.runs.push((
                first,
                count,
                std::sync::Arc::new(BrushIndex::new(
                    &faces.collect(first, first + count),
                    first as u32,
                    true,
                )),
            ));
        }
    }

    /// [`BrushIndex::query`] over every run, in the faces' order.
    pub fn query(&self, face: u8, i: (i64, i64), j: (i64, i64), level: u32) -> Vec<u32> {
        let mut out = Vec::new();
        for (_, _, index) in &self.runs {
            out.extend(index.query(face, i, j, level));
        }
        out
    }
}

/// Base layers (inclusive) the adding and removing faces of `faces`
/// reach, joined with `reach`.
pub fn faces_reach<'a>(
    reach: Option<(i32, i32)>,
    faces: impl IntoIterator<Item = &'a FaceBrush>,
) -> Option<(i32, i32)> {
    faces
        .into_iter()
        .filter(|fb| fb.op() <= 1)
        .fold(reach, |r, fb| {
            let (lo, hi) = (fb.k_lo.div_euclid(2), fb.k_hi.div_euclid(2));
            Some(r.map_or((lo, hi), |(a, b)| (a.min(lo), b.max(hi))))
        })
}

/// Where the analytic brushes are on each face: a loose grid of levels
/// (buckets `2^(BUCKET_LOG2 + n)` half cells wide), each brush in the
/// smallest level where it spans at most two buckets a side. A query reads,
/// level by level, the buckets over its rectangle (past `SCAN_BUCKETS` of
/// them, the level's entries instead), so it costs the brushes near it, not
/// the whole history's. Brushes are kept apart by the levels they apply at
/// (`top`: below their size's, and a large brush below its bake level), so
/// a coarse footprint's query skips those baked cells hold.
#[derive(Default)]
pub struct BrushIndex {
    parts: Vec<(u32, Vec<FxHashMap<(u8, i64, i64), Vec<u32>>>)>,
}

/// The levels `fb` applies at, `0..top`: those it spans half a cell of, and
/// a large-layer brush's below its bake level.
pub fn brush_top(fb: &FaceBrush, large: bool) -> u32 {
    let size = 32 - fb.radius_half.leading_zeros();
    if large {
        size.min(crate::planet::bake_level(fb))
    } else {
        size
    }
}

/// The finest buckets: 4096 half cells (204.8 m at 0.1 m voxels).
const BUCKET_LOG2: u32 = 12;
/// Buckets a query reads one by one at a level, at most.
const SCAN_BUCKETS: i64 = 64;

/// `fb`'s half-cell extent along i and j (inclusive).
fn brush_extent(fb: &FaceBrush) -> ((i64, i64), (i64, i64)) {
    let w = i64::from(fb.center[3]);
    let (ci, cj) = (i64::from(fb.center[0]), i64::from(fb.center[1]));
    ((ci - w, ci + w), (cj - w, cj + w))
}

impl BrushIndex {
    /// The index of `faces`, numbered from `first`; `large`: the large
    /// layer's.
    pub fn new(faces: &[FaceBrush], first: u32, large: bool) -> Self {
        let mut index = Self::default();
        for (n, fb) in faces.iter().enumerate() {
            let top = brush_top(fb, large);
            let part = match index.parts.iter().position(|(t, _)| *t == top) {
                Some(part) => part,
                None => {
                    index.parts.push((top, Vec::new()));
                    index.parts.len() - 1
                }
            };
            let levels = &mut index.parts[part].1;
            let (i, j) = brush_extent(fb);
            let mut level = 0;
            while BUCKET_LOG2 + level < 62
                && ((i.1 >> (BUCKET_LOG2 + level)) - (i.0 >> (BUCKET_LOG2 + level)) > 1
                    || (j.1 >> (BUCKET_LOG2 + level)) - (j.0 >> (BUCKET_LOG2 + level)) > 1)
            {
                level += 1;
            }
            if levels.len() <= level as usize {
                levels.resize_with(level as usize + 1, FxHashMap::default);
            }
            let s = BUCKET_LOG2 + level;
            for bi in (i.0 >> s)..=(i.1 >> s) {
                for bj in (j.0 >> s)..=(j.1 >> s) {
                    levels[level as usize]
                        .entry((fb.face(), bi, bj))
                        .or_default()
                        .push(first + n as u32);
                }
            }
        }
        index
    }

    /// Brushes of `face` that apply at `level` and whose extent may meet
    /// half-cell rectangle `i`, `j` (inclusive), in their order. A superset:
    /// callers test each exactly (`brush_meets`).
    pub fn query(&self, face: u8, i: (i64, i64), j: (i64, i64), level: u32) -> Vec<u32> {
        let mut out = Vec::new();
        for (size, buckets) in self
            .parts
            .iter()
            .filter(|(top, _)| level < *top)
            .flat_map(|(_, sizes)| sizes.iter().enumerate())
        {
            let s = BUCKET_LOG2 + size as u32;
            let (bi, bj) = ((i.0 >> s, i.1 >> s), (j.0 >> s, j.1 >> s));
            if (bi.1 - bi.0 + 1).saturating_mul(bj.1 - bj.0 + 1) <= SCAN_BUCKETS {
                for x in bi.0..=bi.1 {
                    for y in bj.0..=bj.1 {
                        out.extend_from_slice(
                            buckets.get(&(face, x, y)).map_or(&[][..], Vec::as_slice),
                        );
                    }
                }
            } else {
                for (&(f, x, y), brushes) in buckets {
                    if f == face && (bi.0..=bi.1).contains(&x) && (bj.0..=bj.1).contains(&y) {
                        out.extend_from_slice(brushes);
                    }
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// Whether `fb` can touch cells of `level` with half-cell bounds `i`, `j`,
/// `k` (inclusive).
pub fn brush_meets(
    fb: &FaceBrush,
    face: u8,
    level: u32,
    i: (i64, i64),
    j: (i64, i64),
    k: (i64, i64),
) -> bool {
    if fb.face() != face || fb.radius_half < (1u32 << level) {
        return false;
    }
    let w = i64::from(fb.center[3]);
    let (ci, cj) = (i64::from(fb.center[0]), i64::from(fb.center[1]));
    ci + w >= i.0
        && ci - w <= i.1
        && cj + w >= j.0
        && cj - w <= j.1
        && i64::from(fb.k_hi) >= k.0
        && i64::from(fb.k_lo) <= k.1
}

impl EditView {
    /// Brushes that add or remove and can touch `fp`'s columns, with their
    /// indices: at any size (one too small to change a cell of this level
    /// still makes the nodes it crosses Mixed, so the cut refines to it).
    fn shapes_over(&self, fp: Footprint) -> impl Iterator<Item = (usize, &FaceBrush)> {
        let (i, j) = fp.span_half();
        self.brushes_near(fp.face, i, j, 0)
            .into_iter()
            .map(|n| (n as usize, &self.faces[n as usize]))
            .filter(move |(_, fb)| {
                fb.op() <= 1 && brush_meets(fb, fp.face, 0, i, j, (i64::MIN / 4, i64::MAX / 4))
            })
    }

    /// Analytic brushes (indices into `faces`, in their order) that apply
    /// at `level` and whose extent may meet half-cell rectangle `i`, `j`.
    pub fn brushes_near(&self, face: u8, i: (i64, i64), j: (i64, i64), level: u32) -> Vec<u32> {
        let mut near = self.index.query(face, i, j, level);
        near.extend(self.recent_index.query(face, i, j, level));
        near
    }

    /// Layers [lo, hi) of `fp`'s level where brush `fb`'s surface can be
    /// over the footprint, with a cell of margin (none: lo >= hi). A box's
    /// heights; a ball's, the heights of its sphere's lower and upper caps
    /// over the footprint (one band where it reaches the sphere's
    /// silhouette; the air inside holds nothing). Along a direction at
    /// angle t from the ball's centre (d from the planet's, radius r) the
    /// caps lie at d cos t -+ sqrt(r^2 - d^2 sin^2 t): the lower rises with
    /// t, the upper falls, so the footprint's nearest and farthest angles
    /// bound them.
    pub fn surface_bands(&self, fb: &FaceBrush, fp: Footprint) -> [(i32, i32); 2] {
        let grid = &self.grid;
        let shift = fp.level + 1;
        let band = |k_lo: i32, k_hi: i32| {
            (
                (k_lo.max(fb.k_lo) >> shift) - 1,
                (k_hi.min(fb.k_hi) >> shift) + 2,
            )
        };
        const NO_BAND: (i32, i32) = (0, 0);
        if fb.shape() != crate::edits::SHAPE_BALL || grid.is_plane() {
            return [band(fb.k_lo, fb.k_hi), NO_BAND];
        }
        let units = f64::from(grid.sphere_constants()[2]) / grid.radius();
        let centre = glam::IVec3::from_slice(&fb.ball[..3]).as_dvec3() / units;
        let (d, r) = (
            centre.length(),
            f64::from(fb.ball[3].unsigned_abs()) / units,
        );
        // The footprint's directions in the brushes' (domain) space: its
        // middle, and the angle to its farthest corner cell (its edges are
        // great circles) and two base cells more.
        let size = CHUNK << fp.level;
        let (i0, j0) = (fp.cx * size, fp.cy * size);
        let towards = |i: i32, j: i32| grid.domain_point(fp.face, i, j, 0).as_dvec3().normalize();
        let middle = towards(i0 + size / 2, j0 + size / 2);
        let spread = [(0, 0), (1, 0), (0, 1), (1, 1)]
            .iter()
            .map(|&(a, b)| middle.angle_between(towards(i0 + a * (size - 1), j0 + b * (size - 1))))
            .fold(0.0, f64::max)
            * 1.01
            + 2.0 * grid.voxel_size() / grid.radius();
        let t = if d > 0.0 {
            middle.angle_between(centre / d)
        } else {
            0.0
        };
        let (near, far) = (
            (t - spread).max(0.0),
            (t + spread).min(std::f64::consts::PI),
        );
        // Half base cells of a radius, two cells of margin.
        let k = |radius: f64, margin: f64| {
            (((radius - grid.radius()) / grid.voxel_size() + margin) * 2.0).clamp(-2.0e9, 2.0e9)
                as i32
        };
        let cap = |t: f64, sign: f64| {
            d * t.cos() + sign * (r * r - (d * t.sin()).powi(2)).max(0.0).sqrt()
        };
        if d <= r {
            // The planet's centre is inside: the upper cap alone.
            return [
                band(k(cap(far, 1.0), -2.0), k(cap(near, 1.0), 2.0)),
                NO_BAND,
            ];
        }
        let silhouette = (r / d).asin();
        if near >= silhouette {
            return [NO_BAND; 2];
        }
        if far >= silhouette {
            return [
                band(k(cap(near, -1.0), -2.0), k(cap(near, 1.0), 2.0)),
                NO_BAND,
            ];
        }
        [
            band(k(cap(near, -1.0), -2.0), k(cap(far, -1.0), 2.0)),
            band(k(cap(far, 1.0), -2.0), k(cap(near, 1.0), 2.0)),
        ]
    }

    /// The layers [lo, hi) (level cells) of `fp`'s nodes holding baked
    /// bricks or over them.
    fn baked_bands(&self, fp: Footprint) -> impl Iterator<Item = (i32, i32)> + '_ {
        self.bricks
            .node_layers(&fp)
            .map(|cz| (cz * CHUNK, cz * CHUNK + CHUNK))
    }

    /// `fp`'s nodes from its `ranges`: the field's surface band and every
    /// edit's are Mixed (merged where fewer than two chunks apart), the
    /// rest of the field's volume Buried, the rest uniform. The field's
    /// band is widened by `DESCENT` cells: a node's descendants keep their
    /// surface within a cell of its own (coarse cells round their tops
    /// down), so a node beside the band may hold their ground.
    pub fn classify(&self, fp: Footprint, ranges: Ranges) -> Class {
        let mut bands: Vec<(i32, i32)> = Vec::new();
        let field = (ranges.surface.0 - DESCENT, ranges.volume.1 + DESCENT);
        if field.1 > field.0 {
            bands.push(field);
        }
        let mut shapes = Vec::new();
        for (n, fb) in self.shapes_over(fp) {
            let before = bands.len();
            bands.extend(self.surface_bands(fb, fp).into_iter().filter(|b| b.1 > b.0));
            if bands.len() > before {
                shapes.push(n as u32);
            }
        }
        bands.extend(self.baked_bands(fp));
        let mut layers: Vec<(i32, i32)> = bands
            .iter()
            .map(|&(a, b)| (a.div_euclid(CHUNK), (b - 1).div_euclid(CHUNK)))
            .collect();
        layers.sort_unstable();
        let mut mixed: Vec<(i32, i32)> = Vec::new();
        for (a, b) in layers {
            match mixed.last_mut() {
                Some(last) if a <= last.1 + 2 => last.1 = last.1.max(b),
                _ => mixed.push((a, b)),
            }
        }
        let floor = ranges
            .volume
            .0
            .min(ranges.surface.0 - (self.volume_depth >> fp.level) - DESCENT);
        let buried = (floor.div_euclid(CHUNK), (field.0 - 1).div_euclid(CHUNK));
        Class {
            mixed,
            buried,
            volume_floor: floor,
            shapes,
        }
    }

    /// The material of an adding brush that makes uniform node `key`
    /// Solid: the last brush adding or removing at its middle, if it adds
    /// (else the node is the field's rock).
    pub fn added_material(&self, key: NodeKey) -> Option<u32> {
        let fp = key.footprint();
        let (i, j, z) = (
            fp.cx * CHUNK + CHUNK / 2,
            fp.cy * CHUNK + CHUNK / 2,
            key.cz * CHUNK + CHUNK / 2,
        );
        let centre = [
            center_half(i, fp.level),
            center_half(j, fp.level),
            center_half(z, fp.level),
        ];
        let mut point = None;
        self.shapes_over(fp)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(|(_, b)| b)
            .find(|b| {
                b.contains(centre, || {
                    *point.get_or_insert_with(|| self.grid.volume_point(fp.face, i, j, z, fp.level))
                })
            })
            .filter(|b| b.op() == 1)
            .map(FaceBrush::material)
    }

    /// Node `cz` of `fp`, of class `class`. Outside every surface band a

    /// node lies wholly inside or outside each brush: at the cell under the
    /// footprint's centre in its middle layer, the last brush that adds or
    /// removes there decides; else the field's buried volume, rock under
    /// it and air over it.
    pub fn kind(&self, fp: Footprint, class: &Class, cz: i32) -> Kind {
        if class.mixed.iter().any(|&(a, b)| (a..=b).contains(&cz)) {
            return Kind::Mixed;
        }
        let (i, j, z) = (
            fp.cx * CHUNK + CHUNK / 2,
            fp.cy * CHUNK + CHUNK / 2,
            cz * CHUNK + CHUNK / 2,
        );
        let centre = [
            center_half(i, fp.level),
            center_half(j, fp.level),
            center_half(z, fp.level),
        ];
        let mut point = None;
        let brush = class
            .shapes
            .iter()
            .rev()
            .map(|&n| &self.faces[n as usize])
            .find(|b| {
                b.contains(centre, || {
                    *point.get_or_insert_with(|| self.grid.volume_point(fp.face, i, j, z, fp.level))
                })
            });
        match brush {
            Some(b) if b.op() == 1 => Kind::Solid,
            Some(_) => Kind::Empty,
            None if (class.buried.0..=class.buried.1).contains(&cz) => Kind::Buried,
            None if z < class.volume_floor => Kind::Solid,
            None => Kind::Empty,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The index finds exactly the brushes a scan of them all does, in order.
    #[test]
    fn the_brush_index_finds_what_a_scan_finds() {
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let faces: Vec<FaceBrush> = (0..3000)
            .map(|_| {
                let face = (next() % 6) as u32;
                // Sizes from a cell to a continent.
                let w = 1i32 << (next() % 28);
                let (ci, cj) = ((next() % (1 << 30)) as i32, (next() % (1 << 30)) as i32);
                FaceBrush {
                    flags: face | ((next() % 3) as u32) << 4,
                    radius_half: w as u32,
                    k_lo: -100,
                    k_hi: 100,
                    center: [ci, cj, 0, w],
                    ball: [0; 4],
                }
            })
            .collect();
        // The first half large (applying below their bake level), indexed
        // in runs as seals add them.
        let large = faces.len() / 2;
        let mut runs = Faces::default();
        let mut index = LargeIndex::default();
        for chunk in faces[..large].chunks(97) {
            let from = runs.large_len();
            runs.extend_large(chunk.iter().copied());
            index.extend(&runs, from);
        }
        assert!(index.runs.len() < 16, "{} runs", index.runs.len());
        let recent = BrushIndex::new(&faces[large..], large as u32, false);

        for _ in 0..2000 {
            let face = (next() % 6) as u8;
            let level = (next() % 12) as u32;
            let w = 1i64 << (next() % 31);
            let (i0, j0) = ((next() % (1 << 30)) as i64, (next() % (1 << 30)) as i64);
            let (i, j) = ((i0, i0 + w), (j0, j0 + w));
            let applies = |n: u32| {
                level < brush_top(&faces[n as usize], (n as usize) < large)
                    && brush_meets(&faces[n as usize], face, level, i, j, (-1000, 1000))
            };
            let mut found: Vec<u32> = index.query(face, i, j, level);
            found.extend(recent.query(face, i, j, level));
            found.retain(|&n| applies(n));
            let scanned: Vec<u32> = (0..faces.len() as u32).filter(|&n| applies(n)).collect();
            assert_eq!(found, scanned);
        }
    }
}
