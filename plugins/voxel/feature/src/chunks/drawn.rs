//! The drawn cut of the chunk octree and its GPU chunk table, kept up to
//! date by the nodes that change: a frame costs what changed, not what is
//! resident (`docs/chunk-octree.md`).
//!
//! The table holds an entry for every drawn node and every ancestor of
//! one, key `(face | level << 8, cx, cy, cz)`, value `(slot, state, kind |
//! under | lo << 8, hi)`: the node's chunk slot (-1: none), whether a pixel
//! shows it (`choice`), what it holds (`KIND_*`; an ancestor's is its drawn
//! self's if it is drawn too, else `KIND_ANCESTOR`), whether a strict
//! ancestor is drawn (`UNDER_DRAWN`) and its content: the
//! box of its cells anything non-empty may occupy, `x | y << 7 | z << 14`
//! for its low and high corners ([lo, hi); lo > hi: nothing), and a Solid
//! or Buried node's material in bits 24..32 of the high corner's word
//! (what its rock shows: `DrawnSet::set_rock`). A drawn
//! Mixed chunk's content is its occupied layers over its footprint, a
//! Solid or Buried node's its whole box, an ancestor's the union of its
//! children's (rounded out to its cells). Empty nodes have no entry: a
//! refined ancestor whose child has none is air there.
//!
//! A switch between levels fades: for `FADE_FRAMES` both sides are drawn
//! and each pixel shows one, by a dither `r` in [0, 1024) that moves each
//! frame (TSR blends them). A node's `state` says which (`choice`): 1
//! drawn, 0 an ancestor; else fading since frame `start`, `2 + (start <<
//! 2) + incoming + 2 coarse` (start modulo 2^20): an incoming node shows
//! where r < q, an outgoing one where r >= q, with q rising from 0 to 1024
//! over the fade, so each point is shown once; where a node does not show,
//! the other level does (finer under the coarse side, coarser over the
//! fine side).
//!
//! The table is open addressing with linear probing over a fixed power of
//! two of entries (an empty key's x is -1), mirrored on the CPU; deletion
//! shifts the probe run back, so lookups never meet a tombstone. Only
//! changed entries are uploaded.

use super::classify::{Kind, NodeKey};
use rustc_hash::{FxHashMap, FxHashSet};

/// A drawn chunk, as the cull and the seed need it.
#[derive(Clone, Copy, Debug)]
pub struct DrawnChunk {
    pub slot: u32,
}

/// A drawn node's chunk: its slot and occupied layers [lo, hi) (the whole
/// chunk until read back).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NodeChunk {
    pub slot: u32,
    pub z: (f64, f64),
}

const NONE: u32 = u32::MAX;
const EMPTY: [i32; 4] = [-1, 0, 0, 0];

/// `KIND_*`: a table entry's kind (`chunk_draw.wgsl`).
pub const KIND_MIXED: i32 = 0;
pub const KIND_SOLID: i32 = 1;
pub const KIND_BURIED: i32 = 2;
pub const KIND_ANCESTOR: i32 = 3;
/// A bit of an entry's kind word: a strict ancestor of the node is drawn
/// (during a switch, a coarse node fading over its finer side). Only then
/// can a search climbing from an ancestor entry meet a node shown over it
/// (`place_at`), so the draw climbs only then.
pub const UNDER_DRAWN: i32 = 16;

/// Frames a switch between levels fades over.
pub const FADE_FRAMES: u64 = 16;

/// A box of a node's cells, [lo, hi) per axis; lo > hi on x when empty.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Content {
    lo: [i32; 3],
    hi: [i32; 3],
}

impl Content {
    const NOTHING: Self = Self {
        lo: [64; 3],
        hi: [0; 3],
    };
    const FULL: Self = Self {
        lo: [0; 3],
        hi: [64; 3],
    };

    fn is_empty(&self) -> bool {
        (0..3).any(|a| self.lo[a] >= self.hi[a])
    }

    fn union(self, o: Self) -> Self {
        if o.is_empty() {
            return self;
        }
        if self.is_empty() {
            return o;
        }
        Self {
            lo: [0, 1, 2].map(|a| self.lo[a].min(o.lo[a])),
            hi: [0, 1, 2].map(|a| self.hi[a].max(o.hi[a])),
        }
    }

    /// A child's content (child `n`: bit 0 x, 1 y, 2 z) in its parent's
    /// cells, rounded out.
    fn in_parent(self, n: usize) -> Self {
        if self.is_empty() {
            return Self::NOTHING;
        }
        let at = [(n & 1) as i32, ((n >> 1) & 1) as i32, ((n >> 2) & 1) as i32];
        Self {
            lo: [0, 1, 2].map(|a| at[a] * 32 + self.lo[a].div_euclid(2)),
            hi: [0, 1, 2].map(|a| at[a] * 32 + (self.hi[a] + 1).div_euclid(2)),
        }
    }

