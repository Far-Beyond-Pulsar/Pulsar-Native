//! Chunk residency (`docs/chunk-octree.md`): which nodes of the chunk
//! octree are drawn, generated next and evicted, for a moving eye.
//!
//! A Mixed node is refined while its cells would project larger than
//! `lod_pixels` pixels from the eye or from where the eye will be
//! `prefetch_seconds` ahead (it stays refined until they fall under
//! `coarsen` of that); out of view, `offscreen_pixels` times that; where
//! no ray has met it for `seen_frames` (occluded, or not yet sampled),
//! `unseen_pixels` times that; where the eye's motion sweeps the image, up
//! to `motion_max` times that. Empty, Solid and Buried nodes are never
//! refined. A refined node keeps being drawn until every Mixed child is
//! generated, so nothing is ever missing: detail arrives as whole nodes
//! replacing their parent. The cut covers the whole world: roots are
//! always visited, and nodes the eye cannot see are drawn as they are.
//! Generated nodes out of the cut for `release_frames` are evicted, and
//! when chunk slots run short, those out of it longest. Plans are
//! incremental ([`Planner`]): they visit what may have changed, not the
//! tree.
//!
//! Pure CPU logic over node keys, ranges and edits, run on a [`Worker`]
//! thread; the GPU work lives in `chunks`.

use super::classify::{Class, EditView, Footprint, Kind, NodeKey, Ranges};
use super::{tile_distance, Eye, CHUNK};
use crate::grid::Grid;
use glam::DVec3;
use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::BinaryHeap;
use std::sync::Arc;

#[derive(Clone, Copy, Debug)]
pub struct PlanConfig {
    /// Radians a pixel spans.
    pub pixel_angle: f64,
    /// Refine while a cell would cover more pixels than this.
    pub lod_pixels: f64,
    /// Out of view (past the view cone widened by `view_guard` radians),
    /// cells may cover this many times `lod_pixels`: a level coarser at 2,
    /// an eighth of the nodes there.
    pub offscreen_pixels: f64,
    pub view_guard: f64,
    /// In view but met by no ray for `seen_frames` frames (occluded), cells
    /// may cover this many times `lod_pixels` (1: rays decide nothing).
    pub unseen_pixels: f64,
    pub seen_frames: u64,
    /// Coarsen again only under this share of `lod_pixels`.
    pub coarsen: f64,
    /// Planes: nodes farther than this are not drawn (spheres draw up to
    /// the horizon).
    pub max_distance: f64,
    /// Also refine for where the eye will be this far ahead.
    pub prefetch_seconds: f64,
    /// Bookkeeping of nodes never generated (wanted once) is dropped after
    /// this many frames out of the cut.
    pub keep_frames: u64,
    /// Generated nodes out of the cut for this many frames are released
    /// (`MAX_RELEASE` a plan, least recently in it first), so residency
    /// follows the cut rather than the eye's history; a cut that comes back
    /// within them finds its chunks, and what it found empty (remembered as
    /// long, or `keep_frames` if longer; 0: chunks released only when chunk
    /// slots run short).
    pub release_frames: u64,
    /// Where the eye's motion sweeps a node's image by `flow` pixels a
    /// frame, its cells may cover `1 + flow / motion_flow` times
    /// `lod_pixels`, at most `motion_max` (0: no motion allowance).
    pub motion_flow: f64,
    pub motion_max: f64,
    /// Buried nodes within this many of their cells of the eye are
    /// generated: those holding an eye under the ground (the rest of its
    /// cave comes as rays reach it).
    pub near_cells: f64,
}

/// The frame time motion allowances assume (60 frames a second).
const MOTION_FRAME_SECONDS: f64 = 1.0 / 60.0;

#[derive(Clone, Copy, Debug)]
struct Node {
    generated: bool,
    /// Generated and found to hold nothing at its level: drawn as air,
    /// still refined (its descendants may hold ground its coarse cells
    /// rounded away).
    empty: bool,
    /// Its children were drawn in its place last time it was visited.
    refined: bool,
    /// In the cut: a root, or a child of an open node.
    in_cut: bool,
    /// Its children are in the cut, in its place.
    open: bool,
    /// Out of the cut: the frame it left it, or a plan last wanted it (its
    /// idle age counts from there).
    used: u64,
    /// The plan that last visited it (`Planner::plans`; 0: never).
    visited: u64,
    /// When it is visited again, on each of `Planner::queues`' readings
    /// (infinite: not for that), and the reading its entry there is queued
    /// at (infinite: none). An entry earlier than its due is queued again
    /// at it when it comes out.
    due: [f64; QUEUES],
    queued: [f64; QUEUES],
    /// The plan (`Planner::plans`) events last marked it for: what it or
    /// its children hold may have changed.
    marked: u64,
    /// Buried, and met by a ray: generated like a Mixed node.
    reached: bool,
    /// The layers (level cells, [lo, hi)) its chunk occupies, once its
    /// result came back (generated or restored).
    occupied: Option<(i32, i32)>,
    /// What it is drawn as while it is a leaf of the cut (Mixed: its
    /// chunk; Empty: air; none: nothing, a root waiting for its chunk).
    leaf: Option<Kind>,
    /// Its place in `Planner::showing` while it is a leaf of the cut in
    /// view.
    showing: Option<u32>,
    /// Edits made it Mixed and its chunk is not generated yet: drawn as it
    /// was (Empty: air; Solid or Buried: rock) meanwhile, without holding
    /// its parent back, so an edit never shows a coarse ancestor in its
    /// place.
    stand_in: Option<Kind>,
    /// The frame an edit last marked it.
    edited: u64,
}

impl Default for Node {
    fn default() -> Self {
        Self {
            generated: false,
            empty: false,
            refined: false,
            in_cut: false,
            open: false,
            used: 0,
            visited: 0,
            due: [f64::INFINITY; QUEUES],
            queued: [f64::INFINITY; QUEUES],
            marked: 0,
            reached: false,
            occupied: None,
            leaf: None,
            showing: None,
            stand_in: None,
            edited: 0,
        }
    }
}

/// Frames rock an edit added stays air while its footprint's new surface
/// is generated, at most (`Planner::surface_pending`).
const ADDED_ROCK_WAITS: u64 = 120;

#[derive(Clone)]
struct Fp {
    ranges: Ranges,
    /// Measured on the GPU (else estimated from the parent's quarter over
    /// it, or the parent's whole ranges while it is estimated too).
    exact: bool,
    /// Measured: each quarter's ranges (32x32 columns, x then y: a child
    /// footprint's; none past a face's edge).
    quarters: [Option<Ranges>; 4],
    /// Levels under the nearest measured ancestor (0: measured, or a root):
    /// an estimate's margin doubles at each.
    guess: u8,
    /// Its classification, and the edits' version it was made for.
    class: Option<(u64, Class)>,
}

/// How long a visit's outcome holds: until the eye has moved `moves`
/// metres more (`Planner::moved`), its velocity changed by `speeds` metres
/// a second more (`Planner::sped`), the view turned `turns` radians more
/// (`Planner::turned`), or frame `until`.
#[derive(Clone, Copy, Debug)]
struct Window {
    moves: f64,
    speeds: f64,
    turns: f64,
    until: u64,
}

impl Window {
    const EVER: Self = Self {
        moves: f64::INFINITY,
        speeds: f64::INFINITY,
        turns: f64::INFINITY,
        until: u64::MAX,
    };

    fn moves(moves: f64) -> Self {
        Self {
            moves,
            ..Self::EVER
        }
    }

    /// `moves` metres of the eye's travel, and the velocity's change that
    /// moves the prefetch eye a quarter of that more (`SPEED_SHARE`).
    fn ahead(moves: f64, seconds: f64) -> Self {
        Self {
            moves,
            speeds: moves * SPEED_SHARE / seconds,
            ..Self::EVER
        }
    }

    fn until(until: u64) -> Self {
        Self {
            until,
            ..Self::EVER
        }
    }

    fn min(self, other: Self) -> Self {
        Self {
            moves: self.moves.min(other.moves),
            speeds: self.speeds.min(other.speeds),
            turns: self.turns.min(other.turns),
            until: self.until.min(other.until),
        }
    }
}

/// A band's refinement this plan ([`Planner::decide`]).
#[derive(Clone, Copy, Debug)]
struct Decision {
    error: f64,
    seen: bool,
    allowance: f64,
    ready: bool,
    exact: bool,
    guess: u8,
    /// Its cells' pixels from the eye now, measured to the cells its
    /// chunks occupy, once known.
    cells: Option<f64>,
    /// How long it holds.
    window: Window,
}

/// A leaf of the cut in view as its last visit measured it: what the plan's
/// measures (`Plan::coarse_share`, `worst_px`) are made of.
#[derive(Clone, Copy, Debug)]
struct Shown {
    key: NodeKey,
    /// Above the finest level, its cells over 1.5 times `lod_pixels`.
    coarse: bool,
    /// Above the finest level, its cells' pixels from the eye, once known
    /// (`Decision::cells`), and its allowances.
    cells: Option<f64>,
    allowance: f64,
    exact: bool,
    guess: u8,
}

/// A queued re-check of node `key` at odometer reading or frame `at`; the
/// earliest first out of a `BinaryHeap`.
#[derive(Clone, Copy, Debug)]
struct Due {
    at: f64,
    key: NodeKey,
}

/// Re-check queues: the eye's travel, the velocity's change, the view's
/// turn, frames (`Window`'s fields).
const QUEUES: usize = 4;

impl PartialEq for Due {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}

impl Eq for Due {}

impl PartialOrd for Due {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Due {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other.at.total_cmp(&self.at)
    }
}

/// A node out of the cut since frame `used`; the longest idle first out of
/// a `BinaryHeap`.
#[derive(Clone, Copy, Debug)]
struct Idle {
    used: u64,
    key: NodeKey,
}

impl PartialEq for Idle {
    fn eq(&self, other: &Self) -> bool {
        self.used == other.used
    }
}

impl Eq for Idle {}

impl PartialOrd for Idle {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Idle {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other.used.cmp(&self.used)
    }
}

/// Idle nodes by what they hold ([`idle_kind`]): generated chunks (released
/// by age and slot pressure), nodes found empty, and bookkeeping of nodes
/// never generated.
const IDLE_CHUNK: usize = 0;
const IDLE_EMPTY: usize = 1;
const IDLE_KNOWN: usize = 2;

fn idle_kind(n: &Node) -> usize {
    match (n.generated, n.empty) {
        (true, false) => IDLE_CHUNK,
        (true, true) => IDLE_EMPTY,
        _ => IDLE_KNOWN,
    }
}

/// What a visit makes of a node: a leaf of the cut (drawn as `leaf`, and
/// measured for the plan while in view), or open (its children in its
/// place).
enum Outcome {
    Leaf {
        leaf: Option<Kind>,
        shown: Option<Shown>,
    },
    Open,
}

/// Where the eye looks: nodes in view are generated first.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub forward: DVec3,
    /// Cosine of the half-angle of the cone around `forward` in view.
    pub cos_half: f64,
}

/// One frame's decisions.
#[derive(Clone, Debug, Default)]
pub struct Plan {
    /// Nodes drawn now and not by the last plan (or drawn as something
    /// else), and the reverse: the change of the cut, whose drawn nodes
    /// cover the world once each (Empty nodes left out).
    pub added: Vec<(NodeKey, Kind)>,
    pub removed: Vec<NodeKey>,
    /// Tests: the whole cut's drawn nodes.
    #[cfg(test)]
    pub drawn: Vec<(NodeKey, Kind)>,
    /// Nodes to generate, most wanted first, those of a footprint
    /// together (at most `MAX_REQUESTS`).
    pub requests: Vec<NodeKey>,
    /// Nodes waiting to be generated in all.
    pub waiting: usize,
    /// Nodes the plan visited.
    pub visits: usize,
    /// Drawn Mixed nodes in view whose cells project larger than wanted,
    /// as a share of them (the refinement still to come).
    pub coarse_share: f64,
    /// Pixels the largest cell of a drawn node in view spans from the eye
    /// (above the finest level; measured to the cells its chunks occupy,
    /// once their results are back), and the largest of those over its
    /// allowances (`PlanConfig::motion_flow`, `unseen_pixels`; 1: as
    /// wanted). Nodes are measured when visited: those the plan did not
    /// visit as their last visit found them (within `LATENESS` of what a
    /// visit now would).
    pub worst_px: f64,
    pub worst_wanted: f64,
    pub worst: Option<Worst>,
    /// Nodes in view this plan switches between levels, and the pixels the
    /// largest cell of those spans from the eye (measured to their cells,
    /// as `worst_px`).
    pub switches: usize,
    pub switch_px: f64,
}

/// The coarsest node in view (`Plan::worst`).
#[derive(Clone, Copy, Debug)]
pub struct Worst {
    pub key: NodeKey,
    pub exact: bool,
    pub guess: u8,
    pub children: usize,
    pub generated: usize,
}

