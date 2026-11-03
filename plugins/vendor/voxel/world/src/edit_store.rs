//! Baked edits: what brushes did to each cell, stored sparsely in bricks.
//!
//! A cell's whole edit history collapses exactly into one [`CellEdit`]:
//! unchanged, carved to air, filled with a material, or painted (recoloured
//! if solid). Applying a brush after any of these yields another of them
//! ([`CellEdit::then`]), so baking brushes in order loses nothing, and the
//! result is independent of the terrain under it: the terrain is only read
//! when a cell is sampled ([`CellEdit::apply`]).
//!
//! Base-level bricks (8x8x8 cells) hold the exact baked edits. Each coarser
//! level is built from the one below by majority of the eight child cells
//! ([`CellEdit::majority`]), so accumulated destruction (a hill removed by
//! thousands of small digs) shows at every distance, where a point sample
//! of each brush would omit brushes smaller than the level's cells. A brush
//! too large to bake into base cells bakes at its bake level directly
//! ([`EditStore::bake_at`]); a coarse cell whose children are mostly
//! unchanged keeps such an edit when its level is rebuilt.
//!
//! Bricks are stored by column (a renderer column's bricks are one lookup)
//! in shards shared copy-on-write: copying a store (what a renderer does to
//! publish an edited world) copies shard pointers, and an edit copies only
//! the shards, columns and bricks it touches. A recorded change keeps the
//! columns it replaced, which is how sealing is undone.
use crate::edits::{center_half, meets, BrushOp, Cube, FaceBrush, Meets, SHAPE_BALL};
use crate::grid::{Grid, VolumeMap, VolumePoints, BRICK};
use glam::{DVec3, IVec3};
use rustc_hash::FxHashMap;
use std::sync::Arc;

/// A cell's baked edit (packed: state in bits 0..2, material in 8..16).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct CellEdit(pub u16);

impl CellEdit {
    pub const UNCHANGED: Self = Self(0);
    pub const AIR: Self = Self(1);
    pub(crate) const SOLID: u16 = 2;
    const PAINT: u16 = 3;

    pub fn solid(material: u32) -> Self {
        Self(Self::SOLID | ((material as u16 & 0xff) << 8))
    }
    pub fn paint(material: u32) -> Self {
        Self(Self::PAINT | ((material as u16 & 0xff) << 8))
    }
    pub fn state(self) -> u16 {
        self.0 & 3
    }
    pub fn material(self) -> u32 {
        u32::from(self.0 >> 8)
    }
    pub fn is_unchanged(self) -> bool {
        self.state() == 0
    }

    /// This edit followed by a brush `op` with `material`.
    pub fn then(self, op: BrushOp, material: u32) -> Self {
        match op {
            BrushOp::Remove => Self::AIR,
            BrushOp::Add => Self::solid(material),
            BrushOp::Paint => match self.state() {
                0 | Self::PAINT => Self::paint(material),
                Self::SOLID => Self::solid(material),
                _ => Self::AIR,
            },
        }
    }

    /// This edit followed by `later` (composition of baked edits).
    #[cfg(test)]
    fn then_edit(self, later: Self) -> Self {
        match later.state() {
            0 => self,
            1 => Self::AIR,
            Self::SOLID => later,
            _ => self.then(BrushOp::Paint, later.material()),
        }
    }

    /// The canonical `(kind, material)` of a cell after this edit, given the
    /// terrain's (material 0 on a solid cell: the terrain rule).
    pub fn apply(self, kind: u32, material: u32) -> (u32, u32) {
        match self.state() {
            0 => (kind, material),
            1 => (0, 0),
            Self::SOLID => (1, self.material()),
            _ if kind == 1 => (1, self.material()),
            _ => (0, 0),
        }
    }

    /// The edit a coarse cell shows for its eight children: unchanged when
    /// at least half of them are, else the most common state (air before
    /// solid before paint on ties) with its most common material.
    pub fn majority(children: [Self; 8]) -> Self {
        let mut counts = [0u8; 4];
        for c in children {
            counts[c.state() as usize] += 1;
        }
        if counts[0] >= 4 {
            return Self::UNCHANGED;
        }
        let state = (1..4u16)
            .max_by_key(|&s| (counts[s as usize], 4 - s))
            .expect("three states");
        if state == 1 {
            return Self::AIR;
        }
        let mut best = (0u8, 0u32);
        for c in children.iter().filter(|c| c.state() == state) {
            let n = children.iter().filter(|o| **o == *c).count() as u8;
            if n > best.0 || (n == best.0 && c.material() < best.1) {
                best = (n, c.material());
            }
        }
        Self(state | ((best.1 as u16) << 8))
    }
}

/// Cells per brick.
pub const BRICK_CELLS: usize = (BRICK * BRICK * BRICK) as usize;

/// One 8x8x8 brick of a level's cells, indexed `x + 8 (y + 8 z)` with
/// `(x, y, z)` the cell's `(i, j, k)` modulo 8.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Brick {
    pub cells: [CellEdit; BRICK_CELLS],
}

impl Default for Brick {
    fn default() -> Self {
        Self {
            cells: [CellEdit::UNCHANGED; BRICK_CELLS],
        }
    }
}

impl Brick {
    pub fn index(i: i32, j: i32, k: i32) -> usize {
        (i.rem_euclid(BRICK) + BRICK * (j.rem_euclid(BRICK) + BRICK * k.rem_euclid(BRICK))) as usize
    }
    pub fn is_unchanged(&self) -> bool {
        self.cells.iter().all(|c| c.is_unchanged())
    }
}

/// A brick: face, level and brick coordinates (cell index divided by 8).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BrickKey {
    pub face: u8,
    pub level: u8,
    pub bi: i32,
    pub bj: i32,
    pub bk: i32,
}

