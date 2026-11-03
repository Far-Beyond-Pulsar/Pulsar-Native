//! Ordered destruction/construction brushes and their spatial index.
//!
//! A brush is authored in planet-centred metres and resolved per cube face
//! for culling. Cubes are tested in the face's integer half-cell index
//! space (one-block cubes are exactly one cell, aligned with the ground);
//! spheres are balls in the seamless integer volume space
//! ([`Grid::volume_point`]), so they are round at any size and depth, the
//! planet's core included. CPU queries and GPU generation apply the exact
//! same integer tests. Later brushes override earlier ones. At LOD level `L`
//! a brush whose radius is below half a level cell is omitted (it is smaller
//! than the point sample).
use crate::grid::{face_axes, Grid, VolumeMap, VOLUME_MAP_ERROR};
use bytemuck::{Pod, Zeroable};
use glam::{DVec3, IVec3};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Largest brush radius in half cells (54 000 km at 0.1 m voxels): the
/// containment test squares offsets exactly in 64 bits, and band bounds
/// (centre plus radius) stay within i32.
pub const MAX_RADIUS_HALF: u32 = 1 << 29;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BrushShape {
    Sphere,
    Cube,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BrushOp {
    /// Carve to air.
    Remove,
    /// Fill with a material.
    Add,
    /// Recolour existing solid cells.
    Paint,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Brush {
    pub center: [f64; 3],
    pub radius: f64,
    pub shape: BrushShape,
    pub op: BrushOp,
    #[serde(default)]
    pub material: u32,
    /// Cube only: half height along the vertical (the radial axis), m; 0 is
    /// a cube of `radius`. A flat-topped box (flatten, smooth).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub height: f64,
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

/// A brush resolved for one face (GPU layout, `FaceBrush` in WGSL).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct FaceBrush {
    /// Face in bits 0..3; op in 4..6; shape in 6..8 (0 ball, 1 cube);
    /// material in 8..16.
    pub flags: u32,
    /// Radius in half base cells: the levels it applies at, and the cube's
    /// half size.
    pub radius_half: u32,
    /// Lowest and highest half-cell heights it can touch (band bounds).
    pub k_lo: i32,
    pub k_hi: i32,
    /// Index-space centre (half base cells i, j, k) and horizontal half
    /// extent (half cells) on this face, for culling.
    pub center: [i32; 4],
    /// Ball: volume-space centre and radius ([`Grid::volume_point`] units).
    pub ball: [i32; 4],
}

/// Shape codes of [`FaceBrush::flags`].
pub const SHAPE_BALL: u32 = 0;
pub const SHAPE_CUBE: u32 = 1;

impl FaceBrush {
    pub fn face(&self) -> u8 {
        (self.flags & 7) as u8
    }
    pub fn op(&self) -> u32 {
        (self.flags >> 4) & 3
    }
    pub fn material(&self) -> u32 {
        (self.flags >> 8) & 0xff
    }
    /// Applies at `level` (radius at least half a level cell).
    pub fn active(&self, level: u32) -> bool {
        self.radius_half >= (1u32 << level)
    }
    pub fn shape(&self) -> u32 {
        (self.flags >> 6) & 3
    }
    /// Horizontal half extent on this face, in base cells (culling).
    pub fn extent_cells(&self) -> i64 {
        i64::from(self.center[3]) / 2 + 1
    }
    /// Exact containment of a level cell centre: `center_half` in half cells
    /// (cubes), `point` its volume point (balls, computed on demand); exact
    /// 64-bit squares (`brush_holds` in chunk_generate.wgsl).
    pub fn contains(&self, center_half: [i32; 3], point: impl FnOnce() -> IVec3) -> bool {
        if self.shape() == SHAPE_CUBE {
            // A box: the radius across, its own range vertically.
            return (0..2).all(|axis| {
                center_half[axis]
                    .wrapping_sub(self.center[axis])
                    .unsigned_abs()
                    <= self.radius_half
            }) && (self.k_lo..=self.k_hi).contains(&center_half[2]);
        }
        let q = point();
        let r = self.ball[3].unsigned_abs();
        let mut sum = 0u64;
        for (v, c) in [q.x, q.y, q.z].into_iter().zip(self.ball) {
            let d = v.wrapping_sub(c).unsigned_abs();
            if d > r {
                return false;
            }
            sum += u64::from(d) * u64::from(d);
        }
        sum <= u64::from(r) * u64::from(r)
    }
}

/// Half-cell centre coordinate of level cell index `i`.
#[inline]
pub fn center_half(i: i32, level: u32) -> i32 {
    (i << (level + 1)).wrapping_add(1 << level)
}

impl Brush {
    /// Resolve into every face whose (extended) grid the brush touches.
    pub fn resolve(&self, grid: &Grid) -> Result<Vec<FaceBrush>, String> {
        if !(self.radius.is_finite() && self.radius > 0.0) {
            return Err("brush radius must be positive".into());
        }
        if self.material > 255 {
            return Err("brush material must fit in 8 bits".into());
        }
        if !(self.height.is_finite() && self.height >= 0.0) {
            return Err("brush height must be finite and non-negative".into());
        }
        let radius_half = (2.0 * self.radius / grid.voxel_size()).round();
        if radius_half > f64::from(MAX_RADIUS_HALF) {
            return Err(format!(
                "brush radius {} m exceeds {:.0} m on this grid",
                self.radius,
                f64::from(MAX_RADIUS_HALF) * grid.voxel_size() / 2.0
            ));
        }
        let radius_half = (radius_half as u32).max(1);
        let center = DVec3::from_array(self.center);
        if self.shape == BrushShape::Sphere {
            return self.resolve_ball(grid, radius_half);
        }
        let n = f64::from(grid.cells());
        let margin = f64::from(radius_half) / 2.0 + 2.0;
        let flags = self.op_flags(SHAPE_CUBE);
        let mut out = Vec::new();
        for &face in grid.faces() {
            let [nrm, _, _] = face_axes(face);
            // On a sphere, only faces whose hemisphere clearly contains the
            // brush centre; extended coordinates degrade far from the face.
            if !grid.is_plane() && center.dot(nrm) <= 0.5 * center.length() {
                continue;
            }
            let Some(c) = grid.face_coords(face, center) else {
                continue;
            };
            if c[0] < -margin || c[0] > n + margin || c[1] < -margin || c[1] > n + margin {
                continue;
            }
            let half = c.map(|v| (v * 2.0).round());
            if half.iter().any(|v| v.abs() > 2.0e9) {
                return Err("brush centre is outside the planet grid".into());
            }
            let r = radius_half as i32;
            // A box's vertical half extent, in half cells (a cube's radius).
            let h = if self.height > 0.0 {
                ((2.0 * self.height / grid.voxel_size()).round() as i32)
                    .clamp(1, MAX_RADIUS_HALF as i32)
            } else {
                r
            };
            out.push(FaceBrush {
                flags: flags | u32::from(face),
                radius_half,
                k_lo: (half[2] as i32).saturating_sub(h),
                k_hi: (half[2] as i32).saturating_add(h),
                center: [half[0] as i32, half[1] as i32, half[2] as i32, r],
                ball: [0; 4],
            });
        }
        if out.is_empty() {
            return Err("brush does not intersect the voxel grid".into());
        }
        Ok(out)
    }

    fn op_flags(&self, shape: u32) -> u32 {
        let op = match self.op {
            BrushOp::Remove => 0u32,
            BrushOp::Add => 1,
            BrushOp::Paint => 2,
        };
        (op << 4) | (shape << 6) | (self.material << 8)
    }

    /// A sphere as a ball in volume space, centred on the volume point of
    /// the base cell holding its centre (the planet's centre for a ball
    /// around the core), on every face it can reach.
    fn resolve_ball(&self, grid: &Grid, radius_half: u32) -> Result<Vec<FaceBrush>, String> {
        let s = grid.voxel_size();
        let center = DVec3::from_array(self.center);
        let flags = self.op_flags(SHAPE_BALL);
        // Volume units per metre: the domain sphere's radius over the
        // planet's (sphere), 1.25 cm units (plane).
        let units = if grid.is_plane() {
            1.0 / crate::grid::DOMAIN_UNIT
        } else {
            f64::from(grid.sphere_constants()[2]) / grid.radius()
        };
        let r_units = self.radius * units;
        if r_units >= f64::from(i32::MAX) / 2.0 {
            return Err("brush radius exceeds the volume space".into());
        }
        let radius_layers = self.radius / s;
        let (ball_centre, centre_layer) = if !grid.is_plane() && center.length() < s {
            (IVec3::ZERO, -(grid.radius() / s))
        } else {
            let (cell, coords) = grid.locate(center);
            (
                grid.volume_point(cell.face, cell.i, cell.j, cell.k, 0),
                coords[2],
            )
        };
        let ball = [
            ball_centre.x,
            ball_centre.y,
            ball_centre.z,
            r_units.round() as i32,
        ];
        // Heights it can touch, in half cells (two layers of margin; the
        // volume radius of a cell is exact up to rounding).
        let half = |layers: f64| (layers * 2.0).clamp(-2.0e9, 2.0e9) as i32;
        let k_lo = half(centre_layer - radius_layers - 2.0);
        let k_hi = half(centre_layer + radius_layers + 2.0);
        let n = grid.cells();
        let whole = |face: u8| FaceBrush {
            flags: flags | u32::from(face),
            radius_half,
            k_lo,
            k_hi,
            center: [n, n, 0, n + 16],
            ball,
        };
        if grid.is_plane() {
            let (cell, _) = grid.locate(center);
            let extent = radius_half.saturating_add(4).min(i32::MAX as u32) as i32;
            return Ok(vec![FaceBrush {
                flags: flags | u32::from(crate::grid::PLANE_FACE),
                radius_half,
                k_lo,
                k_hi,
                center: [cell.i * 2 + 1, cell.j * 2 + 1, cell.k * 2 + 1, extent],
                ball,
            }]);
        }
        // Angular radius of the ball seen from the planet's centre (all
        // directions when it holds the centre).
        let len = center.length();
        let theta = if len > self.radius {
            (self.radius / len).asin()
        } else {
            std::f64::consts::PI
        };
        // A face's pyramid reaches 54.74 degrees from its normal.
        let corner = (1.0f64 / 3.0f64.sqrt()).acos();
        let mut out = Vec::new();
        for &face in grid.faces() {
            let [nrm, _, _] = face_axes(face);
            let angle = if len > 0.0 {
                (center.dot(nrm) / len).clamp(-1.0, 1.0).acos()
            } else {
                0.0
            };
            if theta < std::f64::consts::PI && angle > corner + theta + 0.01 {
                continue;
            }
            // An equal-angle coordinate moves at most 1 / cos(a) times as
            // fast as the great-circle angle, a the angle from the face's
            // normal: a rectangle around the centre's coordinates, or the
            // whole face for wide balls far off the face's axis.
            match grid.face_coords(face, center) {
                Some(c) if theta < 0.5 && angle + theta < 1.3 => {
                    let cells = theta / (angle + theta).cos() / grid.delta() + 4.0;
                    out.push(FaceBrush {
                        flags: flags | u32::from(face),
                        radius_half,
                        k_lo,
                        k_hi,
                        center: [
                            (c[0] * 2.0).round() as i32,
                            (c[1] * 2.0).round() as i32,
                            (c[2] * 2.0).round() as i32,
                            (cells * 2.0).min(f64::from(n) * 2.0 + 32.0) as i32,
                        ],
                        ball,
                    });
                }
                _ => out.push(whole(face)),
            }
        }
        if out.is_empty() {
            return Err("brush does not intersect the voxel grid".into());
        }
        Ok(out)
    }
}

/// An applied brush with stable id (its position in the ordered log).
#[derive(Clone, Debug)]
pub struct Resolved {
    pub brush: Brush,
    pub faces: Vec<FaceBrush>,
    /// Hash of the log up to and including this brush: equal prefix hashes
    /// mean equal logs up to here.
    pub prefix: u64,
}

/// Brushes per shared chunk.
const CHUNK: usize = 1024;

/// Brushes a tree leaf holds before it splits.
const LEAF_BRUSHES: usize = 16;
/// Smallest tree node edge, base cells. A node is never smaller than a
/// 64th of the radius of its smallest brush either: brushes whose
/// surfaces nearly coincide (a stroke dragged over itself) cannot split
/// nodes down along a whole surface.
const MIN_NODE_CELLS: i64 = 16;
/// Root node of every face: base cells `[-2^31, 2^31)` on each axis.
const ROOT_LOG2: u32 = 32;
/// Volume units (at the datum) a cell's integer volume point may be off
/// the f64 map ([`VolumeMap`]), and a few more for the f64 sums.
const VOLUME_SLACK: f64 = VOLUME_MAP_ERROR + 4.0;

/// How a brush meets a box of cells (every level cell centre in it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Meets {
    Outside,
    Crosses,
    Contains,
}

/// A face brush in a leaf: brush id, index into its faces, and whether it
/// holds the leaf's whole box.
#[derive(Clone, Copy, Debug)]
struct Item {
    id: u32,
    index: u8,
    contains: bool,
}

#[derive(Clone)]
enum Node {
    Leaf(Vec<Item>),
    Inner(Box<[Option<Arc<Node>>; 8]>),
}

/// A tree node's box: base cells `[lo, lo + 2^log2)` per axis (i, j, k).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Cube {
    lo: [i64; 3],
    log2: u32,
}