/// The planner keeps the cut between plans and visits only the nodes whose
/// outcome may have changed (`docs/chunk-octree.md`, "Incremental
/// residency"):
/// - Each visit bounds how far the eyes may move and the view turn before
///   its outcome could change (every threshold it tests against the
///   Lipschitz bounds of its distances, cones and motion allowance, up to
///   `LATENESS`), and queues its re-check on odometers of the eye's travel,
///   the velocity's change and the view's turn ([`Window`]); ray reports'
///   expiry and edits' waits on frames.
/// - Events mark the nodes they touch, their footprints' others and their
///   parents; nodes that want others generated are visited every plan
///   (their requests' priorities follow the eye).
/// - The cut's change is the plan's output; nodes leaving it queue by idle
///   age, so eviction pops the longest idle and never looks at the cut.
#[derive(Clone)]
pub struct Planner {
    /// The volume it plans (its keys').
    volume: u16,
    nodes: FxHashMap<NodeKey, Node>,
    footprints: FxHashMap<Footprint, Fp>,
    /// Each footprint's node layers the planner knows (a footprint goes
    /// with its last node, the roots' aside).
    known: FxHashMap<Footprint, Vec<i32>>,
    /// The frame rays last met each node or a node under it.
    seen: FxHashMap<NodeKey, u64>,
    /// Whether any ray report came yet (before one, nothing is unseen).
    rays: bool,
    roots: Vec<NodeKey>,
    /// Roots not in the cut yet.
    fresh_roots: Vec<NodeKey>,
    root_level: u32,
    /// The planet's height range in base layers.
    range: (i32, i32),
    config: PlanConfig,
    edits: Arc<EditView>,
    /// Spheres: the radius of the lowest ground, whose horizon hides what
    /// lies beyond it (none when that ground reaches the centre).
    occluder: Option<f64>,
    /// The last plan's frame + 1 (0: none yet), and plans made.
    last_plan: u64,
    plans: u64,
    /// What the cut draws, and the plan's changes to it (what each node
    /// changed was drawn as before).
    drawn: FxHashMap<NodeKey, Kind>,
    changes: FxHashMap<NodeKey, Option<Kind>>,
    /// Leaves of the cut in view.
    showing: Vec<Shown>,
    /// Nodes to visit next plan.
    dirty: Vec<NodeKey>,
    /// Metres the eye has travelled, metres a second its velocity has
    /// changed, and radians the view has turned over all plans; the last
    /// plan's eye, velocity and view.
    moved: f64,
    sped: f64,
    turned: f64,
    last: Option<(DVec3, DVec3, Option<View>)>,
    /// Re-checks queued on `moved`, `sped`, `turned` and frames.
    queues: [BinaryHeap<Due>; QUEUES],
    /// Nodes out of the cut by idle age, by [`idle_kind`].
    idle: [BinaryHeap<Idle>; 3],
    /// [`LATENESS`] (0 in tests comparing plans with full visits).
    lateness: f64,
    /// Nodes leaving the cut (a collapse's work list).
    leaving: Vec<NodeKey>,
    scratch: PlanScratch,
}

/// Distances and offsets from the eye change at most this many times as
/// fast as it moves (1 in a plane's frame; the linear frame of a sphere's
/// face drifts with the eye by a 64th at most within its reach).
const DRIFT: f64 = 1.25;

/// A window lets the velocity change by what moves the prefetch eye this
/// share of the eye's travel (over `velocity_seconds`).
const SPEED_SHARE: f64 = 0.25;

/// A threshold may be passed by this much (as a share of its measure:
/// distance, pixels, a cone's breadth) before its node is visited again:
/// re-checks are bounded below, so a node exactly at a threshold is not
/// visited every plan.
const LATENESS: f64 = 1.0 / 16.0;

/// Work lists belong to the planner, rather than being allocated for every
/// camera update. Clearing them drops old keys; capacity follows peak work.
#[derive(Default, Clone)]
struct PlanScratch {
    tiles: Vec<i32>,
    wanted: Vec<(f64, NodeKey)>,
    priority_order: Vec<usize>,
    stack: Vec<(NodeKey, bool)>,
    work: Vec<NodeKey>,
    pending: Vec<NodeKey>,
    decisions: FxHashMap<(Footprint, i32), Decision>,
    order: FxHashMap<Footprint, usize>,
    request_groups: Vec<Vec<NodeKey>>,
}

/// A plan's eyes, view and work lists, for its visits.
struct Visit {
    eyes: [Eye; 2],
    velocity: DVec3,
    view: Option<View>,
    guarded: f64,
    frame: u64,
    tiles: Vec<i32>,
    wanted: Vec<(f64, NodeKey)>,
    /// Nodes to visit now (and whether they enter the cut).
    stack: Vec<(NodeKey, bool)>,
    /// Nodes to visit next plan.
    pending: Vec<NodeKey>,
    groups: FxHashMap<(Footprint, i32), Decision>,
    switches: usize,
    switch_px: f64,
    visits: usize,
}

impl Planner {
    /// Roots: every node of the coarsest level from the planet's centre (a
    /// plane's lowest ground) to its highest, and as deep and high as the
    /// edits reach, with the planet's whole height range (`range`, base
    /// layers) as their footprints' estimate; nodes of volume `volume`.
    pub fn new(
        volume: u16,
        grid: &Grid,
        range: (i32, i32),
        config: PlanConfig,
        edits: Arc<EditView>,
    ) -> Self {
        let level = grid.levels() - 1;
        let occluder = (!grid.is_plane())
            .then(|| grid.radius() + f64::from(range.0) * f64::from(grid.layer_mm()) * 1e-3)
            .filter(|r| *r > 0.0);
        let mut planner = Self {
            volume,
            nodes: FxHashMap::default(),
            footprints: FxHashMap::default(),
            known: FxHashMap::default(),
            seen: FxHashMap::default(),
            rays: false,
            roots: Vec::new(),
            fresh_roots: Vec::new(),
            root_level: level,
            range,
            config,
            edits,
            occluder,
            last_plan: 0,
            plans: 0,
            drawn: FxHashMap::default(),
            changes: FxHashMap::default(),
            showing: Vec::new(),
            dirty: Vec::new(),
            moved: 0.0,
            sped: 0.0,
            turned: 0.0,
            last: None,
            queues: Default::default(),
            idle: Default::default(),
            lateness: LATENESS,
            leaving: Vec::new(),
            scratch: PlanScratch::default(),
        };
        planner.place_roots(grid);
        planner
    }

    /// The roots for the planet and its edits: new ones enter the cut at
    /// the next plan, those no longer roots leave it.
    fn place_roots(&mut self, grid: &Grid) {
        let level = self.root_level;
        let n = tiles_at(grid, level);
        let layers = ((self.range.0 >> level) - 1, (self.range.1 >> level) + 1);
        let ranges = Ranges {
            volume: layers,
            surface: layers,
        };
        // Base layers the roots span: the centre (or the lowest ground)
        // to the highest ground, widened by the edits'.
        let bottom = if grid.is_plane() {
            self.range.0
        } else {
            -(grid.radius() / grid.voxel_size()).ceil() as i32
        };
        let (mut lo, mut hi) = (bottom.min(self.range.0), self.range.1);
        if let Some((a, b)) = self.edits.reach {
            lo = lo.min(a);
            hi = hi.max(b);
        }
        if let Some((a, b)) = self.edits.bricks.layers {
            lo = lo.min(a);
            hi = hi.max(b);
        }
        let cz = |k: i32| (k >> level).div_euclid(CHUNK);
        let old: FxHashSet<NodeKey> = self.roots.drain(..).collect();
        for &face in grid.faces() {
            for cx in 0..n {
                for cy in 0..n {
                    let fp = Footprint {
                        volume: self.volume,
                        face,
                        level,
                        cx,
                        cy,
                    };
                    self.footprints.entry(fp).or_insert(Fp {
                        ranges,
                        exact: false,
                        quarters: [None; 4],
                        guess: 0,
                        class: None,
                    });
                    self.roots.extend((cz(lo)..=cz(hi)).map(|z| fp.node(z)));
                }
            }
        }
        let now: FxHashSet<NodeKey> = self.roots.iter().copied().collect();
        self.fresh_roots
            .extend(self.roots.iter().filter(|k| !old.contains(k)).copied());
        let tiles: Vec<i32> = (0..=level).map(|l| tiles_at(grid, l)).collect();
        let gone: Vec<NodeKey> = old.into_iter().filter(|k| !now.contains(k)).collect();
        self.leave(gone, self.last_plan.saturating_sub(1), &tiles);
    }

    pub fn config(&self) -> &PlanConfig {
        &self.config
    }

    /// Seconds over which a window's change of velocity moves the prefetch
    /// eye (at least a tenth of a second: the motion allowance reads the
    /// velocity too).
    fn velocity_seconds(&self) -> f64 {
        self.config.prefetch_seconds.max(0.1)
    }