impl BrickKey {
    pub fn of_cell(face: u8, level: u32, i: i32, j: i32, k: i32) -> Self {
        Self {
            face,
            level: level as u8,
            bi: i.div_euclid(BRICK),
            bj: j.div_euclid(BRICK),
            bk: k.div_euclid(BRICK),
        }
    }
    pub fn parent(self) -> Self {
        Self {
            face: self.face,
            level: self.level + 1,
            bi: self.bi.div_euclid(2),
            bj: self.bj.div_euclid(2),
            bk: self.bk.div_euclid(2),
        }
    }
    /// The column holding this brick.
    pub fn column(self) -> ColumnKey {
        ColumnKey {
            face: self.face,
            level: self.level,
            ci: self.bi,
            cj: self.bj,
        }
    }
}

/// A column of bricks: face, level and brick coordinates across the face.
/// At every level a renderer's column is exactly one brick column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ColumnKey {
    pub face: u8,
    pub level: u8,
    pub ci: i32,
    pub cj: i32,
}

/// A column's stored bricks by height (`bk`), sorted.
pub type Column = Vec<(i32, Arc<Brick>)>;

const SHARDS: usize = 64;

fn shard_of(key: &ColumnKey) -> usize {
    // Neighbouring columns land in different shards.
    let h = (key.ci as u32).wrapping_mul(0x9e37_79b1)
        ^ (key.cj as u32).wrapping_mul(0x85eb_ca77)
        ^ (u32::from(key.level) << 3 | u32::from(key.face));
    (h.rotate_left(7) as usize) % SHARDS
}

type Shard = FxHashMap<ColumnKey, Arc<Column>>;

/// The columns a recorded change replaced and what they held before it
/// (`None`: absent). Restoring them undoes the change exactly.
#[derive(Clone, Default)]
pub struct StoreUndo {
    columns: Vec<(ColumnKey, Option<Arc<Column>>)>,
}

impl StoreUndo {
    pub fn columns(&self) -> impl Iterator<Item = ColumnKey> + '_ {
        self.columns.iter().map(|(key, _)| *key)
    }
    /// Each column the change replaced and the brick heights (`bk`, lowest
    /// and highest) it held before the change or holds in `now`.
    pub fn spans(&self, now: &EditStore) -> Vec<(ColumnKey, [i32; 2])> {
        self.columns
            .iter()
            .filter_map(|(key, before)| {
                let heights = before
                    .iter()
                    .chain(now.column(key))
                    .flat_map(|c| c.iter().map(|(bk, _)| *bk));
                heights
                    .fold(None, |span: Option<[i32; 2]>, bk| {
                        Some(span.map_or([bk, bk], |[lo, hi]| [lo.min(bk), hi.max(bk)]))
                    })
                    .map(|span| (*key, span))
            })
            .collect()
    }
}

/// Sparse baked edits of every level (see the module doc).
#[derive(Clone)]
pub struct EditStore {
    shards: Vec<Arc<Shard>>,
    /// Bricks stored per level.
    counts: Vec<usize>,
    /// While recording: each changed column's contents before its first
    /// change.
    recording: Option<FxHashMap<ColumnKey, Option<Arc<Column>>>>,
}

impl Default for EditStore {
    fn default() -> Self {
        Self {
            shards: (0..SHARDS).map(|_| Arc::new(Shard::default())).collect(),
            counts: Vec::new(),
            recording: None,
        }
    }
}