impl Cube {
    const ROOT: Cube = Cube {
        lo: [-(1i64 << (ROOT_LOG2 - 1)); 3],
        log2: ROOT_LOG2,
    };
    fn size(&self) -> i64 {
        1i64 << self.log2
    }
    /// The brick (8 level cells a side) of `level` brick coordinates.
    pub(crate) fn brick(b: [i64; 3], level: u32) -> Cube {
        Cube {
            lo: b.map(|v| (v * 8) << level),
            log2: 3 + level,
        }
    }
    fn child(&self, c: usize) -> Cube {
        let h = 1i64 << (self.log2 - 1);
        let at = |bit: usize| ((c >> bit) & 1) as i64 * h;
        Cube {
            lo: [self.lo[0] + at(0), self.lo[1] + at(1), self.lo[2] + at(2)],
            log2: self.log2 - 1,
        }
    }
    /// Overlaps the inclusive base-cell box `[lo, hi]`.
    fn overlaps(&self, lo: [i64; 3], hi: [i64; 3]) -> bool {
        (0..3).all(|a| self.lo[a] <= hi[a] && self.lo[a] + self.size() > lo[a])
    }
}

/// How `fb` meets every level cell centre inside `cube`, conservatively:
/// `Contains` and `Outside` are certain, anything else `Crosses`. Boxes
/// are tested exactly in half cells, balls in volume space.
pub(crate) fn meets(fb: &FaceBrush, cube: Cube, grid: &Grid, map: &VolumeMap) -> Meets {
    // Level cell centres inside the cube lie within [2 lo, 2 (lo + size)]
    // half cells.
    let lo = cube.lo.map(|v| 2 * v);
    let hi = cube.lo.map(|v| 2 * (v + cube.size()));
    let extent = i64::from(fb.center[3]);
    for axis in 0..2 {
        let c = i64::from(fb.center[axis]);
        if c + extent < lo[axis] || c - extent > hi[axis] {
            return Meets::Outside;
        }
    }
    let (k_lo, k_hi) = (i64::from(fb.k_lo), i64::from(fb.k_hi));
    if k_hi < lo[2] || k_lo > hi[2] {
        return Meets::Outside;
    }
    if fb.shape() == SHAPE_CUBE {
        let r = i64::from(fb.radius_half);
        let c = [i64::from(fb.center[0]), i64::from(fb.center[1])];
        if (0..2).any(|a| hi[a] < c[a] - r || lo[a] > c[a] + r) {
            return Meets::Outside;
        }
        let inside = (0..2).all(|a| lo[a] >= c[a] - r && hi[a] <= c[a] + r)
            && lo[2] >= k_lo
            && hi[2] <= k_hi;
        return if inside {
            Meets::Contains
        } else {
            Meets::Crosses
        };
    }
    // A ball, in the f64 volume map: the cube's corner points hold every
    // level cell centre inside it in their hull, give or take the map's
    // bend (positions clipped to the face, past which no cell is
    // evaluated).
    let face_end = f64::from(grid.cells());
    let span = |a: usize| {
        let (lo, hi) = (cube.lo[a] as f64, (cube.lo[a] + cube.size()) as f64);
        if a < 2 && !grid.is_plane() {
            (lo.max(0.0), hi.min(face_end))
        } else {
            (lo, hi)
        }
    };
    let spans = [span(0), span(1), span(2)];
    if spans[..2].iter().any(|(lo, hi)| lo >= hi) {
        return Meets::Outside;
    }
    let bounds = map.bounds;
    let centre = DVec3::new(
        f64::from(fb.ball[0]),
        f64::from(fb.ball[1]),
        f64::from(fb.ball[2]),
    );
    let r = f64::from(fb.ball[3].unsigned_abs());
    let lift = 1.0 + spans[2].0.abs().max(spans[2].1.abs()) / bounds.layers;
    // First from the middle alone: no point of the cube is farther from it
    // than the bounds allow. Most cubes a ball reaches are well inside or
    // outside its surface.
    let mid = spans.map(|(lo, hi)| (lo + hi) / 2.0);
    let reach = spans.map(|(lo, hi)| (hi - lo) / 2.0);
    let d = map.point(fb.face(), mid).distance(centre);
    let spread =
        bounds.across * lift * (reach[0] + reach[1]) + bounds.up * reach[2] + VOLUME_SLACK * lift;
    if d + spread < r {
        return Meets::Contains;
    }
    if d - spread > r {
        return Meets::Outside;
    }
    let (mut farthest, mut lo, mut hi) = (
        0.0f64,
        DVec3::splat(f64::INFINITY),
        DVec3::splat(f64::NEG_INFINITY),
    );
    for c in 0..8 {
        let at = |a: usize| {
            if (c >> a) & 1 == 0 {
                spans[a].0
            } else {
                spans[a].1
            }
        };
        let q = map.point(fb.face(), [at(0), at(1), at(2)]);
        farthest = farthest.max(q.distance(centre));
        lo = lo.min(q);
        hi = hi.max(q);
    }
    let s = (spans[0].1 - spans[0].0).max(spans[1].1 - spans[1].0);
    let slack = (bounds.bend * s * s + VOLUME_SLACK) * lift;
    if farthest + slack < r {
        return Meets::Contains;
    }
    let nearest = (lo - centre).max(centre - hi).max(DVec3::ZERO).length();
    if nearest - slack > r {
        Meets::Outside
    } else {
        Meets::Crosses
    }
}