    /// `key`'s node, made (out of the cut, idle from `frame`) when unknown.
    fn node_mut(&mut self, key: NodeKey, frame: u64) -> &mut Node {
        match self.nodes.entry(key) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => {
                self.known.entry(key.footprint()).or_default().push(key.cz);
                self.idle[IDLE_KNOWN].push(Idle { used: frame, key });
                e.insert(Node {
                    used: frame,
                    ..Node::default()
                })
            }
        }
    }

    /// Forget `key`'s node (and its footprint with its last node, a root's
    /// aside).
    fn remove_node(&mut self, key: NodeKey) {
        self.nodes.remove(&key);
        let fp = key.footprint();
        if let Some(known) = self.known.get_mut(&fp) {
            if let Some(i) = known.iter().position(|&z| z == key.cz) {
                known.swap_remove(i);
            }
            if known.is_empty() {
                self.known.remove(&fp);
                if fp.level != self.root_level {
                    self.footprints.remove(&fp);
                }
            }
        }
    }

    /// Visit `key` at the next plan, if it is in the cut.
    fn mark(&mut self, key: NodeKey) {
        let next = self.plans + 1;
        if let Some(n) = self.nodes.get_mut(&key).filter(|n| n.in_cut) {
            n.marked = next;
            self.dirty.push(key);
        }
    }

    /// Visit `fp`'s nodes in the cut at the next plan.
    fn mark_footprint(&mut self, fp: Footprint) {
        let Some(known) = self.known.get(&fp) else {
            return;
        };
        let next = self.plans + 1;
        for &cz in known {
            let key = fp.node(cz);
            if let Some(n) = self.nodes.get_mut(&key).filter(|n| n.in_cut) {
                n.marked = next;
                self.dirty.push(key);
            }
        }
    }

    /// What happened to `key` may change it, its footprint's other nodes
    /// (its band; rock an edit added waiting for its surface) and its
    /// parent's band (ready once its children are generated): they are
    /// visited next plan. Out of the cut, it idles by what it holds now.
    fn changed(&mut self, key: NodeKey) {
        self.mark_footprint(key.footprint());
        if key.level < self.root_level {
            self.mark(key.parent());
        }
        if let Some(n) = self.nodes.get(&key).filter(|n| !n.in_cut) {
            self.idle[idle_kind(n)].push(Idle { used: n.used, key });
        }
    }

    /// Every node of the cut visited next plan (what every outcome depends
    /// on changed: the view's shape, rays' first report).
    fn invalidate(&mut self) {
        let next = self.plans + 1;
        for (k, n) in self.nodes.iter_mut().filter(|(_, n)| n.in_cut) {
            n.marked = next;
            self.dirty.push(*k);
        }
    }

    /// What `key` holds, from its footprint (which must exist).
    fn kind(&mut self, key: NodeKey) -> Kind {
        let edits = &self.edits;
        let fp = self
            .footprints
            .get_mut(&key.footprint())
            .expect("a visited node's footprint");
        if fp.class.as_ref().is_none_or(|(v, _)| *v != edits.version) {
            fp.class = Some((edits.version, edits.classify(key.footprint(), fp.ranges)));
        }
        let class = &fp.class.as_ref().unwrap().1;
        edits.kind(key.footprint(), class, key.cz)
    }

    /// The footprints under `key`'s (those on the face: `tiles` along its
    /// edge), estimated unless measured: from the measured quarter of its
    /// columns over each (a coarse footprint spans all its children's
    /// relief: one on the Moon 200 m across holds crater walls tens of
    /// metres over the plain beside them), else from its ranges. Nodes in
    /// the cut of those whose estimate changed are visited again.
    fn estimate_children(&mut self, key: NodeKey, tiles: i32, stack: &mut Vec<(NodeKey, bool)>) {
        let Some(parent) = self.footprints.get(&key.footprint()) else {
            return;
        };
        let (whole, quarters, guess) = (
            parent.ranges,
            parent.quarters,
            if parent.exact {
                1
            } else {
                parent.guess.saturating_add(1)
            },
        );
        for (q, (a, b)) in [(0, 0), (1, 0), (0, 1), (1, 1)].into_iter().enumerate() {
            let ranges = quarters[q].unwrap_or(whole).child();
            let fp = Footprint {
                volume: key.volume,
                face: key.face,
                level: key.level - 1,
                cx: key.cx * 2 + a,
                cy: key.cy * 2 + b,
            };
            if fp.cx >= tiles || fp.cy >= tiles {
                continue;
            }
            let entry = self.footprints.entry(fp).or_insert(Fp {
                ranges,
                exact: false,
                quarters: [None; 4],
                guess,
                class: None,
            });
            if !entry.exact && entry.ranges != ranges {
                entry.ranges = ranges;
                entry.guess = guess;
                entry.class = None;
                for &cz in self.known.get(&fp).into_iter().flatten() {
                    let k = fp.node(cz);
                    if let Some(n) = self.nodes.get_mut(&k).filter(|n| n.in_cut) {
                        n.marked = self.plans;
                        stack.push((k, false));
                    }
                }
            }
        }
    }

    fn guess_of(&self, key: NodeKey) -> u8 {
        self.footprints
            .get(&key.footprint())
            .map_or(0, |f| if f.exact { 0 } else { f.guess })
    }

    /// Whether layers `layers` of `fp` cannot be seen from `eye`: below
    /// its horizon over the lowest ground (spheres; an eye under that ground
    /// sees past it), or past `max_distance` (planes). Conservative: their
    /// nearest point against the horizon distances of the eye and of their
    /// top (`distance`: from the eye to them). And how far the eye may move
    /// before that could change (the horizon distance grows `e / horizon`
    /// metres a metre the eye climbs).
    fn beyond(
        &self,
        grid: &Grid,
        fp: Footprint,
        layers: (i32, i32),
        eye: &Eye,
        distance: f64,
    ) -> (bool, f64) {
        let late = self.lateness / 4.0 * distance;
        let reach = eye.reach_margin(fp, layers) / 2.0;
        let Some(r) = self.occluder else {
            let past = distance - self.config.max_distance;
            return (past > 0.0, ((past.abs() + late) / DRIFT).min(reach));
        };
        let over = |radius: f64| (radius * radius - r * r).max(0.0).sqrt();
        let e = eye.position.length();
        if e <= r {
            return (false, r - e);
        }
        let top = grid.radius() + f64::from(layers.1) * grid.level_size(fp.level);
        let past = distance - (over(e) + over(top));
        let rate = DRIFT + e / over(e).max(1e-6);
        (
            past > 0.0,
            ((past.abs() + late) / rate).min(e - r).min(reach),
        )
    }

    /// Whether layers `layers` of `fp` lie in the cone of half-angle cosine
    /// `cos_half` around the view from `eye` (generous for their extent:
    /// anything beside them is near them; all of them without a view), and
    /// how far the eye may move and the view turn before that could change:
    /// its margin moves at most `(1 + |cos_half|) DRIFT` times as fast as
    /// the eye (the offset along the view and its length both move), and
    /// the breadth of a radian at their distance with the view (each takes
    /// half the margin).
    fn cone(
        &self,
        grid: &Grid,
        fp: Footprint,
        layers: (i32, i32),
        eye: &Eye,
        view: Option<View>,
        cos_half: f64,
    ) -> (bool, Window) {
        let Some(v) = view else {
            return (true, Window::EVER);
        };
        let offset = eye.toward(grid, fp, layers);
        self.cone_at(grid, fp, offset, eye.reach_margin(fp, layers), v, cos_half)
    }

    /// [`Self::cone`] from the eye's `offset` to the layers (`toward`), the
    /// margin to its frame's reach `reach` (`reach_margin`).
    fn cone_at(
        &self,
        grid: &Grid,
        fp: Footprint,
        offset: DVec3,
        reach: f64,
        v: View,
        cos_half: f64,
    ) -> (bool, Window) {
        let length = offset.length();
        let size = grid.level_size(fp.level) * f64::from(CHUNK) * 1.5;
        let margin = (size - length).max(offset.dot(v.forward) - cos_half * length + size);
        let slack = margin.abs() + self.lateness * length;
        (
            margin > 0.0,
            Window {
                moves: (slack / (2.0 * (1.0 + cos_half.abs()) * DRIFT)).min(reach / 2.0),
                speeds: f64::INFINITY,
                turns: slack / (2.0 * length.max(1e-3)),
                until: u64::MAX,
            },
        )
    }

    /// Cells of `key` project this many times `lod_pixels` from the
    /// nearest of `eyes`.
    fn error(&self, grid: &Grid, key: NodeKey, eyes: &[Eye]) -> f64 {
        self.error_box(grid, key.footprint(), key.layers(), eyes)
    }

    /// Cells of `fp`'s layers `layers` project this many times
    /// `lod_pixels` from the nearest of `eyes`.
    fn error_box(&self, grid: &Grid, fp: Footprint, layers: (i32, i32), eyes: &[Eye]) -> f64 {
        let distance = eyes
            .iter()
            .map(|e| tile_distance(grid, fp, layers, e))
            .fold(f64::INFINITY, f64::min);
        grid.level_size(fp.level)
            / distance.max(1e-3)
            / (self.config.pixel_angle * self.config.lod_pixels)
    }

    /// How many times `lod_pixels` cells may cover where the eye's motion
    /// (`velocity`) sweeps their image, from its `offset` to them (`toward`),
    /// and what it is made of: the motion across that offset, and its
    /// length.
    fn motion_allowance(&self, offset: DVec3, velocity: DVec3) -> (f64, f64, f64) {
        let distance = offset.length().max(1e-3);
        if self.config.motion_flow <= 0.0 {
            return (1.0, 0.0, distance);
        }
        let across = velocity - offset * (velocity.dot(offset) / (distance * distance));
        let allowance = (1.0 + across.length() / distance * self.motion_scale())
            .min(self.config.motion_max.max(1.0));
        (allowance, across.length(), distance)
    }

    /// The motion allowance's flow (pixels a frame) over `motion_flow`, per
    /// metre a second across, a metre away.
    fn motion_scale(&self) -> f64 {
        MOTION_FRAME_SECONDS / self.config.pixel_angle / self.config.motion_flow
    }

    /// The range a band's error (`error_box`, `error` now) and its motion
    /// allowance (`motion`, from `across` metres a second over an offset of
    /// `length`: `motion_allowance`) may take while the eye travels `s`
    /// metres more and its velocity changes as `Window::ahead` lets it (the
    /// prefetch eye then travels `(1 + SPEED_SHARE) s`): distances move
    /// `DRIFT` times as fast as the eyes, the offset's direction turns
    /// `2 DRIFT s / (length - DRIFT s)` radians at most, which changes the
    /// motion across it by twice the velocity that much, and by the
    /// velocity's own change.
    #[allow(clippy::too_many_arguments)]
    fn error_bounds(
        &self,
        grid: &Grid,
        fp: Footprint,
        error: f64,
        speed: f64,
        across: f64,
        length: f64,
        s: f64,
    ) -> ((f64, f64), (f64, f64)) {
        let k = grid.level_size(fp.level) / (self.config.pixel_angle * self.config.lod_pixels);
        let distance = k / error;
        let travel = DRIFT * (1.0 + SPEED_SHARE) * s;
        let errors = (k / (distance + travel), k / (distance - travel).max(1e-3));
        if self.config.motion_flow <= 0.0 {
            return (errors, (1.0, 1.0));
        }
        let dv = s * SPEED_SHARE / self.velocity_seconds();
        let v = speed + dv;
        let near = (length - DRIFT * s).max(1e-3);
        let turn = (2.0 * DRIFT * s / near).min(2.0);
        let change = 2.0 * v * turn + dv;
        let most = self.config.motion_max.max(1.0);
        let allowance =
            |across: f64, length: f64| (1.0 + across / length * self.motion_scale()).min(most);
        (
            errors,
            (
                allowance((across - change).max(0.0), length + DRIFT * s),
                allowance((across + change).min(v), near),
            ),
        )
    }

    /// The band `key` switches levels with: its footprint's contiguous Mixed
    /// layers holding it (itself when it is not Mixed by class: a Buried
    /// node generated).
    fn band_of(&mut self, key: NodeKey) -> (i32, i32) {
        let _ = self.kind(key);
        let class = &self.footprints[&key.footprint()]
            .class
            .as_ref()
            .expect("classified above")
            .1;
        class
            .mixed
            .iter()
            .find(|(a, b)| (*a..=*b).contains(&key.cz))
            .copied()
            .unwrap_or((key.cz, key.cz))
    }

    /// The layers (level cells) the chunks of nodes `members` (a band)
    /// occupy, once every one's result came back (the same whether
    /// generated or restored).
    fn occupied_of(&self, members: impl Iterator<Item = NodeKey>) -> Option<(i32, i32)> {
        let mut occupied: Option<(i32, i32)> = None;
        for m in members {
            match self.nodes.get(&m) {
                Some(n) if n.generated && n.empty => {}
                Some(Node {
                    generated: true,
                    occupied: Some(o),
                    ..
                }) => occupied = Some(occupied.map_or(*o, |a| (a.0.min(o.0), a.1.max(o.1)))),
                _ => return None,
            }
        }
        occupied
    }

    /// Whether a Mixed node of `fp` (classified) is not generated yet.
    fn surface_pending(&self, fp: Footprint) -> bool {
        let Some((_, class)) = self.footprints.get(&fp).and_then(|f| f.class.as_ref()) else {
            return false;
        };
        class.mixed.iter().any(|&(a, b)| {
            (a..=b).any(|cz| !self.nodes.get(&fp.node(cz)).is_some_and(|n| n.generated))
        })
    }

    /// Whether `key`'s band (`band`, its footprint's layers) is refined,
    /// from its box: its error, in view, allowances; ready once every Mixed
    /// child of every node in it is generated (the children wanted, and the
    /// nodes under the eye when it is far too coarse, are queued in
    /// `wanted`); and how long that holds. `distances`: from the eye and the
    /// prefetch eye to the band.
    fn decide(
        &mut self,
        grid: &Grid,
        key: NodeKey,
        band: (i32, i32),
        distances: [f64; 2],
        v: &mut Visit,
    ) -> Decision {
        let (velocity, view, guarded, frame) = (v.velocity, v.view, v.guarded, v.frame);
        let fp = key.footprint();
        // Refinement measures to the band's chunks, air over its ground
        // included (under a hovering eye, more than its cells' own distance
        // asks); the plan's worst cell, to the cells drawn (`cells`).
        let layers = (band.0 * CHUNK, (band.1 + 1) * CHUNK);
        let members = (band.0..=band.1).map(|cz| fp.node(cz));
        let cell = grid.level_size(fp.level) / (self.config.pixel_angle * self.config.lod_pixels);
        let error = cell / distances[0].min(distances[1]).max(1e-3);
        let now = cell / distances[0].max(1e-3);
        let cells = self
            .occupied_of(members.clone())
            .map(|o| self.error_box(grid, fp, o, &v.eyes[..1]));
        // In view: the band's nearest point within the cone.
        let offset = v.eyes[0].toward(grid, fp, layers);
        let reach0 = v.eyes[0].reach_margin(fp, layers);
        let ((seen, seen_window), (guarded_in, guarded_window)) = match view {
            Some(w) => (
                self.cone_at(grid, fp, offset, reach0, w, w.cos_half),
                self.cone_at(grid, fp, offset, reach0, w, guarded),
            ),
            None => ((true, Window::EVER), (true, Window::EVER)),
        };
        let (motion, across, length) = self.motion_allowance(offset, velocity);
        // Met by rays lately (or no ray report yet): as fine as wanted.
        let last_met = members
            .clone()
            .filter_map(|m| self.seen.get(&m).copied())
            .max();
        let met = !self.rays
            || self.config.unseen_pixels <= 1.0
            || last_met.is_some_and(|f| f + self.config.seen_frames >= frame);
        let refined = members
            .clone()
            .any(|m| self.nodes.get(&m).is_some_and(|n| n.refined));
        // Rays not meeting a band only keep it from refining: a refined
        // band they stop meeting (its children hidden) stays refined, or its
        // coarser cells, poking out, would be met and refine it again.
        let unmet = !met && !refined;
        let offscreen = !seen && !guarded_in;
        let allowance = motion
            * if offscreen {
                self.config.offscreen_pixels
            } else if unmet {
                self.config.unseen_pixels
            } else {
                1.0
            };
        // Bands in view first, by what they need now.
        let priority = if seen {
            now / allowance * 1e6
        } else {
            now / allowance
        };
        let wanted_error = error / allowance;
        let finer = key.level > 0
            && if refined {
                wanted_error > self.config.coarsen
            } else {
                wanted_error > 1.0
            };
        // Children wait for their footprint's measured ranges unless the
        // band is far too coarse (an eye that dropped from orbit) and a
        // measured ancestor is near: then they come estimated.
        let deep = wanted_error >= DEEP_REFINE;
        let guess = self.guess_of(key);
        let exact = self.footprints.get(&fp).is_some_and(|f| f.exact);
        let refine = finer && (exact || deep && guess < GUESS_LEVELS);
        let mut ready = refine;
        // A band whose members all stay open, and whose nodes no event
        // marked, has its children as they were: ready, and nothing of them
        // wanted (their own visits want what they need).
        let plan = self.plans;
        let unchanged = members.clone().all(|m| {
            self.nodes
                .get(&m)
                .is_some_and(|n| n.open && n.marked != plan)
        });
        if refine && !unchanged {
            let tiles = v.tiles[key.level as usize - 1];
            self.estimate_children(key, tiles, &mut v.stack);
            for m in members.clone() {
                // A member drawn as it was before an edit hands that to its
                // children (rock's children are rock, air's air).
                let inherited = self.nodes.get(&m).and_then(|n| n.stand_in);
                for child in m.children(tiles) {
                    let kind = self.kind(child);
                    let needs =
                        kind != Kind::Empty && self.needs_data(grid, child, kind, &v.eyes[0]);
                    let entry = self.node_mut(child, frame);
                    if !entry.in_cut {
                        entry.used = frame;
                    }
                    if !entry.generated && needs {
                        if entry.stand_in.is_none() {
                            entry.stand_in = inherited;
                            entry.edited = frame;
                        }
                        // A Buried child is rock meanwhile, an edited one
                        // what it was; another Mixed one has no stand-in
                        // but its parent.
                        if kind == Kind::Mixed && entry.stand_in.is_none() {
                            ready = false;
                        }
                        v.wanted.push((priority, child));
                    }
                }
            }
        }
        // Far too coarse: the nodes under the eye down to the level it
        // wants are wanted now (each level's eight, along the child
        // nearest it holding ground, from the member nearest it), so the
        // cut refines through them as they come.
        // The chain holds while the eye's nearest node at each step stays
        // the nearest, its error stays on its side of 1, and Buried nodes
        // under it stay near or far.
        let mut chain = f64::INFINITY;
        if refine && deep {
            let late = self.lateness.exp();
            let eye = &v.eyes[0];
            let near = |k: &NodeKey| tile_distance(grid, k.footprint(), k.layers(), eye);
            // The nearest of `keys` and how far the eye may move before
            // another could be nearer (both distances move).
            let nearest = |keys: &mut dyn Iterator<Item = NodeKey>| {
                let (mut first, mut second) = ((f64::INFINITY, None), f64::INFINITY);
                for k in keys {
                    let d = near(&k);
                    if d < first.0 {
                        second = first.0;
                        first = (d, Some(k));
                    } else if d < second {
                        second = d;
                    }
                }
                (first.1, (second - first.0) / (2.0 * DRIFT))
            };
            let (start, margin) = nearest(&mut members.clone());
            chain = chain.min(margin);
            let mut at = start.expect("a band has a member");
            loop {
                let tiles = v.tiles[at.level as usize - 1];
                let mixed: Vec<NodeKey> = at
                    .children(tiles)
                    .filter(|&c| self.kind(c) == Kind::Mixed)
                    .collect();
                let (next, margin) = nearest(&mut mixed.into_iter());
                let Some(next) = next else { break };
                chain = chain.min(margin);
                if next.level == 0 || self.guess_of(next) >= GUESS_LEVELS {
                    break;
                }
                // Its error from either eye, the prefetch eye moving a
                // quarter more (`Window::ahead`).
                let error = self.error(grid, next, &v.eyes);
                let k = grid.level_size(next.level)
                    / (self.config.pixel_angle * self.config.lod_pixels);
                let distance = k / error;
                let travel = DRIFT * (1.0 + SPEED_SHARE);
                chain = chain.min(if error > 1.0 {
                    (k * late - distance) / travel
                } else {
                    (distance - k / late) / travel
                });
                if error <= 1.0 {
                    break;
                }
                let tiles = v.tiles[next.level as usize - 1];
                self.estimate_children(next, tiles, &mut v.stack);
                for c in next.children(tiles) {
                    let kind = self.kind(c);
                    let needs = kind != Kind::Empty && self.needs_data(grid, c, kind, eye);
                    chain = chain.min(self.near_window(grid, c, kind, eye).moves);
                    let entry = self.node_mut(c, frame);
                    if !entry.in_cut {
                        entry.used = frame;
                    }
                    if !entry.generated && needs {
                        v.wanted.push((priority, c));
                    }
                }
                at = next;
            }
        }
        // How long this holds, for the band as it now is (refined if
        // ready): the thresholds its error and allowance test, its cones,
        // and the frame rays' last report of it expires.
        let threshold = if key.level == 0 {
            None
        } else if exact {
            Some(if ready { self.config.coarsen } else { 1.0 })
        } else {
            (guess < GUESS_LEVELS).then_some(DEEP_REFINE)
        };
        // Thresholds of the wanted error (refining, coarsening, far too
        // coarse) and of the error (the plan's measure of a leaf in view),
        // each on the side it is now; one may be passed by `lateness`.
        let late = self.lateness.exp();
        let side = |now: f64, t: f64, (lo, hi): (f64, f64)| {
            if now > t {
                lo * late > t
            } else {
                hi <= t * late
            }
        };
        let factor = allowance / motion;
        let speed = velocity.length();
        let stable = |s: f64| {
            let (errors, motions) = self.error_bounds(grid, fp, error, speed, across, length, s);
            let wanted = (
                errors.0 / (motions.1 * factor),
                errors.1 / (motions.0 * factor),
            );
            threshold.is_none_or(|t| side(wanted_error, t, wanted))
                && !(refine && exact && !side(wanted_error, DEEP_REFINE, wanted))
                && (ready || key.level == 0 || side(error, 1.5, errors))
        };
        let distance =
            grid.level_size(fp.level) / (error * self.config.pixel_angle * self.config.lod_pixels);
        let reach = (2.0 * distance.min(length))
            .min(reach0 / 2.0)
            .min(v.eyes[1].reach_margin(fp, layers) / 2.0);
        // The nodes under the eye of a band far too coarse follow it.
        let moves = widest(reach, stable).min(chain.max(0.0));
        let seconds = self.velocity_seconds();
        let mut window = Window::ahead(moves, seconds);
        if self.config.offscreen_pixels != 1.0 {
            window = window.min(guarded_window);
        }
        if !ready {
            // The plan's measures of a leaf in view.
            window = window.min(seen_window);
            if met && self.rays && self.config.unseen_pixels > 1.0 {
                if let Some(f) = last_met {
                    window = window.min(Window::until(f + self.config.seen_frames + 1));
                }
            }
        }
        Decision {
            error,
            seen,
            allowance,
            ready,
            exact,
            guess,
            cells,
            window,
        }
    }

    /// Whether `kind` at `key` must be generated before it is drawn: Mixed,
    /// or Buried and reached by a ray or near the eye.
    fn needs_data(&self, grid: &Grid, key: NodeKey, kind: Kind, eye: &Eye) -> bool {
        match kind {
            Kind::Mixed => true,
            Kind::Buried => {
                self.nodes.get(&key).is_some_and(|n| n.reached)
                    || tile_distance(grid, key.footprint(), key.layers(), eye)
                        < self.config.near_cells * grid.level_size(key.level)
            }
            Kind::Empty | Kind::Solid => false,
        }
    }

    /// How long `needs_data` holds for `kind` at `key`: a Buried node not
    /// reached needs it while near the eye.
    fn near_window(&self, grid: &Grid, key: NodeKey, kind: Kind, eye: &Eye) -> Window {
        if kind != Kind::Buried || self.nodes.get(&key).is_some_and(|n| n.reached) {
            return Window::EVER;
        }
        let distance = tile_distance(grid, key.footprint(), key.layers(), eye);
        let near = self.config.near_cells * grid.level_size(key.level);
        Window::moves(
            (((distance - near).abs() + self.lateness * distance) / DRIFT)
                .min(eye.reach_margin(key.footprint(), key.layers()) / 2.0),
        )
    }

    /// Plan for an eye at `eye` moving at `velocity`: the nodes of the cut
    /// events touched, whose re-checks are due or that want others
    /// generated are visited (coarsest first, refining or coarsening the
    /// cut under them as they decide).
    pub fn plan(
        &mut self,
        grid: &Grid,
        eye: DVec3,
        velocity: DVec3,
        view: Option<View>,
        frame: u64,
    ) -> Plan {
        // The eyes' travel and the view's turn since the last plan.
        if let Some((e, v, w)) = self.last {
            self.moved += (eye - e).length();
            self.sped += (velocity - v).length();
            match (w, view) {
                (Some(a), Some(b)) if a.cos_half == b.cos_half => {
                    self.turned += a.forward.angle_between(b.forward);
                }
                (None, None) => {}
                _ => self.invalidate(),
            }
        }
        self.last = Some((eye, velocity, view));
        self.plans += 1;
        let PlanScratch {
            mut tiles,
            mut wanted,
            mut priority_order,
            mut stack,
            mut work,
            mut pending,
            decisions: mut groups,
            mut order,
            request_groups: mut requests,
        } = std::mem::take(&mut self.scratch);
        // Re-checks due.
        work.clear();
        work.append(&mut self.dirty);
        let readings = [self.moved, self.sped, self.turned, frame as f64];
        for (i, (heap, now)) in self.queues.iter_mut().zip(readings).enumerate() {
            while heap.peek().is_some_and(|d| d.at <= now) {
                let d = heap.pop().unwrap();
                let Some(n) = self.nodes.get_mut(&d.key) else {
                    continue;
                };
                if n.queued[i] != d.at {
                    continue;
                }
                n.queued[i] = f64::INFINITY;
                if !n.in_cut {
                    continue;
                }
                let due = n.due[i];
                if due <= now {
                    work.push(d.key);
                } else if due.is_finite() {
                    heap.push(Due {
                        at: due,
                        key: d.key,
                    });
                    n.queued[i] = due;
                }
            }
        }
        // Coarsest first: a node coarsening takes its subtree out of the
        // cut before its nodes would be visited.
        work.sort_unstable_by_key(|k| std::cmp::Reverse(k.level));
        tiles.clear();
        tiles.extend((0..=self.root_level).map(|l| tiles_at(grid, l)));
        wanted.clear();
        stack.clear();
        pending.clear();
        groups.clear();
        let guarded = view.map_or(-1.0, |v| {
            (v.cos_half.clamp(-1.0, 1.0).acos() + self.config.view_guard)
                .min(std::f64::consts::PI)
                .cos()
        });
        let mut visit = Visit {
            eyes: [
                Eye::new(grid, eye),
                Eye::new(grid, eye + velocity * self.config.prefetch_seconds),
            ],
            velocity,
            view,
            guarded,
            frame,
            tiles,
            wanted,
            stack,
            pending,
            groups,
            switches: 0,
            switch_px: 0.0,
            visits: 0,
        };
        let fresh = std::mem::take(&mut self.fresh_roots);
        for (key, entering) in fresh
            .iter()
            .map(|&k| (k, true))
            .chain(work.iter().map(|&k| (k, false)))
        {
            visit.stack.push((key, entering));
            while let Some((key, entering)) = visit.stack.pop() {
                self.visit(grid, key, entering, &mut visit);
            }
        }
        let Visit {
            tiles,
            mut wanted,
            stack,
            mut pending,
            groups,
            switches,
            switch_px,
            visits,
            ..
        } = visit;
        let next = self.plans + 1;
        for key in &pending {
            if let Some(n) = self.nodes.get_mut(key) {
                n.marked = next;
            }
        }
        self.dirty.append(&mut pending);
        // The plan's measures over the leaves in view.
        let mut plan = Plan {
            switches,
            switch_px,
            visits,
            ..Plan::default()
        };
        let mut coarse = 0usize;
        let mut worst: Option<Shown> = None;
        for s in &self.showing {
            coarse += usize::from(s.coarse);
            if let Some(cells) = s.cells {
                plan.worst_wanted = plan.worst_wanted.max(cells / s.allowance);
                if cells > plan.worst_px {
                    plan.worst_px = cells;
                    worst = Some(*s);
                }
            }
        }
        plan.coarse_share = coarse as f64 / self.showing.len().max(1) as f64;
        plan.worst = worst.map(|s| {
            let (children, generated) = s.key.children(tiles[s.key.level as usize - 1]).fold(
                (0, 0),
                |(count, generated), c| {
                    (
                        count + 1,
                        generated + usize::from(self.nodes.get(&c).is_some_and(|n| n.generated)),
                    )
                },
            );
            Worst {
                key: s.key,
                exact: s.exact,
                guess: s.guess,
                children,
                generated,
            }
        });
        // The cut's change.
        for (key, before) in self.changes.drain() {
            let now = self.drawn.get(&key).copied();
            if now != before {
                if before.is_some() {
                    plan.removed.push(key);
                }
                if let Some(kind) = now {
                    plan.added.push((key, kind));
                }
            }
        }
        plan.waiting = wanted.len();
        if wanted.len() > MAX_REQUESTS {
            wanted.select_nth_unstable_by(MAX_REQUESTS, |a, b| b.0.total_cmp(&a.0));
            wanted.truncate(MAX_REQUESTS);
        }
        // Preserve the stable priority order without stable sort's temporary
        // heap buffer: the original position breaks ties in the index list.
        priority_order.clear();
        priority_order.extend(0..wanted.len());
        priority_order
            .sort_unstable_by(|&a, &b| wanted[b].0.total_cmp(&wanted[a].0).then(a.cmp(&b)));
        // A footprint's nodes together, where its most wanted one is.
        order.clear();
        let mut group_count = 0;
        for &index in &priority_order {
            let key = wanted[index].1;
            let n = *order.entry(key.footprint()).or_insert_with(|| {
                if group_count == requests.len() {
                    requests.push(Vec::new());
                }
                group_count += 1;
                group_count - 1
            });
            if !requests[n].contains(&key) {
                requests[n].push(key);
            }
        }
        plan.requests.extend(
            requests[..group_count]
                .iter_mut()
                .flat_map(|group| group.drain(..)),
        );
        wanted.clear();
        self.scratch = PlanScratch {
            tiles,
            wanted,
            priority_order,
            stack,
            work,
            pending,
            decisions: groups,
            order,
            request_groups: requests,
        };
        self.compact();
        self.last_plan = frame + 1;
        #[cfg(test)]
        plan.drawn
            .extend(self.drawn.iter().map(|(k, kind)| (*k, *kind)));
        plan
    }

    /// Visit `key` (`entering`: its parent just opened, or it is a new
    /// root; else only if it is in the cut and not visited this plan yet):
    /// what it is drawn as, or its children in its place, and when to
    /// visit it again.
    fn visit(&mut self, grid: &Grid, key: NodeKey, entering: bool, v: &mut Visit) {
        let frame = v.frame;
        let this_plan = self.plans;
        match self.nodes.get(&key) {
            Some(n) if n.visited == this_plan || !(n.in_cut || entering) => return,
            None if !entering => return,
            _ => {}
        }
        v.visits += 1;
        let kind = self.kind(key);
        let n = self.node_mut(key, frame);
        n.visited = this_plan;
        n.in_cut = true;
        let node = *n;
        let wanted = v.wanted.len();
        let fp = key.footprint();
        let (outcome, window) = if kind == Kind::Empty {
            self.nodes.get_mut(&key).unwrap().stand_in = None;
            (
                Outcome::Leaf {
                    leaf: Some(Kind::Empty),
                    shown: None,
                },
                Window::EVER,
            )
        } else if !self.needs_data(grid, key, kind, &v.eyes[0]) {
            let near = self.near_window(grid, key, kind, &v.eyes[0]);
            // Rock an edit added where there was air (a mound's inside)
            // stays air until its footprint's new surface is generated:
            // drawn at once, its boxes covered the view while the surface
            // came.
            if node.stand_in == Some(Kind::Empty)
                && frame < node.edited + ADDED_ROCK_WAITS
                && self.surface_pending(fp)
            {
                (
                    Outcome::Leaf {
                        leaf: Some(Kind::Empty),
                        shown: None,
                    },
                    near.min(Window::until(node.edited + ADDED_ROCK_WAITS)),
                )
            } else {
                self.nodes.get_mut(&key).unwrap().stand_in = None;
                (
                    Outcome::Leaf {
                        leaf: Some(kind),
                        shown: None,
                    },
                    near,
                )
            }
        } else if !node.generated {
            let near = self.near_window(grid, key, kind, &v.eyes[0]);
            // An edited node too coarse for the eye in view hands what it
            // stands in as to its children, which are visited in its place:
            // the edit is generated at the level the eye wants, not first
            // at the coarse level it was drawn at (a 60 m stroke drew rock
            // boxes 200 m across, then refined).
            let mut opened = None;
            if let Some(shown) = node.stand_in.filter(|_| key.level > 0) {
                let (in_view, cone) =
                    self.cone(grid, fp, key.layers(), &v.eyes[0], v.view, v.guarded);
                let error = self.error(grid, key, &v.eyes);
                let exact = self.footprints.get(&fp).is_some_and(|f| f.exact);
                if in_view && error > 1.0 && (exact || self.guess_of(key) < GUESS_LEVELS) {
                    let tiles = v.tiles[key.level as usize - 1];
                    self.estimate_children(key, tiles, &mut v.stack);
                    self.nodes.get_mut(&key).unwrap().refined = true;
                    for child in key.children(tiles) {
                        let c = self.node_mut(child, frame);
                        if !c.generated && c.stand_in.is_none() {
                            c.stand_in = Some(shown);
                            c.edited = node.edited;
                        }
                    }
                    let late = self.lateness.exp();
                    let distance = grid.level_size(fp.level)
                        / (error * self.config.pixel_angle * self.config.lod_pixels);
                    let reach = (2.0 * distance)
                        .min(v.eyes[0].reach_margin(fp, key.layers()) / 2.0)
                        .min(v.eyes[1].reach_margin(fp, key.layers()) / 2.0);
                    let moves = widest(reach, |s| {
                        self.error_bounds(grid, fp, error, 0.0, 0.0, 1.0, s).0 .0 * late > 1.0
                    });
                    let seconds = self.velocity_seconds();
                    opened = Some(near.min(cone).min(Window::ahead(moves, seconds)));
                }
            }
            match opened {
                Some(window) => (Outcome::Open, window),
                None => {
                    // Else it is drawn as it was until its chunk comes; a
                    // Buried node a ray met, or holding the eye, is rock (it
                    // never holds its parent back). Else only a root can be
                    // ungenerated here: children are visited once every
                    // Mixed one is generated or stands in.
                    let shown = node
                        .stand_in
                        .or((kind == Kind::Buried).then_some(Kind::Buried));
                    v.wanted.push((f64::INFINITY, key));
                    (
                        Outcome::Leaf {
                            leaf: shown,
                            shown: None,
                        },
                        near,
                    )
                }
            }
        } else {
            // Generated: drawn from its chunk (a Buried one too). The node's
            // band (its footprint's contiguous Mixed layers) switches levels
            // as one: a coarse cell's ground, rounded, may lie one level
            // finer under a node above or below it, so nodes of a band
            // refined apart would leave the surface between them (holes
            // through a fade's dither). Bands apart (caves, digs) switch
            // apart. Every test below is the band's.
            let band = self.band_of(key);
            let band_layers = (band.0 * CHUNK, (band.1 + 1) * CHUNK);
            let drawn = if node.empty { Kind::Empty } else { Kind::Mixed };
            // Bands that cannot be seen are drawn as they are, not refined;
            // those the prefetch eye sees are ready when they come over the
            // horizon.
            let distances = [
                tile_distance(grid, fp, band_layers, &v.eyes[0]),
                tile_distance(grid, fp, band_layers, &v.eyes[1]),
            ];
            let (hidden, now) = self.beyond(grid, fp, band_layers, &v.eyes[0], distances[0]);
            let (hidden_ahead, ahead) =
                self.beyond(grid, fp, band_layers, &v.eyes[1], distances[1]);
            let horizon = Window::ahead(
                now.min(ahead / (1.0 + SPEED_SHARE)),
                self.velocity_seconds(),
            );
            if hidden && hidden_ahead {
                self.nodes.get_mut(&key).unwrap().refined = false;
                (
                    Outcome::Leaf {
                        leaf: Some(drawn),
                        shown: None,
                    },
                    horizon,
                )
            } else {
                let d = match v.groups.get(&(fp, band.0)) {
                    Some(d) => *d,
                    None => {
                        let d = self.decide(grid, key, band, distances, v);
                        v.groups.insert((fp, band.0), d);
                        // Its other members follow it.
                        v.stack.extend(
                            (band.0..=band.1)
                                .filter(|&cz| cz != key.cz)
                                .map(|cz| (fp.node(cz), false)),
                        );
                        d
                    }
                };
                let shows = d.seen && !hidden;
                // A switch shows where the last plan drew the side it leaves.
                let showed = |nodes: &FxHashMap<NodeKey, Node>, k: &NodeKey| {
                    nodes.get(k).is_some_and(|n| n.showing.is_some())
                };
                if shows
                    && node.refined != d.ready
                    && if d.ready {
                        showed(&self.nodes, &key)
                    } else {
                        key.children(v.tiles[key.level as usize - 1])
                            .any(|c| showed(&self.nodes, &c))
                    }
                {
                    v.switches += 1;
                    v.switch_px = v.switch_px.max(d.cells.unwrap_or(0.0));
                }
                let n = self.nodes.get_mut(&key).unwrap();
                if n.refined != d.ready {
                    // Its next decision tests other thresholds.
                    v.pending.push(key);
                }
                n.refined = d.ready;
                let window = d.window.min(horizon);
                if d.ready {
                    (Outcome::Open, window)
                } else {
                    // Measured to the cells drawn (an eye in a chunk's air is
                    // not at its cells); a band whose chunks' results are not
                    // back yet counts once they are.
                    let fine = key.level > 0;
                    let shown = shows.then_some(Shown {
                        key,
                        coarse: d.error > 1.5 && fine,
                        cells: d.cells.filter(|_| fine),
                        allowance: d.allowance,
                        exact: d.exact,
                        guess: d.guess,
                    });
                    (
                        Outcome::Leaf {
                            leaf: Some(drawn),
                            shown,
                        },
                        window,
                    )
                }
            }
        };
        match outcome {
            Outcome::Open => {
                let n = self.nodes.get_mut(&key).unwrap();
                n.leaf = None;
                let was_open = std::mem::replace(&mut n.open, true);
                let showing = n.showing.take();
                self.unshow(showing);
                self.set_drawn(key, None);
                if !was_open {
                    v.stack.extend(
                        key.children(v.tiles[key.level as usize - 1])
                            .map(|c| (c, true)),
                    );
                }
            }
            Outcome::Leaf { leaf, shown } => {
                let n = self.nodes.get_mut(&key).unwrap();
                n.leaf = leaf;
                let was_open = std::mem::take(&mut n.open);
                if was_open {
                    self.close(key, frame, &v.tiles);
                }
                self.set_drawn(key, leaf.filter(|k| *k != Kind::Empty));
                match shown {
                    Some(s) => self.show(s),
                    None => {
                        let showing = self.nodes.get_mut(&key).unwrap().showing.take();
                        self.unshow(showing);
                    }
                }
            }
        }
        // A node wanting others is visited every plan (their priorities
        // follow the eye); else when its outcome may change.
        let pending = v.wanted.len() > wanted;
        if pending {
            v.pending.push(key);
        }
        let readings = [self.moved, self.sped, self.turned, 0.0];
        let windows = [
            window.moves,
            window.speeds,
            if v.view.is_some() {
                window.turns
            } else {
                f64::INFINITY
            },
            if window.until == u64::MAX {
                f64::INFINITY
            } else {
                window.until as f64
            },
        ];
        let n = self.nodes.get_mut(&key).unwrap();
        for (i, (reading, window)) in readings.into_iter().zip(windows).enumerate() {
            let due = if pending {
                f64::INFINITY
            } else {
                reading + window.max(0.0)
            };
            n.due[i] = due;
            if due < n.queued[i] {
                self.queues[i].push(Due { at: due, key });
                n.queued[i] = due;
            }
        }
    }

    /// Node `key` drawn as `kind` (none: not drawn), its change noted for
    /// the plan.
    fn set_drawn(&mut self, key: NodeKey, kind: Option<Kind>) {
        let before = self.drawn.get(&key).copied();
        if before == kind {
            return;
        }
        match kind {
            Some(k) => self.drawn.insert(key, k),
            None => self.drawn.remove(&key),
        };
        self.changes.entry(key).or_insert(before);
    }

    /// `s` as its node's leaf in view.
    fn show(&mut self, s: Shown) {
        let n = self.nodes.get_mut(&s.key).unwrap();
        match n.showing {
            Some(i) => self.showing[i as usize] = s,
            None => {
                n.showing = Some(self.showing.len() as u32);
                self.showing.push(s);
            }
        }
    }

    /// The leaf in view at `showing`, if any, gone.
    fn unshow(&mut self, showing: Option<u32>) {
        let Some(i) = showing else { return };
        let i = i as usize;
        self.showing.swap_remove(i);
        if let Some(moved) = self.showing.get(i) {
            self.nodes.get_mut(&moved.key).unwrap().showing = Some(i as u32);
        }
    }

    /// `key`'s children out of the cut, and theirs.
    fn close(&mut self, key: NodeKey, frame: u64, tiles: &[i32]) {
        self.leave(key.children(tiles[key.level as usize - 1]), frame, tiles);
    }

    /// Nodes `keys` and those under them out of the cut, idle from
    /// `frame`.
    fn leave(&mut self, keys: impl IntoIterator<Item = NodeKey>, frame: u64, tiles: &[i32]) {
        let mut leaving = std::mem::take(&mut self.leaving);
        leaving.extend(keys);
        while let Some(k) = leaving.pop() {
            let Some(n) = self.nodes.get_mut(&k).filter(|n| n.in_cut) else {
                continue;
            };
            n.in_cut = false;
            n.used = frame;
            n.due = [f64::INFINITY; QUEUES];
            n.leaf = None;
            let open = std::mem::take(&mut n.open);
            let showing = n.showing.take();
            let idle = idle_kind(n);
            self.unshow(showing);
            self.set_drawn(k, None);
            self.idle[idle].push(Idle {
                used: frame,
                key: k,
            });
            if open {
                leaving.extend(k.children(tiles[k.level as usize - 1]));
            }
        }
        self.leaving = leaving;
    }

    /// Queues whose void entries outnumber the nodes four to one rebuilt
    /// without them.
    fn compact(&mut self) {
        let limit = 4 * (self.nodes.len() + 1024);
        let nodes = &self.nodes;
        for (i, heap) in self.queues.iter_mut().enumerate() {
            if heap.len() > limit {
                heap.retain(|d| nodes.get(&d.key).is_some_and(|n| n.queued[i] == d.at));
            }
        }
        for (kind, heap) in self.idle.iter_mut().enumerate() {
            if heap.len() > limit {
                heap.retain(|i| {
                    nodes
                        .get(&i.key)
                        .is_some_and(|n| !n.in_cut && idle_kind(n) == kind && n.used >= i.used)
                });
            }
        }
    }

    /// The edits changed: classifications follow, roots cover them.
    pub fn set_edits(&mut self, grid: &Grid, edits: Arc<EditView>) {
        self.edits = edits;
        self.place_roots(grid);
    }

    pub fn apply(&mut self, event: Event, frame: u64) {
        match event {
            Event::Generated(k) => {
                // (Its layers stay until its result comes back: a chunk
                // generated again holds about what it held.)
                let n = self.node_mut(k, frame);
                n.generated = true;
                n.empty = false;
                n.stand_in = None;
                self.changed(k);
            }
            Event::Emptied(k) => {
                let n = self.node_mut(k, frame);
                n.generated = true;
                n.empty = true;
                n.occupied = None;
                n.stand_in = None;
                self.changed(k);
            }
            Event::Occupied(k, layers) => {
                if let Some(n) = self.nodes.get_mut(&k).filter(|n| n.generated) {
                    n.occupied = Some(layers);
                    self.mark_footprint(k.footprint());
                }
            }
            Event::Stale(k) => {
                if let Some(n) = self.nodes.get_mut(&k) {
                    n.generated = false;
                    n.empty = false;
                    n.occupied = None;
                    self.changed(k);
                }
            }
            Event::Edited(face, boxes) => {
                // The boxes' nodes, looked up where they are fewer than the
                // nodes known, else found by a scan.
                let volume: u64 = boxes.iter().map(NodeBox::volume).sum();
                let keys: Vec<NodeKey> = if volume < self.nodes.len() as u64 {
                    boxes
                        .iter()
                        .flat_map(|b| b.keys(self.volume, face))
                        .filter(|k| self.nodes.contains_key(k))
                        .collect()
                } else {
                    self.nodes
                        .keys()
                        .filter(|k| k.face == face && boxes.iter().any(|b| b.contains(k)))
                        .copied()
                        .collect()
                };
                for k in keys {
                    let n = self.nodes.get_mut(&k).unwrap();
                    n.edited = frame;
                    // Drawn as it was until it is generated again: air (one
                    // generated empty: its chunk dropped, known only here)
                    // or rock.
                    let was = n.leaf.filter(|_| n.in_cut);
                    if n.generated && n.empty {
                        n.generated = false;
                        n.empty = false;
                        n.stand_in = Some(Kind::Empty);
                    } else if !n.generated {
                        if let Some(kind) = was.filter(|k| *k != Kind::Mixed) {
                            n.stand_in = Some(kind);
                        }
                    }
                    self.changed(k);
                }
            }
            Event::Measured(fp, quarters) => {
                if let (Some(f), Some(ranges)) =
                    (self.footprints.get_mut(&fp), Ranges::hull(&quarters))
                {
                    if f.ranges != ranges || !f.exact {
                        f.class = None;
                    }
                    f.ranges = ranges;
                    f.quarters = quarters;
                    f.exact = true;
                    f.guess = 0;
                    self.mark_footprint(fp);
                }
            }
            Event::Seen(k) => {
                if !self.rays {
                    self.rays = true;
                    self.invalidate();
                }
                // It and its ancestors (the drawn node over it among them);
                // those rays had not met lately are visited again.
                let mut at = Some(k);
                while let Some(key) = at {
                    let before = self.seen.insert(key, frame);
                    if before == Some(frame) {
                        break;
                    }
                    if before.is_none_or(|f| f + self.config.seen_frames < frame) {
                        self.mark(key);
                    }
                    at = (key.level < self.root_level).then(|| key.parent());
                }
            }
            Event::Reached(k) => {
                self.node_mut(k, frame).reached = true;
                self.changed(k);
            }
            Event::Reports(keys) => {
                for &k in keys.iter() {
                    let key = NodeKey::from_table_key(k);
                    if key.volume == self.volume {
                        self.apply(Event::Seen(key), frame);
                    }
                }
            }
        }
    }

    /// The idle node of `heap` idle longest, if `due` holds for the frame
    /// it went idle; entries of nodes that came back to the cut, hold
    /// something else or were wanted since are skipped (the last queued
    /// again at their new idle frame).
    fn pop_idle(&mut self, heap: usize, due: impl Fn(u64) -> bool) -> Option<NodeKey> {
        loop {
            let top = *self.idle[heap].peek()?;
            if !due(top.used) {
                return None;
            }
            self.idle[heap].pop();
            let Some(n) = self.nodes.get(&top.key) else {
                continue;
            };
            if n.in_cut || idle_kind(n) != heap {
                continue;
            }
            if n.used != top.used {
                let used = n.used;
                self.idle[heap].push(Idle { used, key: top.key });
                continue;
            }
            return Some(top.key);
        }
    }

    /// Bookkeeping of nodes out of the cut dropped (those never generated
    /// after `keep_frames`, those found empty after `release_frames`, or
    /// `keep_frames` if longer), and generated nodes evicted, least
    /// recently in the cut first: those out of it for `release_frames`
    /// (`MAX_RELEASE` at most), and when chunk slots run short, `need` at
    /// least (never one in the cut in the last `IDLE_FRAMES`: it may still
    /// be fading out). Only idle nodes are looked at.
    pub fn evict(&mut self, frame: u64, need: usize) -> Vec<NodeKey> {
        let keep = self.config.keep_frames;
        let release = self.config.release_frames;
        for (heap, age) in [(IDLE_KNOWN, keep), (IDLE_EMPTY, keep.max(release))] {
            while let Some(key) = self.pop_idle(heap, |used| used + age < frame) {
                self.remove_node(key);
            }
        }
        let mut evicted = Vec::new();
        let idle = IDLE_FRAMES.max(release);
        loop {
            let count = evicted.len();
            let Some(key) = self.pop_idle(IDLE_CHUNK, |used| {
                count < need && used + IDLE_FRAMES < frame
                    || release > 0 && count < MAX_RELEASE && used + idle < frame
            }) else {
                break;
            };
            self.remove_node(key);
            evicted.push(key);
        }
        let seen_keep = self.config.seen_frames;
        if self.seen.len() > 4 * self.nodes.len().max(4096) {
            self.seen.retain(|_, f| *f + seen_keep >= frame);
        }
        evicted
    }
}