impl EditStore {
    pub fn is_empty(&self) -> bool {
        self.counts.iter().all(|&n| n == 0)
    }
    /// Bricks stored at each level.
    pub fn brick_counts(&self) -> &[usize] {
        &self.counts
    }
    /// A column's stored bricks by height, if any.
    pub fn column(&self, key: &ColumnKey) -> Option<&Arc<Column>> {
        self.shards[shard_of(key)].get(key)
    }
    pub fn brick(&self, key: &BrickKey) -> Option<&Arc<Brick>> {
        let column = self.column(&key.column())?;
        column
            .binary_search_by_key(&key.bk, |(bk, _)| *bk)
            .ok()
            .map(|at| &column[at].1)
    }
    /// The baked edit of a level cell.
    pub fn cell(&self, face: u8, level: u32, i: i32, j: i32, k: i32) -> CellEdit {
        self.brick(&BrickKey::of_cell(face, level, i, j, k))
            .map_or(CellEdit::UNCHANGED, |b| b.cells[Brick::index(i, j, k)])
    }
    /// Every stored brick (any order).
    pub fn bricks(&self) -> impl Iterator<Item = (BrickKey, &Arc<Brick>)> + '_ {
        self.shards
            .iter()
            .flat_map(|shard| shard.iter())
            .flat_map(|(c, column)| {
                column.iter().map(move |(bk, brick)| {
                    (
                        BrickKey {
                            face: c.face,
                            level: c.level,
                            bi: c.ci,
                            bj: c.cj,
                            bk: *bk,
                        },
                        brick,
                    )
                })
            })
    }

    /// Store a brick (`None` or an unchanged brick removes it).
    pub fn set(&mut self, key: BrickKey, brick: Option<Brick>) {
        self.set_column(
            key.column(),
            vec![(key.bk, brick.filter(|b| !b.is_unchanged()).map(Arc::new))],
        );
    }

    /// Store bricks of one column by height (`None` removes one; none may
    /// be unchanged).
    fn set_column(&mut self, column_key: ColumnKey, bricks: Vec<(i32, Option<Arc<Brick>>)>) {
        let shard = Arc::make_mut(&mut self.shards[shard_of(&column_key)]);
        if let Some(recording) = &mut self.recording {
            recording
                .entry(column_key)
                .or_insert_with(|| shard.get(&column_key).cloned());
        }
        let level = column_key.level as usize;
        if self.counts.len() <= level {
            self.counts.resize(level + 1, 0);
        }
        let column = Arc::make_mut(shard.entry(column_key).or_default());
        for (bk, brick) in bricks {
            match (column.binary_search_by_key(&bk, |(bk, _)| *bk), brick) {
                (Ok(at), Some(b)) => column[at].1 = b,
                (Err(at), Some(b)) => {
                    column.insert(at, (bk, b));
                    self.counts[level] += 1;
                }
                (Ok(at), None) => {
                    column.remove(at);
                    self.counts[level] -= 1;
                }
                (Err(_), None) => {}
            }
        }
        if column.is_empty() {
            shard.remove(&column_key);
        }
    }

    /// Record the columns every change replaces until [`Self::end_record`].
    pub fn begin_record(&mut self) {
        self.recording = Some(FxHashMap::default());
    }
    /// What changed since [`Self::begin_record`], to undo it with
    /// [`Self::restore`].
    pub fn end_record(&mut self) -> StoreUndo {
        let mut columns: Vec<_> = self
            .recording
            .take()
            .unwrap_or_default()
            .into_iter()
            .collect();
        columns.sort_unstable_by_key(|(key, _)| *key);
        StoreUndo { columns }
    }
    /// Put back the columns a recorded change replaced.
    pub fn restore(&mut self, undo: StoreUndo) {
        for (key, before) in undo.columns {
            let level = key.level as usize;
            let shard = Arc::make_mut(&mut self.shards[shard_of(&key)]);
            let now = match &before {
                Some(column) => shard.insert(key, Arc::clone(column)),
                None => shard.remove(&key),
            };
            let (now, before) = (now.map_or(0, |c| c.len()), before.map_or(0, |c| c.len()));
            if self.counts.len() <= level {
                self.counts.resize(level + 1, 0);
            }
            self.counts[level] = self.counts[level] + before - now;
        }
    }

    /// Bake a brush resolved on one face into the base level, with the
    /// exact containment test generation uses. Returns the base bricks it
    /// changed.
    pub fn bake(&mut self, grid: &Grid, fb: &FaceBrush, op: BrushOp) -> Vec<BrickKey> {
        let (lo, hi) = brush_box(grid, fb, 0);
        self.bake_box(grid, fb, op, 0, lo, hi)
    }

    /// The cells of `level` brush `fb` holds, by brick: what baking it at
    /// that level ([`Self::bake`] at the base level, [`Self::bake_at`]
    /// above) changes. Bricks it holds no cell of are left out.
    pub fn held(grid: &Grid, fb: &FaceBrush, level: u32) -> Vec<(BrickKey, Mask)> {
        let (lo, hi) = brush_box(grid, fb, level);
        let footprint = Footprint::new(grid, fb);
        let stacks = box_stacks(lo, hi);
        let face = fb.face();
        par_map(&stacks, 1, |stack| {
            stack
                .bricks
                .iter()
                .zip(footprint.held(level, stack))
                .filter(|(_, mask)| *mask != [0; 8])
                .map(|(s, mask)| {
                    (
                        BrickKey {
                            face,
                            level: level as u8,
                            bi: stack.bi,
                            bj: stack.bj,
                            bk: s.bk,
                        },
                        mask,
                    )
                })
                .collect::<Vec<_>>()
        })
        .into_iter()
        .flatten()
        .collect()
    }

    /// Apply a brush to the cells already baked (a brush that stays analytic
    /// under the store: see [`crate::planet`]). Returns the bricks changed.
    pub fn apply_over(&mut self, grid: &Grid, fb: &FaceBrush, op: BrushOp) -> Vec<BrickKey> {
        let face = fb.face();
        let r = fb.extent_cells();
        let b = i64::from(BRICK);
        let ci = i64::from(fb.center[0]) / 2;
        let cj = i64::from(fb.center[1]) / 2;
        let (ci0, ci1) = ((ci - r).div_euclid(b), (ci + r).div_euclid(b));
        let (cj0, cj1) = ((cj - r).div_euclid(b), (cj + r).div_euclid(b));
        let (k0, k1) = (i64::from(fb.k_lo) / 2 - 1, i64::from(fb.k_hi) / 2 + 1);
        let in_range = |c: &ColumnKey| {
            c.face == face
                && c.level == 0
                && (ci0..=ci1).contains(&i64::from(c.ci))
                && (cj0..=cj1).contains(&i64::from(c.cj))
        };
        // The stored columns in the brush's rectangle: looked up when the
        // rectangle is smaller than the store, else found by a scan.
        let stored: usize = self.shards.iter().map(|s| s.len()).sum();
        let area = (ci1 - ci0 + 1).saturating_mul(cj1 - cj0 + 1);
        let columns: Vec<ColumnKey> = if (area as u64) < stored as u64 {
            (ci0..=ci1)
                .flat_map(|i| {
                    (cj0..=cj1).map(move |j| ColumnKey {
                        face,
                        level: 0,
                        ci: i as i32,
                        cj: j as i32,
                    })
                })
                .filter(|c| self.column(c).is_some())
                .collect()
        } else {
            self.shards
                .iter()
                .flat_map(|s| s.keys().copied())
                .filter(in_range)
                .collect()
        };
        // Every cell of the stored bricks in the band, culled with the
        // brush's own bounds; whole bricks it holds need no cell tests (a
        // large brush holds or misses most of the bricks in its rectangle).
        let stacks: Vec<Stack> = columns
            .iter()
            .filter_map(|column| {
                let bricks: Vec<StackBrick> = self
                    .column(column)?
                    .iter()
                    .filter(|(bk, _)| i64::from(*bk) * b <= k1 && i64::from(*bk) * b + b > k0)
                    .map(|&(bk, _)| StackBrick {
                        bk,
                        z: [0, b],
                        cull: true,
                    })
                    .collect();
                (!bricks.is_empty()).then_some(Stack {
                    bi: column.ci,
                    bj: column.cj,
                    x: [0, b],
                    y: [0, b],
                    bricks,
                })
            })
            .collect();
        self.bake_stacks(&Footprint::new(grid, fb), 0, &stacks, op)
    }

    /// Bake a large brush at `level` directly (its bake level: a level whose
    /// cells it spans at most `BAKE_RADIUS_HALF` half cells of; coarser ones
    /// follow by majority). Each cell of `level` whose centre it holds takes
    /// the brush after its baked edit. Returns the bricks it changed.
    pub fn bake_at(
        &mut self,
        grid: &Grid,
        fb: &FaceBrush,
        op: BrushOp,
        level: u32,
    ) -> Vec<BrickKey> {
        let (lo, hi) = brush_box(grid, fb, level);
        self.bake_box(grid, fb, op, level, lo, hi)
    }

    /// Bake `fb` into every cell of `level` inside the inclusive cell box
    /// `[lo, hi]` whose centre it holds. Returns the bricks it changed.
    #[allow(clippy::too_many_arguments)]
    fn bake_box(
        &mut self,
        grid: &Grid,
        fb: &FaceBrush,
        op: BrushOp,
        level: u32,
        lo: [i64; 3],
        hi: [i64; 3],
    ) -> Vec<BrickKey> {
        self.bake_stacks(&Footprint::new(grid, fb), level, &box_stacks(lo, hi), op)
    }

    /// Apply a brush to the cells of each stack it holds (the stacks built
    /// side by side); bricks it holds no cell of are left alone. Returns the
    /// bricks it reached.
    fn bake_stacks(
        &mut self,
        footprint: &Footprint,
        level: u32,
        stacks: &[Stack],
        op: BrushOp,
    ) -> Vec<BrickKey> {
        let material = footprint.fb.material();
        let face = footprint.fb.face();
        let store = &*self;
        let built = par_map(stacks, 1, |stack| {
            let column = ColumnKey {
                face,
                level: level as u8,
                ci: stack.bi,
                cj: stack.bj,
            };
            let stored = store.column(&column);
            let mut bricks = Vec::new();
            for (s, mask) in stack.bricks.iter().zip(footprint.held(level, stack)) {
                if mask == [0; 8] {
                    continue;
                }
                let old = stored.and_then(|c| {
                    c.binary_search_by_key(&s.bk, |(bk, _)| *bk)
                        .ok()
                        .map(|at| &c[at].1)
                });
                let mut brick = old.map_or_else(Brick::default, |b| (**b).clone());
                for (z, &bits) in mask.iter().enumerate() {
                    let mut bits = bits;
                    while bits != 0 {
                        let cell = &mut brick.cells[z * 64 + bits.trailing_zeros() as usize];
                        bits &= bits - 1;
                        *cell = cell.then(op, material);
                    }
                }
                bricks.push((s.bk, (!brick.is_unchanged()).then(|| Arc::new(brick))));
            }
            (column, bricks)
        });
        let mut changed = Vec::new();
        for (column, bricks) in built {
            if bricks.is_empty() {
                continue;
            }
            changed.extend(bricks.iter().map(|(bk, _)| BrickKey {
                face,
                level: level as u8,
                bi: column.ci,
                bj: column.cj,
                bk: *bk,
            }));
            self.set_column(column, bricks);
        }
        changed
    }

    /// Rebuild the coarser levels above changed bricks (of any level), up to
    /// `levels` (exclusive). A coarse cell whose children are mostly
    /// unchanged keeps its own edit: a large brush baked at its level
    /// directly. Returns every brick of every level that changed.
    pub fn rebuild_coarse(&mut self, changed: Vec<BrickKey>, levels: u32) -> Vec<BrickKey> {
        let levels = levels.max(1) as usize;
        // Bricks that changed, by level: each level's are built after the
        // level under it, so a coarse brick reads its children's new cells.
        let mut changed_at: Vec<Vec<BrickKey>> = vec![Vec::new(); levels];
        for key in changed.iter() {
            if let Some(at) = changed_at.get_mut(usize::from(key.level)) {
                at.push(*key);
            }
        }
        let mut all = changed;
        let mut current = std::mem::take(&mut changed_at[0]);
        for level in 1..levels {
            let mut targets: Vec<BrickKey> = current.iter().map(|k| k.parent()).collect();
            targets.sort_unstable();
            targets.dedup();
            // Bricks of one level are independent: built side by side.
            let store = &*self;
            let built = par_map(&targets, COARSE_PER_TASK, |&key| {
                let brick = store.coarsen(key);
                let same = store
                    .brick(&key)
                    .map_or(brick.is_unchanged(), |old| **old == brick);
                (key, (!same).then_some(brick))
            });
            let mut next = std::mem::take(&mut changed_at[level]);
            for (key, brick) in built {
                if let Some(brick) = brick {
                    self.set(key, Some(brick));
                    next.push(key);
                    all.push(key);
                }
            }
            if next.is_empty() && changed_at[level..].iter().all(Vec::is_empty) {
                break;
            }
            current = next;
        }
        all
    }

    /// A coarse brick from the eight finer bricks under it.
    fn coarsen(&self, key: BrickKey) -> Brick {
        let mut out = Brick::default();
        let own = self.brick(&key);
        // Child brick `c` (bits x, y, z) holds the children of the coarse
        // cells whose coordinates have bit 2 set as `c` has.
        let children: [Option<&Arc<Brick>>; 8] = std::array::from_fn(|c| {
            let at = |v: i32, bit: usize| v * 2 + ((c >> bit) & 1) as i32;
            self.brick(&BrickKey {
                face: key.face,
                level: key.level - 1,
                bi: at(key.bi, 0),
                bj: at(key.bj, 1),
                bk: at(key.bk, 2),
            })
        });
        for z in 0..BRICK {
            for y in 0..BRICK {
                for x in 0..BRICK {
                    let index = Brick::index(x, y, z);
                    let Some(brick) = children[(x / 4 + 2 * (y / 4) + 4 * (z / 4)) as usize] else {
                        // No edited child: half or more are unchanged.
                        out.cells[index] = own.map_or(CellEdit::UNCHANGED, |b| b.cells[index]);
                        continue;
                    };
                    let cell = std::array::from_fn(|c| {
                        let (dx, dy, dz) = ((c & 1) as i32, ((c >> 1) & 1) as i32, (c >> 2) as i32);
                        brick.cells[Brick::index(x * 2 + dx, y * 2 + dy, z * 2 + dz)]
                    });
                    // Children only gain edits (undo restores bricks), so a
                    // majority of unchanged children means the cell's own
                    // edit came at this level.
                    let majority = CellEdit::majority(cell);
                    out.cells[index] = if majority.is_unchanged() {
                        own.map_or(CellEdit::UNCHANGED, |b| b.cells[index])
                    } else {
                        majority
                    };
                }
            }
        }
        out
    }

    /// One above the highest baked solid cell of level column (i, j), if
    /// any.
    pub fn solid_top(&self, face: u8, level: u32, i: i32, j: i32) -> Option<i32> {
        let key = ColumnKey {
            face,
            level: level as u8,
            ci: i.div_euclid(BRICK),
            cj: j.div_euclid(BRICK),
        };
        self.column(&key)?.iter().rev().find_map(|(bk, brick)| {
            (0..BRICK)
                .rev()
                .find(|&z| brick.cells[Brick::index(i, j, z)].state() == CellEdit::SOLID)
                .map(|z| bk * BRICK + z + 1)
        })
    }
}