/// What insertions read: the log's brushes and the grid's volume map.
struct TreeContext<'a> {
    chunks: &'a [Arc<Vec<Resolved>>],
    grid: &'a Grid,
    map: VolumeMap,
}

impl TreeContext<'_> {
    fn brush(&self, id: u32, index: u8) -> &FaceBrush {
        let id = id as usize;
        &self.chunks[id / CHUNK][id % CHUNK].faces[usize::from(index)]
    }
    /// Smallest edge a node holding `items` may split into.
    fn split_limit(&self, items: &[Item]) -> i64 {
        let radius = items
            .iter()
            .map(|it| self.brush(it.id, it.index).radius_half)
            .min()
            .unwrap_or(0);
        MIN_NODE_CELLS.max(i64::from(radius) / 128)
    }
}

/// Add a face brush to a leaf's ordered list, keeping only what can still
/// change a cell in the leaf. A Remove or Add holding the whole box
/// replaces the brushes before it, but the larger ones (at levels too
/// coarse for it they still apply). A Remove or Paint inside a box an
/// earlier Remove emptied, with no Add since, changes nothing: the dug
/// surfaces under the air of hundreds of overlapping strokes leave the
/// lists. Wherever the brush applies the emptying one does too (it is at
/// least as large).
fn leaf_insert(items: &mut Vec<Item>, item: Item, fb: &FaceBrush, ctx: &TreeContext) {
    let op = fb.op();
    if item.contains && op < 2 {
        items.retain(|it| ctx.brush(it.id, it.index).radius_half > fb.radius_half);
        items.push(item);
        return;
    }
    if op != 1 {
        let emptied = items.iter().rposition(|it| {
            let b = ctx.brush(it.id, it.index);
            it.contains && b.op() == 0 && b.radius_half >= fb.radius_half
        });
        if let Some(at) = emptied {
            if items[at + 1..]
                .iter()
                .all(|it| ctx.brush(it.id, it.index).op() != 1)
            {
                return;
            }
        }
    }
    items.push(item);
}

