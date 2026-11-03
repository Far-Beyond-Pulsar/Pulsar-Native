//! The world's CPU queries, off the frame.
//!
//! What a voxel world is at a cell depends on its features: a tree's trunk
//! and crown are evaluated from the placements around the cell, so a query
//! that counts them (`Matter::All`) costs tens of microseconds a cell, a
//! player's collision capsule ten milliseconds and a ray through a forest
//! tens. [`WorldQueries`] keeps those off the frame:
//!
//! * **Bricks** of [`QUERY_BRICK`]^3 base cells (each cell solid or not,
//!   with and without its features, and its material) are sampled by
//!   worker threads ([`Planet::sample_brick`]: exactly the cells' own
//!   queries, columns and features evaluated once a brick) around the
//!   points the host keeps in focus ([`WorldQueries::focus`]: a camera, the
//!   players, physics bodies). Bricks provably in open air are not sampled.
//! * **Reads** ([`WorldQueries::solid`], [`WorldQueries::material`],
//!   [`WorldQueries::raycast`]) are synchronous and exact over the resident
//!   bricks, and [`Resident::Pending`] where a brick is not resident yet
//!   (which asks for it): a host treats that as not loaded, as streaming
//!   engines treat unloaded collision.
//! * **Asks** ([`WorldQueries::ask`], [`WorldQueries::watch`]) answer what
//!   bricks do not hold (a ray past them, the ground under an aircraft)
//!   against the latest snapshot on the workers, a frame or so later.
//! * **Changes** never block a read: [`WorldQueries::edit`] (brushes) and
//!   [`WorldQueries::publish`] (a snapshot) only queue. A worker rebuilds
//!   the resident bricks a change touches aside (copies with the brushes
//!   applied, or an undo's bricks sampled again), on every core, then swaps
//!   them all in at once: a read sees the world before the change or after
//!   it, never part of it, and waits at most for the swap of the bricks'
//!   pointers. Bricks are immutable and shared (`Arc`), so a read holding
//!   one finishes on it. Changes show in order, a few milliseconds after
//!   they are made ([`WorldQueries::editing`] counts those not shown).
//!   Publishing any snapshot works out what changed from the edit
//!   histories, so hosts only hand over their planets; a snapshot holding
//!   brushes reads do not show yet shows them first, so it never shows
//!   part of an edit either.

use crate::edits::{center_half, meets, Brush, Cube, FaceBrush, Meets};
use crate::grid::{Cell, Grid, VolumeMap, VolumePoints};
use crate::planet::{Matter, Planet, RayHit, Walked};
use crate::terrain::material;
use glam::DVec3;
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};

/// Edge of a query brick, in base cells.
pub const QUERY_BRICK: i32 = 16;
const CELLS: usize = (QUERY_BRICK * QUERY_BRICK * QUERY_BRICK) as usize;
const WORDS: usize = CELLS / 64;

/// A brick's place: its face and its cell indices over [`QUERY_BRICK`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BrickKey {
    pub face: u8,
    pub i: i32,
    pub j: i32,
    pub k: i32,
}

impl BrickKey {
    pub fn of(cell: Cell) -> Self {
        Self {
            face: cell.face,
            i: cell.i.div_euclid(QUERY_BRICK),
            j: cell.j.div_euclid(QUERY_BRICK),
            k: cell.k.div_euclid(QUERY_BRICK),
        }
    }

    /// Its first cell.
    pub fn origin(&self) -> [i32; 3] {
        [
            self.i * QUERY_BRICK,
            self.j * QUERY_BRICK,
            self.k * QUERY_BRICK,
        ]
    }

    /// `cell`'s index in it (`Planet::sample_brick`'s order).
    fn index(cell: Cell) -> usize {
        let (i, j, k) = (
            cell.i.rem_euclid(QUERY_BRICK),
            cell.j.rem_euclid(QUERY_BRICK),
            cell.k.rem_euclid(QUERY_BRICK),
        );
        ((j * QUERY_BRICK + i) * QUERY_BRICK + k) as usize
    }

    /// Its middle, planet-centred.
    fn center(&self, planet: &Planet) -> DVec3 {
        let half = QUERY_BRICK / 2;
        let [i, j, k] = self.origin();
        planet
            .grid()
            .cell_center(Cell::new(self.face, i + half, j + half, k + half))
    }
}

/// A brick's cells.
pub struct Brick {
    /// Solid with the features, and without them (the ground).
    all: [u64; WORDS],
    ground: [u64; WORDS],
    /// Each cell's material as `Planet::sample_kind` gives it (0 on a solid
    /// cell: the terrain's rule), when any is not 0.
    materials: Option<Box<[u32]>>,
    /// The query tick it was last read or wanted at (eviction).
    used: AtomicU64,
}

impl Brick {
    fn air(tick: u64) -> Self {
        Self {
            all: [0; WORDS],
            ground: [0; WORDS],
            materials: None,
            used: AtomicU64::new(tick),
        }
    }

    fn sample(planet: &Planet, key: BrickKey, tick: u64) -> Self {
        // Open air: nothing within the brick's reach of its middle.
        let diagonal = f64::from(QUERY_BRICK) * planet.grid().voxel_size();
        if planet.air_clearance(key.center(planet)) > diagonal {
            return Self::air(tick);
        }
        let cells = planet.sample_brick(key.face, key.origin(), QUERY_BRICK, true);
        let mut brick = Self::air(tick);
        for (index, cell) in cells.iter().enumerate() {
            brick.set(index, cell.kind == 1, cell.ground, cell.material);
        }
        brick
    }

    fn set(&mut self, index: usize, solid: bool, ground: bool, material: u32) {
        let (word, bit) = (index / 64, 1u64 << (index % 64));
        self.all[word] = if solid {
            self.all[word] | bit
        } else {
            self.all[word] & !bit
        };
        self.ground[word] = if ground {
            self.ground[word] | bit
        } else {
            self.ground[word] & !bit
        };
        if material != 0 || self.materials.is_some() {
            self.materials
                .get_or_insert_with(|| vec![0; CELLS].into_boxed_slice())[index] = material;
        }
    }

    fn solid(&self, index: usize, matter: Matter) -> bool {
        let words = match matter {
            Matter::All => &self.all,
            Matter::Ground => &self.ground,
        };
        words[index / 64] & (1 << (index % 64)) != 0
    }

    fn raw_material(&self, index: usize) -> u32 {
        self.materials.as_ref().map_or(0, |m| m[index])
    }

    fn copy(&self) -> Self {
        Self {
            all: self.all,
            ground: self.ground,
            materials: self.materials.clone(),
            used: AtomicU64::new(self.used.load(Ordering::Relaxed)),
        }
    }

    /// `stamps` applied to its cells, as the planet applies its recent
    /// brushes: over everything else, in order. A brush is tested against
    /// the whole brick, then its eight 8^3 octants (`edits::meets`, exact
    /// where it is certain), and cell by cell only where its surface
    /// crosses: a brush costs its surface, not its volume.
    fn apply(&mut self, grid: &Grid, key: BrickKey, stamps: &Stamps) {
        let lo = key.origin();
        for fb in stamps.faces.iter().filter(|fb| fb.face() == key.face) {
            match meets(
                fb,
                Cube::brick([key.i, key.j, key.k].map(i64::from), 1),
                grid,
                &stamps.map,
            ) {
                Meets::Outside => {}
                Meets::Contains => self.fill(fb),
                Meets::Crosses => {
                    for octant in 0..8 {
                        let at = [0, 1, 2].map(|a| lo[a] + 8 * ((octant >> a) & 1));
                        match meets(
                            fb,
                            Cube::brick(at.map(|v| i64::from(v / 8)), 0),
                            grid,
                            &stamps.map,
                        ) {
                            Meets::Outside => {}
                            how => self.stamp_octant(
                                fb,
                                key.face,
                                at,
                                how == Meets::Contains,
                                &stamps.points,
                            ),
                        }
                    }
                }
            }
        }
    }