/// The inclusive box of `level` cells brush `fb` may hold (clipped to its
/// face): the cells a bake at that level visits.
fn brush_box(grid: &Grid, fb: &FaceBrush, level: u32) -> ([i64; 3], [i64; 3]) {
    // At the base level its extent; above, one cell more for rounding.
    let r = if level == 0 {
        fb.extent_cells()
    } else {
        (fb.extent_cells() >> level) + 1
    };
    let ci = (i64::from(fb.center[0]) / 2) >> level;
    let cj = (i64::from(fb.center[1]) / 2) >> level;
    let (k0, k1) = (
        (i64::from(fb.k_lo) / 2 >> level) - 1,
        (i64::from(fb.k_hi) / 2 >> level) + 1,
    );
    let ([i0, i1], [j0, j1]) = (
        clamp_to_face(grid, level, ci - r, ci + r),
        clamp_to_face(grid, level, cj - r, cj + r),
    );
    ([i0, j0, k0], [i1, j1, k1])
}

/// The brick stacks of the inclusive cell box `[lo, hi]`, each clipped to
/// the box.
fn box_stacks(lo: [i64; 3], hi: [i64; 3]) -> Vec<Stack> {
    let b = i64::from(BRICK);
    // The box's cells of brick coordinate `v` on `axis`, in brick cells.
    let clip = |axis: usize, v: i64| {
        [
            (lo[axis] - v * b).clamp(0, b),
            (hi[axis] + 1 - v * b).clamp(0, b),
        ]
    };
    let (blo, bhi) = (lo.map(|v| v.div_euclid(b)), hi.map(|v| v.div_euclid(b)));
    let mut stacks = Vec::new();
    for bj in blo[1]..=bhi[1] {
        for bi in blo[0]..=bhi[0] {
            let bricks: Vec<StackBrick> = (blo[2]..=bhi[2])
                .map(|bk| StackBrick {
                    bk: bk as i32,
                    z: clip(2, bk),
                    cull: false,
                })
                .filter(|s| s.z[0] < s.z[1])
                .collect();
            let (x, y) = (clip(0, bi), clip(1, bj));
            if x[0] < x[1] && y[0] < y[1] && !bricks.is_empty() {
                stacks.push(Stack {
                    bi: bi as i32,
                    bj: bj as i32,
                    x,
                    y,
                    bricks,
                });
            }
        }
    }
    stacks
}

