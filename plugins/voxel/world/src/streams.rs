//! Streams: the planet maps' drainage refined below their texels, down to
//! creeks (`docs/planet-maps.md`, Streams).
//!
//! The maps route water between texels (25 km on Earth): every land texel
//! drains to a neighbour, its **stem** a segment from its node to the
//! downstream texel's node (the large ones are the maps' rivers). Each
//! refinement level splits every cell of the level above into 4 x 4
//! cells, each with a node (a jittered point of the cell). A cell belongs
//! to the parent-level cell whose stem passes nearest its node (among the
//! stems' first 60%), so basins follow the streams, not the grid; a
//! parent's cells drain to its stem (a priority flood over their nodes'
//! heights, from the cells on the stem), each to its steepest lower
//! neighbour or, on the stem, to a confluence just downstream on it.
//! Discharge is the wet area gathered within the parent; beds never rise
//! downstream nor sink under the stem at the confluence. Every cell's
//! segment is then a stream of its level, carved like a small river:
//! everything is a function of the maps, the seed and the terrain, so each
//! level is consistent with the one above, and the CPU and GPU carve the
//! same segments (the GPU reads them from the engine's region pool).
//!
//! A stream of level `r` reaches at most a quarter of its cell from its
//! segment (valley, walls and meanders), so a point meets only the
//! segments of its own cell and the eight around it. Levels fade out
//! where their reach spans fewer than eight of a column's cells (none
//! under four): coarse levels are band-limited samples of the same field.
use crate::landform::{smooth_min, LandformConstants, SingleRate};
use crate::noise::{hash3, mul_shr_signed, unit_q30};
use crate::planet_maps::{self, CubeGrid, MapView, PlanetMaps, RiverSpec, FLAG_RIVERS};
use glam::{DVec3, IVec3};
use rustc_hash::{FxHashMap, FxHashSet};
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::sync::{Arc, Mutex};

/// Refinement levels below the map texels at most.
pub const MAX_LEVELS: u32 = 3;
/// Cells a side of a level's cell in the next level.
pub const SPLIT: i32 = 4;
/// A stream's reach (valley, walls, meanders) as a share of its cell
/// (Q8): a point meets only its own cell's and the eight neighbours'.
pub const REACH_Q8: i32 = 64;
/// Share of a stem a cell's node is matched against (its first 60%).
const MEMBER_SPAN: f64 = 0.6;
/// Height (mm) over which a cell's neighbours are not looked at for the
/// sea (a level's relief between neighbouring nodes stays under it).
const SEA_NEAR: i32 = 150_000;
/// Steepest a stream's valley walls rise (run over rise, Q6).
const MAX_WALL_Q6: i32 = 96;

/// A cell: a map texel (`level` 0) or one of a refinement level.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Cell {
    pub level: u8,
    pub slot: u8,
    pub i: i32,
    pub j: i32,
}

impl Cell {
    /// The cell of the level above holding this one's centre (on its face).
    pub fn parent(self) -> Cell {
        Cell {
            level: self.level - 1,
            slot: self.slot,
            i: self.i.div_euclid(SPLIT),
            j: self.j.div_euclid(SPLIT),
        }
    }
}

/// What a cell's segment is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// No stream (sea, a texel without an outlet).
    None,
    /// A map texel's river or lake: the maps' records carve it.
    River,
    Stream,
}

/// A cell's segment: from its node to its target (the downstream cell's
/// node or a confluence on its parent's stem), the bed at both ends (mm),
/// the wet area it drains (km²) and its valley (domain units and mm).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stream {
    pub kind: Kind,
    pub node: IVec3,
    pub target: IVec3,
    pub bed: [i32; 2],
    pub discharge: f32,
    /// Floodplain and channel half-widths, the valley's reach (all domain
    /// units), the channel's depth and the deepest cut (mm), wall slope
    /// (rise over run, Q6).
    pub floodplain: i32,
    pub channel: i32,
    pub reach: i32,
    pub depth: i32,
    pub cap: i32,
    pub wall: i32,
}

impl Stream {
    const NONE: Stream = Stream {
        kind: Kind::None,
        node: IVec3::ZERO,
        target: IVec3::ZERO,
        bed: [0; 2],
        discharge: 0.0,
        floodplain: 0,
        channel: 0,
        reach: 0,
        depth: 0,
        cap: 0,
        wall: 0,
    };
}

/// A parent cell's flood: its member cells and their streams.
struct Flood {
    streams: FxHashMap<Cell, Stream>,
}

/// A process-wide cache, sharded by key.
struct Cache<K, V> {
    shards: Vec<Mutex<FxHashMap<K, V>>>,
    /// Entries a shard keeps (then it starts over: a flight across a
    /// planet visits endless cells).
    limit: usize,
}

impl<K: std::hash::Hash + Eq + Copy, V: Clone> Cache<K, V> {
    fn new(limit: usize) -> Self {
        Self {
            shards: (0..64).map(|_| Mutex::new(FxHashMap::default())).collect(),
            limit,
        }
    }
    fn shard(&self, key: &K) -> &Mutex<FxHashMap<K, V>> {
        use std::hash::{BuildHasher, BuildHasherDefault};
        let h = BuildHasherDefault::<rustc_hash::FxHasher>::default().hash_one(key);
        &self.shards[(h >> 58) as usize]
    }
    fn get(&self, key: &K) -> Option<V> {
        self.shard(key)
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .cloned()
    }
    fn insert(&self, key: K, value: V) {
        let mut shard = self.shard(&key).lock().unwrap_or_else(|e| e.into_inner());
        if shard.len() >= self.limit {
            shard.clear();
        }
        shard.insert(key, value);
    }
}

/// A world's streams: the maps' drainage refined `levels` times.
pub struct Streams {
    maps: Arc<PlanetMaps>,
    k: Box<LandformConstants>,
    /// The maps' header without rivers: the terrain streams are routed
    /// over (worn, not carved).
    header: [[i32; 4]; planet_maps::MAP_HEADER],
    rivers: RiverSpec,
    levels: u32,
    seed: u32,
    /// Heights per level, band-limited to half its cells.
    rates: Vec<SingleRate>,
    heights: Cache<Cell, i32>,
    owners: Cache<Cell, Option<Cell>>,
    floods: Cache<Cell, Arc<Flood>>,
    regions: Cache<[u32; 2], Arc<[u32]>>,
}