    /// Brush `fb` over every cell.
    fn fill(&mut self, fb: &FaceBrush) {
        match fb.op() {
            0 => {
                self.all = [0; WORDS];
                self.ground = [0; WORDS];
                self.materials = None;
            }
            1 => {
                self.all = [!0; WORDS];
                self.ground = [!0; WORDS];
                self.materials =
                    (fb.material() != 0).then(|| vec![fb.material(); CELLS].into_boxed_slice());
            }
            _ => (0..CELLS).for_each(|index| self.stamp(index, fb)),
        }
    }

    /// Brush `fb` over the 8^3 cells from `at`: all of them (`whole`), or
    /// those whose centres it holds.
    fn stamp_octant(
        &mut self,
        fb: &FaceBrush,
        face: u8,
        at: [i32; 3],
        whole: bool,
        points: &VolumePoints,
    ) {
        for j in at[1]..at[1] + 8 {
            for i in at[0]..at[0] + 8 {
                let mut column = None;
                for k in at[2]..at[2] + 8 {
                    let center = [center_half(i, 0), center_half(j, 0), center_half(k, 0)];
                    let held = whole
                        || fb.contains(center, || {
                            points.at(
                                *column.get_or_insert_with(|| points.column(face, i, j, 0)),
                                k,
                                0,
                            )
                        });
                    if held {
                        self.stamp(BrickKey::index(Cell::new(face, i, j, k)), fb);
                    }
                }
            }
        }
    }

    /// Brush `fb` over cell `index`: removed, added, or painted if solid.
    fn stamp(&mut self, index: usize, fb: &FaceBrush) {
        match fb.op() {
            0 => self.set(index, false, false, 0),
            1 => self.set(index, true, true, fb.material()),
            _ if self.solid(index, Matter::All) => {
                let ground = self.solid(index, Matter::Ground);
                self.set(index, true, ground, fb.material());
            }
            _ => {}
        }
    }
}

/// Brushes resolved for applying to many bricks: every brush's face
/// brushes in order, and the grid's volume maps.
struct Stamps {
    faces: Vec<FaceBrush>,
    map: VolumeMap,
    points: VolumePoints,
}

impl Stamps {
    fn new(grid: &Grid, brushes: &[Brush]) -> Self {
        Self {
            faces: brushes
                .iter()
                .flat_map(|b| b.resolve(grid).unwrap_or_default())
                .collect(),
            map: grid.volume_map(),
            points: grid.volume_points(),
        }
    }
}

/// Where brushes may reach: the bricks they can touch.
struct Reach {
    /// Each brush's centre and the farthest a touched brick's middle can be
    /// from it, and a ball holding them all.
    brushes: Vec<(DVec3, f64)>,
    all: (DVec3, f64),
}

impl Reach {
    fn new(grid: &Grid, brushes: &[Brush]) -> Self {
        let brick_m = f64::from(QUERY_BRICK) * grid.voxel_size();
        let brushes: Vec<(DVec3, f64)> = brushes
            .iter()
            .map(|brush| {
                // A cube brush reaches sqrt(3) radii (and its height) from its
                // centre; a brick sqrt(3) halves from its middle.
                let reach =
                    brush.radius.max(brush.height) * 1.7321 + brick_m * 0.8661 + grid.voxel_size();
                (DVec3::from_array(brush.center), reach)
            })
            .collect();
        let middle = brushes.iter().map(|(c, _)| *c).sum::<DVec3>() / brushes.len().max(1) as f64;
        let radius = brushes
            .iter()
            .map(|(c, r)| c.distance(middle) + r)
            .fold(-1.0, f64::max);
        Self {
            brushes,
            all: (middle, radius),
        }
    }

    fn touches(&self, planet: &Planet, key: &BrickKey) -> bool {
        if self.brushes.is_empty() {
            return false;
        }
        let c = key.center(planet);
        c.distance(self.all.0) <= self.all.1
            && self.brushes.iter().any(|(p, r)| c.distance(*p) <= *r)
    }
}

/// A read over resident bricks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Resident<T> {
    Ready(T),
    /// A brick it needs is not resident yet; it was asked for.
    Pending,
}

impl<T> Resident<T> {
    pub fn ready(self) -> Option<T> {
        match self {
            Resident::Ready(v) => Some(v),
            Resident::Pending => None,
        }
    }
}