/// The inclusive cell range `[lo, hi]` of `level` clipped to the face (not
/// on an infinite plane).
fn clamp_to_face(grid: &Grid, level: u32, lo: i64, hi: i64) -> [i64; 2] {
    if grid.shape() == crate::grid::Shape::InfinitePlane {
        return [lo, hi];
    }
    [lo.max(0), hi.min(i64::from(grid.cells() >> level) - 1)]
}

/// A brick's cells a brush holds, one bit per cell: word `z`, bit `x + 8 y`
/// (the cell's brick index is `64 z` plus the bit).
pub type Mask = [u64; 8];

/// One brick column's share of a brush's cells: its cell columns `x`, `y`
/// in `[from, to)` (brick cells) and the bricks of the column to fill.
struct Stack {
    bi: i32,
    bj: i32,
    x: [i64; 2],
    y: [i64; 2],
    bricks: Vec<StackBrick>,
}

/// A brick of a [`Stack`]: its height, its cell rows `z` in `[from, to)`,
/// and whether the brush's own bounds cull it first ([`meets`]).
struct StackBrick {
    bk: i32,
    z: [i64; 2],
    cull: bool,
}

/// Volume units a ball's distance to a cell may be off along the line of
/// its column ([`VolumePoints::line`]: under 2 per component) and the f64
/// roots: within it of the radius, cells take the exact test.
const COLUMN_MARGIN: f64 = 6.0;

/// How a brush meets one cell column.
enum ColumnTest {
    /// It holds none of the column's cells.
    Empty,
    /// The exact test, cell by cell (boxes).
    Exact,
    /// A ball: cell heights (half base layers) in `inner` are certainly
    /// inside it, outside `outer` certainly not; between (everywhere past
    /// the line's bound), the exact test over the column's domain point.
    Ball {
        point: IVec3,
        inner: [f64; 2],
        outer: [f64; 2],
    },
}