impl Streams {
    /// The streams of `maps` (baked with rivers `rivers`) over the terrain
    /// `k`, `levels` levels below the texels (at most [`MAX_LEVELS`]).
    pub fn new(
        maps: Arc<PlanetMaps>,
        k: &LandformConstants,
        rivers: &RiverSpec,
        levels: u32,
    ) -> Self {
        let g = maps.cube();
        let mut header = maps.header;
        header[0][0] &= !FLAG_RIVERS;
        let levels = levels.min(MAX_LEVELS);
        let rates = (0..=levels)
            .map(|r| {
                // Columns of about an eighth of a cell: wavelengths of half
                // a cell resolved, finer ones left out (they route nothing).
                let cell_du = g.texel_du() / f64::from(SPLIT.pow(r));
                let level =
                    (64 - ((cell_du / 2.0) as u64).max(1).leading_zeros()).saturating_sub(5);
                SingleRate::new(k, level)
            })
            .collect();
        Self {
            seed: maps.header[3][3] as u32,
            maps,
            k: Box::new(*k),
            header,
            rivers: rivers.clone(),
            levels,
            rates,
            heights: Cache::new(1 << 14),
            owners: Cache::new(1 << 14),
            floods: Cache::new(256),
            regions: Cache::new(1024),
        }
    }

    /// Cell `c`'s stream's box on its face (`in_box`): its segment's ends
    /// in the face's coordinates, grown by its reach over the cell's
    /// shortest side, a quarter more (the gnomonic scale
    /// changes over the cells around), a cell's tenth more; clamped to the
    /// cells around (a column only looks at those).
    fn record_box(&self, c: Cell, s: &Stream) -> u32 {
        if s.kind != Kind::Stream {
            return 0;
        }
        let header = &self.maps.header;
        let shift = 16 - 2 * u32::from(c.level);
        let at = |p: IVec3| {
            let (_, x, y) = planet_maps::coords_on(header, p, u32::from(c.slot));
            let unit = f64::from(1u32 << shift);
            (
                f64::from(x - (c.i << shift)) / unit * 32.0,
                f64::from(y - (c.j << shift)) / unit * 32.0,
            )
        };
        let (a, b) = (at(s.node), at(s.target));
        let side = self.cell_reach(c) * 256.0 / f64::from(REACH_Q8);
        let grow = (f64::from(s.reach) / side * 1.25 + 0.1) * 32.0;
        let byte = |v: f64| (v.clamp(-127.0, 127.0) as i32 as i8 as u8) as u32;
        byte((a.0.min(b.0) - grow).floor())
            | byte((a.0.max(b.0) + grow).ceil()) << 8
            | byte((a.1.min(b.1) - grow).floor()) << 16
            | byte((a.1.max(b.1) + grow).ceil()) << 24
    }

    /// A region's words (`TerrainRegions::region`), made once.
    fn region_words(&self, key: [u32; 2]) -> Arc<[u32]> {
        if let Some(w) = self.regions.get(&key) {
            return w;
        }
        let mut w = vec![0u32; REGION_WORDS];
        crate::terrain::TerrainRegions::region(self, key, &mut w);
        let w: Arc<[u32]> = Arc::from(w);
        self.regions.insert(key, w.clone());
        w
    }

    pub fn levels(&self) -> u32 {
        self.levels
    }

    /// The most the streams lower a height (mm: their deepest cut) and the
    /// steepest they slope (height units per domain unit, Q8: walls of at
    /// most 1.5 rise over run, their reach's easing as steep, meanders
    /// bending both by up to a factor 1.7).
    pub fn bounds(&self) -> (i32, i32) {
        let rv = &self.rivers;
        let cut = ((rv.incision_m * 3.0 + rv.depth_m) * 1000.0).ceil() as i32;
        let mm_per_du = 1000.0 * crate::grid::DOMAIN_UNIT;
        let slope = (f64::from(MAX_WALL_Q6) / 64.0 + 1.5) * 1.7 * mm_per_du * 256.0;
        (cut, slope.ceil() as i32)
    }

    fn grid(&self) -> &CubeGrid {
        self.maps.cube()
    }

    /// Cells a side of a face at level `r`.
    fn side(&self, r: u8) -> i32 {
        self.grid().n * SPLIT.pow(u32::from(r))
    }

    /// The real cell at `(i, j)` of `slot` (level `r`), which may lie past
    /// the face: on a sphere the neighbouring face's cell under its centre;
    /// past a plane's edge none.
    pub fn real(&self, r: u8, slot: u8, i: i32, j: i32) -> Option<Cell> {
        let n = self.side(r);
        if (0..n).contains(&i) && (0..n).contains(&j) {
            return Some(Cell {
                level: r,
                slot,
                i,
                j,
            });
        }
        let g = self.grid();
        if g.is_plane() {
            return None;
        }
        let per = f64::from(SPLIT.pow(u32::from(r)));
        let d = g.local(
            slot as usize,
            (f64::from(i) + 0.5) / per,
            (f64::from(j) + 0.5) / per,
        );
        let (s, x, y) = g.locate(d);
        Some(Cell {
            level: r,
            slot: s as u8,
            i: ((x * per) as i32).min(n - 1),
            j: ((y * per) as i32).min(n - 1),
        })
    }

    /// A cell's node: a point of the middle half of the cell (a texel's is
    /// the maps' river node), domain units.
    pub fn node(&self, c: Cell) -> IVec3 {
        let g = self.grid();
        if c.level == 0 {
            return planet_maps::texel_node(g, self.seed, c.slot as usize, c.i, c.j);
        }
        let salt = u32::from(c.level) << 3 | u32::from(c.slot);
        let j = |s: u32| {
            f64::from(hash3(c.i, c.j, salt as i32, self.seed ^ s) & 0xffff) / 65_536.0 * 0.5 - 0.25
        };
        let per = f64::from(SPLIT.pow(u32::from(c.level)));
        let x = (f64::from(c.i) + 0.5 + j(0x2b7e_1516)) / per;
        let y = (f64::from(c.j) + 0.5 + j(0x28ae_d2a6)) / per;
        g.point(c.slot as usize, x, y)
    }

    /// Cell length at level `r` (domain units, at face centres).
    fn cell_du(&self, r: u8) -> f64 {
        self.grid().texel_du() / f64::from(SPLIT.pow(u32::from(r)))
    }

    /// A level-`r` stream's largest reach (domain units): a quarter of a
    /// cell at face centres (gnomonic cells shrink toward the edges).
    pub fn reach(&self, r: u8) -> i32 {
        level_reach(&self.maps.header, r)
    }

    /// A cell's stream reach (domain units): a quarter of its shorter side.
    fn cell_reach(&self, c: Cell) -> f64 {
        let g = self.grid();
        let per = SPLIT.pow(u32::from(c.level));
        let texel = g.id(usize::from(c.slot), c.i / per, c.j / per);
        g.side_m(texel) / f64::from(per) * g.du_per_m() * f64::from(REACH_Q8) / 256.0
    }