/// What happened to nodes since the worker's last plan.
#[derive(Clone, Debug)]
pub enum Event {
    /// Queued for generation: drawn from the next plan.
    Generated(NodeKey),
    /// Generated and found to hold nothing.
    Emptied(NodeKey),
    /// Its chunk came back (generated or restored) occupying these layers
    /// (level cells, [lo, hi)).
    Occupied(NodeKey, (i32, i32)),
    /// Its chunk dropped (out of the cut, or evicted): wanted again before
    /// it is drawn.
    Stale(NodeKey),
    /// A footprint's ranges by quarter (`Fp::quarters`).
    Measured(Footprint, [Option<Ranges>; 4]),
    /// A ray met it or a node under it.
    Seen(NodeKey),
    /// A frame's ray reports as read back: the table keys of the nodes rays
    /// met (any volume's, repeats among them), each of this volume's
    /// [`Event::Seen`]. Decoded here, on the planner's thread: the render
    /// thread hands the frame's words over as they came (one copy).
    Reports(Arc<[[i32; 4]]>),
    /// A ray met this Buried node: generate it.
    Reached(NodeKey),
    /// Edits changed what the nodes of `face` in these boxes hold: those
    /// generated empty (their chunks dropped, known only here) are
    /// generated again when wanted.
    Edited(u8, Arc<[NodeBox]>),
}