/// Which cells of a face's bricks a brush holds, exactly as
/// [`FaceBrush::contains`] decides: column by column, a ball's distance
/// along the column's line in f64 for all but the cells near its surface,
/// which take the exact test, each column's domain point computed once.
struct Footprint<'a> {
    fb: &'a FaceBrush,
    grid: &'a Grid,
    map: VolumeMap,
    points: VolumePoints,
}

impl<'a> Footprint<'a> {
    fn new(grid: &'a Grid, fb: &'a FaceBrush) -> Self {
        Self {
            fb,
            grid,
            map: grid.volume_map(),
            points: grid.volume_points(),
        }
    }

    /// The cells of each brick of `stack` (cells of `level`) the brush
    /// holds.
    fn held(&self, level: u32, stack: &Stack) -> Vec<Mask> {
        let mut masks = vec![[0u64; 8]; stack.bricks.len()];
        let row = (stack.y[0]..stack.y[1]).fold(0u64, |row, y| {
            row | ((1u64 << (stack.x[1] - stack.x[0])) - 1) << (stack.x[0] + 8 * y)
        });
        // Bricks to test cell by cell, and their cells' lowest and highest.
        let mut tested = vec![true; stack.bricks.len()];
        let mut span: Option<[i32; 2]> = None;
        for ((mask, s), tested) in masks.iter_mut().zip(&stack.bricks).zip(&mut tested) {
            if s.cull {
                let cube = Cube::brick([stack.bi.into(), stack.bj.into(), s.bk.into()], level);
                match meets(self.fb, cube, self.grid, &self.map) {
                    Meets::Outside => *tested = false,
                    Meets::Contains => {
                        mask[s.z[0] as usize..s.z[1] as usize].fill(row);
                        *tested = false;
                    }
                    Meets::Crosses => {}
                }
            }
            if *tested {
                let (lo, hi) = (
                    s.bk * BRICK + s.z[0] as i32,
                    s.bk * BRICK + s.z[1] as i32 - 1,
                );
                span = Some(span.map_or([lo, hi], |[a, b]| [a.min(lo), b.max(hi)]));
            }
        }
        let Some(span) = span else { return masks };
        let face = self.fb.face();
        for y in stack.y[0]..stack.y[1] {
            for x in stack.x[0]..stack.x[1] {
                let (i, j) = (stack.bi * BRICK + x as i32, stack.bj * BRICK + y as i32);
                let test = self.column(face, i, j, level, span);
                if matches!(test, ColumnTest::Empty) {
                    continue;
                }
                let bit = 1u64 << (x + 8 * y);
                for ((mask, s), _) in masks
                    .iter_mut()
                    .zip(&stack.bricks)
                    .zip(&tested)
                    .filter(|(_, tested)| **tested)
                {
                    for z in s.z[0]..s.z[1] {
                        if self.holds(&test, face, i, j, s.bk * BRICK + z as i32, level) {
                            mask[z as usize] |= bit;
                        }
                    }
                }
            }
        }
        masks
    }

    /// Whether the brush holds level cell `(i, j, k)`, under its column's
    /// test.
    #[inline]
    fn holds(&self, test: &ColumnTest, face: u8, i: i32, j: i32, k: i32, level: u32) -> bool {
        let centre = [
            center_half(i, level),
            center_half(j, level),
            center_half(k, level),
        ];
        match *test {
            ColumnTest::Empty => false,
            ColumnTest::Exact => self
                .fb
                .contains(centre, || self.points.point(face, i, j, k, level)),
            ColumnTest::Ball {
                point,
                inner,
                outer,
            } => {
                let h = f64::from(centre[2]);
                if inner[0] <= h && h <= inner[1] {
                    true
                } else if h < outer[0] || h > outer[1] {
                    false
                } else {
                    self.fb.contains(centre, || self.points.at(point, k, level))
                }
            }
        }
    }

    /// How the brush meets level cell column `(i, j)` over its cells
    /// `span` (inclusive).
    fn column(&self, face: u8, i: i32, j: i32, level: u32, span: [i32; 2]) -> ColumnTest {
        if self.fb.shape() != SHAPE_BALL {
            return ColumnTest::Exact;
        }
        let point = self.points.column(face, i, j, level);
        let (base, rise, limit) = self.points.line(point);
        // Heights of the span's ends, unwrapped: within the bound (and far
        // from wrapping), every height between lies on the line.
        let reach =
            span.map(|k| ((i64::from(k) << (level + 1)) + (1i64 << level)).unsigned_abs() as f64);
        let a = rise.length_squared();
        if reach[0].max(reach[1]) > limit.min(2f64.powi(30)) || a == 0.0 {
            return ColumnTest::Ball {
                point,
                inner: [f64::INFINITY, f64::NEG_INFINITY],
                outer: [f64::NEG_INFINITY, f64::INFINITY],
            };
        }
        // Heights where the line is within `radius` of the centre: the
        // roots of |d + rise h|^2 = radius^2.
        let ball = self.fb.ball;
        let d = base - DVec3::new(f64::from(ball[0]), f64::from(ball[1]), f64::from(ball[2]));
        let (b, c) = (d.dot(rise), d.length_squared());
        let within = |radius: f64| {
            let disc = b * b - a * (c - radius * radius);
            (radius > 0.0 && disc >= 0.0).then(|| {
                let s = disc.sqrt();
                [(-b - s) / a, (-b + s) / a]
            })
        };
        let r = f64::from(ball[3].unsigned_abs());
        match within(r + COLUMN_MARGIN) {
            None => ColumnTest::Empty,
            Some(outer) => ColumnTest::Ball {
                point,
                inner: within(r - COLUMN_MARGIN).unwrap_or([f64::INFINITY, f64::NEG_INFINITY]),
                outer,
            },
        }
    }
}

/// Coarse bricks a [`EditStore::rebuild_coarse`] task builds, at least.
const COARSE_PER_TASK: usize = 4;