    /// The worn terrain's height at a cell's node, band-limited to its
    /// level.
    fn height(&self, c: Cell) -> i32 {
        if let Some(h) = self.heights.get(&c) {
            return h;
        }
        let view = MapView {
            header: &self.header,
            tables: &self.maps.tables,
            streams: None,
        };
        let h = self.rates[usize::from(c.level)].height_in(&self.k, Some(view), self.node(c));
        self.heights.insert(c, h);
        h
    }

    /// The moisture (0..1) at a cell's node.
    fn wet(&self, c: Cell) -> f64 {
        let view = MapView {
            header: &self.maps.header,
            tables: &self.maps.tables,
            streams: None,
        };
        f64::from(planet_maps::climate(view, self.node(c), 0).1) / f64::from(planet_maps::WET)
    }

    /// A cell's stream.
    pub fn stream(&self, c: Cell) -> Stream {
        if c.level == 0 {
            return self.stem(c);
        }
        let Some(owner) = self.owner(c) else {
            return Stream::NONE;
        };
        self.flood(owner)
            .streams
            .get(&c)
            .copied()
            .unwrap_or(Stream::NONE)
    }

    /// A texel's stem: from its node to its downstream texel's.
    fn stem(&self, c: Cell) -> Stream {
        let g = self.grid();
        let id = g.id(c.slot as usize, c.i, c.j);
        let d = &self.maps.drains()[id];
        if d.flags & planet_maps::DRAINS_OCEAN as u8 != 0 {
            return Stream::NONE;
        }
        let node = self.node(c);
        let (target, bed_b) = match d.down {
            u32::MAX => (node, d.bed),
            down => {
                let (s, x, y) = g.at(down as usize);
                let t = Cell {
                    level: 0,
                    slot: s as u8,
                    i: x,
                    j: y,
                };
                (self.node(t), self.maps.drains()[down as usize].bed)
            }
        };
        let river = d.flags & (planet_maps::DRAINS_RIVER | planet_maps::DRAINS_LAKE) as u8 != 0;
        let mut s = self.geometry(
            self.cell_reach(c),
            f64::from(d.discharge),
            self.height(c),
            d.bed,
        );
        s.kind = if river { Kind::River } else { Kind::Stream };
        s.node = node;
        s.target = target;
        s.bed = [d.bed, bed_b.min(d.bed)];
        s
    }

    /// A stream's valley at level `r` for wet area `q` (km²), its node's
    /// ground `ground` and bed `bed` (mm): the rivers' laws scaled down
    /// (floodplain and channel with the square root of the discharge,
    /// incision and depth with its fourth root), within the level's reach.
    fn geometry(&self, reach: f64, q: f64, ground: i32, bed: i32) -> Stream {
        let rv = &self.rivers;
        // Streams are what drains less than a river's source.
        let ratio = (q / rv.source_km2.max(1.0)).clamp(1e-6, 1.0);
        let du = 1.0 / crate::grid::DOMAIN_UNIT;
        // Valley, meanders (a third of the floodplain and channel) and
        // walls share the reach; the floodplain and meanders at most half.
        let floodplain = (rv.floodplain_m * ratio.sqrt() * du).min(reach * 0.375);
        let channel = (floodplain * rv.channel_ratio).min(1_000.0 * du);
        let depth = rv.depth_m * ratio.sqrt().sqrt();
        let incision = rv.incision_m * ratio.sqrt().sqrt();
        let run = (reach - floodplain * 4.0 / 3.0).max(reach * 0.25);
        let rise = f64::from(ground - bed).max(0.0) / 1000.0 + incision;
        let wall =
            ((rise / (run * crate::grid::DOMAIN_UNIT * 0.5)).max(rv.wall) * 64.0).ceil() as i32;
        Stream {
            floodplain: floodplain.round() as i32,
            channel: channel.round() as i32,
            reach: reach.round() as i32,
            depth: ((depth * 1000.0).round() as i32).min(32_767),
            cap: (((incision * 3.0 + depth) * 1000.0).min(reach * crate::grid::DOMAIN_UNIT * 500.0))
                .round() as i32,
            wall: wall.clamp(1, MAX_WALL_Q6),
            discharge: q as f32,
            ..Stream::NONE
        }
    }

    /// The parent-level cell whose stem passes nearest cell `c`'s node
    /// (among the 3 x 3 around its parent; `None`: no stem near).
    fn owner(&self, c: Cell) -> Option<Cell> {
        if let Some(o) = self.owners.get(&c) {
            return o;
        }
        let p = c.parent();
        let at = self.node(c).as_dvec3();
        let mut best: Option<(f64, Cell)> = None;
        for dj in -1..=1 {
            for di in -1..=1 {
                let Some(q) = self.real(p.level, p.slot, p.i + di, p.j + dj) else {
                    continue;
                };
                let s = self.stream(q);
                if s.kind == Kind::None {
                    continue;
                }
                let a = s.node.as_dvec3();
                let b = a + (s.target.as_dvec3() - a) * MEMBER_SPAN;
                let d = distance(at, a, b);
                if best.map_or(true, |(bd, bc)| d < bd || (d == bd && q < bc)) {
                    best = Some((d, q));
                }
            }
        }
        let o = best.map(|b| b.1);
        self.owners.insert(c, o);
        o
    }

    /// Parent `p`'s flood: its member cells routed to its stem.
    fn flood(&self, p: Cell) -> Arc<Flood> {
        if let Some(f) = self.floods.get(&p) {
            return f;
        }
        let f = Arc::new(self.route(p));
        self.floods.insert(p, f.clone());
        f
    }