    fn packed(&self) -> (i32, i32) {
        let pack = |c: [i32; 3]| c[0] | (c[1] << 7) | (c[2] << 14);
        if self.is_empty() {
            return (pack(Self::NOTHING.lo), pack(Self::NOTHING.hi));
        }
        (pack(self.lo), pack(self.hi))
    }
}

/// A node of the cut, drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Drawn {
    kind: Kind,
    chunk: Option<NodeChunk>,
    /// A Solid or Buried node's material.
    rock: u8,
}

impl Drawn {
    fn content(&self) -> Content {
        match (self.kind, self.chunk) {
            (Kind::Mixed, Some(c)) => Content {
                lo: [0, 0, c.z.0.floor() as i32],
                hi: [64, 64, c.z.1.ceil() as i32],
            },
            (Kind::Mixed, None) | (Kind::Empty, _) => Content::NOTHING,
            (Kind::Solid | Kind::Buried, _) => Content::FULL,
        }
    }

    fn kind_code(&self) -> i32 {
        match self.kind {
            Kind::Mixed | Kind::Empty => KIND_MIXED,
            Kind::Solid => KIND_SOLID,
            Kind::Buried => KIND_BURIED,
        }
    }
}

/// An entry of the table: a drawn node or an ancestor of one.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Entry {
    content: Content,
    value: [i32; 4],
}

/// A node fading in or out from frame `start`, on the coarse side of its
/// switch or the fine side.
#[derive(Clone, Copy, Debug)]
struct Fade {
    start: u64,
    incoming: bool,
    coarse: bool,
}

/// Fade starts are kept modulo this.
const START_MASK: u64 = (1 << 20) - 1;

/// What a pixel with dither `r` does at a node of `state` at `frame`: 1
/// shows it, 0 looks finer, 2 coarser (`chunk_draw.wgsl`'s `choice`).
#[cfg(test)]
fn choice(state: i32, r: i32, frame: u64) -> i32 {
    if state < 2 {
        return state;
    }
    let f = state - 2;
    let elapsed = frame.wrapping_sub((f >> 2) as u64) & START_MASK;
    let q = (elapsed * 1024 / FADE_FRAMES).min(1024) as i32;
    if (r < q) == (f & 1 != 0) {
        1
    } else if f & 2 != 0 {
        0
    } else {
        2
    }
}

pub fn key_hash(k: [i32; 4]) -> u32 {
    let mut h = (k[0] as u32).wrapping_mul(0x9E37_79B1);
    h = (h ^ k[1] as u32).wrapping_mul(0x85EB_CA77);
    h = (h ^ k[2] as u32).wrapping_mul(0xC2B2_AE3D);
    h = (h ^ k[3] as u32).wrapping_mul(0x27D4_EB2F);
    h ^ (h >> 15)
}

/// The eight possible children of `key` (whether or not on its face).
fn children(key: NodeKey) -> impl Iterator<Item = (usize, NodeKey)> {
    (0..8usize).filter(move |_| key.level > 0).map(move |n| {
        (
            n,
            NodeKey {
                volume: key.volume,
                face: key.face,
                level: key.level - 1,
                cx: key.cx * 2 + (n & 1) as i32,
                cy: key.cy * 2 + ((n >> 1) & 1) as i32,
                cz: key.cz * 2 + (n >> 2) as i32,
            },
        )
    })
}

pub struct DrawnSet {
    /// Drawn chunks with a slot (the cull's list).
    pub list: Vec<DrawnChunk>,
    /// Index in `list` by slot.
    position: Vec<u32>,
    drawn: FxHashMap<NodeKey, Drawn>,
    /// The table's entries: drawn nodes and their ancestors.
    entries: FxHashMap<NodeKey, Entry>,
    /// Entries whose subtree changed, to settle by level.
    touched: Vec<NodeKey>,
    fades: FxHashMap<NodeKey, Fade>,
    /// Fading nodes strictly under each node (only nodes with any).
    fade_below: FxHashMap<NodeKey, u32>,
    /// Each volume's coarsest level (`ChunkWorld`'s volume indices).
    top_levels: Vec<u32>,
    /// Entries: key then value.
    table: Vec<[i32; 4]>,
    mask: u32,
    /// Entries changed since the last upload.
    dirty: Vec<u32>,
    /// Bumped whenever `list` changes (its slots are uploaded then).
    version: u64,
    /// Materials of uniform nodes about to be added (`set_rock`).
    rocks: FxHashMap<NodeKey, u8>,
}