/// A question the workers answer against the latest snapshot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Query {
    /// The first solid cell along a ray (`Planet::raycast_of`).
    Ray {
        origin: DVec3,
        direction: DVec3,
        max: f64,
        matter: Matter,
    },
    /// Height above the ground directly below (`Planet::ground_height_of`).
    Height { at: DVec3, matter: Matter },
    /// The point `clearance` over the ground under `at`
    /// (`Planet::surface_point_of`).
    Surface {
        at: DVec3,
        clearance: f64,
        matter: Matter,
    },
    /// Distance to the nearest possible solid cell (`Planet::air_clearance`).
    Clearance { at: DVec3 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Answer {
    Ray(Option<RayHit>),
    Height(f64),
    Surface(DVec3),
    Clearance(f64),
}

impl Query {
    fn answer(&self, planet: &Planet) -> Answer {
        match *self {
            Query::Ray {
                origin,
                direction,
                max,
                matter,
            } => Answer::Ray(planet.raycast_of(origin, direction, max, matter)),
            Query::Height { at, matter } => Answer::Height(planet.ground_height_of(at, matter)),
            Query::Surface {
                at,
                clearance,
                matter,
            } => Answer::Surface(planet.surface_point_of(at, clearance, matter)),
            Query::Clearance { at } => Answer::Clearance(planet.air_clearance(at)),
        }
    }
}

/// A ticket for an [`WorldQueries::ask`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Ticket(u64);

/// A region kept resident.
#[derive(Clone, Copy, Debug)]
struct Focus {
    center: DVec3,
    radius: f64,
    /// Its bricks, as of the last time they were listed (where).
    listed_at: Option<DVec3>,
}

/// A change of the world reads see, queued.
enum Change {
    /// Brushes made (`edit`), applied over it.
    Edit(Vec<Brush>),
    /// A snapshot (`publish`): its history continued, undone, or another
    /// world.
    Publish(Arc<Planet>),
}

/// Work and results shared with the workers.
struct State {
    /// The world reads see: a snapshot, and brushes shown ahead of it
    /// (`edit`), its history continued from its end. Bricks sampled from an
    /// older snapshot have these applied.
    planet: Arc<Planet>,
    pending: Vec<Brush>,
    /// Bumped when that world is replaced rather than continued (an undo,
    /// another world): bricks sampled before are dropped.
    epoch: u64,
    /// Changes not shown yet, in order; whether a worker is making one, and
    /// the bricks inserted meanwhile (it rebuilds those it touches too).
    changes: VecDeque<Change>,
    changing: bool,
    inserted: Vec<BrickKey>,
    foci: FxHashMap<u64, Focus>,
    /// Bricks the foci (or reads) want and are not resident, nearest first
    /// when taken; and those taken by a worker.
    wanted: FxHashSet<BrickKey>,
    sampling: FxHashSet<BrickKey>,
    /// Bricks a newer snapshot must re-sample (they hold edits it now
    /// includes, or changed under them).
    stale: FxHashSet<BrickKey>,
    asks: VecDeque<(Ticket, Query)>,
    answers: FxHashMap<Ticket, Answer>,
    /// Each watch's query and generation (bumped as it or the snapshot
    /// changes), the generation a worker took, and its latest answer.
    watches: FxHashMap<u64, (Query, u64)>,
    watch_taken: FxHashMap<u64, u64>,
    watched: FxHashMap<u64, (Query, Answer)>,
    next_ticket: u64,
    stop: bool,
}

impl State {
    /// The length of the edit history reads see.
    fn visible(&self) -> usize {
        self.planet.edits().len() + self.pending.len()
    }

    /// Brush `index` of the history reads see (`None`: sealed).
    fn brush(&self, index: usize) -> Option<Brush> {
        match index.checked_sub(self.planet.edits().len()) {
            Some(ahead) => self.pending.get(ahead).copied(),
            None => self.planet.edits().brush(index),
        }
    }
}

struct Shared {
    state: Mutex<State>,
    work: Condvar,
    bricks: RwLock<FxHashMap<BrickKey, Arc<Brick>>>,
    /// `State::planet`, for reads: they never wait for the workers'
    /// bookkeeping.
    planet: RwLock<Arc<Planet>>,
    tick: AtomicU64,
}

/// See the module doc.
pub struct WorldQueries {
    shared: Arc<Shared>,
    workers: Vec<std::thread::JoinHandle<()>>,
    /// Resident bricks kept at most (about 1 KiB each, 17 with materials).
    budget: usize,
}

/// How far past a focus its bricks are kept.
const KEEP_MARGIN: f64 = 1.5;

impl WorldQueries {
    /// Queries over `planet` on `threads` workers (at least one).
    pub fn new(planet: Arc<Planet>, threads: usize) -> Self {
        let shared = Arc::new(Shared {
            planet: RwLock::new(planet.clone()),
            state: Mutex::new(State {
                planet,
                pending: Vec::new(),
                epoch: 0,
                changes: VecDeque::new(),
                changing: false,
                inserted: Vec::new(),
                foci: Default::default(),
                wanted: Default::default(),
                sampling: Default::default(),
                stale: Default::default(),
                asks: VecDeque::new(),
                answers: Default::default(),
                watches: Default::default(),
                watch_taken: Default::default(),
                watched: Default::default(),
                next_ticket: 1,
                stop: false,
            }),
            work: Condvar::new(),
            bricks: RwLock::new(Default::default()),
            tick: AtomicU64::new(1),
        });
        let workers = (0..threads.max(1))
            .map(|n| {
                let shared = shared.clone();
                std::thread::Builder::new()
                    .name(format!("voxel-queries-{n}"))
                    .spawn(move || work(&shared))
                    .expect("spawn a voxel query worker")
            })
            .collect();
        Self {
            shared,
            workers,
            budget: 16_384,
        }
    }

    /// The snapshot queries are answered against.
    pub fn planet(&self) -> Arc<Planet> {
        self.shared.planet.read().expect("planet").clone()
    }

    /// Answer against `planet` from now on (queued after the changes made
    /// before, shown in a few milliseconds): bricks its edits changed from
    /// the previous snapshot's (brushes appended or undone) are rebuilt
    /// aside and swapped in at once, and sampled again from it later;
    /// another world (a new recipe, a rebuilt history) drops them all.
    pub fn publish(&self, planet: Arc<Planet>) {
        let mut state = self.shared.state.lock().expect("queries");
        let latest = state.changes.iter().rev().find_map(|change| match change {
            Change::Publish(planet) => Some(planet),
            Change::Edit(_) => None,
        });
        if Arc::ptr_eq(latest.unwrap_or(&state.planet), &planet) {
            return;
        }
        // A snapshot still waiting last in line is replaced: the newer one
        // holds what it does.
        match state.changes.back_mut() {
            Some(Change::Publish(waiting)) => *waiting = planet,
            _ => state.changes.push_back(Change::Publish(planet)),
        }
        self.shared.work.notify_one();
    }

    /// Show `brushes` (ahead of a snapshot that includes them, which then
    /// samples the bricks they touched again): queued after the changes made
    /// before, and shown together, in a few milliseconds.
    pub fn edit(&self, brushes: &[Brush]) {
        if brushes.is_empty() {
            return;
        }
        let mut state = self.shared.state.lock().expect("queries");
        state.changes.push_back(Change::Edit(brushes.to_vec()));
        self.shared.work.notify_one();
    }

    /// Changes ([`Self::edit`], [`Self::publish`]) reads do not show yet (0:
    /// they show every one made).
    pub fn editing(&self) -> usize {
        let state = self.shared.state.lock().expect("queries");
        state.changes.len() + usize::from(state.changing)
    }

    /// Keep the bricks within `radius` of `center` resident (focus `id`,
    /// replacing its last place): sampled nearest first, kept while any
    /// focus is near. Call as what it follows moves.
    pub fn focus(&self, id: u64, center: DVec3, radius: f64) {
        let tick = self.shared.tick.fetch_add(1, Ordering::Relaxed) + 1;
        let mut state = self.shared.state.lock().expect("queries");
        let brick_m = f64::from(QUERY_BRICK) * state.planet.grid().voxel_size();
        let focus = state.foci.entry(id).or_insert(Focus {
            center,
            radius,
            listed_at: None,
        });
        focus.center = center;
        focus.radius = radius;
        let moved = focus
            .listed_at
            .is_none_or(|at| at.distance(center) > brick_m * 0.5);
        if moved {
            list_focus(&mut state, id, tick);
            self.evict(&state, tick);
            self.shared.work.notify_all();
        }
    }

    /// Stop keeping focus `id`'s bricks.
    pub fn unfocus(&self, id: u64) {
        self.shared.state.lock().expect("queries").foci.remove(&id);
    }

    /// Bricks wanted and not resident yet (0: every focus is loaded).
    pub fn loading(&self) -> usize {
        let state = self.shared.state.lock().expect("queries");
        state.wanted.len() + state.sampling.len()
    }

    /// Whether every brick within `radius` of `center` is resident; those
    /// that are not are asked for. A body waits on this for the space it
    /// moves in, not a whole focus.
    pub fn resident_within(&self, center: DVec3, radius: f64) -> bool {
        let planet = self.planet();
        let keys = region(&planet, center, radius);
        let missing: Vec<BrickKey> = {
            let bricks = self.shared.bricks.read().expect("bricks");
            keys.into_iter()
                .filter(|key| !bricks.contains_key(key))
                .collect()
        };
        if missing.is_empty() {
            return true;
        }
        self.want(&missing);
        false
    }

    /// Bricks resident.
    pub fn resident(&self) -> usize {
        self.shared.bricks.read().expect("bricks").len()
    }

    /// Whether base cell `cell` is solid `matter`.
    pub fn solid(&self, cell: Cell, matter: Matter) -> Resident<bool> {
        self.read(cell, |brick, index| brick.solid(index, matter))
    }

    /// Base cell `cell`'s material (`Planet::material`: `material::AIR` for
    /// air, the terrain's own where no edit or feature set one).
    pub fn material(&self, cell: Cell) -> Resident<u32> {
        match self.read(cell, |brick, index| {
            (brick.solid(index, Matter::All), brick.raw_material(index))
        }) {
            Resident::Ready((false, _)) => Resident::Ready(material::AIR),
            Resident::Ready((true, m)) if m != 0 => Resident::Ready(m),
            Resident::Ready((true, _)) => Resident::Ready(self.planet().terrain_material(cell)),
            Resident::Pending => Resident::Pending,
        }
    }

    fn read<T>(&self, cell: Cell, f: impl FnOnce(&Brick, usize) -> T) -> Resident<T> {
        let key = BrickKey::of(cell);
        if let Some(brick) = self.shared.bricks.read().expect("bricks").get(&key) {
            brick
                .used
                .store(self.shared.tick.load(Ordering::Relaxed), Ordering::Relaxed);
            return Resident::Ready(f(brick, BrickKey::index(cell)));
        }
        self.want(&[key]);
        Resident::Pending
    }

    /// The first solid `matter` cell along a ray, exactly as
    /// `Planet::raycast_of`, over the resident bricks: `Pending` when the
    /// ray reaches one that is not resident before a hit (and it is asked
    /// for).
    pub fn raycast(
        &self,
        origin: DVec3,
        direction: DVec3,
        max: f64,
        matter: Matter,
    ) -> Resident<Option<RayHit>> {
        let planet = self.planet();
        let bricks = self.shared.bricks.read().expect("bricks");
        let tick = self.shared.tick.load(Ordering::Relaxed);
        let mut last: Option<(BrickKey, Arc<Brick>)> = None;
        let walked = planet.walk_cells(origin, direction, max, |cell| {
            let key = BrickKey::of(cell);
            if last.as_ref().is_none_or(|(k, _)| *k != key) {
                let brick = bricks.get(&key)?.clone();
                brick.used.store(tick, Ordering::Relaxed);
                last = Some((key, brick));
            }
            let (_, brick) = last.as_ref().expect("set above");
            Some(brick.solid(BrickKey::index(cell), matter))
        });
        drop(bricks);
        match walked {
            Walked::Hit(hit) => Resident::Ready(Some(hit)),
            Walked::Miss => Resident::Ready(None),
            Walked::Stopped(t) => {
                // The cell it stopped at: just past `t` along the ray.
                let at = origin + direction.normalize() * (t + planet.grid().voxel_size() * 0.5);
                self.want(&[BrickKey::of(planet.grid().locate(at).0)]);
                Resident::Pending
            }
        }
    }

    fn want(&self, keys: &[BrickKey]) {
        let mut state = self.shared.state.lock().expect("queries");
        let bricks = self.shared.bricks.read().expect("bricks");
        let mut added = false;
        for key in keys {
            if !bricks.contains_key(key) && !state.sampling.contains(key) {
                added |= state.wanted.insert(*key);
            }
        }
        drop(bricks);
        if added {
            self.shared.work.notify_one();
        }
    }

    /// Ask the workers `query` against the latest snapshot.
    pub fn ask(&self, query: Query) -> Ticket {
        let mut state = self.shared.state.lock().expect("queries");
        let ticket = Ticket(state.next_ticket);
        state.next_ticket += 1;
        state.asks.push_back((ticket, query));
        self.shared.work.notify_one();
        ticket
    }

    /// The answer to `ticket`, once (`None` while it is worked on).
    pub fn answer(&self, ticket: Ticket) -> Option<Answer> {
        self.shared
            .state
            .lock()
            .expect("queries")
            .answers
            .remove(&ticket)
    }

    /// Keep `query` answered as watch `id` (replacing its last query): the
    /// workers answer the latest one whenever it or the snapshot changes.
    pub fn watch(&self, id: u64, query: Query) {
        let mut state = self.shared.state.lock().expect("queries");
        let generation = match state.watches.get(&id) {
            Some((q, _)) if *q == query => return,
            Some((_, g)) => g + 1,
            None => 1,
        };
        state.watches.insert(id, (query, generation));
        self.shared.work.notify_one();
    }

    /// Watch `id`'s latest answer and the query it answers (`None` until
    /// the first is in).
    pub fn watched(&self, id: u64) -> Option<(Query, Answer)> {
        self.shared
            .state
            .lock()
            .expect("queries")
            .watched
            .get(&id)
            .copied()
    }

    /// Stop answering watch `id`.
    pub fn unwatch(&self, id: u64) {
        let mut state = self.shared.state.lock().expect("queries");
        state.watches.remove(&id);
        state.watch_taken.remove(&id);
        state.watched.remove(&id);
    }

    /// Drop resident bricks over budget no focus wants, least recently used
    /// first.
    fn evict(&self, state: &State, tick: u64) {
        let bricks_len = self.shared.bricks.read().expect("bricks").len();
        if bricks_len <= self.budget {
            return;
        }
        let planet = &state.planet;
        let brick_m = f64::from(QUERY_BRICK) * planet.grid().voxel_size();
        let mut bricks = self.shared.bricks.write().expect("bricks");
        let mut far: Vec<(u64, BrickKey)> = bricks
            .iter()
            .filter(|(key, _)| {
                let c = key.center(planet);
                !state
                    .foci
                    .values()
                    .any(|f| c.distance(f.center) <= f.radius * KEEP_MARGIN + brick_m)
            })
            .map(|(key, brick)| (brick.used.load(Ordering::Relaxed), *key))
            .collect();
        far.sort_unstable_by_key(|(used, _)| *used);
        let over = bricks.len().saturating_sub(self.budget);
        for (used, key) in far.into_iter().take(over) {
            if used < tick {
                bricks.remove(&key);
            }
        }
    }
}

/// [`WorldQueries`] of placed volumes (`crate::volume`), read by world
/// positions: each volume's bricks are its own world's (in its frame); a
/// point or ray is moved into each volume it can meet by its transform, and
/// the nearest hit wins. A host keeps it in step with the scene's volumes
/// ([`Self::set_volume`] every frame a volume moves or is edited).
#[derive(Default)]
pub struct VolumeQueries {
    volumes: Vec<QueriedVolume>,
    threads: usize,
}

struct QueriedVolume {
    id: u64,
    transform: crate::volume::VolumeTransform,
    bounds: Option<f64>,
    queries: WorldQueries,
}

impl VolumeQueries {
    /// Each volume's queries on `threads` workers.
    pub fn new(threads: usize) -> Self {
        Self {
            volumes: Vec::new(),
            threads,
        }
    }

    /// Volume `id` is `planet` placed by `transform` (published to its
    /// queries when it changed; added when new).
    pub fn set_volume(
        &mut self,
        id: u64,
        planet: Arc<Planet>,
        transform: crate::volume::VolumeTransform,
    ) {
        let bounds = crate::volume::bounding_radius(&planet);
        match self.volumes.iter_mut().find(|v| v.id == id) {
            Some(v) => {
                v.transform = transform;
                v.bounds = bounds;
                v.queries.publish(planet);
            }
            None => self.volumes.push(QueriedVolume {
                id,
                transform,
                bounds,
                queries: WorldQueries::new(planet, self.threads),
            }),
        }
    }

    /// Stop querying volume `id`.
    pub fn remove_volume(&mut self, id: u64) {
        self.volumes.retain(|v| v.id != id);
    }

    /// Volume `id`'s queries (in its own frame) and where it is.
    pub fn volume(&self, id: u64) -> Option<(&WorldQueries, crate::volume::VolumeTransform)> {
        self.volumes
            .iter()
            .find(|v| v.id == id)
            .map(|v| (&v.queries, v.transform))
    }

    /// Whether world point `p` lies within volume `v`'s bounds widened by
    /// `margin`.
    fn near(v: &QueriedVolume, p: DVec3, margin: f64) -> bool {
        v.bounds
            .is_none_or(|r| p.distance(v.transform.position) <= r + margin)
    }

    /// Keep the bricks within `radius` of world point `center` resident in
    /// every volume they reach (focus `id`).
    pub fn focus(&self, id: u64, center: DVec3, radius: f64) {
        for v in &self.volumes {
            if Self::near(v, center, radius) {
                v.queries.focus(id, v.transform.to_local(center), radius);
            } else {
                v.queries.unfocus(id);
            }
        }
    }

    /// Stop keeping focus `id`'s bricks.
    pub fn unfocus(&self, id: u64) {
        for v in &self.volumes {
            v.queries.unfocus(id);
        }
    }

    /// Bricks wanted and not resident yet, over every volume.
    pub fn loading(&self) -> usize {
        self.volumes.iter().map(|v| v.queries.loading()).sum()
    }

    /// Whether world point `p` is solid `matter` in any volume (`Pending`
    /// while a volume holding it has not sampled it).
    pub fn solid_at(&self, p: DVec3, matter: Matter) -> Resident<bool> {
        let mut pending = false;
        for v in self.volumes.iter().filter(|v| Self::near(v, p, 0.0)) {
            let local = v.transform.to_local(p);
            let planet = v.queries.planet();
            match v.queries.solid(planet.grid().locate(local).0, matter) {
                Resident::Ready(true) => return Resident::Ready(true),
                Resident::Ready(false) => {}
                Resident::Pending => pending = true,
            }
        }
        if pending {
            Resident::Pending
        } else {
            Resident::Ready(false)
        }
    }

    /// The first solid `matter` cell along a world ray, over every volume
    /// it can meet ([`WorldQueries::raycast`] in each one's frame): the
    /// nearest hit, `Pending` while a volume the ray enters before it has
    /// not sampled its bricks.
    pub fn raycast(
        &self,
        origin: DVec3,
        direction: DVec3,
        max: f64,
        matter: Matter,
    ) -> Resident<Option<crate::volume::VolumeHit>> {
        let dir = direction.normalize();
        // The volumes the ray meets, nearest entry first.
        let mut met: Vec<(f64, usize)> = self
            .volumes
            .iter()
            .enumerate()
            .filter_map(|(n, v)| match v.bounds {
                None => Some((0.0, n)),
                Some(r) => crate::volume::ray_ball(origin, dir, v.transform.position, r)
                    .filter(|&(enter, leave)| leave >= 0.0 && enter <= max)
                    .map(|(enter, _)| (enter.max(0.0), n)),
            })
            .collect();
        met.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut best: Option<crate::volume::VolumeHit> = None;
        for (enter, n) in met {
            let limit = best.map_or(max, |b| b.hit.distance);
            if enter > limit {
                break;
            }
            let v = &self.volumes[n];
            let t = &v.transform;
            match v
                .queries
                .raycast(t.to_local(origin), t.dir_to_local(dir), limit, matter)
            {
                Resident::Pending => return Resident::Pending,
                Resident::Ready(Some(hit)) if hit.distance <= limit => {
                    best = Some(crate::volume::VolumeHit {
                        index: n,
                        id: v.id,
                        hit,
                        normal: t.dir_to_world(hit.normal),
                    });
                }
                Resident::Ready(_) => {}
            }
        }
        Resident::Ready(best)
    }
}

impl Drop for WorldQueries {
    fn drop(&mut self) {
        self.shared.state.lock().expect("queries").stop = true;
        self.shared.work.notify_all();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

/// List focus `id`'s bricks as wanted ([`region`]; resident ones are
/// dropped when taken).
fn list_focus(state: &mut State, id: u64, _tick: u64) {
    let Some(focus) = state.foci.get_mut(&id) else {
        return;
    };
    focus.listed_at = Some(focus.center);
    let (center, radius) = (focus.center, focus.radius);
    let planet = state.planet.clone();
    state.wanted.extend(region(&planet, center, radius));
}

/// The bricks whose cells come within `radius` of `center`: those of a
/// lattice of points finer than a brick, located on the grid, so faces'
/// edges are crossed like anywhere else (none above the world).
fn region(planet: &Planet, center: DVec3, radius: f64) -> FxHashSet<BrickKey> {
    let grid = planet.grid();
    let brick_m = f64::from(QUERY_BRICK) * grid.voxel_size();
    let up = grid.up(center);
    let a = up.any_orthonormal_vector();
    let b = up.cross(a);
    let step = brick_m * 0.5;
    let n = (radius / step).ceil() as i32;
    let mut keys = FxHashSet::default();
    for x in -n..=n {
        for y in -n..=n {
            for z in -n..=n {
                let offset = (a * f64::from(x) + b * f64::from(y) + up * f64::from(z)) * step;
                if offset.length() > radius + step {
                    continue;
                }
                let p = center + offset;
                if grid.radial(p) > planet.outer_radius() + brick_m {
                    continue;
                }
                keys.insert(BrickKey::of(grid.locate(p).0));
            }
        }
    }
    keys
}

/// The edit histories' difference between `old` and `new`, as brushes:
/// those appended or undone. `None` when it is not that (another world, a
/// history rebuilt, brushes since sealed).
fn changed_brushes(old: &Planet, new: &Planet) -> Option<Vec<Brush>> {
    if old.recipe_fingerprint() != new.recipe_fingerprint() {
        return None;
    }
    let (a, b) = (old.edits(), new.edits());
    let (shorter, longer) = if b.len() >= a.len() { (a, b) } else { (b, a) };
    if longer.prefix_hash(shorter.len()) != Some(shorter.hash()) {
        return None;
    }
    (shorter.len()..longer.len())
        .map(|i| longer.brush(i))
        .collect()
}

enum Job {
    /// The next change, against the world reads see (its snapshot and the
    /// brushes shown ahead of it).
    Change(Change, Arc<Planet>, Vec<Brush>),
    Watch(u64, Query, u64, Arc<Planet>),
    Ask(Ticket, Query, Arc<Planet>),
    /// A brick sampled from a snapshot, the brushes shown ahead of it
    /// applied, for a world of an epoch.
    Brick(BrickKey, Arc<Planet>, Vec<Brush>, u64),
}

/// How a change rebuilds the bricks it touches.
enum Rebuild {
    /// Brushes applied over them.
    Apply(Stamps),
    /// Sampled again from a snapshot (an undo).
    Sample(Arc<Planet>),
}

impl Rebuild {
    fn build(&self, grid: &Grid, key: BrickKey, old: &Brick, tick: u64) -> Brick {
        match self {
            Rebuild::Apply(stamps) => {
                let mut brick = old.copy();
                brick.apply(grid, key, stamps);
                brick
            }
            Rebuild::Sample(planet) => Brick::sample(planet, key, tick),
        }
    }
}

/// Makes `change` of the world `base` and `pending` (shown ahead of it):
/// works out the bricks it rebuilds and how the world reads see changes,
/// then [`rebuild`]s them.
fn make_change(shared: &Shared, change: Change, base: Arc<Planet>, pending: Vec<Brush>) {
    let grid = *base.grid();
    match change {
        Change::Edit(brushes) => {
            let how = Rebuild::Apply(Stamps::new(&grid, &brushes));
            rebuild(
                shared,
                &base,
                &Reach::new(&grid, &brushes),
                &Reach::new(&grid, &[]),
                &how,
                |state| {
                    state.pending.extend(brushes);
                },
            );
        }
        Change::Publish(planet) => {
            let (base_len, held) = (base.edits().len(), planet.edits().len());
            let visible = base_len + pending.len();
            let changed = changed_brushes(&base, &planet);
            // Whether it holds the brushes shown ahead of `base` as far as
            // its history goes (a host may have dropped one), and the
            // brushes it holds past them.
            let continued = held >= base_len
                && (base_len..held.min(visible))
                    .all(|i| planet.edits().brush(i) == Some(pending[i - base_len]));
            let extras = changed.as_ref().filter(|_| continued).and_then(|_| {
                (visible..held)
                    .map(|i| planet.edits().brush(i))
                    .collect::<Option<Vec<Brush>>>()
            });
            match (changed, extras) {
                (Some(changed), None) if !continued => {
                    // An undo (or a history that went another way from the
                    // brushes shown ahead): the bricks it and those brushes
                    // (void now) touched are sampled from it.
                    let touched: Vec<Brush> = changed.iter().chain(&pending).copied().collect();
                    let how = Rebuild::Sample(planet.clone());
                    rebuild(
                        shared,
                        &base,
                        &Reach::new(&grid, &touched),
                        &Reach::new(&grid, &[]),
                        &how,
                        |state| {
                            state.planet = planet;
                            state.pending.clear();
                            state.epoch += 1;
                            adopted(shared, state);
                        },
                    );
                }
                (Some(changed), Some(extras)) => {
                    // Its history continued: what it holds past what reads
                    // show is applied first; the bricks its new brushes
                    // touched are sampled from it later.
                    let how = Rebuild::Apply(Stamps::new(&grid, &extras));
                    rebuild(
                        shared,
                        &base,
                        &Reach::new(&grid, &extras),
                        &Reach::new(&grid, &changed),
                        &how,
                        |state| {
                            let shown = (held - base_len).min(state.pending.len());
                            state.pending.drain(..shown);
                            state.planet = planet;
                            adopted(shared, state);
                        },
                    );
                }
                _ => {
                    // Another world: every brick goes.
                    let mut state = shared.state.lock().expect("queries");
                    shared.bricks.write().expect("bricks").clear();
                    state.stale.clear();
                    state.wanted.clear();
                    state.planet = planet;
                    state.pending.clear();
                    state.epoch += 1;
                    adopted(shared, &mut state);
                    let foci: Vec<u64> = state.foci.keys().copied().collect();
                    for id in foci {
                        list_focus(&mut state, id, 0);
                    }
                    state.changing = false;
                    shared.work.notify_all();
                }
            }
        }
    }
}

/// A snapshot became the world reads see: watches are answered again.
fn adopted(shared: &Shared, state: &mut State) {
    *shared.planet.write().expect("planet") = state.planet.clone();
    for (_, generation) in state.watches.values_mut() {
        *generation += 1;
    }
}

/// Rebuilds the resident bricks `rebuilt` reaches aside (`how`, in parallel,
/// no lock held), then swaps them all in under the locks with `commit`'s
/// change of the world reads see: a read sees the world before or after,
/// never a part. Bricks samplers inserted meanwhile (from the world before)
/// are rebuilt too, before the swap. Bricks `stale` reaches are sampled
/// again later.
fn rebuild(
    shared: &Shared,
    base: &Planet,
    rebuilt: &Reach,
    stale: &Reach,
    how: &Rebuild,
    commit: impl FnOnce(&mut State),
) {
    let tick = shared.tick.load(Ordering::Relaxed);
    // The resident keys are copied under the lock and tested without it
    // (a brick inserted meanwhile is in `State::inserted`).
    let resident: Vec<BrickKey> = shared
        .bricks
        .read()
        .expect("bricks")
        .keys()
        .copied()
        .collect();
    let touched: Vec<BrickKey> = resident
        .iter()
        .filter(|key| rebuilt.touches(base, key))
        .copied()
        .collect();
    let mut resampled: Vec<BrickKey> = resident
        .into_iter()
        .filter(|key| stale.touches(base, key))
        .collect();
    let mut todo: Vec<(BrickKey, Arc<Brick>)> = {
        let bricks = shared.bricks.read().expect("bricks");
        touched
            .into_iter()
            .filter_map(|key| Some((key, bricks.get(&key)?.clone())))
            .collect()
    };
    let mut built: FxHashMap<BrickKey, (Arc<Brick>, Arc<Brick>)> = FxHashMap::default();
    for attempt in 0.. {
        built.extend(
            todo.par_iter()
                .map(|(key, old)| {
                    (
                        *key,
                        (
                            old.clone(),
                            Arc::new(how.build(base.grid(), *key, old, tick)),
                        ),
                    )
                })
                .collect::<Vec<_>>(),
        );
        let mut state = shared.state.lock().expect("queries");
        let mut bricks = shared.bricks.write().expect("bricks");
        let mut inserted = std::mem::take(&mut state.inserted);
        inserted.sort_unstable_by_key(|k| (k.face, k.i, k.j, k.k));
        inserted.dedup();
        resampled.extend(inserted.iter().filter(|key| stale.touches(base, key)));
        todo = inserted
            .into_iter()
            .filter(|key| rebuilt.touches(base, key))
            .filter_map(|key| {
                let now = bricks.get(&key)?;
                built
                    .get(&key)
                    .is_none_or(|(old, _)| !Arc::ptr_eq(old, now))
                    .then(|| (key, now.clone()))
            })
            .collect();
        if !todo.is_empty() && attempt < 3 {
            continue;
        }
        // Still racing samplers: those bricks go, and are sampled again
        // with the change shown.
        for (key, _) in todo.drain(..) {
            bricks.remove(&key);
            state.wanted.insert(key);
        }
        for (key, (old, new)) in built.drain() {
            if bricks.get(&key).is_some_and(|now| Arc::ptr_eq(now, &old)) {
                bricks.insert(key, new);
            }
        }
        drop(bricks);
        state.stale.extend(resampled.drain(..));
        commit(&mut state);
        state.changing = false;
        shared.work.notify_all();
        return;
    }
}

fn work(shared: &Shared) {
    loop {
        let job = {
            let mut state = shared.state.lock().expect("queries");
            loop {
                if state.stop {
                    return;
                }
                if let Some(job) = take(&mut state, shared) {
                    break job;
                }
                state = shared.work.wait(state).expect("queries");
            }
        };
        match job {
            Job::Change(change, base, pending) => make_change(shared, change, base, pending),
            Job::Watch(id, query, generation, planet) => {
                let answer = query.answer(&planet);
                let mut state = shared.state.lock().expect("queries");
                if state
                    .watches
                    .get(&id)
                    .is_some_and(|(_, g)| *g == generation)
                {
                    state.watched.insert(id, (query, answer));
                }
            }
            Job::Ask(ticket, query, planet) => {
                let answer = query.answer(&planet);
                shared
                    .state
                    .lock()
                    .expect("queries")
                    .answers
                    .insert(ticket, answer);
            }
            Job::Brick(key, planet, pending, epoch) => {
                sample(shared, key, &planet, &pending, epoch)
            }
        }
    }
}

/// Samples brick `key` from `planet` with `pending` (shown ahead of it)
/// applied, and inserts it once it holds exactly what reads show: edits
/// shown while it was sampled are applied first (no lock held); a world
/// replaced meanwhile (`epoch`) drops it.
fn sample(shared: &Shared, key: BrickKey, planet: &Planet, pending: &[Brush], epoch: u64) {
    let grid = planet.grid();
    let mut brick = Brick::sample(planet, key, shared.tick.load(Ordering::Relaxed));
    if !pending.is_empty() {
        brick.apply(grid, key, &Stamps::new(grid, pending));
    }
    let mut have = planet.edits().len() + pending.len();
    loop {
        let mut state = shared.state.lock().expect("queries");
        if state.epoch != epoch {
            // Sampled from a world since replaced: wanted from the new one.
            state.sampling.remove(&key);
            state.wanted.insert(key);
            return;
        }
        let shown = state.visible();
        if shown == have {
            state.sampling.remove(&key);
            if !std::ptr::eq(&*state.planet, planet) {
                state.stale.insert(key);
            }
            if state.changing {
                state.inserted.push(key);
            }
            shared
                .bricks
                .write()
                .expect("bricks")
                .insert(key, Arc::new(brick));
            return;
        }
        let Some(newer) = (have..shown)
            .map(|i| state.brush(i))
            .collect::<Option<Vec<Brush>>>()
        else {
            // Shown brushes since sealed: sampled again.
            state.sampling.remove(&key);
            state.wanted.insert(key);
            return;
        };
        drop(state);
        brick.apply(grid, key, &Stamps::new(grid, &newer));
        have = shown;
    }
}

/// The next job: the next change, watches, then asks, then the wanted
/// brick nearest a focus, then stale bricks.
fn take(state: &mut State, shared: &Shared) -> Option<Job> {
    if !state.changing {
        if let Some(change) = state.changes.pop_front() {
            // Edits made one after another show together.
            let change = match change {
                Change::Edit(mut brushes) => {
                    while let Some(Change::Edit(more)) = state.changes.front_mut() {
                        brushes.append(more);
                        state.changes.pop_front();
                    }
                    Change::Edit(brushes)
                }
                publish => publish,
            };
            state.changing = true;
            state.inserted.clear();
            return Some(Job::Change(
                change,
                state.planet.clone(),
                state.pending.clone(),
            ));
        }
    }
    let planet = state.planet.clone();
    let due = state
        .watches
        .iter()
        .find(|(id, (_, generation))| state.watch_taken.get(id) != Some(generation))
        .map(|(id, (query, generation))| (*id, *query, *generation));
    if let Some((id, query, generation)) = due {
        // One worker takes each generation.
        state.watch_taken.insert(id, generation);
        return Some(Job::Watch(id, query, generation, planet));
    }
    if let Some((ticket, query)) = state.asks.pop_front() {
        return Some(Job::Ask(ticket, query, planet));
    }
    let resident = shared.bricks.read().expect("bricks");
    state.wanted.retain(|key| !resident.contains_key(key));
    drop(resident);
    let nearest = |keys: &FxHashSet<BrickKey>| {
        keys.iter()
            .map(|key| {
                let c = key.center(&planet);
                let d = state
                    .foci
                    .values()
                    .map(|f| c.distance(f.center))
                    .fold(f64::INFINITY, f64::min);
                (d, *key)
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, key)| key)
    };
    let key = nearest(&state.wanted).map(|k| {
        state.wanted.remove(&k);
        k
    });
    let key = key.or_else(|| {
        let k = nearest(&state.stale)?;
        state.stale.remove(&k);
        Some(k)
    })?;
    state.sampling.insert(key);
    Some(Job::Brick(key, planet, state.pending.clone(), state.epoch))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature_recipe::FeatureRecipe;
    use crate::layers::TerrainLayers;
    use crate::{BrushOp, BrushShape, PlanetRecipe};
    use std::time::{Duration, Instant};

    fn earth() -> Planet {
        Planet::new(PlanetRecipe {
            voxel_size_m: 0.1,
            terrain: TerrainLayers::earth().source(7),
            features: FeatureRecipe::earth().grow(0.1, 7),
            ..Default::default()
        })
        .unwrap()
    }

    /// A point 1.7 m over the ground under the most feature cover of a few
    /// directions: a forest.
    fn forest(planet: &Planet) -> DVec3 {
        (0..64)
            .map(|t| {
                let a = t as f64 * 0.37;
                let dir = DVec3::new(a.cos(), 0.8 + 0.1 * (a * 1.3).sin(), a.sin()).normalize();
                let eye = planet.surface_point_of(dir * planet.outer_radius(), 1.7, Matter::Ground);
                (
                    planet.ground_height_of(eye, Matter::Ground)
                        - planet.ground_height_of(eye, Matter::All),
                    eye,
                )
            })
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .unwrap()
            .1
    }

    /// Until every change shows and every brick is resident.
    fn settle(queries: &WorldQueries) {
        let started = Instant::now();
        while queries.editing() > 0 || queries.loading() > 0 {
            assert!(
                started.elapsed() < Duration::from_secs(120),
                "bricks still loading: {}",
                queries.loading()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn ready<T: Copy + std::fmt::Debug>(read: impl Fn() -> Resident<T>) -> T {
        let started = Instant::now();
        loop {
            if let Resident::Ready(v) = read() {
                return v;
            }
            assert!(
                started.elapsed() < Duration::from_secs(60),
                "never resident"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Two placed volumes, a turned Earth and a block over its ground, read
    /// by world positions: rays and solid points are the exact casts'
    /// (`Volumes`, each in its frame), the block's hits nearer than the
    /// ground's, once their bricks are resident.
    #[test]
    fn volume_reads_are_the_volumes_exact_answers() {
        use crate::volume::{VolumeTransform, Volumes, VoxelVolume};
        let earth = Arc::new(earth());
        let place = VolumeTransform::new(
            DVec3::new(-4.0e6, 9.0e6, 2.5e5),
            glam::DQuat::from_axis_angle(DVec3::new(1.0, -0.3, 0.6).normalize(), 0.9),
        );
        let ground = earth.surface_point(DVec3::new(0.3, 0.9, -0.2).normalize(), 0.0);
        let up = ground.normalize();
        let mut block = Planet::new(PlanetRecipe {
            shape: crate::grid::Shape::Plane,
            plane_size_m: 12.8,
            terrain: TerrainLayers::flat_at(1.0).source(3),
            ..Default::default()
        })
        .unwrap();
        block
            .apply(crate::Brush {
                center: [0.0, 2.0, 0.0],
                radius: 1.0,
                shape: BrushShape::Cube,
                op: BrushOp::Add,
                material: material::PLANKS,
                height: 0.0,
            })
            .unwrap();
        let block_place = VolumeTransform::new(
            place.to_world(ground + up * 6.0),
            place.rotation * glam::DQuat::from_rotation_arc(DVec3::Y, up),
        );
        let volumes = Volumes::new(vec![
            VoxelVolume::new(1, earth.clone(), place),
            VoxelVolume::new(2, Arc::new(block), block_place),
        ]);
        let mut queries = VolumeQueries::new(4);
        for v in &volumes.volumes {
            queries.set_volume(v.id, v.planet.clone(), v.transform);
        }
        let eye = place.to_world(ground + up * 14.0 + up.any_orthonormal_vector() * 9.0);
        queries.focus(1, eye, 24.0);
        let started = Instant::now();
        while queries.loading() > 0 {
            assert!(
                started.elapsed() < Duration::from_secs(120),
                "bricks still loading"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let target = place.to_world(ground + up * 2.0);
        let mut by_volume = [0; 3];
        for n in 0..64 {
            let a = f64::from(n) * 0.61;
            let aim = target
                + place.dir_to_world(up.any_orthonormal_vector() * (a.cos() * 7.0))
                + place.dir_to_world(up.cross(up.any_orthonormal_vector()) * (a.sin() * 7.0));
            let dir = (aim - eye).normalize();
            let exact = volumes.raycast(eye, dir, 22.0, Matter::All);
            let read = ready(|| queries.raycast(eye, dir, 22.0, Matter::All));
            assert_eq!(
                read.map(|h| (h.id, h.hit.cell)),
                exact.map(|h| (h.id, h.hit.cell)),
                "ray {n}"
            );
            by_volume[exact.map_or(0, |h| h.id as usize)] += 1;
            if let Some(h) = exact {
                // Just inside the hit cell: solid there.
                let inside = eye + dir * (h.hit.distance + 0.02);
                assert_eq!(
                    ready(|| queries.solid_at(inside, Matter::All)),
                    volumes.solid(inside, Matter::All)
                );
            }
        }
        assert!(by_volume[1] > 4 && by_volume[2] > 4, "{by_volume:?}");
    }

    /// Around a focus in a forest, every read is the planet's own answer:
    /// cells with and without their features, materials and rays.
    #[test]
    fn reads_over_bricks_are_the_planets_answers() {
        let planet = Arc::new(earth());
        let eye = forest(&planet);
        let queries = WorldQueries::new(planet.clone(), 4);
        queries.focus(1, eye, 6.0);
        settle(&queries);
        assert!(queries.resident() > 50, "{} bricks", queries.resident());
        let (cell, _) = planet.grid().locate(eye);
        let mut features = 0;
        for di in -20..20 {
            for dk in -30..10 {
                let c = Cell::new(cell.face, cell.i + di, cell.j + di / 2, cell.k + dk);
                let all = queries.solid(c, Matter::All).ready().expect("resident");
                assert_eq!(all, planet.solid_of(c, Matter::All), "{c:?}");
                assert_eq!(
                    queries.solid(c, Matter::Ground).ready(),
                    Some(planet.solid_of(c, Matter::Ground)),
                    "{c:?}"
                );
                assert_eq!(
                    queries.material(c).ready(),
                    Some(planet.material(c)),
                    "{c:?}"
                );
                features += u32::from(all != planet.solid_of(c, Matter::Ground));
            }
        }
        assert!(features > 0, "the forest has features");
        let up = planet.grid().up(eye);
        let side = up.any_orthonormal_vector();
        for n in 0..32 {
            let a = f64::from(n) * 0.4;
            let d = (side * a.cos() + up.cross(side) * a.sin() - up * 0.3).normalize();
            for matter in [Matter::All, Matter::Ground] {
                let read = queries
                    .raycast(eye, d, 5.0, matter)
                    .ready()
                    .expect("within the focus");
                assert_eq!(
                    read,
                    planet.raycast_of(eye, d, 5.0, matter),
                    "ray {n} {matter:?}"
                );
            }
        }
        // Past the focus a ray is pending, and asked: answered later.
        let far = eye + side * 40.0;
        assert_eq!(
            queries.solid(planet.grid().locate(far).0, Matter::All),
            Resident::Pending
        );
        let c = planet.grid().locate(far).0;
        assert_eq!(
            ready(|| queries.solid(c, Matter::All)),
            planet.solid_of(c, Matter::All)
        );
    }

    /// An edit shows once made (before its snapshot), as a whole; the
    /// snapshot that holds it re-samples its bricks to the planet's own
    /// cells.
    #[test]
    fn edits_show_before_and_settle_on_their_snapshot() {
        let mut planet = earth();
        let eye = forest(&planet);
        let queries = WorldQueries::new(Arc::new(planet.clone()), 2);
        queries.focus(1, eye, 4.0);
        settle(&queries);
        let up = planet.grid().up(eye);
        let ground = planet.surface_point_of(eye, 0.0, Matter::Ground) - up * 0.3;
        let dig = Brush {
            center: ground.to_array(),
            radius: 0.8,
            shape: BrushShape::Sphere,
            op: BrushOp::Remove,
            material: 0,
            height: 0.0,
        };
        let block = Brush {
            center: (eye + up.any_orthonormal_vector() * 1.0).to_array(),
            radius: 0.3,
            shape: BrushShape::Cube,
            op: BrushOp::Add,
            material: 9,
            height: 0.0,
        };
        let (dug, built) = (
            planet.grid().locate(ground).0,
            planet.grid().locate(DVec3::from_array(block.center)).0,
        );
        assert!(planet.solid(dug) && !planet.solid(built));
        queries.edit(&[dig]);
        queries.edit(&[block]);
        settle(&queries);
        assert_eq!(queries.solid(dug, Matter::All), Resident::Ready(false));
        assert_eq!(queries.solid(built, Matter::All), Resident::Ready(true));
        assert_eq!(queries.material(built), Resident::Ready(9));
        planet.apply(dig).unwrap();
        planet.apply(block).unwrap();
        let exact = |queries: &WorldQueries, planet: &Planet| {
            let (cell, _) = planet.grid().locate(eye);
            for di in -12..12 {
                for dk in -20..12 {
                    let c = Cell::new(cell.face, cell.i + di, cell.j, cell.k + dk);
                    assert_eq!(
                        queries.solid(c, Matter::All).ready(),
                        Some(planet.solid(c)),
                        "{c:?}"
                    );
                    assert_eq!(
                        queries.material(c).ready(),
                        Some(planet.material(c)),
                        "{c:?}"
                    );
                }
            }
        };
        // Bricks edited ahead of the snapshot are already its cells.
        exact(&queries, &planet);
        queries.publish(Arc::new(planet.clone()));
        settle(&queries);
        exact(&queries, &planet);
        // A snapshot with a brush never shown (another editor's) shows it.
        let pit = Brush {
            center: (ground - up.any_orthonormal_vector() * 1.5).to_array(),
            radius: 0.6,
            shape: BrushShape::Cube,
            op: BrushOp::Remove,
            material: 0,
            height: 0.0,
        };
        planet.apply(pit).unwrap();
        queries.publish(Arc::new(planet.clone()));
        settle(&queries);
        exact(&queries, &planet);
    }

    /// While edits and an undo are made, every read sees the world before or
    /// after each one, never part of one: a ray through a block across
    /// several bricks hits its front or nothing new, never a brick behind.
    #[test]
    fn a_change_shows_whole_or_not_at_all() {
        use std::sync::atomic::AtomicBool;
        let planet = Arc::new(earth());
        let eye = forest(&planet);
        let up = planet.grid().up(eye);
        let ahead = up.any_orthonormal_vector();
        let side = up.cross(ahead);
        // A 2.4 m block 3.5 m ahead, through the eye's height: two bricks
        // and more deep.
        let block = Brush {
            center: (eye + ahead * 3.5).to_array(),
            radius: 1.2,
            shape: BrushShape::Cube,
            op: BrushOp::Add,
            material: 9,
            height: 0.0,
        };
        let mut built = (*planet).clone();
        built.apply(block).unwrap();
        let built = Arc::new(built);
        let rays: Vec<DVec3> = (0..24)
            .map(|n| {
                let a = f64::from(n) / 24.0 - 0.5;
                (ahead + side * (a * 0.5) - up * (0.05 + 0.1 * (f64::from(n) * 0.7).sin().abs()))
                    .normalize()
            })
            .collect();
        let answers: Vec<_> = rays
            .iter()
            .map(|d| {
                (
                    planet.raycast_of(eye, *d, 6.0, Matter::All),
                    built.raycast_of(eye, *d, 6.0, Matter::All),
                )
            })
            .collect();
        assert!(
            answers.iter().filter(|(a, b)| a != b).count() > 8,
            "the block changes most rays"
        );
        let queries = Arc::new(WorldQueries::new(planet.clone(), 3));
        queries.focus(1, eye, 7.0);
        settle(&queries);
        let stop = Arc::new(AtomicBool::new(false));
        let probe = {
            let (queries, stop, rays, answers) =
                (queries.clone(), stop.clone(), rays.clone(), answers.clone());
            std::thread::spawn(move || {
                let (mut reads, mut seen_after) = (0, false);
                while !stop.load(Ordering::Relaxed) {
                    for (d, (before, after)) in rays.iter().zip(&answers) {
                        let read = queries
                            .raycast(eye, *d, 6.0, Matter::All)
                            .ready()
                            .expect("resident");
                        assert!(read == *before || read == *after, "a ray saw part of a change: {read:?}, before {before:?}, after {after:?}");
                        seen_after |= before != after && read == *after;
                        reads += 1;
                    }
                }
                (reads, seen_after)
            })
        };
        for _ in 0..3 {
            queries.edit(&[block]);
            settle(&queries);
            queries.publish(built.clone());
            settle(&queries);
            // An undo.
            queries.publish(planet.clone());
            settle(&queries);
        }
        stop.store(true, Ordering::Relaxed);
        let (reads, seen_after) = probe.join().unwrap();
        assert!(
            reads > 100 && seen_after,
            "{reads} reads, the block seen {seen_after}"
        );
        for (d, (before, _)) in rays.iter().zip(&answers) {
            assert_eq!(
                queries.raycast(eye, *d, 6.0, Matter::All),
                Resident::Ready(*before)
            );
        }
    }

    /// Asks and watches are answered off the frame, against the latest
    /// snapshot.
    #[test]
    fn asks_and_watches_are_answered() {
        let planet = Arc::new(earth());
        let eye = forest(&planet);
        let queries = WorldQueries::new(planet.clone(), 2);
        let up = planet.grid().up(eye);
        let ray = Query::Ray {
            origin: eye,
            direction: up.any_orthonormal_vector(),
            max: 100.0,
            matter: Matter::All,
        };
        let ticket = queries.ask(ray);
        let answer = ready(|| {
            queries
                .answer(ticket)
                .map_or(Resident::Pending, Resident::Ready)
        });
        assert_eq!(answer, ray.answer(&planet));
        queries.watch(
            7,
            Query::Height {
                at: eye,
                matter: Matter::Ground,
            },
        );
        let (query, height) = ready(|| {
            queries
                .watched(7)
                .map_or(Resident::Pending, Resident::Ready)
        });
        assert_eq!(
            height,
            Answer::Height(planet.ground_height_of(eye, Matter::Ground))
        );
        assert_eq!(
            query,
            Query::Height {
                at: eye,
                matter: Matter::Ground
            }
        );
        let higher = eye + up * 10.0;
        queries.watch(
            7,
            Query::Height {
                at: higher,
                matter: Matter::Ground,
            },
        );
        let height = ready(|| match queries.watched(7) {
            Some((Query::Height { at, .. }, answer)) if at == higher => Resident::Ready(answer),
            _ => Resident::Pending,
        });
        assert_eq!(
            height,
            Answer::Height(planet.ground_height_of(higher, Matter::Ground))
        );
    }
}