    fn route(&self, p: Cell) -> Flood {
        let r = p.level + 1;
        let stem = self.stream(p);
        let (a, b) = (stem.node.as_dvec3(), stem.target.as_dvec3());
        // Members' parents lie next to it (owners are among the 3 x 3).
        let span = SPLIT;
        let mut members: Vec<Cell> = Vec::new();
        let mut seen = FxHashSet::default();
        for dj in -span..SPLIT + span {
            for di in -span..SPLIT + span {
                let Some(c) = self.real(r, p.slot, p.i * SPLIT + di, p.j * SPLIT + dj) else {
                    continue;
                };
                // Cells in the sea are no stream's.
                if seen.insert(c)
                    && self.owner(c) == Some(p)
                    && (!self.maps.ocean() || self.height(c) >= 0)
                {
                    members.push(c);
                }
            }
        }
        members.sort();
        let index: FxHashMap<Cell, usize> =
            members.iter().enumerate().map(|(n, &c)| (c, n)).collect();
        let nodes: Vec<IVec3> = members.iter().map(|&c| self.node(c)).collect();
        let heights: Vec<i32> = members.iter().map(|&c| self.height(c)).collect();
        let around: Vec<Vec<usize>> = members
            .iter()
            .map(|&c| {
                let mut out = Vec::with_capacity(8);
                for (di, dj) in [
                    (-1, -1),
                    (0, -1),
                    (1, -1),
                    (-1, 0),
                    (1, 0),
                    (-1, 1),
                    (0, 1),
                    (1, 1),
                ] {
                    if let Some(&m) = self
                        .real(r, c.slot, c.i + di, c.j + dj)
                        .and_then(|n| index.get(&n))
                    {
                        if !out.contains(&m) {
                            out.push(m);
                        }
                    }
                }
                out
            })
            .collect();
        // Sea cells next to a member: it drains into the sea there.
        let sea: Vec<Option<IVec3>> = members
            .iter()
            .zip(&heights)
            .map(|(&c, &h)| {
                // Ground a node's level over the sea has none beside it.
                if !self.maps.ocean() || h > SEA_NEAR {
                    return None;
                }
                [(0, -1), (-1, 0), (1, 0), (0, 1)]
                    .iter()
                    .find_map(|&(di, dj)| {
                        let n = self.real(r, c.slot, c.i + di, c.j + dj)?;
                        (self.height(n) < 0).then(|| self.node(n))
                    })
            })
            .collect();
        let cell = self.cell_du(r);
        let near: Vec<f64> = nodes.iter().map(|n| distance(n.as_dvec3(), a, b)).collect();
        let mut seeds: Vec<usize> = (0..members.len())
            .filter(|&m| near[m] <= 0.6 * cell || sea[m].is_some())
            .collect();
        if seeds.is_empty() && !members.is_empty() {
            seeds.push(
                (0..members.len())
                    .min_by(|&x, &y| near[x].total_cmp(&near[y]).then(x.cmp(&y)))
                    .unwrap(),
            );
        }
        // Priority flood from the seeds; cells it cannot reach (cut off
        // among the members) drain to the stem themselves.
        let mut filled = vec![i32::MIN; members.len()];
        let mut is_seed = vec![false; members.len()];
        let mut heap = BinaryHeap::new();
        loop {
            for &s in &seeds {
                if filled[s] == i32::MIN {
                    filled[s] = heights[s];
                    is_seed[s] = true;
                    heap.push(Reverse((heights[s], s)));
                }
            }
            while let Some(Reverse((level, m))) = heap.pop() {
                for &n in &around[m] {
                    if filled[n] == i32::MIN {
                        filled[n] = heights[n].max(level.saturating_add(1));
                        heap.push(Reverse((filled[n], n)));
                    }
                }
            }
            seeds = (0..members.len())
                .filter(|&m| filled[m] == i32::MIN)
                .take(1)
                .collect();
            if seeds.is_empty() {
                break;
            }
        }
        // Downstream: the steepest lower neighbour; seeds to the sea or a
        // confluence on the stem a little downstream of the node's foot.
        let down: Vec<Option<usize>> = (0..members.len())
            .map(|m| {
                if is_seed[m] {
                    return None;
                }
                let mut best: Option<(f64, usize)> = None;
                for &n in &around[m] {
                    if filled[n] < filled[m] {
                        let slope = f64::from(filled[m] - filled[n])
                            / (nodes[m] - nodes[n]).as_dvec3().length().max(1.0);
                        if best.map_or(true, |(s, b)| slope > s || (slope == s && n < b)) {
                            best = Some((slope, n));
                        }
                    }
                }
                best.map(|b| b.1)
            })
            .collect();
        let mut order: Vec<usize> = (0..members.len()).collect();
        order.sort_by_key(|&m| Reverse((filled[m], m)));
        let per = SPLIT.pow(u32::from(r));
        let mut q: Vec<f64> = members
            .iter()
            .map(|&c| {
                let texel = self.grid().id(usize::from(c.slot), c.i / per, c.j / per);
                self.grid().area_km2(texel) / f64::from(per * per) * self.wet(c)
            })
            .collect();
        for &m in &order {
            if let Some(d) = down[m] {
                q[d] += q[m];
            }
        }
        // Beds, from the stem up.
        let mut streams = FxHashMap::default();
        let mut bed = vec![0i32; members.len()];
        for &m in order.iter().rev() {
            let (target, below) = match (down[m], sea[m]) {
                (Some(d), _) => (nodes[d], bed[d]),
                (None, Some(s)) => (s, 0),
                (None, None) => {
                    let (foot, t) = closest(nodes[m].as_dvec3(), a, b);
                    let length = (b - a).length();
                    let pull = if length > 0.0 {
                        (near[m] * 0.3 / length).min(1.0 - t)
                    } else {
                        0.0
                    };
                    let t = t + pull;
                    let foot = foot + (b - a) * pull;
                    let lerp = f64::from(stem.bed[0]) + f64::from(stem.bed[1] - stem.bed[0]) * t;
                    (
                        IVec3::new(
                            foot.x.round() as i32,
                            foot.y.round() as i32,
                            foot.z.round() as i32,
                        ),
                        lerp.round() as i32,
                    )
                }
            };
            let rv = &self.rivers;
            let incision = rv.incision_m * (q[m] / rv.source_km2.max(1.0)).max(1e-6).sqrt().sqrt();
            bed[m] = below.max(heights[m] - (incision * 1000.0).round() as i32);
            let mut s = self.geometry(self.cell_reach(members[m]), q[m], heights[m], bed[m]);
            s.kind = Kind::Stream;
            s.node = nodes[m];
            s.target = target;
            s.bed = [bed[m], below];
            streams.insert(members[m], s);
        }
        Flood { streams }
    }