/// `f` of every item in order, on the thread pool in runs of at least
/// `per_task` items.
fn par_map<T: Sync, R: Send>(
    items: &[T],
    per_task: usize,
    f: impl Fn(&T) -> R + Sync + Send,
) -> Vec<R> {
    use rayon::prelude::*;
    if items.len() <= per_task {
        return items.iter().map(f).collect();
    }
    items.par_iter().with_min_len(per_task).map(f).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_edits_compose_like_brushes_applied_in_order() {
        use BrushOp::*;
        // Every sequence of up to three brushes over air and solid terrain:
        // the baked edit applied to the terrain equals the brushes replayed.
        let ops = [(Remove, 0), (Add, 7), (Add, 9), (Paint, 4), (Paint, 5)];
        let replay = |seq: &[(BrushOp, u32)], kind: u32| {
            let (mut k, mut m) = (kind, 0);
            for &(op, mat) in seq {
                match op {
                    Remove => (k, m) = (0, 0),
                    Add => (k, m) = (1, mat),
                    Paint => {
                        if k == 1 {
                            m = mat
                        }
                    }
                }
            }
            (k, m)
        };
        let mut seqs: Vec<Vec<(BrushOp, u32)>> = vec![vec![]];
        for _ in 0..3 {
            let mut next = Vec::new();
            for s in &seqs {
                for &o in &ops {
                    let mut t = s.clone();
                    t.push(o);
                    next.push(t);
                }
            }
            seqs.extend(next);
        }
        for seq in &seqs {
            let edit = seq
                .iter()
                .fold(CellEdit::UNCHANGED, |e, &(op, m)| e.then(op, m));
            for kind in [0, 1] {
                assert_eq!(
                    edit.apply(kind, 0),
                    replay(seq, kind),
                    "{seq:?} over kind {kind}"
                );
            }
            // Splitting the sequence anywhere and composing the halves agrees.
            for cut in 0..=seq.len() {
                let a = seq[..cut]
                    .iter()
                    .fold(CellEdit::UNCHANGED, |e, &(op, m)| e.then(op, m));
                let b = seq[cut..]
                    .iter()
                    .fold(CellEdit::UNCHANGED, |e, &(op, m)| e.then(op, m));
                assert_eq!(a.then_edit(b), edit, "{seq:?} cut at {cut}");
            }
        }
    }

    #[test]
    fn coarse_cells_take_the_majority() {
        let u = CellEdit::UNCHANGED;
        let a = CellEdit::AIR;
        let s = CellEdit::solid(3);
        assert_eq!(
            CellEdit::majority([a, a, a, a, u, u, u, u]),
            u,
            "half unchanged stays unchanged"
        );
        assert_eq!(CellEdit::majority([a, a, a, a, a, u, u, u]), a);
        assert_eq!(
            CellEdit::majority([a, a, a, s, s, s, u, u]),
            a,
            "air wins ties"
        );
        assert_eq!(
            CellEdit::majority([s, s, s, s, s, CellEdit::solid(4), u, u]),
            s
        );
    }

    /// Cell by cell over the box a bake visits, with the containment test
    /// generation uses: what [`EditStore::bake`] (`base`) and
    /// [`EditStore::bake_at`] must produce.
    fn bake_cell_by_cell(
        store: &mut EditStore,
        grid: &Grid,
        fb: &FaceBrush,
        op: BrushOp,
        level: u32,
        base: bool,
    ) -> Vec<BrickKey> {
        let face = fb.face();
        let r = if base {
            fb.extent_cells()
        } else {
            (fb.extent_cells() >> level) + 1
        };
        let ci = (i64::from(fb.center[0]) / 2) >> level;
        let cj = (i64::from(fb.center[1]) / 2) >> level;
        let n = i64::from(grid.cells() >> level);
        let clamp = |lo: i64, hi: i64| {
            if grid.shape() == crate::grid::Shape::InfinitePlane {
                (lo, hi)
            } else {
                (lo.max(0), hi.min(n - 1))
            }
        };
        let (i0, i1) = clamp(ci - r, ci + r);
        let (j0, j1) = clamp(cj - r, cj + r);
        let (k0, k1) = (
            (i64::from(fb.k_lo) / 2 >> level) - 1,
            (i64::from(fb.k_hi) / 2 >> level) + 1,
        );
        let mut touched: FxHashMap<BrickKey, Brick> = FxHashMap::default();
        for k in k0..=k1 {
            for j in j0..=j1 {
                for i in i0..=i1 {
                    let (i, j, k) = (i as i32, j as i32, k as i32);
                    let centre = [
                        center_half(i, level),
                        center_half(j, level),
                        center_half(k, level),
                    ];
                    if !fb.contains(centre, || grid.volume_point(face, i, j, k, level)) {
                        continue;
                    }
                    let key = BrickKey::of_cell(face, level, i, j, k);
                    let brick = touched.entry(key).or_insert_with(|| {
                        store
                            .brick(&key)
                            .map_or_else(Brick::default, |b| (**b).clone())
                    });
                    let cell = &mut brick.cells[Brick::index(i, j, k)];
                    *cell = cell.then(op, fb.material());
                }
            }
        }
        let keys = touched.keys().copied().collect();
        for (key, brick) in touched {
            store.set(key, Some(brick));
        }
        keys
    }

    /// Cell by cell over the stored base bricks in reach the brush's bounds
    /// do not cull: what [`EditStore::apply_over`] must produce.
    fn apply_over_cell_by_cell(
        store: &mut EditStore,
        grid: &Grid,
        fb: &FaceBrush,
        op: BrushOp,
    ) -> Vec<BrickKey> {
        let face = fb.face();
        let (r, b) = (fb.extent_cells(), i64::from(BRICK));
        let (ci, cj) = (i64::from(fb.center[0]) / 2, i64::from(fb.center[1]) / 2);
        let (k0, k1) = (i64::from(fb.k_lo) / 2 - 1, i64::from(fb.k_hi) / 2 + 1);
        let map = grid.volume_map();
        let keys: Vec<BrickKey> = store
            .bricks()
            .map(|(key, _)| key)
            .filter(|key| key.face == face && key.level == 0)
            .filter(|key| {
                ((ci - r).div_euclid(b)..=(ci + r).div_euclid(b)).contains(&i64::from(key.bi))
            })
            .filter(|key| {
                ((cj - r).div_euclid(b)..=(cj + r).div_euclid(b)).contains(&i64::from(key.bj))
            })
            .filter(|key| i64::from(key.bk) * b <= k1 && i64::from(key.bk) * b + b > k0)
            .collect();
        let mut changed = Vec::new();
        for key in keys {
            let mut brick = (**store.brick(&key).unwrap()).clone();
            let whole = meets(
                fb,
                Cube::brick([key.bi.into(), key.bj.into(), key.bk.into()], 0),
                grid,
                &map,
            );
            let mut any = false;
            for z in 0..BRICK {
                for y in 0..BRICK {
                    for x in 0..BRICK {
                        let (i, j, k) =
                            (key.bi * BRICK + x, key.bj * BRICK + y, key.bk * BRICK + z);
                        let centre = [center_half(i, 0), center_half(j, 0), center_half(k, 0)];
                        let held = match whole {
                            Meets::Outside => false,
                            Meets::Contains => true,
                            Meets::Crosses => {
                                fb.contains(centre, || grid.volume_point(face, i, j, k, 0))
                            }
                        };
                        if held {
                            let cell = &mut brick.cells[Brick::index(i, j, k)];
                            *cell = cell.then(op, fb.material());
                            any = true;
                        }
                    }
                }
            }
            if any {
                store.set(key, Some(brick));
                changed.push(key);
            }
        }
        changed
    }

    fn sorted(store: &EditStore) -> Vec<(BrickKey, Brick)> {
        let mut bricks: Vec<_> = store
            .bricks()
            .map(|(key, brick)| (key, (**brick).clone()))
            .collect();
        bricks.sort_unstable_by_key(|(key, _)| *key);
        bricks
    }

    /// Bakes by column lines and whole bricks change exactly the cells the
    /// containment test holds, cell by cell: balls of every size (baked at
    /// the base level, at their bake level and over baked cells), boxes,
    /// on a sphere, at its face edges and on planes.
    #[test]
    fn bakes_match_the_cell_by_cell_containment_test() {
        use crate::edits::{Brush, BrushShape};
        use crate::grid::Shape;
        let grids = [
            Grid::new(6_371_000.0, 0.1).unwrap(),
            Grid::new(1_737_000.0, 0.5).unwrap(),
            Grid::plane(Shape::Plane, 4096.0, 0.1).unwrap(),
            Grid::plane(Shape::InfinitePlane, 0.0, 0.25).unwrap(),
        ];
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f64 / (1u64 << 53) as f64
        };
        for (g, grid) in grids.iter().enumerate() {
            let n = f64::from(grid.cells());
            let s = grid.voxel_size();
            let faces = grid.faces();
            let mut fast = EditStore::default();
            let mut exact = EditStore::default();
            for brush_index in 0..24 {
                // Around one spot (later brushes reach earlier bakes), half
                // the time at a face edge or corner.
                let face = faces[brush_index % faces.len().min(2)];
                let edge = brush_index % 4 < 2;
                let spot = |v: f64| {
                    if edge {
                        v * 6.0 - 3.0
                    } else {
                        n * 0.37 + v * 40.0
                    }
                };
                let (fi, fj) = (
                    spot(next()),
                    if brush_index % 4 == 0 {
                        spot(next())
                    } else {
                        n * 0.37 + next() * 40.0
                    },
                );
                let centre = grid.position(face, [fi, fj, next() * 20.0 - 10.0]);
                let radius = [0.05, 0.3, 0.8, 1.6, 3.2, 4.0, 6.0][brush_index % 7]
                    * (0.75 + 0.5 * next())
                    * (s / 0.1).sqrt();
                let shape = if brush_index % 5 == 3 {
                    BrushShape::Cube
                } else {
                    BrushShape::Sphere
                };
                let op = [BrushOp::Remove, BrushOp::Add, BrushOp::Paint][brush_index % 3];
                let height = if shape == BrushShape::Cube && brush_index % 2 == 0 {
                    radius * 0.4
                } else {
                    0.0
                };
                let brush = Brush {
                    center: centre.to_array(),
                    radius,
                    shape,
                    op,
                    material: 1 + brush_index as u32,
                    height,
                };
                let Ok(resolved) = brush.resolve(grid) else {
                    continue;
                };
                for fb in &resolved {
                    let (mut a, mut b) = if fb.radius_half <= crate::planet::BAKE_RADIUS_HALF {
                        (
                            fast.bake(grid, fb, op),
                            bake_cell_by_cell(&mut exact, grid, fb, op, 0, true),
                        )
                    } else {
                        let level = crate::planet::bake_level(fb);
                        let mut a = fast.apply_over(grid, fb, op);
                        let mut b = apply_over_cell_by_cell(&mut exact, grid, fb, op);
                        a.extend(fast.bake_at(grid, fb, op, level));
                        b.extend(bake_cell_by_cell(&mut exact, grid, fb, op, level, false));
                        (a, b)
                    };
                    a.sort_unstable();
                    b.sort_unstable();
                    assert_eq!(
                        a, b,
                        "grid {g}, brush {brush_index} {brush:?}: bricks reached"
                    );
                    assert!(
                        sorted(&fast) == sorted(&exact),
                        "grid {g}, brush {brush_index} {brush:?}: baked cells differ"
                    );
                }
            }
            assert!(
                fast.brick_counts().iter().sum::<usize>() > 0,
                "grid {g} baked nothing"
            );
        }
    }
}