/// Nodes of one level: footprints `x` by `y` and layers `z` (inclusive).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeBox {
    pub level: u32,
    pub x: [i32; 2],
    pub y: [i32; 2],
    pub z: [i32; 2],
}

impl NodeBox {
    fn volume(&self) -> u64 {
        let side = |r: [i32; 2]| (i64::from(r[1]) - i64::from(r[0]) + 1).max(0) as u64;
        side(self.x)
            .saturating_mul(side(self.y))
            .saturating_mul(side(self.z))
    }
    /// Its nodes on `face` of `volume`.
    fn keys(&self, volume: u16, face: u8) -> impl Iterator<Item = NodeKey> + '_ {
        (self.x[0]..=self.x[1]).flat_map(move |cx| {
            (self.y[0]..=self.y[1]).flat_map(move |cy| {
                (self.z[0]..=self.z[1]).map(move |cz| NodeKey {
                    volume,
                    face,
                    level: self.level,
                    cx,
                    cy,
                    cz,
                })
            })
        })
    }
    pub fn contains(&self, k: &NodeKey) -> bool {
        k.level == self.level
            && (self.x[0]..=self.x[1]).contains(&k.cx)
            && (self.y[0]..=self.y[1]).contains(&k.cy)
            && (self.z[0]..=self.z[1]).contains(&k.cz)
    }
}