    /// The streams' carve of column height `h` (mm) at domain point `p`
    /// (reference level `level`): every resolved level's segments around
    /// `p`, each lowering the column toward its valley within its reach,
    /// the lowest kept; and whether `p` lies in a channel whose bed is its
    /// ground.
    pub fn carve(&self, m: MapView, p: IVec3, level: u32, h: i32) -> (i32, bool) {
        let mut out = h;
        let mut inside = false;
        let cell = 8i32 << level.min(24);
        // The meanders' noise, only where a channel may be.
        let mut bent: Option<[IVec3; 3]> = None;
        let mut last: Option<([u32; 2], Arc<[u32]>)> = None;
        for r in 0..=self.levels as u8 {
            if resolved(self.reach(r), cell) == 0 {
                continue;
            }
            let shift = 16 - 2 * u32::from(r);
            for (slot, xq, yq) in cells_around(m.header, p, r) {
                let (i, j) = (xq >> shift, yq >> shift);
                for dj in -1..=1 {
                    for di in -1..=1 {
                        let n = self.side(r);
                        let (ci, cj) = (i + di, j + dj);
                        if !(0..n).contains(&ci) || !(0..n).contains(&cj) {
                            continue;
                        }
                        // The record as the GPU reads it: from its region.
                        let (key, record) = if r == 0 {
                            (region_key(0, slot, ci, cj), 16)
                        } else {
                            (
                                region_key(r - 1, slot, ci >> 2, cj >> 2),
                                ((ci & 3) + 4 * (cj & 3)) as usize,
                            )
                        };
                        if last.as_ref().map_or(true, |l| l.0 != key) {
                            last = Some((key, self.region_words(key)));
                        }
                        let words = &last.as_ref().unwrap().1;
                        let w = &words[record * RECORD_WORDS..][..RECORD_WORDS];
                        // The record's box first: one word, most cells' last.
                        if w[10] == 0
                            || !in_box(w[12], xq - (ci << shift), yq - (cj << shift), shift)
                        {
                            continue;
                        }
                        let s = unpack(w);
                        if !near_box(&s, p) {
                            continue;
                        }
                        let at = surface(&s, p, &mut bent, self.seed, cell);
                        if at.dist >= s.reach {
                            continue;
                        }
                        let carved = smooth_min(h, at.z, s.cap / 8);
                        let cut = (h - carved).min(s.cap);
                        let w =
                            mul_shr_signed(weight(at.dist, s.reach), resolved(s.reach, cell), 8);
                        let lowered = h - mul_shr_signed(cut, w, 16);
                        if lowered < out {
                            out = lowered;
                        }
                        inside |= at.inside && at.z <= out;
                    }
                }
            }
        }
        (out, inside)
    }
}

/// How much of a level shows at cells `cell` domain units wide (Q8): all
/// where its reach spans eight cells, none under four (farther, a
/// stream's valley is under a few pixels, and every region out to the
/// horizon would be needed).
fn resolved(reach: i32, cell: i32) -> i32 {
    (((reach.min(1 << 23)) << 6) / cell.max(1) - 256).clamp(0, 256)
}

/// `a / b` in Q16 for `0 <= a <= b < 2^23`, exactly in 32 bits (two
/// division steps; `stream_ratio` in WGSL).
fn ratio_q16(a: i32, b: i32) -> i32 {
    let n = a << 8;
    ((n / b) << 8) + ((n % b) << 8) / b
}

/// The carve's weight (Q16) at distance `d` from a stream of reach
/// `reach`: whole over its first half, easing to none at its end.
fn weight(d: i32, reach: i32) -> i32 {
    let half = reach / 2;
    if d <= half {
        return 65_536;
    }
    let x = ratio_q16((d - half).min(reach - half), (reach - half).max(1));
    // 1 - smoothstep.
    let s = mul_shr_signed(mul_shr_signed(x, x, 16), 3 * 65_536 - 2 * x, 16);
    65_536 - s
}

/// A stream at a point: its surface (bed, walls and channel as a
/// river's), whether the point lies in the channel, its distance to the
/// axis.
struct At {
    z: i32,
    inside: bool,
    dist: i32,
}

/// A stream at `p`. Its valley runs straight (a stream's reach is short
/// next to the valleys' swing); its channel meanders on its floodplain as
/// a river's, the noise (`bent`, the bend fields at `p` with `seed`) made
/// only where a channel may be.
fn surface(s: &Stream, p: IVec3, bent: &mut Option<[IVec3; 3]>, seed: u32, cell: i32) -> At {
    let span = s.target - s.node;
    let e = unit_q30(span);
    let length = mul_shr_signed(span.x, e.x, 30)
        + mul_shr_signed(span.y, e.y, 30)
        + mul_shr_signed(span.z, e.z, 30);
    let m = planet_maps::meander_amplitudes(s.floodplain, s.channel);
    let (dist, a) = planet_maps::axis(p, s.node, e, length);
    let t = if length > 0 {
        ratio_q16(a, length.max(a))
    } else {
        0
    };
    let bed = s.bed[0] - mul_shr_signed(s.bed[0] - s.bed[1], t, 16);
    let mut z = bed + planet_maps::wall_rise((dist - s.floodplain).max(0), s.floodplain, s.wall);
    let mut inside = false;
    let flow = if dist < s.floodplain && s.channel > 0 {
        let b = *bent.get_or_insert_with(|| planet_maps::bends(p, seed));
        let channel = planet_maps::swing(planet_maps::swing(p, b[1], m[1]), b[2], m[2]);
        planet_maps::axis(channel, s.node, e, length).0
    } else {
        i32::MAX
    };
    if flow < s.channel {
        let q = (flow << 14) / s.channel.max(1);
        let cut = s.depth - mul_shr_signed(s.depth, (q * q) >> 14, 14);
        let resolved = (((s.channel << 7) / cell.max(1)) - 256).clamp(0, 256);
        z -= (cut * resolved) >> 8;
        inside = resolved > 0;
    }
    At { z, inside, dist }
}

/// Whether `p` lies within a stream's box: its segment's bounds grown by
/// its reach (outside, the stream cannot touch `p`; `stream_near_box` in
/// WGSL).
fn near_box(s: &Stream, p: IVec3) -> bool {
    let grow = s.reach;
    let lo = s.node.min(s.target) - IVec3::splat(grow);
    let hi = s.node.max(s.target) + IVec3::splat(grow);
    p.cmpge(lo).all() && p.cmple(hi).all()
}

/// A level-`r` stream's reach (domain units): a quarter of its cell at
/// face centres (`maps` row 4's last word holds a texel's).
pub fn level_reach(header: &[[i32; 4]; planet_maps::MAP_HEADER], r: u8) -> i32 {
    header[4][3] >> (2 * u32::from(r))
}

/// Domain point `p` on its face and on the faces it lies within a
/// level-`r` cell of (near edges and corners): slot and texel coordinates
/// (Q16) there.
fn cells_around(
    header: &[[i32; 4]; planet_maps::MAP_HEADER],
    p: IVec3,
    r: u8,
) -> Vec<(u8, i32, i32)> {
    let shift = 16 - 2 * u32::from(r);
    let n = header[0][1] << (2 * u32::from(r));
    let (slot, xq, yq) = planet_maps::coords(header, p);
    let mut out = vec![(slot as u8, xq, yq)];
    if header[2][0] > 0 {
        return out;
    }
    let margin = 1 << shift;
    let edge = |q: i32| {
        if q < margin {
            -1
        } else if q >= (n << shift) - margin {
            1
        } else {
            0
        }
    };
    let [_, a, b] = crate::grid::FACE_BASIS[slot as usize].map(IVec3::from_array);
    for (side, axis) in [(edge(xq), a), (edge(yq), b)] {
        if side != 0 {
            let face = planet_maps::face_of_normal(axis * side);
            let (_, x, y) = planet_maps::coords_on(header, p, face);
            out.push((face as u8, x, y));
        }
    }
    out
}