/// Insert face brush `(id, index)` under `slot` (a node with box `cube`).
fn insert(slot: &mut Option<Arc<Node>>, cube: Cube, id: u32, index: u8, ctx: &TreeContext) {
    let fb = *ctx.brush(id, index);
    let how = meets(&fb, cube, ctx.grid, &ctx.map);
    if how == Meets::Outside {
        return;
    }
    let item = Item {
        id,
        index,
        contains: how == Meets::Contains,
    };
    let Some(node) = slot.as_mut() else {
        *slot = Some(Arc::new(Node::Leaf(vec![item])));
        return;
    };
    let node = Arc::make_mut(node);
    match node {
        Node::Inner(children) if item.contains && fb.op() < 2 => {
            // The brush replaces the subtree but its larger brushes.
            let mut kept = Vec::new();
            for child in children.iter().flatten() {
                collect(child, &mut kept);
            }
            kept.sort_unstable_by_key(|it| (it.id, it.index));
            kept.dedup_by_key(|it| (it.id, it.index));
            let mut items = Vec::new();
            for it in kept {
                let b = *ctx.brush(it.id, it.index);
                if b.radius_half > fb.radius_half {
                    let m = meets(&b, cube, ctx.grid, &ctx.map);
                    if m != Meets::Outside {
                        leaf_insert(
                            &mut items,
                            Item {
                                contains: m == Meets::Contains,
                                ..it
                            },
                            &b,
                            ctx,
                        );
                    }
                }
            }
            leaf_insert(&mut items, item, &fb, ctx);
            *node = leaf(items, cube, ctx);
        }
        Node::Inner(children) => {
            for (c, child) in children.iter_mut().enumerate() {
                insert(child, cube.child(c), id, index, ctx);
            }
        }
        Node::Leaf(items) => {
            leaf_insert(items, item, &fb, ctx);
            if items.len() > LEAF_BRUSHES && cube.size() / 2 >= ctx.split_limit(items) {
                let items = std::mem::take(items);
                *node = leaf(items, cube, ctx);
            }
        }
    }
}