impl Event {
    /// Whether a plan must see it before it can be settled: all but ray
    /// reports of nodes met, which come every frame (a plan that wants
    /// nothing and switches nothing is settled whatever rays report).
    pub fn structural(&self) -> bool {
        !matches!(self, Event::Seen(_) | Event::Reports(_))
    }
}

/// A volume's plan the worker made, and the nodes it evicted.
pub struct Planned {
    /// The volume planned.
    pub volume: u16,
    pub plan: Plan,
    /// Nodes drawn now and not by the last plan sent (or drawn as
    /// something else), and the reverse.
    pub added: Vec<(NodeKey, Kind)>,
    pub removed: Vec<NodeKey>,
    pub evicted: Vec<NodeKey>,
    /// What the plan was made for.
    pub made_for: PlannedFor,
}

/// A job's plans, a volume each (the volumes the job held).
pub struct Plans {
    pub volumes: Vec<Planned>,
    /// Structural events up to this number had reached the planners, and
    /// this many frames of ray reports.
    pub events_seen: u64,
    pub reports_seen: u64,
    /// The frame whose eye they were made for, and the worker's
    /// milliseconds on them, and of those its events, plans and evictions;
    /// the nodes the plans visited.
    pub frame: u64,
    pub ms: f64,
    pub parts: [f64; 3],
    pub visits: usize,
}

/// What a volume's plan was made for: the eye and the view (in the
/// volume's frame), and the version of the edits it classified with
/// (`EditView::version`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlannedFor {
    pub eye: DVec3,
    pub view: Option<View>,
    pub edits: u64,
}

/// A volume's part of a plan: its eye moving at `velocity` and view (in
/// its frame), what happened to its nodes since its last plan, generated
/// nodes to evict for chunk slots, and its edits when they changed.
pub struct VolumeJob {
    pub volume: u16,
    pub eye: DVec3,
    pub velocity: DVec3,
    pub view: Option<View>,
    pub events: Vec<Event>,
    pub need: usize,
    pub edits: Option<Arc<EditView>>,
}

/// A volume coming or going.
enum Change {
    Add {
        volume: u16,
        grid: Grid,
        range: (i32, i32),
        config: PlanConfig,
        edits: Arc<EditView>,
    },
    Remove(u16),
}

struct Job {
    events_seen: u64,
    reports_seen: u64,
    frame: u64,
    changes: Vec<Change>,
    volumes: Vec<VolumeJob>,
}

/// A volume's planner on the worker.
struct Planning {
    grid: Grid,
    planner: Planner,
}

/// The planners of a world's volumes on their own thread: a job at a time
/// (every volume's plan), the latest plans taken without waiting.
pub struct Worker {
    jobs: std::sync::mpsc::Sender<Job>,
    results: std::sync::Mutex<std::sync::mpsc::Receiver<Plans>>,
    busy: bool,
    /// Volumes added or removed since the last job.
    changes: Vec<Change>,
    /// Structural events and frames of ray reports submitted so far.
    events_sent: u64,
    reports_sent: u64,
}

impl Worker {
    pub fn spawn() -> Self {
        let (jobs, inbox) = std::sync::mpsc::channel::<Job>();
        let (outbox, results) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("chunk residency".into())
            .spawn(move || {
                let mut planning: FxHashMap<u16, Planning> = FxHashMap::default();
                while let Ok(job) = inbox.recv() {
                    for change in job.changes {
                        match change {
                            Change::Add {
                                volume,
                                grid,
                                range,
                                config,
                                edits,
                            } => {
                                planning.insert(
                                    volume,
                                    Planning {
                                        grid,
                                        planner: Planner::new(volume, &grid, range, config, edits),
                                    },
                                );
                            }
                            Change::Remove(volume) => {
                                planning.remove(&volume);
                            }
                        }
                    }
                    let started = std::time::Instant::now();
                    let mut parts = [0.0f64; 3];
                    let mut visits = 0;
                    let mut volumes = Vec::with_capacity(job.volumes.len());
                    for v in job.volumes {
                        let Some(p) = planning.get_mut(&v.volume) else {
                            continue;
                        };
                        if let Some(edits) = v.edits {
                            p.planner.set_edits(&p.grid, edits);
                        }
                        let mut clock = std::time::Instant::now();
                        let mut lap = |part: &mut f64| {
                            *part += clock.elapsed().as_secs_f64() * 1e3;
                            clock = std::time::Instant::now();
                        };
                        for event in v.events {
                            p.planner.apply(event, job.frame);
                        }
                        lap(&mut parts[0]);
                        let mut plan = p
                            .planner
                            .plan(&p.grid, v.eye, v.velocity, v.view, job.frame);
                        lap(&mut parts[1]);
                        let evicted = p.planner.evict(job.frame, v.need);
                        lap(&mut parts[2]);
                        visits += plan.visits;
                        let added = std::mem::take(&mut plan.added);
                        let removed = std::mem::take(&mut plan.removed);
                        volumes.push(Planned {
                            volume: v.volume,
                            plan,
                            added,
                            removed,
                            evicted,
                            made_for: PlannedFor {
                                eye: v.eye,
                                view: v.view,
                                edits: p.planner.edits.version,
                            },
                        });
                    }
                    let plans = Plans {
                        volumes,
                        events_seen: job.events_seen,
                        reports_seen: job.reports_seen,
                        frame: job.frame,
                        ms: started.elapsed().as_secs_f64() * 1e3,
                        parts,
                        visits,
                    };
                    if outbox.send(plans).is_err() {
                        break;
                    }
                }
            })
            .expect("chunk residency thread");
        Self {
            jobs,
            results: std::sync::Mutex::new(results),
            busy: false,
            changes: Vec::new(),
            events_sent: 0,
            reports_sent: 0,
        }
    }

    /// Plan volume `volume` (of `grid`, its height range `range` in base
    /// layers, its edits `edits`) from the next job on.
    pub fn add_volume(
        &mut self,
        volume: u16,
        grid: Grid,
        range: (i32, i32),
        config: PlanConfig,
        edits: Arc<EditView>,
    ) {
        self.changes.push(Change::Add {
            volume,
            grid,
            range,
            config,
            edits,
        });
    }

    /// Stop planning volume `volume` (its nodes' bookkeeping dropped).
    pub fn remove_volume(&mut self, volume: u16) {
        self.changes.push(Change::Remove(volume));
    }

    /// Whether a job is under way (a submit now would be dropped: callers
    /// keep their events until it is not).
    pub fn busy(&self) -> bool {
        self.busy
    }

    /// Plan every volume of `volumes` for `frame` (`reports` frames of ray
    /// reports among their events), unless a job is under way.
    pub fn submit(&mut self, frame: u64, reports: &mut u64, volumes: Vec<VolumeJob>) {
        if self.busy {
            return;
        }
        self.events_sent += volumes
            .iter()
            .flat_map(|v| &v.events)
            .filter(|e| e.structural())
            .count() as u64;
        self.reports_sent += std::mem::take(reports);
        self.busy = self
            .jobs
            .send(Job {
                events_seen: self.events_sent,
                reports_seen: self.reports_sent,
                frame,
                changes: std::mem::take(&mut self.changes),
                volumes,
            })
            .is_ok();
    }

    /// The finished plans, if any.
    pub fn poll(&mut self) -> Option<Plans> {
        let plans = self.results.get_mut().ok()?.try_recv().ok()?;
        self.busy = false;
        Some(plans)
    }

    /// Structural events submitted so far ([`Event::structural`]).
    pub fn events_sent(&self) -> u64 {
        self.events_sent
    }

    /// Frames of ray reports submitted so far.
    pub fn reports_sent(&self) -> u64 {
        self.reports_sent
    }
}

/// Frames a node must go unvisited before eviction: longer than a fade
/// between levels (`drawn::FADE_FRAMES`) and a plan's lag.
const IDLE_FRAMES: u64 = 32;

/// Nodes released for age a plan at most (`PlanConfig::release_frames`):
/// a flight's trail goes over some frames, not in one.
const MAX_RELEASE: usize = 4096;

/// Requests a plan lists at most (more than a frame generates).
pub const MAX_REQUESTS: usize = 4096;

/// Cells this many times `lod_pixels` across are far too coarse: their
/// children and the nodes under the eye are wanted before measurements.
const DEEP_REFINE: f64 = 4.0;

/// Levels under a measured footprint whose ranges may be estimated: a
/// margin of two of its cells is eight cells three levels down.
const GUESS_LEVELS: u8 = 3;

/// The widest travel, halving from `reach`, over which `stable` holds (0
/// if none does down to a thousandth of it).
fn widest(reach: f64, stable: impl Fn(f64) -> bool) -> f64 {
    let mut s = reach;
    for _ in 0..10 {
        if stable(s) {
            return s;
        }
        s *= 0.5;
    }
    0.0
}