impl DrawnSet {
    /// `slots` chunk slots, a table of `entries` (a power of two); each
    /// volume's nodes up to its coarsest level ([`Self::set_top_level`]).
    pub fn new(slots: usize, entries: usize) -> Self {
        assert!(entries.is_power_of_two());
        Self {
            list: Vec::new(),
            position: vec![NONE; slots],
            drawn: FxHashMap::default(),
            entries: FxHashMap::default(),
            touched: Vec::new(),
            fades: FxHashMap::default(),
            fade_below: FxHashMap::default(),
            top_levels: Vec::new(),
            table: vec![EMPTY; 2 * entries],
            mask: entries as u32 - 1,
            dirty: Vec::new(),
            version: 0,
            rocks: FxHashMap::default(),
        }
    }

    /// What Solid or Buried node `key`, added by the next update, shows
    /// where a ray hits it (a ray meets such rock where an edit uncovers
    /// it before its chunk is generated).
    pub fn set_rock(&mut self, key: NodeKey, material: u8) {
        self.rocks.insert(key, material);
    }

    /// `slots` chunk slots (no fewer than before) and a table of
    /// `entries` (a power of two); whether the table grew (its entries
    /// placed again: upload it whole).
    pub fn grow(&mut self, slots: usize, entries: usize) -> bool {
        self.position.resize(slots, NONE);
        if entries <= self.table.len() / 2 {
            return false;
        }
        let old = std::mem::replace(&mut self.table, vec![EMPTY; 2 * entries]);
        self.mask = entries as u32 - 1;
        for pair in old.chunks_exact(2) {
            if pair[0][0] != -1 {
                self.put(pair[0], pair[1]);
            }
        }
        self.dirty.clear();
        true
    }

    pub fn mask(&self) -> u32 {
        self.mask
    }

    /// Volume `volume`'s nodes go up to `top`.
    pub fn set_top_level(&mut self, volume: u16, top: u32) {
        let v = usize::from(volume);
        if self.top_levels.len() <= v {
            self.top_levels.resize(v + 1, 0);
        }
        self.top_levels[v] = top;
    }

    fn top_level(&self, volume: u16) -> u32 {
        self.top_levels
            .get(usize::from(volume))
            .copied()
            .unwrap_or(0)
    }

    /// Changes whenever the drawn chunks do.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// The whole table (its first upload).
    pub fn table(&self) -> &[[i32; 4]] {
        &self.table
    }

    /// At `frame`: nodes `removed` stop being drawn, drawn nodes that
    /// `changed` take their chunks again, nodes `added` are drawn as their
    /// kind with the chunk `chunk_of` gives. A node replaced by finer or
    /// coarser ones fades out as they fade in.
    pub fn update(
        &mut self,
        added: &[(NodeKey, Kind)],
        removed: &[NodeKey],
        changed: &FxHashSet<NodeKey>,
        frame: u64,
        mut chunk_of: impl FnMut(NodeKey) -> Option<NodeChunk>,
    ) {
        let ended: Vec<NodeKey> = self
            .fades
            .iter()
            .filter(|(_, f)| frame >= f.start + FADE_FRAMES)
            .map(|(k, _)| *k)
            .collect();
        for key in ended {
            self.end_fade(key);
        }
        // A switch the plan changes again, or whose nodes change, ends at
        // once (with every switch it overlaps).
        if !self.fades.is_empty() {
            let touching: Vec<NodeKey> = added
                .iter()
                .map(|a| a.0)
                .chain(removed.iter().copied())
                .chain(
                    changed
                        .iter()
                        .filter(|k| self.fades.contains_key(k))
                        .copied(),
                )
                .collect();
            for key in touching {
                self.snap(key);
            }
        }
        let gone: FxHashSet<NodeKey> = removed
            .iter()
            .filter(|k| self.drawn.contains_key(k))
            .copied()
            .collect();
        // A node drawn as something else is removed, then added.
        let came: FxHashMap<NodeKey, Kind> = added.iter().copied().collect();
        let gone_above = self.ancestors_of(gone.iter().copied());
        let came_above = self.ancestors_of(came.keys().copied().filter(|k| !gone.contains(k)));
        for &key in &gone {
            if came.contains_key(&key) {
                self.drop_node(key);
                continue;
            }
            // Finer nodes came under it, or a coarser one over it.
            let coarse = came_above.contains(&key);
            if coarse || self.ancestors(key).any(|a| came.contains_key(&a)) {
                self.start_fade(
                    key,
                    Fade {
                        start: frame,
                        incoming: false,
                        coarse,
                    },
                );
                self.touched.push(key);
            } else {
                self.drop_node(key);
            }
        }
        for key in changed {
            if let Some(d) = self.drawn.get(key).copied() {
                self.drop_node(*key);
                self.add_node(
                    *key,
                    Drawn {
                        chunk: chunk_of(*key),
                        ..d
                    },
                );
            }
        }
        for (&key, &kind) in &came {
            if self.drawn.contains_key(&key) {
                continue;
            }
            let coarse = gone_above.contains(&key);
            if !gone.contains(&key) && (coarse || self.ancestors(key).any(|a| gone.contains(&a))) {
                self.start_fade(
                    key,
                    Fade {
                        start: frame,
                        incoming: true,
                        coarse,
                    },
                );
            }
            let chunk = if kind == Kind::Mixed {
                chunk_of(key)
            } else {
                None
            };
            let rock = self.rocks.get(&key).copied().unwrap_or(0);
            self.add_node(key, Drawn { kind, chunk, rock });
        }
        self.rocks.clear();
        self.settle_entries();
    }