/// A node holding `items` (in order): a leaf, split while too long.
fn leaf(items: Vec<Item>, cube: Cube, ctx: &TreeContext) -> Node {
    if items.len() <= LEAF_BRUSHES || cube.size() / 2 < ctx.split_limit(&items) {
        return Node::Leaf(items);
    }
    let mut children: [Option<Arc<Node>>; 8] = Default::default();
    for (c, child) in children.iter_mut().enumerate() {
        for it in &items {
            insert(child, cube.child(c), it.id, it.index, ctx);
        }
    }
    Node::Inner(Box::new(children))
}

fn collect(node: &Node, out: &mut Vec<Item>) {
    match node {
        Node::Leaf(items) => out.extend_from_slice(items),
        Node::Inner(children) => children.iter().flatten().for_each(|c| collect(c, out)),
    }
}

fn gather(node: &Node, cube: Cube, lo: [i64; 3], hi: [i64; 3], out: &mut Vec<(u32, u8)>) {
    if !cube.overlaps(lo, hi) {
        return;
    }
    match node {
        Node::Leaf(items) => out.extend(items.iter().map(|it| (it.id, it.index))),
        Node::Inner(children) => {
            for (c, child) in children.iter().enumerate() {
                if let Some(child) = child {
                    gather(child, cube.child(c), lo, hi, out);
                }
            }
        }
    }
}

/// The log's spatial index: per face, an adaptive octree over base cells
/// whose leaves keep, in order, only the face brushes that can still
/// change a cell inside them ([`leaf_insert`]). A query returns the
/// brushes of the leaves a box overlaps: what a column or a cell needs,
/// however long the history above it. Nodes are shared copy-on-write, so
/// copying a log copies its roots.
#[derive(Clone, Default)]
struct EditTree {
    roots: [Option<Arc<Node>>; 8],
}

/// Ordered edit log plus its spatial index ([`EditTree`]).
///
/// Copies share structure: brushes live in shared chunks of [`CHUNK`] and
/// the tree's nodes are shared, so copying a log with tens of thousands of
/// edits (what a renderer does to extend a published world) copies a few
/// pointers.
#[derive(Clone, Default)]
pub struct EditLog {
    chunks: Vec<Arc<Vec<Resolved>>>,
    len: usize,
    tree: EditTree,
}