/// Words of a stream record in a region (`map_stream` in WGSL): node,
/// target, beds, floodplain, channel | depth << 17, reach (0: none), cap |
/// wall << 24, and its box on its face (`in_box`).
pub const RECORD_WORDS: usize = 13;
/// Records of a region: a parent cell's 16 cells, then (texels) its stem.
pub const REGION_RECORDS: usize = 17;
pub const REGION_WORDS: usize = 224;

/// Whether a point `(du, dv)` (Q16 texel units) from a level's cell
/// (`shift`: 16 less twice the level) lies in a record's box: signed bytes
/// of thirty-seconds of a cell from the cell's corner, u min and max, v min
/// and max (`stream_in_box` in WGSL).
fn in_box(b: u32, du: i32, dv: i32, shift: u32) -> bool {
    let (u, v) = ((du << 5) >> shift, (dv << 5) >> shift);
    let at = |k: u32| i32::from((b >> (8 * k)) as u8 as i8);
    u >= at(0) && u <= at(1) && v >= at(2) && v <= at(3)
}

/// A region's key: its parent cell (`map_stream_key` in WGSL).
pub fn region_key(level: u8, slot: u8, i: i32, j: i32) -> [u32; 2] {
    [
        u32::from(level) | u32::from(slot) << 2 | (i as u32) << 5,
        j as u32,
    ]
}

fn pack(s: &Stream, b: u32, out: &mut [u32]) {
    if s.kind != Kind::Stream {
        out.fill(0);
        return;
    }
    out[12] = b;
    out[0..3].copy_from_slice(&s.node.to_array().map(|v| v as u32));
    out[3..6].copy_from_slice(&s.target.to_array().map(|v| v as u32));
    out[6] = s.bed[0] as u32;
    out[7] = s.bed[1] as u32;
    out[8] = s.floodplain as u32;
    out[9] = s.channel as u32 | (s.depth as u32) << 17;
    out[10] = s.reach as u32;
    out[11] = s.cap as u32 | (s.wall as u32) << 24;
}

/// A record as the carve reads it (`kind` Stream when its reach is not 0).
fn unpack(w: &[u32]) -> Stream {
    Stream {
        kind: if w[10] == 0 { Kind::None } else { Kind::Stream },
        node: IVec3::new(w[0] as i32, w[1] as i32, w[2] as i32),
        target: IVec3::new(w[3] as i32, w[4] as i32, w[5] as i32),
        bed: [w[6] as i32, w[7] as i32],
        discharge: 0.0,
        floodplain: w[8] as i32,
        channel: (w[9] & 0x1_ffff) as i32,
        reach: w[10] as i32,
        depth: (w[9] >> 17) as i32,
        cap: (w[11] & 0xff_ffff) as i32,
        wall: (w[11] >> 24) as i32,
    }
}

impl crate::terrain::TerrainRegions for Streams {
    fn words(&self) -> usize {
        REGION_WORDS
    }

    /// The baked maps (one per world and process) and the levels.
    fn identity(&self) -> u64 {
        (Arc::as_ptr(&self.maps) as u64) ^ u64::from(self.levels) << 56
    }

    fn keys(
        &self,
        grid: &crate::grid::Grid,
        face: u8,
        level: u32,
        i: [i32; 2],
        j: [i32; 2],
        out: &mut Vec<[u32; 2]>,
    ) {
        let header = &self.maps.header;
        let cell = 8i32 << (level + grid.level_offset()).min(24);
        let corners = [
            (i[0], j[0]),
            (i[1], j[0]),
            (i[0], j[1]),
            (i[1], j[1]),
            ((i[0] + i[1]) / 2, (j[0] + j[1]) / 2),
        ]
        .map(|(a, b)| grid.domain_point(face, a, b, level));
        let start = out.len();
        for r in 0..=self.levels as u8 {
            if resolved(self.reach(r), cell) == 0 {
                continue;
            }
            // The cells around the footprint, two over (halos, features'
            // ground reads), per face it meets.
            let mut boxes: Vec<(u8, [i32; 4])> = Vec::new();
            let shift = 16 - 2 * u32::from(r);
            for &p in &corners {
                for (slot, xq, yq) in cells_around(header, p, r) {
                    let (ci, cj) = (xq >> shift, yq >> shift);
                    match boxes.iter_mut().find(|b| b.0 == slot) {
                        Some(b) => {
                            b.1 = [
                                b.1[0].min(ci),
                                b.1[1].min(cj),
                                b.1[2].max(ci),
                                b.1[3].max(cj),
                            ]
                        }
                        None => boxes.push((slot, [ci, cj, ci, cj])),
                    }
                }
            }
            let n = self.side(r);
            for (slot, b) in boxes {
                let lo = [(b[0] - 2).max(0), (b[1] - 2).max(0)];
                let hi = [(b[2] + 2).min(n - 1), (b[3] + 2).min(n - 1)];
                // A texel's stem lives in its own region, a refined cell's
                // record in its parent's.
                let (shift, parent) = if r == 0 { (0, 0) } else { (2, r - 1) };
                for cj in lo[1] >> shift..=hi[1] >> shift {
                    for ci in lo[0] >> shift..=hi[0] >> shift {
                        out.push(region_key(parent, slot, ci, cj));
                    }
                }
            }
        }
        out[start..].sort_unstable();
        let mut k = start;
        for n in start..out.len() {
            if n == start || out[n] != out[k - 1] {
                out[k] = out[n];
                k += 1;
            }
        }
        out.truncate(k);
    }

    fn region(&self, key: [u32; 2], out: &mut [u32]) {
        out.fill(0);
        let (level, slot) = ((key[0] & 3) as u8, ((key[0] >> 2) & 7) as u8);
        let (i, j) = ((key[0] >> 5) as i32, key[1] as i32);
        if u32::from(level) < self.levels {
            for k in 0..(SPLIT * SPLIT) as usize {
                let c = Cell {
                    level: level + 1,
                    slot,
                    i: i * SPLIT + k as i32 % SPLIT,
                    j: j * SPLIT + k as i32 / SPLIT,
                };
                let s = self.stream(c);
                pack(
                    &s,
                    self.record_box(c, &s),
                    &mut out[k * RECORD_WORDS..][..RECORD_WORDS],
                );
            }
        }
        if level == 0 {
            let c = Cell {
                level: 0,
                slot,
                i,
                j,
            };
            let stem = self.stream(c);
            pack(
                &stem,
                self.record_box(c, &stem),
                &mut out[16 * RECORD_WORDS..][..RECORD_WORDS],
            );
        }
    }
}