    fn start_fade(&mut self, key: NodeKey, fade: Fade) {
        if self.fades.insert(key, fade).is_none() {
            for a in self.ancestors(key) {
                *self.fade_below.entry(a).or_default() += 1;
            }
        }
    }

    fn take_fade(&mut self, key: NodeKey) -> Option<Fade> {
        let fade = self.fades.remove(&key)?;
        for a in self.ancestors(key) {
            if let Some(n) = self.fade_below.get_mut(&a) {
                *n -= 1;
                if *n == 0 {
                    self.fade_below.remove(&a);
                }
            }
        }
        Some(fade)
    }

    /// The fade of `key` ends: an outgoing node leaves.
    fn end_fade(&mut self, key: NodeKey) {
        if self.take_fade(key).is_some_and(|f| !f.incoming) {
            self.drop_node(key);
        } else {
            self.touched.push(key);
        }
    }

    /// End the fades overlapping `key` now, and those overlapping them.
    fn snap(&mut self, key: NodeKey) {
        let mut work = vec![key];
        while let Some(t) = work.pop() {
            let mut hit: Vec<NodeKey> = std::iter::once(t)
                .chain(self.ancestors(t))
                .filter(|k| self.fades.contains_key(k))
                .collect();
            if self.fade_below.contains_key(&t) {
                let mut down = vec![t];
                while let Some(d) = down.pop() {
                    for (_, c) in children(d) {
                        if self.fades.contains_key(&c) {
                            hit.push(c);
                        }
                        if self.fade_below.contains_key(&c) {
                            down.push(c);
                        }
                    }
                }
            }
            for k in hit {
                self.end_fade(k);
                work.push(k);
            }
        }
    }

    fn ancestors(&self, key: NodeKey) -> impl Iterator<Item = NodeKey> + use<> {
        let top = self.top_level(key.volume);
        std::iter::successors(Some(key), move |k| (k.level < top).then(|| k.parent())).skip(1)
    }

    /// Every strict ancestor of `nodes`.
    fn ancestors_of(&self, nodes: impl Iterator<Item = NodeKey>) -> FxHashSet<NodeKey> {
        let mut out = FxHashSet::default();
        for key in nodes {
            for a in self.ancestors(key) {
                if !out.insert(a) {
                    break;
                }
            }
        }
        out
    }

    fn state_of(&self, key: NodeKey) -> i32 {
        self.fades.get(&key).map_or(1, |f| {
            2 + (((f.start & START_MASK) as i32) << 2)
                + i32::from(f.incoming)
                + 2 * i32::from(f.coarse)
        })
    }

    /// Whether `node` is drawn.
    pub fn holds(&self, node: NodeKey) -> bool {
        self.drawn.contains_key(&node)
    }

    /// Whether `node` is drawn or an ancestor of a drawn node.
    pub fn in_cut(&self, node: NodeKey) -> bool {
        self.entries.contains_key(&node)
    }

    /// Drawn nodes of volume `volume`.
    pub fn nodes_of(&self, volume: u16) -> Vec<NodeKey> {
        self.drawn
            .keys()
            .filter(|k| k.volume == volume)
            .copied()
            .collect()
    }