/// Footprints along a face edge at `level`.
pub fn tiles_at(grid: &Grid, level: u32) -> i32 {
    ((grid.cells() >> level) + CHUNK - 1) / CHUNK
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> PlanConfig {
        PlanConfig {
            pixel_angle: 7.67e-4,
            lod_pixels: 1.0,
            offscreen_pixels: 2.0,
            view_guard: 0.35,
            unseen_pixels: 1.0,
            seen_frames: 90,
            coarsen: 0.8,
            max_distance: 400_000.0,
            prefetch_seconds: 0.5,
            keep_frames: 60,
            release_frames: 0,
            motion_flow: 0.0,
            motion_max: 1.0,
            near_cells: 2.0,
        }
    }

    fn no_edits(grid: &Grid) -> Arc<EditView> {
        Arc::new(EditView {
            grid: *grid,
            faces: Default::default(),
            index: Default::default(),
            recent_index: Default::default(),
            reach: None,
            large_reach: None,
            large_faces: 0,
            large_brushes: 0,
            large_prefix: 0,
            recent_counts: Vec::new(),
            baked: Default::default(),
            bricks: Default::default(),
            version: 0,
            volume_depth: 0,
        })
    }

    /// An Earth whose ground lies between -200 and 200 m.
    fn setup() -> (Grid, DVec3, Planner) {
        let grid = Grid::new(6_371_000.0, 0.1).unwrap();
        let eye = grid.position(
            2,
            [
                f64::from(grid.cells()) * 0.47,
                f64::from(grid.cells()) * 0.53,
                20.0,
            ],
        );
        let planner = Planner::new(0, &grid, (-2000, 2000), config(), no_edits(&grid));
        (grid, eye, planner)
    }

    /// Generate every request as if the budget were unlimited, measuring
    /// each footprint at the ground band (-20..20 m).
    fn settle(
        grid: &Grid,
        eye: DVec3,
        view: Option<View>,
        planner: &mut Planner,
        frame: &mut u64,
    ) -> Plan {
        loop {
            *frame += 1;
            let plan = planner.plan(grid, eye, DVec3::ZERO, view, *frame);
            if plan.requests.is_empty() {
                return plan;
            }
            for k in plan.requests {
                planner.apply(Event::Generated(k), *frame);
                let band = ((-200 >> k.level) - 1, (200 >> k.level) + 1);
                planner.apply(
                    Event::Measured(
                        k.footprint(),
                        [Some(Ranges {
                            volume: band,
                            surface: band,
                        }); 4],
                    ),
                    *frame,
                );
            }
        }
    }

    /// The drawn nodes over a base column: one shows at every height.
    fn assert_covers_once(grid: &Grid, plan: &Plan) {
        let drawn: FxHashSet<NodeKey> = plan.drawn.iter().map(|d| d.0).collect();
        for &(k, _) in &plan.drawn {
            let mut p = k;
            while p.level + 1 < grid.levels() {
                p = p.parent();
                assert!(!drawn.contains(&p), "{k:?} under drawn {p:?}");
            }
        }
    }

    #[test]
    fn a_settled_cut_reaches_level_zero_once_over_each_point() {
        let (grid, eye, mut planner) = setup();
        let mut frame = 0;
        let plan = settle(&grid, eye, None, &mut planner, &mut frame);
        assert!(plan
            .drawn
            .iter()
            .any(|(k, kind)| k.level == 0 && *kind == Kind::Mixed));
        assert_eq!(plan.coarse_share, 0.0);
        assert_covers_once(&grid, &plan);
        // Uniform nodes are never refined: the planet's core is drawn coarse.
        assert!(plan
            .drawn
            .iter()
            .any(|(k, kind)| *kind == Kind::Solid && k.level > 10));
    }

    /// A footprint's band switches levels as one: every node of a drawn
    /// Mixed node's band is drawn at its level too (or holds nothing), so
    /// ground a coarse cell rounded into the node above or below is never
    /// left between levels.
    #[test]
    fn a_band_switches_levels_as_one() {
        let (grid, eye, mut planner) = setup();
        let mut frame = 0;
        for eye in [
            eye,
            eye + eye.normalize() * 3_000.0,
            eye + eye.normalize() * 60_000.0,
        ] {
            let plan = settle(&grid, eye, None, &mut planner, &mut frame);
            let drawn: FxHashSet<NodeKey> = plan.drawn.iter().map(|d| d.0).collect();
            for &(k, kind) in &plan.drawn {
                if kind != Kind::Mixed {
                    continue;
                }
                let band = planner.band_of(k);
                for cz in band.0..=band.1 {
                    let m = k.footprint().node(cz);
                    assert!(
                        drawn.contains(&m) || planner.nodes.get(&m).is_some_and(|n| n.empty),
                        "{k:?} drawn, {m:?} of its band {band:?} not"
                    );
                }
            }
        }
    }

    /// Height is a dimension of the cut: an eye high over the ground
    /// refines the ground under it less than an eye on it.
    #[test]
    fn nodes_refine_by_their_own_distance() {
        let (grid, eye, mut planner) = setup();
        let mut frame = 0;
        let low = settle(&grid, eye, None, &mut planner, &mut frame);
        let finest = |p: &Plan| {
            p.drawn
                .iter()
                .filter(|(_, kind)| *kind == Kind::Mixed)
                .map(|(k, _)| k.level)
                .min()
                .unwrap()
        };
        let (_, _, mut high_planner) = setup();
        let high = settle(
            &grid,
            eye + eye.normalize() * 2_000.0,
            None,
            &mut high_planner,
            &mut frame,
        );
        assert!(
            finest(&low) == 0 && finest(&high) >= 3,
            "{} {}",
            finest(&low),
            finest(&high)
        );
    }

    #[test]
    fn a_parent_stays_drawn_until_all_its_mixed_children_are_generated() {
        let (grid, eye, mut planner) = setup();
        let mut frame = 0;
        settle(&grid, eye, None, &mut planner, &mut frame);
        // A drawn level-1 Mixed node's children dropped (stale): it is
        // drawn again until they come back.
        frame += 1;
        let plan = planner.plan(&grid, eye, DVec3::ZERO, None, frame);
        let child = plan
            .drawn
            .iter()
            .find(|(k, kind)| k.level == 0 && *kind == Kind::Mixed)
            .unwrap()
            .0;
        let parent = child.parent();
        planner.apply(Event::Stale(child), frame);
        frame += 1;
        let plan = planner.plan(&grid, eye, DVec3::ZERO, None, frame);
        assert!(plan.drawn.iter().any(|d| d.0 == parent));
        assert!(plan.requests.contains(&child));
        planner.apply(Event::Generated(child), frame);
        frame += 1;
        let plan = planner.plan(&grid, eye, DVec3::ZERO, None, frame);
        assert!(!plan.drawn.iter().any(|d| d.0 == parent));
    }

    /// An edit over a node drawn as air (generated empty) wants it again
    /// without its parent drawn in its place: it stays air until its chunk
    /// comes, the cut as it was.
    #[test]
    fn an_edited_node_stands_in_as_it_was() {
        let (grid, eye, mut planner) = setup();
        let mut frame = 0;
        settle(&grid, eye, None, &mut planner, &mut frame);
        frame += 1;
        let plan = planner.plan(&grid, eye, DVec3::ZERO, None, frame);
        let child = plan
            .drawn
            .iter()
            .find(|(k, kind)| k.level == 0 && *kind == Kind::Mixed)
            .unwrap()
            .0;
        let parent = child.parent();
        planner.apply(Event::Emptied(child), frame);
        frame += 1;
        let plan = planner.plan(&grid, eye, DVec3::ZERO, None, frame);
        assert!(!plan.drawn.iter().any(|d| d.0 == child || d.0 == parent));
        let around = |v: i32| [v, v];
        planner.apply(
            Event::Edited(
                child.face,
                vec![NodeBox {
                    level: 0,
                    x: around(child.cx),
                    y: around(child.cy),
                    z: around(child.cz),
                }]
                .into(),
            ),
            frame,
        );
        frame += 1;
        let plan = planner.plan(&grid, eye, DVec3::ZERO, None, frame);
        assert!(plan.requests.contains(&child), "the edited node is wanted");
        assert!(
            !plan.drawn.iter().any(|d| d.0 == child || d.0 == parent),
            "neither it nor its parent drawn meanwhile"
        );
        planner.apply(Event::Generated(child), frame);
        frame += 1;
        let plan = planner.plan(&grid, eye, DVec3::ZERO, None, frame);
        assert!(plan.drawn.iter().any(|d| d.0 == child));
        assert!(!plan.drawn.iter().any(|d| d.0 == parent));
    }

    /// Generated nodes stay until slots run short; then the least
    /// recently visited go, never one the plan drew.
    #[test]
    fn eviction_is_by_pressure_least_recently_visited_first() {
        let (grid, eye, mut planner) = setup();
        let mut frame = 0;
        settle(&grid, eye, None, &mut planner, &mut frame);
        let far = eye + eye.normalize() * 200_000.0;
        let here = settle(&grid, far, None, &mut planner, &mut frame);
        frame += 500;
        planner.plan(&grid, far, DVec3::ZERO, None, frame);
        frame += IDLE_FRAMES + 1;
        planner.plan(&grid, far, DVec3::ZERO, None, frame);
        assert!(
            planner.evict(frame, 0).is_empty(),
            "nothing evicted without pressure"
        );
        let fine = planner
            .nodes
            .iter()
            .filter(|(k, n)| k.level == 0 && n.generated)
            .count();
        assert!(fine > 0, "the first eye's fine nodes are still resident");
        let evicted = planner.evict(frame, fine);
        assert_eq!(evicted.len(), fine);
        let drawn: FxHashSet<NodeKey> = here.drawn.iter().map(|d| d.0).collect();
        assert!(evicted.iter().all(|k| !drawn.contains(k)));
    }

    /// With `release_frames`, nodes the cut left go once that old without
    /// pressure, a plan's `MAX_RELEASE` at a time, never one the plan drew;
    /// what is left is about what a fresh planner settles with there.
    #[test]
    fn nodes_the_cut_left_are_released_by_age() {
        let (grid, eye, _) = setup();
        let release = 200;
        let mut planner = Planner::new(
            0,
            &grid,
            (-2000, 2000),
            PlanConfig {
                release_frames: release,
                ..config()
            },
            no_edits(&grid),
        );
        let resident = |p: &Planner| p.nodes.values().filter(|n| n.generated && !n.empty).count();
        let mut frame = 0;
        settle(&grid, eye, None, &mut planner, &mut frame);
        let far = eye + eye.normalize() * 200_000.0;
        let here = settle(&grid, far, None, &mut planner, &mut frame);
        let before = resident(&planner);
        frame += 1;
        planner.plan(&grid, far, DVec3::ZERO, None, frame);
        assert!(
            planner.evict(frame, 0).is_empty(),
            "nothing released before {release} frames"
        );
        frame += release;
        planner.plan(&grid, far, DVec3::ZERO, None, frame);
        let mut released = Vec::new();
        loop {
            let batch = planner.evict(frame, 0);
            assert!(batch.len() <= MAX_RELEASE);
            if batch.is_empty() {
                break;
            }
            released.extend(batch);
        }
        let drawn: FxHashSet<NodeKey> = here.drawn.iter().map(|d| d.0).collect();
        assert!(released.iter().all(|k| !drawn.contains(k)));
        assert!(
            released.iter().any(|k| k.level == 0),
            "the first eye's fine nodes released"
        );
        let (_, _, mut fresh) = setup();
        let mut fresh_frame = 0;
        settle(&grid, far, None, &mut fresh, &mut fresh_frame);
        // What is left the plan visits (a band refined in passing may stay
        // refined), within a fifth of a fresh cut's.
        assert!(planner
            .nodes
            .values()
            .all(|n| n.in_cut || !n.generated || n.empty || n.used + release >= frame));
        let (left, wanted) = (resident(&planner), resident(&fresh));
        assert!(
            left * 5 <= wanted * 6,
            "{before} resident, {left} after release, {wanted} fresh"
        );
    }

    /// In view but met by no ray: a level coarser; met: as fine as wanted.
    #[test]
    fn rays_decide_where_detail_goes() {
        let (grid, eye, _) = setup();
        let up = eye.normalize();
        let forward = (up.any_orthonormal_vector() - up * 0.3).normalize();
        let view = Some(View {
            forward,
            cos_half: 0.6f64.cos(),
        });
        let cut = |seen: bool| {
            let mut planner = Planner::new(
                0,
                &grid,
                (-2000, 2000),
                PlanConfig {
                    unseen_pixels: 2.0,
                    ..config()
                },
                no_edits(&grid),
            );
            let mut frame = 0;
            // One ray report somewhere else: rays are on, nothing here met.
            planner.apply(
                Event::Seen(NodeKey {
                    volume: 0,
                    face: 0,
                    level: 0,
                    cx: 0,
                    cy: 0,
                    cz: 0,
                }),
                0,
            );
            loop {
                frame += 1;
                if seen {
                    // Every drawn node met (a clear view).
                    let drawn: Vec<NodeKey> = planner
                        .plan(&grid, eye, DVec3::ZERO, view, frame)
                        .drawn
                        .iter()
                        .map(|d| d.0)
                        .collect();
                    for k in drawn {
                        planner.apply(Event::Seen(k), frame);
                    }
                }
                let plan = planner.plan(&grid, eye, DVec3::ZERO, view, frame);
                if plan.requests.is_empty() {
                    return plan;
                }
                for k in plan.requests {
                    planner.apply(Event::Generated(k), frame);
                    let band = ((-200 >> k.level) - 1, (200 >> k.level) + 1);
                    planner.apply(
                        Event::Measured(
                            k.footprint(),
                            [Some(Ranges {
                                volume: band,
                                surface: band,
                            }); 4],
                        ),
                        frame,
                    );
                }
            }
        };
        let (met, unmet) = (cut(true), cut(false));
        let mixed = |p: &Plan| p.drawn.iter().filter(|d| d.1 == Kind::Mixed).count();
        // In view only (out of view is a level coarser either way).
        assert!(
            mixed(&unmet) * 4 < mixed(&met) * 3,
            "{} {}",
            mixed(&unmet),
            mixed(&met)
        );
        assert_eq!(met.coarse_share, 0.0);
    }

    /// An eye 20 m over flat ground is inside the level-2 chunk over it
    /// (25.6 m tall), not at its cells: with that node drawn for want of
    /// its children, the plan's worst cell is measured to the cells its
    /// chunks occupy (about 43 pixels), not to a chunk at no distance; a
    /// band whose chunks' results are not back does not count yet.
    #[test]
    fn the_worst_cell_is_measured_to_the_cells_drawn() {
        let grid = Grid::new(6_371_000.0, 0.1).unwrap();
        let at = [
            f64::from(grid.cells()) * 0.47,
            f64::from(grid.cells()) * 0.53,
        ];
        let eye = grid.position(2, [at[0], at[1], 200.0]);
        let mut planner = Planner::new(0, &grid, (-2000, 2000), config(), no_edits(&grid));
        let mut frame = 0;
        // Ground within a metre of layer 0 everywhere.
        loop {
            frame += 1;
            let plan = planner.plan(&grid, eye, DVec3::ZERO, None, frame);
            if plan.requests.is_empty() {
                break;
            }
            for k in plan.requests {
                planner.apply(Event::Generated(k), frame);
                let band = ((-10 >> k.level) - 1, (10 >> k.level) + 1);
                planner.apply(
                    Event::Measured(
                        k.footprint(),
                        [Some(Ranges {
                            volume: band,
                            surface: band,
                        }); 4],
                    ),
                    frame,
                );
            }
        }
        // The level-2 node holding the eye, its children dropped.
        let tiles: Vec<i32> = (0..grid.levels()).map(|l| tiles_at(&grid, l)).collect();
        let held = NodeKey {
            volume: 0,
            face: 2,
            level: 2,
            cx: (at[0] as i32 >> 2) / CHUNK,
            cy: (at[1] as i32 >> 2) / CHUNK,
            cz: 0,
        };
        assert!(held.layers().1 << 2 > 200, "the eye is inside {held:?}");
        for child in held.children(tiles[1]) {
            planner.apply(Event::Stale(child), frame);
        }
        frame += 1;
        let plan = planner.plan(&grid, eye, DVec3::ZERO, None, frame);
        assert!(plan.drawn.iter().any(|d| d.0 == held), "{held:?} drawn");
        assert!(
            plan.requests.iter().any(|k| k.level == 1),
            "its children wanted"
        );
        assert_eq!(plan.worst_px, 0.0, "no cells known");
        // Its band's chunks back, the ground's top 8 m over the plain.
        planner.apply(Event::Occupied(held, (0, 20)), frame);
        planner.apply(Event::Occupied(NodeKey { cz: -1, ..held }, (-3, 0)), frame);
        frame += 1;
        let plan = planner.plan(&grid, eye, DVec3::ZERO, None, frame);
        let cells_px = grid.level_size(2) / 12.0 / config().pixel_angle;
        assert!(
            plan.worst_px > cells_px * 0.9 && plan.worst_px < cells_px * 1.1,
            "worst {} px, its cells' {cells_px:.1}",
            plan.worst_px
        );
    }

    /// A measured footprint's children are estimated from the quarter of
    /// its columns over each: a crater wall in one quarter raises that
    /// child's band only.
    #[test]
    fn children_are_estimated_from_their_quarter() {
        let (grid, _, mut planner) = setup();
        let key = NodeKey {
            volume: 0,
            face: 2,
            level: 6,
            cx: 10,
            cy: 12,
            cz: 0,
        };
        planner.footprints.insert(
            key.footprint(),
            Fp {
                ranges: Ranges {
                    volume: (0, 0),
                    surface: (0, 0),
                },
                exact: false,
                quarters: [None; 4],
                guess: 0,
                class: None,
            },
        );
        let plain = Ranges {
            volume: (-3, 2),
            surface: (-2, 2),
        };
        let wall = Ranges {
            volume: (-3, 40),
            surface: (-2, 40),
        };
        planner.apply(
            Event::Measured(
                key.footprint(),
                [Some(plain), Some(plain), Some(wall), None],
            ),
            1,
        );
        assert_eq!(
            planner.footprints[&key.footprint()].ranges,
            Ranges {
                volume: (-3, 40),
                surface: (-2, 40)
            }
        );
        planner.estimate_children(key, tiles_at(&grid, key.level - 1), &mut Vec::new());
        let child = |a: i32, b: i32| {
            planner.footprints[&Footprint {
                volume: 0,
                face: 2,
                level: 5,
                cx: 20 + a,
                cy: 24 + b,
            }]
                .ranges
        };
        assert_eq!(child(0, 0), plain.child());
        assert_eq!(child(1, 0), plain.child());
        assert_eq!(child(0, 1), wall.child());
        // A quarter that measured nothing (past a face's edge): the whole.
        assert_eq!(child(1, 1), wall.child());
        let _ = grid;
    }

    #[test]
    fn prefetch_refines_ahead_of_a_moving_eye() {
        let (grid, eye, mut planner) = setup();
        let mut frame = 0;
        settle(&grid, eye, None, &mut planner, &mut frame);
        let ahead = (grid.position(
            2,
            [
                f64::from(grid.cells()) * 0.47 + 2000.0,
                f64::from(grid.cells()) * 0.53,
                20.0,
            ],
        ) - eye)
            .normalize();
        frame += 1;
        let plan = planner.plan(&grid, eye, ahead * 400.0, None, frame);
        assert!(
            plan.requests.iter().any(|k| k.level == 0),
            "nothing requested ahead"
        );
    }

    /// Plans visiting only what may have changed draw what visiting the
    /// whole cut draws and want the same nodes, with no lateness: over a
    /// plane and an Earth, an eye cruising and climbing while the view
    /// turns, rays reporting for a while (then their reports expire), a
    /// budget of requests generated a plan.
    #[test]
    fn incremental_plans_match_full_visits() {
        let plane = Grid::plane(crate::grid::Shape::Plane, 40_000.0, 0.1).unwrap();
        let earth = Grid::new(6_371_000.0, 0.1).unwrap();
        for grid in [plane, earth] {
            let mut planner = Planner::new(
                0,
                &grid,
                (-2000, 2000),
                super::super::ResidencyConfig::default().plan,
                no_edits(&grid),
            );
            planner.lateness = 0.0;
            let face = grid.faces()[0];
            let cells = f64::from(grid.cells());
            let at = |x: f64, h: f64| grid.position(face, [cells * 0.47 + x, cells * 0.53, h]);
            let path = |t: f64| at(3000.0 * t, 400.0 + 4000.0 * t);
            let mut frame = 0;
            let mut visited = (0usize, 0usize);
            let mut truncated = 0;
            for step in 0..150 {
                let t = f64::from(step) * 0.1;
                let eye = path(t);
                let up = grid.up(eye);
                let a = (at(3000.0 * t + 1.0, 400.0) - at(3000.0 * t, 400.0)).normalize();
                let b = up.cross(a);
                let turn = 0.05 * f64::from(step);
                let view = Some(View {
                    forward: (a * turn.cos() + b * turn.sin() - up * 0.3).normalize(),
                    cos_half: 0.6f64.cos(),
                });
                let velocity = (path(t + 0.1) - eye) / 0.1;
                frame += 6;
                let mut full = planner.clone();
                full.invalidate();
                let expect = full.plan(&grid, eye, velocity, view, frame);
                let plan = planner.plan(&grid, eye, velocity, view, frame);
                visited.0 += full
                    .nodes
                    .values()
                    .filter(|n| n.visited == full.plans)
                    .count();
                visited.1 += planner
                    .nodes
                    .values()
                    .filter(|n| n.visited == planner.plans)
                    .count();
                let cut = |p: &Plan| p.drawn.iter().copied().collect::<FxHashMap<_, _>>();
                let (want, got) = (cut(&expect), cut(&plan));
                let differ: Vec<_> = want
                    .iter()
                    .filter(|(k, kind)| got.get(k) != Some(kind))
                    .map(|(k, _)| *k)
                    .chain(got.keys().filter(|k| !want.contains_key(k)).copied())
                    .collect();
                let over = if grid.is_plane() {
                    "the plane"
                } else {
                    "Earth"
                };
                assert!(
                    differ.is_empty(),
                    "plan {step} over {over}: {} drawn of {} differ, {:?}",
                    differ.len(),
                    want.len(),
                    &differ[..differ.len().min(8)]
                );
                let requests = |p: &Plan| p.requests.iter().copied().collect::<FxHashSet<_>>();
                let (rw, rg) = (requests(&expect), requests(&plan));
                assert_eq!(expect.waiting, plan.waiting, "plan {step} over {over}");
                // Past `MAX_REQUESTS`, which of equally wanted nodes make
                // the list depends on their order.
                let (rw, rg) = if plan.waiting > MAX_REQUESTS {
                    truncated += 1;
                    (FxHashSet::default(), FxHashSet::default())
                } else {
                    (rw, rg)
                };
                assert_eq!(rw, rg, "plan {step} over {over}");
                if step < 60 && step % 2 == 0 {
                    for &(k, kind) in &plan.drawn {
                        if kind == Kind::Mixed {
                            planner.apply(Event::Seen(k), frame);
                        }
                    }
                }
                for &k in plan.requests.iter().take(300) {
                    planner.apply(Event::Generated(k), frame);
                    let band = ((-200 >> k.level) - 1, (200 >> k.level) + 1);
                    planner.apply(
                        Event::Measured(
                            k.footprint(),
                            [Some(Ranges {
                                volume: band,
                                surface: band,
                            }); 4],
                        ),
                        frame,
                    );
                }
                planner.evict(frame, 0);
            }
            eprintln!(
                "{}: full visits {} nodes, incremental {}; {truncated} plans over MAX_REQUESTS",
                if grid.is_plane() { "plane" } else { "Earth" },
                visited.0,
                visited.1
            );
        }
    }

    /// The planner's CPU over an Earth cruise (40 m up at 300 m/s) and a
    /// descent (400 km to 2 m in 20 s), a plan every 6 frames, every request
    /// generated at once over ground within 20 m of layer 0, with the
    /// default residency settings: `cargo test --release --lib
    /// plan_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn plan_cost() {
        let grid = Grid::new(6_371_000.0, 0.1).unwrap();
        let at = |x: f64, h: f64| {
            grid.position(
                2,
                [
                    f64::from(grid.cells()) * 0.47 + x,
                    f64::from(grid.cells()) * 0.53,
                    h,
                ],
            )
        };
        let flights: [(&str, Box<dyn Fn(f64) -> (DVec3, DVec3)>); 2] = [
            (
                "cruise",
                Box::new(|t: f64| (at(3000.0 * t, 400.0), at(3000.0 * t + 1.0, 400.0))),
            ),
            (
                "descent",
                Box::new(|t: f64| {
                    let h = 2.0 * 200_000.0f64.powf(1.0 - t / 20.0);
                    (at(3.0 * h, h * 10.0), at(3.0 * h + 1.0, h * 10.0))
                }),
            ),
        ];
        for (name, path) in flights {
            let mut planner = Planner::new(
                0,
                &grid,
                (-2000, 2000),
                super::super::ResidencyConfig::default().plan,
                no_edits(&grid),
            );
            let mut frame = 0;
            let mut ms = Vec::new();
            let mut counts = [0usize; 4];
            let mut seconds = 0.0;
            while seconds <= 20.0 {
                let (eye, ahead) = path(seconds);
                let up = eye.normalize();
                let forward = ((ahead - eye).normalize() - up * 0.15).normalize();
                let view = Some(View {
                    forward,
                    cos_half: 0.6f64.cos(),
                });
                let velocity = (path(seconds + 0.1).0 - eye) / 0.1;
                frame += 6;
                let started = std::time::Instant::now();
                let plan = planner.plan(&grid, eye, velocity, view, frame);
                planner.evict(frame, 0);
                let took = started.elapsed().as_secs_f64() * 1e3;
                if seconds > 1.0 {
                    ms.push(took);
                    counts[0] += plan.visits;
                    counts[1] += planner.dirty.len();
                    counts[2] += plan.waiting;
                    counts[3] += planner.drawn.len();
                }
                for k in plan.requests {
                    planner.apply(Event::Generated(k), frame);
                    let band = ((-200 >> k.level) - 1, (200 >> k.level) + 1);
                    planner.apply(
                        Event::Measured(
                            k.footprint(),
                            [Some(Ranges {
                                volume: band,
                                surface: band,
                            }); 4],
                        ),
                        frame,
                    );
                }
                seconds += 0.1;
            }
            ms.sort_by(f64::total_cmp);
            let resident = planner
                .nodes
                .values()
                .filter(|n| n.generated && !n.empty)
                .count();
            eprintln!(
                "plan_cost {name}: {} plans, ms p50 {:.2} p95 {:.2} max {:.2}; nodes {} resident {resident}; a plan visits {} (pending {}), waiting {}, drawn {}",
                ms.len(),
                ms[ms.len() / 2],
                ms[ms.len() * 95 / 100],
                ms[ms.len() - 1],
                planner.nodes.len(),
                counts[0] / ms.len(),
                counts[1] / ms.len(),
                counts[2] / ms.len(),
                counts[3] / ms.len(),
            );
        }
    }

    /// A climb faster than the plan: from the ground to orbit in one plan,
    /// the cut still covers everything once.
    #[test]
    fn a_sudden_climb_leaves_no_overlap() {
        let (grid, eye, mut planner) = setup();
        let mut frame = 0;
        settle(&grid, eye, None, &mut planner, &mut frame);
        frame += 1;
        let plan = planner.plan(
            &grid,
            eye.normalize() * (grid.radius() + 400_000.0),
            DVec3::ZERO,
            None,
            frame,
        );
        assert_covers_once(&grid, &plan);
        assert!(!plan.drawn.is_empty());
    }
}