/// Distance from `p` to segment `a`-`b`.
fn distance(p: DVec3, a: DVec3, b: DVec3) -> f64 {
    (p - closest(p, a, b).0).length()
}

/// The point of segment `a`-`b` closest to `p`, and its position along
/// it (0..1).
fn closest(p: DVec3, a: DVec3, b: DVec3) -> (DVec3, f64) {
    let ab = b - a;
    let l2 = ab.length_squared();
    let t = if l2 > 0.0 {
        ((p - a).dot(ab) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (a + ab * t, t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::Grid;
    use crate::layers::TerrainLayers;
    use crate::terrain::{Column, TerrainField};

    /// Earth with three levels of streams.
    fn earth() -> (Grid, crate::landform::LandformField) {
        let grid = Grid::new(6_371_000.0, 0.1).unwrap();
        let mut stack = TerrainLayers::earth();
        for l in &mut stack.layers {
            if l.kind == crate::layers::LayerKind::Rivers {
                l.octaves = 3;
            }
        }
        let field = stack.field(&grid, 7).unwrap();
        (grid, field)
    }

    /// Land texels near the planet's top (where voxel_flight starts).
    fn land_cells(s: &Streams, count: usize) -> Vec<Cell> {
        let g = s.grid();
        let mut out = Vec::new();
        let (slot, x, y) = g.locate(DVec3::Y);
        for dy in -12..12 {
            for dx in -12..12 {
                let c = Cell {
                    level: 0,
                    slot: slot as u8,
                    i: x as i32 + dx,
                    j: y as i32 + dy,
                };
                if s.stream(c).kind == Kind::Stream && out.len() < count {
                    out.push(c);
                }
            }
        }
        out
    }

    /// Every level's streams run downhill: beds never rise toward their
    /// targets, a stream's target bed is its downstream member's bed, and
    /// every refined cell over land drains somewhere.
    #[test]
    fn streams_drain_downhill_level_by_level() {
        let (_, field) = earth();
        let s = field.streams().expect("streams");
        let texels = land_cells(s, 12);
        assert!(!texels.is_empty());
        let mut counts = [0usize; 4];
        for t in &texels {
            let mut cells = vec![*t];
            for r in 1..=s.levels() as u8 {
                cells = cells
                    .iter()
                    .flat_map(|c| {
                        (0..SPLIT * SPLIT).map(move |k| Cell {
                            level: r,
                            slot: c.slot,
                            i: c.i * SPLIT + k % SPLIT,
                            j: c.j * SPLIT + k / SPLIT,
                        })
                    })
                    .filter(|c| r < 3 || (c.i + c.j) % 7 == 0)
                    .collect();
                for &c in &cells {
                    let st = s.stream(c);
                    if st.kind != Kind::Stream {
                        continue;
                    }
                    counts[usize::from(r)] += 1;
                    assert!(st.bed[0] >= st.bed[1], "{c:?}: {st:?}");
                    assert!(st.reach <= s.reach(r) + 1, "{c:?}: {st:?}");
                    assert!(st.floodplain * 4 / 3 <= st.reach, "{c:?}: {st:?}");
                }
            }
        }
        eprintln!("streams checked by level: {counts:?}");
        assert!(
            counts[1] > 50 && counts[2] > 200 && counts[3] > 200,
            "{counts:?}"
        );
    }

    /// The carved field stays continuous at every level: lines of
    /// level-0 columns through stream nodes have no steps (bounded second
    /// differences), and streams cut the terrain.
    #[test]
    fn streams_carve_without_steps() {
        let (grid, field) = earth();
        let s = field.streams().unwrap();
        let mut dry = TerrainLayers::earth();
        dry.layers
            .retain(|l| l.kind != crate::layers::LayerKind::Rivers);
        let texels = land_cells(s, 6);
        let (mut worst, mut cut, mut samples) = (0, 0i64, 0usize);
        for t in &texels {
            for r in 1..=s.levels() as u8 {
                let per = SPLIT.pow(u32::from(r));
                let c = Cell {
                    level: r,
                    slot: t.slot,
                    i: t.i * per + per / 2,
                    j: t.j * per + per / 3,
                };
                let st = s.stream(c);
                if st.kind != Kind::Stream {
                    continue;
                }
                let (cell, _) = grid.locate(st.node.as_dvec3().normalize() * grid.radius());
                let at = |i: i32, j: i32| Column {
                    face: cell.face,
                    i,
                    j,
                    level: 0,
                };
                let span = (st.reach as f64 * crate::grid::DOMAIN_UNIT / 0.1) as i32 + 50;
                for (di, dj) in [(1, 0), (0, 1)] {
                    let h: Vec<i32> = (-span..span)
                        .step_by(((span / 400).max(1)) as usize)
                        .map(|t| field.height(at(cell.i + di * t, cell.j + dj * t)))
                        .collect();
                    let step = (span / 400).max(1);
                    for w in h.windows(3) {
                        worst = worst.max((w[0] - 2 * w[1] + w[2]).abs() / (step * step));
                        samples += 1;
                    }
                }
                let _ = &dry;
                cut += i64::from(field.height(at(cell.i, cell.j)));
            }
        }
        eprintln!("worst second difference {worst} mm per column² over {samples}, {cut}");
        assert!(worst < 400, "{worst}");
    }

    /// A footprint's region keys hold every region the carve reads at its
    /// columns (the engine keeps those resident: the GPU then reads what
    /// the CPU does), at every level, near the top and across face edges.
    #[test]
    fn footprint_keys_hold_the_regions_read() {
        use crate::terrain::TerrainRegions;
        let (grid, field) = earth();
        let s = field.streams().unwrap();
        let header = &field.constants().maps;
        let mut checked = 0;
        for level in 0..14u32 {
            let cells = grid.cells() >> level;
            for (face, cx, cy) in [
                (2u8, cells / 128, cells / 128 + 3),
                (2, 0, cells / 64 - 1),
                (0, cells / 64 - 1, cells / 128),
            ] {
                let (i0, j0) = (cx * 64, cy * 64);
                let mut keys = Vec::new();
                s.keys(&grid, face, level, [i0, i0 + 63], [j0, j0 + 63], &mut keys);
                let cell = 8i32 << (level + grid.level_offset()).min(24);
                for (di, dj) in [(0, 0), (63, 0), (0, 63), (63, 63), (31, 17), (5, 60)] {
                    let p = grid.domain_point(face, i0 + di, j0 + dj, level);
                    for r in 0..=s.levels() as u8 {
                        if resolved(s.reach(r), cell) == 0 {
                            continue;
                        }
                        for (slot, xq, yq) in cells_around(header, p, r) {
                            let (i, j) =
                                (xq >> (16 - 2 * u32::from(r)), yq >> (16 - 2 * u32::from(r)));
                            for (ci, cj) in
                                (-1..=1).flat_map(|b| (-1..=1).map(move |a| (i + a, j + b)))
                            {
                                let n = s.side(r);
                                if !(0..n).contains(&ci) || !(0..n).contains(&cj) {
                                    continue;
                                }
                                let key = if r == 0 {
                                    region_key(0, slot, ci, cj)
                                } else {
                                    region_key(r - 1, slot, ci >> 2, cj >> 2)
                                };
                                assert!(
                                    keys.binary_search(&key).is_ok(),
                                    "level {level} face {face} r {r}: {key:?} missing"
                                );
                                checked += 1;
                            }
                        }
                    }
                }
            }
        }
        eprintln!("{checked} reads held");
        assert!(checked > 1000);
    }

    /// What streams cost the CPU (`--ignored --nocapture`): a cold area's
    /// regions at every level, then the carve of columns there.
    #[test]
    #[ignore]
    fn stream_costs() {
        use crate::terrain::TerrainRegions;
        let (grid, field) = earth();
        let s = field.streams().unwrap();
        let mut stack = TerrainLayers::earth();
        for l in &mut stack.layers {
            if l.kind == crate::layers::LayerKind::Rivers {
                l.octaves = 0;
            }
        }
        let rivers_only = stack.field(&grid, 7).unwrap();
        for (n, d) in [
            DVec3::Y,
            DVec3::new(0.3, 1.0, -0.2),
            DVec3::new(1.0, 0.1, 0.4),
        ]
        .iter()
        .enumerate()
        {
            let (cell, _) = grid.locate(d.normalize() * grid.radius());
            let started = std::time::Instant::now();
            let mut keys = Vec::new();
            s.keys(
                &grid,
                cell.face,
                0,
                [cell.i, cell.i + 63],
                [cell.j, cell.j + 63],
                &mut keys,
            );
            let mut words = vec![0u32; REGION_WORDS];
            for &k in &keys {
                s.region(k, &mut words);
            }
            let cold = started.elapsed().as_secs_f64() * 1e3;
            let started = std::time::Instant::now();
            for t in 0..10_000 {
                let at = Column {
                    face: cell.face,
                    i: cell.i + t % 100,
                    j: cell.j + t / 100,
                    level: 0,
                };
                std::hint::black_box(field.height(at));
            }
            let warm = started.elapsed().as_secs_f64() * 1e6 / 10_000.0;
            let started = std::time::Instant::now();
            for t in 0..10_000 {
                let at = Column {
                    face: cell.face,
                    i: cell.i + t % 100,
                    j: cell.j + t / 100,
                    level: 0,
                };
                std::hint::black_box(rivers_only.height(at));
            }
            let without = started.elapsed().as_secs_f64() * 1e6 / 10_000.0;
            eprintln!(
                "area {n}: {} regions cold in {cold:.0} ms; level-0 column height {warm:.1} us ({without:.1} us without streams)",
                keys.len()
            );
        }
    }

    /// Cold columns (`--ignored --nocapture`): level-0 heights scattered
    /// over 50 km on one thread, as a CPU probe or ray in a new area pays.
    #[test]
    #[ignore]
    fn cold_column_costs() {
        let (grid, field) = earth();
        let (cell, _) = grid.locate(DVec3::Y * grid.radius());
        let mut rng = 0x2545_f491_4f6c_dd1du64;
        let mut next = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            (rng % 500_000) as i32 - 250_000
        };
        for round in 0..2 {
            let started = std::time::Instant::now();
            for _ in 0..200 {
                let at = Column {
                    face: cell.face,
                    i: cell.i + next(),
                    j: cell.j + next(),
                    level: 0,
                };
                std::hint::black_box(field.height(at));
            }
            eprintln!(
                "round {round}: {:.2} ms a cold column",
                started.elapsed().as_secs_f64() * 1e3 / 200.0
            );
        }
    }

    /// A view's regions (`--ignored --nocapture`): footprints in rings
    /// around the planet's top, as the chunk world reserves them (level-0
    /// keys where near features read the ground), made in parallel.
    #[test]
    #[ignore]
    fn view_region_costs() {
        use crate::terrain::TerrainRegions;
        use rayon::prelude::*;
        let (grid, field) = earth();
        let s = field.streams().unwrap();
        let (cell, _) = grid.locate(DVec3::Y * grid.radius());
        let mut keys = Vec::new();
        for level in 0..16u32 {
            let (ci, cj) = ((cell.i >> level) / 64, (cell.j >> level) / 64);
            let read = if level < crate::features::NEAR_LEVELS {
                0
            } else {
                level
            };
            let up = level - read;
            for dj in -3..=3 {
                for di in -3..=3 {
                    let (fx, fy) = (ci + di, cj + dj);
                    s.keys(
                        &grid,
                        cell.face,
                        read,
                        [fx * 64 << up, ((fx * 64 + 64) << up) - 1],
                        [fy * 64 << up, ((fy * 64 + 64) << up) - 1],
                        &mut keys,
                    );
                }
            }
        }
        keys.sort_unstable();
        keys.dedup();
        let by_level: Vec<usize> = (0..4)
            .map(|l| keys.iter().filter(|k| k[0] & 3 == l).count())
            .collect();
        let started = std::time::Instant::now();
        let n = keys
            .par_iter()
            .map(|&k| {
                let mut w = vec![0u32; REGION_WORDS];
                s.region(k, &mut w);
                w[10]
            })
            .filter(|&r| r != 0)
            .count();
        eprintln!(
            "{} regions ({by_level:?} by parent level, {n} with streams) in {:.2} s on {} threads",
            keys.len(),
            started.elapsed().as_secs_f64(),
            rayon::current_num_threads()
        );
    }

    /// Coarser levels never carve deeper than finer ones show: a level's
    /// streams fade out as its cells outgrow their reach.
    #[test]
    fn coarse_levels_fade_streams_out() {
        for r in 0..=3u8 {
            let reach = 100_000 >> (2 * r);
            assert_eq!(resolved(reach, reach / 8), 256);
            assert_eq!(resolved(reach, reach / 4), 0);
            assert!(resolved(reach, reach / 6) > 0 && resolved(reach, reach / 6) < 256);
        }
        assert_eq!(weight(0, 1000), 65_536);
        assert_eq!(weight(500, 1000), 65_536);
        assert_eq!(weight(1000, 1000), 0);
        let mut last = 65_536;
        for d in 500..=1000 {
            let w = weight(d, 1000);
            assert!(w <= last);
            last = w;
        }
    }
}