/// FNV-1a over a brush, continuing `seed`.
/// Hash of a brush history (what [`crate::planet::Edits::hash`] reports
/// for a world with exactly these brushes applied in order).
pub fn history_hash<'a>(brushes: impl IntoIterator<Item = &'a Brush>) -> u64 {
    brushes
        .into_iter()
        .fold(0, |hash, brush| brush_hash(hash, brush))
}

pub(crate) fn brush_hash(seed: u64, brush: &Brush) -> u64 {
    let mut h = seed ^ 0xcbf2_9ce4_8422_2325;
    let mut eat = |v: u64| {
        for byte in v.to_le_bytes() {
            h = (h ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
        }
    };
    for c in brush.center {
        eat(c.to_bits());
    }
    eat(brush.radius.to_bits());
    eat(brush.shape as u64);
    eat(brush.op as u64);
    eat(u64::from(brush.material));
    // Boxes only: cubes and balls keep the hashes they always had.
    if brush.height != 0.0 {
        eat(brush.height.to_bits());
    }
    h
}

impl EditLog {
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn brushes(&self) -> impl Iterator<Item = &Brush> {
        self.chunks
            .iter()
            .flat_map(|chunk| chunk.iter())
            .map(|r| &r.brush)
    }
    pub fn resolved(&self, id: u32) -> &Resolved {
        let id = id as usize;
        &self.chunks[id / CHUNK][id % CHUNK]
    }
    /// Hash of the first `id + 1` brushes (see [`Resolved::prefix`]).
    pub fn prefix_hash(&self, id: u32) -> u64 {
        self.resolved(id).prefix
    }
    pub fn push(&mut self, grid: &Grid, brush: Brush) -> Result<u32, String> {
        let faces = brush.resolve(grid)?;
        let id = self.len as u32;
        let prefix = brush_hash(if id == 0 { 0 } else { self.prefix_hash(id - 1) }, &brush);
        if self.len % CHUNK == 0 {
            self.chunks.push(Arc::new(Vec::with_capacity(CHUNK)));
        }
        // Copies only this chunk when another log still shares it.
        Arc::make_mut(self.chunks.last_mut().expect("pushed above")).push(Resolved {
            brush,
            faces,
            prefix,
        });
        self.len += 1;
        self.index(grid, id);
        Ok(id)
    }
    fn index(&mut self, grid: &Grid, id: u32) {
        let ctx = TreeContext {
            chunks: &self.chunks,
            grid,
            map: grid.volume_map(),
        };
        for (index, fb) in ctx.chunks[id as usize / CHUNK][id as usize % CHUNK]
            .faces
            .iter()
            .enumerate()
        {
            insert(
                &mut self.tree.roots[usize::from(fb.face())],
                Cube::ROOT,
                id,
                index as u8,
                &ctx,
            );
        }
    }
    /// Remove the most recent brush (undo); the index is rebuilt (the
    /// recent log holds a few dozen brushes).
    pub fn pop(&mut self, grid: &Grid) -> Option<Brush> {
        let chunk = self.chunks.last_mut()?;
        let last = Arc::make_mut(chunk).pop()?;
        if chunk.is_empty() {
            self.chunks.pop();
        }
        self.len -= 1;
        self.tree = EditTree::default();
        for id in 0..self.len as u32 {
            self.index(grid, id);
        }
        Some(last.brush)
    }
    /// Ordered face-brush references that can change a level cell of the
    /// base-cell box `[i0, i1] × [j0, j1] × [k0, k1]` of `face` at LOD
    /// `level` ([`ALL_LAYERS`]: a whole column). Returned as `(brush id,
    /// index into that brush's faces)`.
    pub fn query(
        &self,
        face: u8,
        [i0, i1]: [i64; 2],
        [j0, j1]: [i64; 2],
        [k0, k1]: [i64; 2],
        level: u32,
    ) -> Vec<(u32, u8)> {
        let Some(root) = self
            .tree
            .roots
            .get(usize::from(face))
            .and_then(|r| r.as_ref())
        else {
            return Vec::new();
        };
        let mut found = Vec::new();
        gather(root, Cube::ROOT, [i0, j0, k0], [i1, j1, k1], &mut found);
        found.sort_unstable();
        found.dedup();
        found.retain(|&(id, index)| {
            let fb = &self.resolved(id).faces[usize::from(index)];
            let r = fb.extent_cells();
            let ci = i64::from(fb.center[0]) / 2;
            let cj = i64::from(fb.center[1]) / 2;
            fb.active(level)
                && ci + r >= i0
                && ci - r <= i1
                && cj + r >= j0
                && cj - r <= j1
                && i64::from(fb.k_hi) >= 2 * k0
                && i64::from(fb.k_lo) <= 2 * k1 + 2
        });
        found
    }
}

/// Every layer of a column, for [`EditLog::query`].
pub const ALL_LAYERS: [i64; 2] = [i32::MIN as i64, i32::MAX as i64];

/// Apply ordered brushes to a canonical terrain `(kind, material)` at a level
/// cell centre. Kind: 0 air, 1 solid.
/// `point` is the cell's volume point (computed once, when a ball needs it).
pub fn apply(
    brushes: impl Iterator<Item = FaceBrush>,
    center: [i32; 3],
    point: impl Fn() -> IVec3,
    mut kind: u32,
    mut material: u32,
) -> (u32, u32) {
    let mut q = None;
    for b in brushes {
        if !b.contains(center, || *q.get_or_insert_with(&point)) {
            continue;
        }
        match b.op() {
            0 => {
                kind = 0;
                material = 0;
            }
            1 => {
                kind = 1;
                material = b.material();
            }
            _ => {
                if kind == 1 {
                    material = b.material();
                }
            }
        }
    }
    (kind, material)
}

#[cfg(test)]
mod tree {
    use super::*;
    use crate::grid::Shape;

    /// A small deterministic generator.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }
        fn range(&mut self, lo: f64, hi: f64) -> f64 {
            lo + (hi - lo) * self.next()
        }
    }

    /// An editor session over one spot: strokes of large digs dragged over
    /// each other (from orbit and from close), builds and paint on the dug
    /// walls, boxes, and small brushes that only apply at fine levels.
    fn session(grid: &Grid, ground: DVec3, rng: &mut Lcg) -> EditLog {
        let up = ground.normalize();
        let side = up.any_orthonormal_vector();
        let ahead = up.cross(side);
        let mut log = EditLog::default();
        for stroke in 0..12 {
            let radius = [2_000.0, 1_000.0, 500.0, 300.0, 60.0, 8.0][stroke % 6];
            let depth = rng.range(-0.5, 0.3) * radius;
            let start = side * rng.range(-3_000.0, 3_000.0) + ahead * rng.range(-3_000.0, 3_000.0);
            let heading = side * rng.range(-1.0, 1.0) + ahead * rng.range(-1.0, 1.0);
            for n in 0..40 {
                let at = ground + start + heading * (n as f64 * radius * 0.15) + up * depth;
                let r = rng.next();
                let (op, shape) = match stroke % 4 {
                    3 if r < 0.3 => (BrushOp::Add, BrushShape::Sphere),
                    3 if r < 0.5 => (BrushOp::Paint, BrushShape::Sphere),
                    2 if r < 0.2 => (BrushOp::Remove, BrushShape::Cube),
                    _ => (BrushOp::Remove, BrushShape::Sphere),
                };
                let radius = radius * rng.range(0.9, 1.1);
                log.push(
                    grid,
                    Brush {
                        center: at.to_array(),
                        radius,
                        shape,
                        op,
                        material: 3 + (n % 5) as u32,
                        height: 0.0,
                    },
                )
                .unwrap();
            }
        }
        log
    }

    /// Every face brush of `log` on `face` that applies at `level`, in order.
    fn every(log: &EditLog, face: u8, level: u32) -> Vec<FaceBrush> {
        (0..log.len() as u32)
            .flat_map(|id| log.resolved(id).faces.iter().copied())
            .filter(|fb| fb.face() == face && fb.active(level))
            .collect()
    }

    fn listed(log: &EditLog, refs: &[(u32, u8)]) -> Vec<FaceBrush> {
        refs.iter()
            .map(|&(id, index)| log.resolved(id).faces[usize::from(index)])
            .collect()
    }

    /// A cell's edits from the pruned lists (of the cell, and of its whole
    /// column) equal every brush applied in order, over solid and over air,
    /// at every level, near and inside the dug region.
    #[test]
    fn pruned_lists_apply_like_every_brush() {
        let grid = Grid::new(6_371_000.0, 0.1).unwrap();
        let mut rng = Lcg(7);
        // On a cube edge: brushes resolve onto two faces.
        let ground = DVec3::new(-0.67, -0.1, 0.73).normalize() * grid.radius();
        let log = session(&grid, ground, &mut rng);
        let up = ground.normalize();
        let side = up.any_orthonormal_vector();
        let ahead = up.cross(side);
        let (mut cells, mut changed) = (0, 0);
        for _ in 0..6_000 {
            let level = (rng.next() * 13.0) as u32;
            let p = ground
                + side * rng.range(-6_000.0, 6_000.0)
                + ahead * rng.range(-6_000.0, 6_000.0)
                + up * rng.range(-2_600.0, 600.0);
            let (cell, _) = grid.locate(p);
            let (face, i, j, k) = (cell.face, cell.i >> level, cell.j >> level, cell.k >> level);
            let range = |v: i32| [i64::from(v) << level, ((i64::from(v) + 1) << level) - 1];
            let center = [
                center_half(i, level),
                center_half(j, level),
                center_half(k, level),
            ];
            let point = || grid.volume_point(face, i, j, k, level);
            let everything = every(&log, face, level);
            let of_cell = listed(&log, &log.query(face, range(i), range(j), range(k), level));
            let of_column = listed(
                &log,
                &log.query(face, range(i), range(j), ALL_LAYERS, level),
            );
            for (kind, material) in [(1, 0), (0, 0), (1, 9)] {
                let want = apply(everything.iter().copied(), center, point, kind, material);
                assert_eq!(
                    apply(of_cell.iter().copied(), center, point, kind, material),
                    want,
                    "cell {face} {i} {j} {k} level {level}"
                );
                assert_eq!(
                    apply(of_column.iter().copied(), center, point, kind, material),
                    want,
                    "column {face} {i} {j} {k} level {level}"
                );
                changed += usize::from(want != (kind, material));
            }
            cells += 1;
        }
        // The samples reach the edits.
        assert!(changed > cells / 4, "{changed} of {cells} cells changed");
    }

    /// Where a long editing session dug, a column's list holds the surfaces
    /// left exposed, not the history: after four passes of a 2 km dig (400
    /// brushes over the same line), no column along it lists more than 32
    /// brushes at levels 0, 2, 4, 6 or 8.
    #[test]
    fn a_long_dig_leaves_short_column_lists() {
        let grid = Grid::new(6_371_000.0, 0.1).unwrap();
        let mut rng = Lcg(11);
        let ground = DVec3::new(0.2, 1.0, 0.1).normalize() * grid.radius();
        let up = ground.normalize();
        let side = up.any_orthonormal_vector();
        let ahead = up.cross(side);
        let mut log = EditLog::default();
        let started = std::time::Instant::now();
        // Four passes of a 2 km dig over the same line, as a hand drags.
        for pass in 0..4 {
            for n in 0..100 {
                let at = ground
                    + side * (n as f64 * 150.0 - 7_500.0 + rng.range(-200.0, 200.0))
                    + ahead * rng.range(-300.0, 300.0)
                    - up * (pass as f64 * 150.0);
                log.push(
                    &grid,
                    Brush {
                        center: at.to_array(),
                        radius: 2_000.0,
                        shape: BrushShape::Sphere,
                        op: BrushOp::Remove,
                        material: 0,
                        height: 0.0,
                    },
                )
                .unwrap();
            }
        }
        let per_brush = started.elapsed().as_secs_f64() * 1e3 / log.len() as f64;
        eprintln!("{per_brush:.3} ms a brush");
        let (cell, _) = grid.locate(ground);
        for level in [0u32, 2, 4, 6, 8] {
            let (mut longest, mut total, mut columns) = (0usize, 0usize, 0usize);
            for step in -20..=20 {
                let p = ground + side * (f64::from(step) * 300.0);
                let (c, _) = grid.locate(p);
                let (i, j) = (c.i >> level, c.j >> level);
                let range = |v: i32| [i64::from(v) << level, ((i64::from(v) + 1) << level) - 1];
                let n = log
                    .query(cell.face, range(i), range(j), ALL_LAYERS, level)
                    .len();
                longest = longest.max(n);
                total += n;
                columns += 1;
            }
            eprintln!(
                "level {level}: {:.1} brushes a column, at most {longest} (of {})",
                total as f64 / columns as f64,
                log.len()
            );
            assert!(
                longest <= 32,
                "level {level}: a column lists {longest} of {} brushes",
                log.len()
            );
        }
    }

    /// Copies share the tree; an undo leaves the index a fresh log has.
    #[test]
    fn copies_share_structure_and_undo_rebuilds_the_index() {
        let grid = Grid::plane(Shape::Plane, 512.0, 0.1).unwrap();
        let brush = |k: usize| Brush {
            center: [
                ((k * 37) % 400) as f64 - 200.0,
                1.0,
                ((k * 91) % 400) as f64 - 200.0,
            ],
            radius: if k % 97 == 0 { 6.0 } else { 0.3 },
            shape: if k % 3 == 0 {
                BrushShape::Cube
            } else {
                BrushShape::Sphere
            },
            op: if k % 2 == 0 {
                BrushOp::Add
            } else {
                BrushOp::Remove
            },
            material: 13,
            height: 0.0,
        };
        let mut a = EditLog::default();
        for k in 0..9_000 {
            a.push(&grid, brush(k)).unwrap();
        }
        let t = std::time::Instant::now();
        for _ in 0..100 {
            std::hint::black_box(a.clone());
        }
        assert!(
            t.elapsed().as_millis() < 50,
            "{:?} for 100 copies",
            t.elapsed()
        );
        let mut b = EditLog::default();
        let mut fresh = EditLog::default();
        for k in 0..60 {
            b.push(&grid, brush(k)).unwrap();
            if k < 50 {
                fresh.push(&grid, brush(k)).unwrap();
            }
        }
        for _ in 0..10 {
            b.pop(&grid).unwrap();
        }
        assert_eq!(b.prefix_hash(49), fresh.prefix_hash(49));
        let face = crate::grid::PLANE_FACE;
        let c = grid.cells() as i64 / 2;
        for level in [0u32, 3, 6] {
            let box_ = [c - 2_000, c + 2_000];
            assert_eq!(
                b.query(face, box_, box_, ALL_LAYERS, level),
                fresh.query(face, box_, box_, ALL_LAYERS, level),
                "level {level}"
            );
        }
    }
}