    /// Drawn Solid and Buried nodes.
    pub fn uniform_nodes(&self) -> impl Iterator<Item = NodeKey> + '_ {
        self.drawn
            .iter()
            .filter(|(_, d)| d.kind != Kind::Mixed)
            .map(|(k, _)| *k)
    }

    /// What `node` is drawn as.
    pub fn kind(&self, node: NodeKey) -> Option<Kind> {
        self.drawn.get(&node).map(|d| d.kind)
    }

    /// `node`'s drawn state (diagnostics): drawn, its entry's state (1
    /// drawn, 0 an ancestor, else fading) and its fade (start, incoming,
    /// coarse side).
    pub fn describe(&self, node: NodeKey) -> (bool, Option<i32>, Option<(u64, bool, bool)>) {
        (
            self.drawn.contains_key(&node),
            self.entries.get(&node).map(|e| e.value[1]),
            self.fades
                .get(&node)
                .map(|f| (f.start, f.incoming, f.coarse)),
        )
    }

    /// Nodes drawn.
    pub fn node_count(&self) -> usize {
        self.drawn.len()
    }

    /// Drawn nodes fading in or out.
    pub fn fading(&self) -> usize {
        self.fades.len()
    }

    fn drop_node(&mut self, key: NodeKey) {
        if let Some(d) = self.drawn.remove(&key) {
            if let Some(c) = d.chunk {
                self.remove(c.slot);
            }
            self.touched.push(key);
            self.touch_below(key);
        }
    }

    fn add_node(&mut self, key: NodeKey, drawn: Drawn) {
        if let Some(c) = drawn.chunk {
            self.version += 1;
            self.position[c.slot as usize] = self.list.len() as u32;
            self.list.push(DrawnChunk { slot: c.slot });
        }
        self.drawn.insert(key, drawn);
        self.touched.push(key);
        self.touch_below(key);
    }

    /// Touch the entries under `key` (whether a strict ancestor is drawn,
    /// `UNDER_DRAWN`, changed for them): through entries, as each entry's
    /// parent has one.
    fn touch_below(&mut self, key: NodeKey) {
        let mut down = vec![key];
        while let Some(d) = down.pop() {
            for (_, c) in children(d) {
                if self.entries.contains_key(&c) {
                    self.touched.push(c);
                    down.push(c);
                }
            }
        }
    }

    /// Recompute the touched entries, finest first, and their ancestors
    /// while they change; an entry with nothing drawn under it leaves.
    fn settle_entries(&mut self) {
        let mut by_level: Vec<FxHashSet<NodeKey>> =
            vec![Default::default(); self.top_levels.iter().max().map_or(0, |t| *t as usize) + 1];
        for key in std::mem::take(&mut self.touched) {
            by_level[key.level as usize].insert(key);
        }
        for level in 0..by_level.len() {
            for key in std::mem::take(&mut by_level[level]) {
                let old = self.entries.get(&key).copied();
                let drawn = self.drawn.get(&key).copied();
                let below = children(key)
                    .filter_map(|(n, c)| self.entries.get(&c).map(|e| e.content.in_parent(n)))
                    .reduce(Content::union);
                let new = match (drawn, below) {
                    (None, None) => None,
                    (d, b) => {
                        let content = d
                            .map_or(Content::NOTHING, |d| d.content())
                            .union(b.unwrap_or(Content::NOTHING));
                        let (lo, hi) = content.packed();
                        let under = if self.ancestors(key).any(|a| self.drawn.contains_key(&a)) {
                            UNDER_DRAWN
                        } else {
                            0
                        };
                        let value = match d {
                            Some(d) => [
                                d.chunk.map_or(-1, |c| c.slot as i32),
                                self.state_of(key),
                                d.kind_code() | under | (lo << 8),
                                hi | (i32::from(d.rock) << 24),
                            ],

                            None => [-1, 0, KIND_ANCESTOR | under | (lo << 8), hi],
                        };
                        Some(Entry { content, value })
                    }
                };
                if new == old {
                    continue;
                }
                match new {
                    Some(e) => {
                        self.entries.insert(key, e);
                        self.put(key.table_key(), e.value);
                    }
                    None => {
                        self.entries.remove(&key);
                        self.take(key.table_key());
                    }
                }
                if key.level < self.top_level(key.volume) {
                    by_level[level + 1].insert(key.parent());
                }
            }
        }
    }

    fn remove(&mut self, slot: u32) {
        let at = self.position[slot as usize];
        if at == NONE {
            return;
        }
        self.version += 1;
        self.list.swap_remove(at as usize);
        self.position[slot as usize] = NONE;
        if let Some(moved) = self.list.get(at as usize) {
            self.position[moved.slot as usize] = at;
        }
    }

    /// Set entry `key`'s value, adding the entry if absent.
    fn put(&mut self, key: [i32; 4], value: [i32; 4]) {
        let mut i = key_hash(key) & self.mask;
        while self.table[2 * i as usize][0] != -1 && self.table[2 * i as usize] != key {
            i = (i + 1) & self.mask;
        }
        self.table[2 * i as usize] = key;
        self.table[2 * i as usize + 1] = value;
        self.dirty.push(i);
    }

    /// Remove entry `key` (present).
    fn take(&mut self, key: [i32; 4]) {
        // Find the entry, then shift the run after it back.
        let mut i = key_hash(key) & self.mask;
        while self.table[2 * i as usize] != key {
            i = (i + 1) & self.mask;
        }
        let mut j = i;
        loop {
            j = (j + 1) & self.mask;
            let k = self.table[2 * j as usize];
            if k[0] == -1 {
                break;
            }
            let home = key_hash(k) & self.mask;
            // The entry at j may move to i unless its home lies in (i, j].
            let stays = if i <= j {
                i < home && home <= j
            } else {
                i < home || home <= j
            };
            if !stays {
                self.table[2 * i as usize] = k;
                self.table[2 * i as usize + 1] = self.table[2 * j as usize + 1];
                self.dirty.push(i);
                i = j;
            }
        }
        self.table[2 * i as usize] = EMPTY;
        self.dirty.push(i);
    }

    pub fn contains(&self, slot: u32) -> bool {
        self.position[slot as usize] != NONE
    }

    /// The entries changed since the last call, each as its index, key
    /// and value (`chunk_table.wgsl`).
    pub fn take_changes(&mut self) -> Vec<[i32; 4]> {
        let mut dirty = std::mem::take(&mut self.dirty);
        dirty.sort_unstable();
        dirty.dedup();
        dirty
            .into_iter()
            .flat_map(|i| {
                [
                    [i as i32, 0, 0, 0],
                    self.table[2 * i as usize],
                    self.table[2 * i as usize + 1],
                ]
            })
            .collect()
    }

    /// Entry `key`'s value (as the GPU lookup finds it).
    #[cfg(test)]
    fn find(&self, key: [i32; 4]) -> Option<[i32; 4]> {
        let mut i = key_hash(key) & self.mask;
        loop {
            let k = self.table[2 * i as usize];
            if k[0] == -1 {
                return None;
            }
            if k == key {
                return Some(self.table[2 * i as usize + 1]);
            }
            i = (i + 1) & self.mask;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rng(mut state: u64) -> impl FnMut() -> u64 {
        move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        }
    }

    fn node(level: u32, cx: i32, cy: i32, cz: i32) -> NodeKey {
        NodeKey {
            volume: 0,
            face: 0,
            level,
            cx,
            cy,
            cz,
        }
    }

    /// A random cut of a level-`top` root: refined with probability 2/3
    /// each, a third of leaves Empty (not drawn), the rest Mixed.
    fn random_cut(top: u32, next: &mut impl FnMut() -> u64) -> Vec<(NodeKey, Kind)> {
        let mut drawn = Vec::new();
        let mut open = vec![node(top, 0, 0, 0)];
        while let Some(k) = open.pop() {
            if k.level > 0 && next() % 3 != 0 {
                open.extend(children(k).map(|c| c.1));
            } else if next() % 3 != 0 {
                drawn.push((
                    k,
                    if next() % 4 == 0 {
                        Kind::Solid
                    } else {
                        Kind::Mixed
                    },
                ));
            }
        }
        drawn
    }

    fn diff(a: &[(NodeKey, Kind)], b: &[(NodeKey, Kind)]) -> (Vec<(NodeKey, Kind)>, Vec<NodeKey>) {
        (
            a.iter().filter(|x| !b.contains(x)).copied().collect(),
            b.iter().filter(|x| !a.contains(x)).map(|x| x.0).collect(),
        )
    }

    /// The shown node over level-0 cell `p` for dither `r`, searched as the
    /// draw does: coarser while no entry says otherwise, finer through
    /// ancestors; a refined node without an entry for the child: air.
    fn search(
        set: &DrawnSet,
        p: [i32; 3],
        from: u32,
        r: i32,
        frame: u64,
    ) -> Option<Option<NodeKey>> {
        let at = |l: u32| node(l, p[0] >> l, p[1] >> l, p[2] >> l);
        let state = |l: u32| set.find(at(l).table_key()).map(|v| v[1]);
        let under = |l: u32| {
            set.find(at(l).table_key())
                .is_some_and(|v| v[2] & UNDER_DRAWN != 0)
        };
        let mut level = from;
        // Up to an entry.
        while state(level).is_none() {
            if level >= set.top_level(0) {
                return Some(None);
            }
            level += 1;
        }
        // An ancestor found climbing: a coarser node fading in over it may
        // show here (its finer side has no node over this point), when one
        // over it is drawn.
        if state(level) == Some(0) && under(level) {
            for l in level + 1..=set.top_level(0) {
                match state(l) {
                    Some(0) => continue,
                    Some(s) => {
                        if choice(s, r, frame) == 1 {
                            return Some(Some(at(l)));
                        }
                        break;
                    }
                    None => break,
                }
            }
        }
        // Up past ancestors (over the hidden fine side of a switch), down
        // through them; a missing node below is air.
        let mut way = 0;
        for _ in 0..64 {
            let Some(s) = state(level) else {
                return if way < 0 { Some(None) } else { None };
            };
            let go = if s == 0 && way > 0 {
                2
            } else {
                choice(s, r, frame)
            };
            match go {
                1 => return Some(Some(at(level))),
                0 if level > 0 && way <= 0 => {
                    way = -1;
                    level -= 1;
                }
                2 if level < set.top_level(0) && way >= 0 => {
                    way = 1;
                    level += 1;
                }
                _ => return None,
            }
        }
        None
    }

    /// Random cuts of a level-3 root, a new one every few frames: at every
    /// frame, for every dither and starting level, the search finds the one
    /// node shown over each finest cell (none where it is Empty), and each
    /// drawn node's entry carries its state.
    #[test]
    fn fades_show_every_cell_once() {
        let mut next = rng(0x2545_F491_4F6C_DD1D);
        let mut set = DrawnSet::new(1 << 14, 1 << 14);
        set.set_top_level(0, 3);
        let mut last: Vec<(NodeKey, Kind)> = Vec::new();
        let mut slot = 0u32;
        let mut frame = 0;
        for round in 0..200 {
            let drawn = random_cut(3, &mut next);
            let (added, removed) = diff(&drawn, &last);
            set.update(&added, &removed, &Default::default(), frame, |_| {
                slot = (slot + 1) % (1 << 14);
                Some(NodeChunk {
                    slot,
                    z: (0.0, 64.0),
                })
            });
            last = drawn;
            for _ in 0..(next() % 20) {
                frame += 1;
                set.update(&[], &[], &Default::default(), frame, |_| None);
                for r in [0, 1, 511, 512, 1023] {
                    for _ in 0..40 {
                        let p = [
                            (next() % 8) as i32,
                            (next() % 8) as i32,
                            (next() % 8) as i32,
                        ];
                        // Nodes shown over p by their state.
                        let shown: Vec<NodeKey> = set
                            .drawn
                            .keys()
                            .filter(|k| {
                                (0..3).all(|a| {
                                    [p[0], p[1], p[2]][a] >> k.level == [k.cx, k.cy, k.cz][a]
                                }) && choice(set.find(k.table_key()).unwrap()[1], r, frame) == 1
                            })
                            .copied()
                            .collect();
                        assert!(shown.len() <= 1, "round {round}: {shown:?}");
                        // Each entry knows whether a strict ancestor is drawn.
                        for key in set.entries.keys() {
                            let under = set.find(key.table_key()).unwrap()[2] & UNDER_DRAWN != 0;
                            assert_eq!(
                                under,
                                set.ancestors(*key).any(|a| set.drawn.contains_key(&a)),
                                "round {round} {key:?}"
                            );
                        }
                        for from in 0..=3 {
                            let chain: Vec<_> = (0..=3u32)
                                .map(|l| {
                                    let k = node(l, p[0] >> l, p[1] >> l, p[2] >> l);
                                    (
                                        l,
                                        set.find(k.table_key())
                                            .map(|v| (v[1], choice(v[1], r, frame))),
                                        set.fades.get(&k).map(|f| (f.start, f.incoming, f.coarse)),
                                        set.drawn.get(&k).map(|d| d.kind),
                                    )
                                })
                                .collect();
                            assert_eq!(
                                search(&set, p, from, r, frame),
                                Some(shown.first().copied()),
                                "round {round} frame {frame} r {r} at {p:?} from {from}: {chain:?}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// Every entry's content is the union of what is drawn under it.
    #[test]
    fn ancestors_hold_the_content_of_their_subtrees() {
        let mut next = rng(0x9E37_79B9_7F4A_7C15);
        let mut set = DrawnSet::new(1 << 14, 1 << 14);
        set.set_top_level(0, 3);
        let mut last: Vec<(NodeKey, Kind)> = Vec::new();
        let mut slot = 0u32;
        for round in 0..100 {
            let drawn = random_cut(3, &mut next);
            let (added, removed) = diff(&drawn, &last);
            let layers: FxHashMap<NodeKey, (f64, f64)> = drawn
                .iter()
                .map(|d| (d.0, ((next() % 30) as f64, 34.0 + (next() % 30) as f64)))
                .collect();
            // Frames FADE_FRAMES apart: no fade under way.
            set.update(
                &added,
                &removed,
                &drawn.iter().map(|d| d.0).collect(),
                round * FADE_FRAMES,
                |k| {
                    slot = (slot + 1) % (1 << 14);
                    Some(NodeChunk {
                        slot,
                        z: layers[&k],
                    })
                },
            );
            set.update(
                &[],
                &[],
                &Default::default(),
                (round + 1) * FADE_FRAMES,
                |_| None,
            );
            last = drawn.clone();
            // The expected content of every node over the cut, in level-0 cells.
            for (key, e) in &set.entries {
                let scale = 1 << key.level;
                let mut want: Option<([i32; 3], [i32; 3])> = None;
                for (k, kind) in &drawn {
                    let mut p = *k;
                    while p.level < key.level {
                        p = p.parent();
                    }
                    if p != *key {
                        continue;
                    }
                    let s = 1 << k.level;
                    let z = if *kind == Kind::Mixed {
                        (layers[k].0 as i32, layers[k].1 as i32)
                    } else {
                        (0, 64)
                    };
                    let lo = [k.cx * 64 * s, k.cy * 64 * s, k.cz * 64 * s + z.0 * s];
                    let hi = [
                        k.cx * 64 * s + 64 * s,
                        k.cy * 64 * s + 64 * s,
                        k.cz * 64 * s + z.1 * s,
                    ];
                    want = Some(want.map_or((lo, hi), |(a, b)| {
                        (
                            [0, 1, 2].map(|i| a[i].min(lo[i])),
                            [0, 1, 2].map(|i| b[i].max(hi[i])),
                        )
                    }));
                }
                let (lo, hi) = want.expect("an entry has something drawn under it");
                let origin = [
                    key.cx * 64 * scale,
                    key.cy * 64 * scale,
                    key.cz * 64 * scale,
                ];
                for a in 0..3 {
                    // Rounded out to the node's cells, never inside.
                    assert!(
                        origin[a] + e.content.lo[a] * scale <= lo[a]
                            && origin[a] + e.content.hi[a] * scale >= hi[a],
                        "round {round} {key:?} axis {a}: {:?} vs {lo:?}..{hi:?}",
                        e.content
                    );
                    assert!(
                        lo[a] - (origin[a] + e.content.lo[a] * scale) < scale * 2
                            && origin[a] + e.content.hi[a] * scale - hi[a] < scale * 2,
                        "round {round} {key:?} axis {a}: loose"
                    );
                }
            }
        }
    }

    #[test]
    fn the_table_finds_every_drawn_chunk_after_nodes_come_and_go_and_it_grows() {
        // A small table so probe runs collide and wrap.
        let mut set = DrawnSet::new(4096, 1024);
        set.set_top_level(0, 0);
        let mut next = rng(0x2545_F491_4F6C_DD1D);
        let at = |n: i32| node(0, n, n / 3, n % 5);
        let mut last: Vec<(NodeKey, Kind)> = Vec::new();
        for round in 0..200 {
            let drawn: Vec<(NodeKey, Kind)> = (0..300)
                .filter(|_| next() % 2 == 0)
                .map(|n| (at(n), Kind::Mixed))
                .collect();
            let changed: FxHashSet<NodeKey> = drawn
                .iter()
                .filter(|_| next() % 7 == 0)
                .map(|d| d.0)
                .collect();
            let (added, removed) = diff(&drawn, &last);
            // Halfway, the slots and the table grow (entries placed again).
            if round == 100 {
                assert!(set.grow(8192, 4096));
                assert!(!set.grow(8192, 2048));
            }
            set.update(&added, &removed, &changed, round, |k| {
                Some(NodeChunk {
                    slot: k.cx as u32,
                    z: (0.0, 64.0),
                })
            });
            last = drawn.clone();
            set.take_changes();
            assert_eq!(set.list.len(), drawn.len(), "round {round}");
            for (k, _) in &drawn {
                assert_eq!(
                    set.find(k.table_key()).map(|v| (v[0], v[1])),
                    Some((k.cx, 1)),
                    "round {round}"
                );
            }
            for n in 0..300 {
                if !drawn.iter().any(|d| d.0 == at(n)) {
                    assert_eq!(set.find(at(n).table_key()), None);
                }
            }
        }
    }
}
