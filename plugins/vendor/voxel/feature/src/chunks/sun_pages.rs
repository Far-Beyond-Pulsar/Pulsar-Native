//! The sun's occlusion pages (`helio_pass_shadow_matrix::sun_occlusion`):
//! which pages of the virtual clipmap are resident, which to trace next and
//! where each sits in the atlas, as Unreal's virtual shadow maps keep a
//! directional light's pages, planned on the CPU from the pages the frame's
//! pixels asked for (read back a frame or two later; meanwhile a reader
//! takes the nearest coarser level resident).
//!
//! An epoch is the sun the pages were traced for: their grid's axes (right
//! and down across the sun, forward from it) and anchor (planet-centred;
//! texels counted from it, so pages stay put as the eye moves and never
//! re-snap). When the sun has turned [`TURN`] from the current epoch's, a
//! new epoch starts and asks for its own pages; readers take the last
//! epoch's until the new one has every page asked for, then the last is
//! dropped. A page's texels are kept until the terrain under them changes
//! (`invalidate`: generated or dug chunks' boxes and moved volumes,
//! projected along the sun: traced again at once), the cut it was traced
//! through refines (traced again in the background once the world settles:
//! a still view's breathing cut never churns), or
//! nothing asks for it and its physical page is needed.

use glam::DVec3;
use helio_pass_shadow_matrix::{
    sun_occlusion_entry, sun_occlusion_entry_physical, sun_occlusion_table_index,
    SunOcclusionEpoch, SunOcclusionFrame, SunOcclusionLevel, SUN_OCCLUSION_ATLAS_PAGES,
    SUN_OCCLUSION_LEVELS, SUN_OCCLUSION_PAGE, SUN_OCCLUSION_PHYSICAL, SUN_OCCLUSION_TABLE_WORDS,
    SUN_OCCLUSION_WINDOW,
};
use rustc_hash::{FxHashMap, FxHashSet};

pub(super) const LEVELS: usize = SUN_OCCLUSION_LEVELS;
const PAGE: i32 = SUN_OCCLUSION_PAGE as i32;
const PAGE_SHIFT: u32 = SUN_OCCLUSION_PAGE.trailing_zeros();
const WINDOW: i32 = SUN_OCCLUSION_WINDOW as i32;
/// Pages traced a frame at most: 48 of 64^2 texels in one view, fewer rays
/// than the one 512^2 level a frame the clipmap cost before it was paged
/// (pixels whose pages are not there yet read a coarser level meanwhile).
pub(super) const TRACE_PAGES: usize = 48;
/// A trace view's side in pages (views are aligned to these blocks).
pub(super) const VIEW_PAGES: i32 = 8;
const VIEW_SHIFT: u32 = VIEW_PAGES.trailing_zeros();
/// Trace views a frame at most (each culls the drawn chunks and seeds its
/// rays: the per-view cost), and of those, views of background work (pages
/// next to fresh requests, pages of a coarser cut).
pub(super) const MAX_VIEWS: usize = 1;
const BACKGROUND_VIEWS: usize = 1;
/// Frames a page stays asked for after the last request naming it.
const ASKED_FRAMES: u64 = 30;
/// Frames since a page was last asked for before its physical page may go
/// to another.
const KEEP_FRAMES: u64 = 8;
/// The sun's turn (rad) that starts a new epoch once the last is done
/// (shadows that much off their direction, as a level of the old clipmap
/// was let lag), and the turn past which the last is dropped at once.
const TURN: f64 = 0.002;
const JUMP: f64 = 0.05;
/// Frames a new epoch may take to replace the last before it is dropped
/// anyway (pages the atlas cannot hold never complete it).
const HANDOVER_FRAMES: u64 = 120;

/// Why a resident page is traced again: the terrain under it changed (at
/// once), or the cut it was traced through has refined since (in the
/// background, once the world has settled).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Stale {
    No,
    Coarse,
    Changed,
}

/// A page of a level: its coordinates in the level's pages from the
/// epoch's anchor.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(super) struct PageKey {
    pub level: u8,
    pub page: [i32; 2],
}

/// An epoch's grid: the sun its pages were traced for (`sun`, toward it),
/// their axes (right, down, forward from the sun) and anchor; its table
/// slot. What the render thread needs of an epoch to trace and read its
/// pages (a copy comes with each [`Planned`]).
#[derive(Clone, Copy, Debug)]
pub(super) struct EpochGrid {
    pub id: u64,
    pub sun: DVec3,
    pub axes: [DVec3; 3],
    pub anchor: DVec3,
    pub slot: usize,
}

impl EpochGrid {
    /// Planet-centred `p` along right and down from the anchor (m).
    pub fn across(&self, p: DVec3) -> [f64; 2] {
        let d = p - self.anchor;
        [d.dot(self.axes[0]), d.dot(self.axes[1])]
    }

    /// The point of the plane through the anchor across the sun at `at`
    /// (m along right and down).
    pub fn point(&self, at: [f64; 2]) -> DVec3 {
        self.anchor + self.axes[0] * at[0] + self.axes[1] * at[1]
    }

    /// `p`'s texel in a level of texels `texel` m wide: whole, fraction.
    fn texel_of(&self, p: DVec3, texel: f64) -> ([i64; 2], [f64; 2]) {
        let c = self.across(p).map(|c| c / texel);
        let whole = c.map(f64::floor);
        (
            [whole[0] as i64, whole[1] as i64],
            [c[0] - whole[0], c[1] - whole[1]],
        )
    }
}

/// An epoch: its grid ([`EpochGrid`]), its resident pages and the pages
/// asked for (the last frame each was asked in).
pub(super) struct Epoch {
    grid: EpochGrid,
    created: u64,
    resident: FxHashMap<PageKey, u16>,
    asked: FxHashMap<PageKey, u64>,
    /// Pages asked for that may want tracing (missing, or stale), kept as
    /// they change: a plan looks at these, not at every page asked.
    todo: FxHashSet<PageKey>,
    /// Pages asked for the first time since the last plan (their
    /// neighbours are prefetched: the view moving onto them).
    newly: Vec<PageKey>,
    /// The latest frame whose requests were read for it.
    heard: Option<u64>,
}

impl Epoch {
    fn new(id: u64, sun: DVec3, anchor: DVec3, slot: usize, frame: u64) -> Self {
        let forward = -sun;
        let hint = if forward.y.abs() < 0.9 {
            DVec3::Y
        } else {
            DVec3::X
        };
        let right = forward.cross(hint).normalize();
        let up = right.cross(forward);
        Self {
            grid: EpochGrid {
                id,
                sun,
                axes: [right, -up, forward],
                anchor,
                slot,
            },
            created: frame,
            resident: FxHashMap::default(),
            asked: FxHashMap::default(),
            todo: FxHashSet::default(),
            newly: Vec::new(),
            heard: None,
        }
    }

    /// Whether every page asked for is resident and its terrain unchanged.
    fn complete(&self, physical: &[Physical]) -> bool {
        self.heard.is_some()
            && self.todo.iter().all(|k| {
                !self.asked.contains_key(k)
                    || self
                        .resident
                        .get(k)
                        .is_some_and(|&p| physical[p as usize].stale != Stale::Changed)
            })
    }
}

impl std::ops::Deref for Epoch {
    type Target = EpochGrid;

    fn deref(&self) -> &EpochGrid {
        &self.grid
    }
}

/// A physical page: the epoch and page it holds, the last frame it was
/// asked for, and whether it is traced again ([`Stale`]). (Its near plane
/// is the render thread's: [`SunPlanner::traced`].)
#[derive(Clone, Copy)]
struct Physical {
    owner: Option<(u64, PageKey)>,
    used: u64,
    stale: Stale,
}

/// A trace view of the current epoch: a level, its top-left page (a block
/// of [`VIEW_PAGES`] pages aligned to it), and the pages to trace in it
/// with the physical pages they go to.
pub(super) struct Window {
    pub level: u8,
    pub origin: [i32; 2],
    pub pages: Vec<(PageKey, u16)>,
}

/// Pages asked for in a frame, as the shadow pass marked them: the epoch
/// and the eye's page in each level (the window's centre) that frame.
#[derive(Clone, Copy)]
pub(super) struct Asking {
    pub epoch: u64,
    pub frame: u64,
    pub eye_pages: [[i32; 2]; LEVELS],
}

/// What the planner did this frame (diagnostics).
#[derive(Clone, Copy, Debug, Default)]
pub struct SunPageStats {
    pub resident: usize,
    pub asked: usize,
    /// Pages asked for and missing, whose terrain changed, traced through a
    /// coarser cut (counted once the world settled), next to fresh
    /// requests.
    pub missing: usize,
    pub stale: usize,
    pub coarse: usize,
    pub prefetch: usize,
    pub traced: usize,
    pub views: usize,
    pub epochs: usize,
    pub out_of_pages: bool,
    /// Render-thread CPU time of the frame's sun occlusion (ms), and by
    /// part: the update's occluder boxes, the requests' readback, handing
    /// the inputs over and taking the plan (never waiting for it), the
    /// views' recording, uploads; and the planner's own time on its thread
    /// for the last plan taken (requests, invalidation, planning: off the
    /// frame).
    pub cpu_ms: f64,
    pub cpu_parts: [f64; 5],
    pub planner_ms: f64,
}

pub(super) struct SunPages {
    /// The finest texel (m).
    pub finest: f64,
    /// The current epoch, then the last one (until replaced).
    pub epochs: Vec<Epoch>,
    physical: Vec<Physical>,
    /// The page tables as uploaded (`sun_occlusion_table_index`), and the
    /// rows changed since the last upload.
    pub table: Vec<u32>,
    pub dirty_rows: FxHashSet<usize>,
    next_epoch: u64,
    pub stats: SunPageStats,
}

impl SunPages {
    /// Pages for a world whose finest voxel is `voxel` m: the finest texel a
    /// power of two near half of it.
    pub fn new(voxel: f64) -> Self {
        Self {
            finest: (voxel * 0.5).log2().floor().exp2(),
            epochs: Vec::new(),
            physical: vec![
                Physical {
                    owner: None,
                    used: 0,
                    stale: Stale::No,
                };
                SUN_OCCLUSION_PHYSICAL as usize
            ],
            table: vec![0; SUN_OCCLUSION_TABLE_WORDS as usize],
            dirty_rows: FxHashSet::default(),
            next_epoch: 0,
            stats: SunPageStats::default(),
        }
    }

    /// The current epoch.
    pub fn current(&self) -> Option<&Epoch> {
        self.epochs.first()
    }

    /// The eye's page in each level of the current epoch.
    #[cfg(test)]
    fn eye_pages(&self, eye: DVec3) -> [[i32; 2]; LEVELS] {
        eye_pages(self.finest, self.current().map(|e| &e.grid), eye)
    }

    /// The table's rows changed since the last call, as runs of rows: the
    /// first word of each and its words.
    pub fn take_rows(&mut self) -> Vec<(usize, Vec<u32>)> {
        let mut rows: Vec<usize> = self.dirty_rows.drain().collect();
        rows.sort_unstable();
        let row = SUN_OCCLUSION_WINDOW as usize;
        let mut runs = Vec::new();
        let mut i = 0;
        while i < rows.len() {
            let mut j = i + 1;
            while j < rows.len() && rows[j] == rows[j - 1] + 1 {
                j += 1;
            }
            let (from, to) = (rows[i] * row, (rows[j - 1] + 1) * row);
            runs.push((from, self.table[from..to].to_vec()));
            i = j;
        }
        runs
    }

    /// The frame's epochs for the sun toward `sun` and the eye at `eye`
    /// (planet-centred): a new one when the sun turned past [`TURN`] (and
    /// the last is done) or [`JUMP`], or the eye left the anchor's texel
    /// range; the last dropped once the current has every page asked for.
    /// Then the pages kept around the eye asked for.
    pub fn begin(&mut self, frame: u64, eye: DVec3, sun: DVec3) {
        let sun = sun.normalize();
        let reach = self.finest * f64::from(1u32 << 30);
        let state = self.current().map(|c| {
            (
                c.sun.dot(sun).clamp(-1.0, 1.0).acos(),
                (eye - c.anchor).length() > reach,
            )
        });
        let start = match state {
            None => true,
            Some((turn, far)) => {
                if turn > JUMP || far {
                    self.drop_last();
                    true
                } else {
                    turn > TURN && self.epochs.len() == 1
                }
            }
        };
        if self.epochs.len() == 2
            && (self.epochs[0].complete(&self.physical)
                || frame > self.epochs[0].created + HANDOVER_FRAMES)
        {
            self.drop_last();
        }
        if start {
            let slot = self.epochs.first().map_or(0, |c| 1 - c.slot);
            let id = self.next_epoch;
            self.next_epoch += 1;
            self.epochs.insert(0, Epoch::new(id, sun, eye, slot, frame));
        }
        // Every level keeps the 2x2 pages nearest the eye resident whether
        // asked or not (80 pages): what readers without requests (GI hits,
        // fog) find around the eye at their own footprint.
        let finest = self.finest;
        let e = &mut self.epochs[0];
        for level in 0..LEVELS {
            let texel = finest * (level as f64).exp2();
            let (t, _) = e.texel_of(eye, texel);
            let page = t.map(|t| (t >> PAGE_SHIFT) as i32);
            // Toward the eye's side of its page (kept inside a view block,
            // the pages a reader near the eye needs went missing at block
            // edges: GI and fog found no level there).
            let side = t.map(|t| {
                if (t & (PAGE as i64 - 1)) < PAGE as i64 / 2 {
                    -1
                } else {
                    1
                }
            });
            for (dx, dy) in [(0, 0), (side[0], 0), (0, side[1]), (side[0], side[1])] {
                let key = PageKey {
                    level: level as u8,
                    page: [page[0] + dx, page[1] + dy],
                };
                if e.asked.insert(key, frame).is_none() {
                    e.todo.insert(key);
                }
                if let Some(&p) = e.resident.get(&key) {
                    self.physical[p as usize].used = frame;
                }
            }
        }
    }

    /// Drops the last epoch: its pages freed, their table entries cleared.
    fn drop_last(&mut self) {
        if self.epochs.len() < 2 {
            return;
        }
        let last = self.epochs.pop().unwrap();
        for (key, &p) in &last.resident {
            self.clear_entry(last.slot, *key, p);
            self.physical[p as usize].owner = None;
        }
    }

    fn clear_entry(&mut self, slot: usize, key: PageKey, physical: u16) {
        let i = sun_occlusion_table_index(slot, key.level as usize, key.page);
        if sun_occlusion_entry_physical(self.table[i], key.page) == Some(u32::from(physical)) {
            self.table[i] = 0;
            self.dirty_rows.insert(i / SUN_OCCLUSION_WINDOW as usize);
        }
    }

    /// The pages a frame's shadow pass asked for (`asking`; `words`, its
    /// bits: a level's window after another, rows of the window).
    pub fn asked(&mut self, asking: &Asking, words: &[u32]) {
        let Some(e) = self.epochs.iter_mut().find(|e| e.id == asking.epoch) else {
            return;
        };
        let per_level = (WINDOW * WINDOW) as usize;
        e.heard = Some(e.heard.map_or(asking.frame, |h| h.max(asking.frame)));
        for (w, &word) in words.iter().enumerate() {
            let mut bits = word;
            while bits != 0 {
                let b = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                let index = w * 32 + b;
                let level = index / per_level;
                if level >= LEVELS {
                    continue;
                }
                let at = index % per_level;
                let eye = asking.eye_pages[level];
                let key = PageKey {
                    level: level as u8,
                    page: [
                        eye[0] + (at as i32 % WINDOW) - WINDOW / 2,
                        eye[1] + (at as i32 / WINDOW) - WINDOW / 2,
                    ],
                };
                match e.asked.entry(key) {
                    std::collections::hash_map::Entry::Occupied(mut f) => {
                        let f = f.get_mut();
                        *f = (*f).max(asking.frame);
                    }
                    std::collections::hash_map::Entry::Vacant(v) => {
                        v.insert(asking.frame);
                        e.newly.push(key);
                        e.todo.insert(key);
                    }
                }
                if let Some(&p) = e.resident.get(&key) {
                    let used = &mut self.physical[p as usize].used;
                    *used = (*used).max(asking.frame);
                }
            }
        }
    }

    /// Pages whose rays cross any of `boxes` (world) traced again: before
    /// others where the terrain changed in them, in the background where
    /// the cut refined (`why`). Each box marks the pages over it
    /// in the levels where it spans at most 2x2 pages and every coarser
    /// one; a finer page is stale when one of its ancestors is marked (the
    /// region marked is at most a page larger than the box at that level).
    /// Hash lookups only: a settle's thousands of boxes a frame against
    /// thousands of pages stay well under a millisecond.
    pub fn invalidate(&mut self, boxes: &[[DVec3; 2]], why: Stale) {
        if boxes.is_empty() {
            return;
        }
        let finest = self.finest;
        let page_m = |level: usize| finest * (level as f64).exp2() * f64::from(PAGE);
        for e in &mut self.epochs {
            if e.resident.is_empty() {
                continue;
            }
            // Only levels holding pages are marked.
            let mut held = [false; LEVELS];
            for k in e.resident.keys() {
                held[usize::from(k.level)] = true;
            }
            let mut marked: FxHashSet<PageKey> = FxHashSet::default();
            // The levels some box's marks start at.
            let mut starts = 0u32;
            for [lo, hi] in boxes {
                // The box's extent across the sun (m from the anchor).
                let mut r = [
                    f64::INFINITY,
                    f64::INFINITY,
                    f64::NEG_INFINITY,
                    f64::NEG_INFINITY,
                ];
                for c in 0..8 {
                    let a = e.across(DVec3::new(
                        if c & 1 == 0 { lo.x } else { hi.x },
                        if c & 2 == 0 { lo.y } else { hi.y },
                        if c & 4 == 0 { lo.z } else { hi.z },
                    ));
                    r = [
                        r[0].min(a[0]),
                        r[1].min(a[1]),
                        r[2].max(a[0]),
                        r[3].max(a[1]),
                    ];
                }
                let span = (r[2] - r[0]).max(r[3] - r[1]).max(0.0);
                let first = (0..LEVELS)
                    .find(|&l| span <= page_m(l))
                    .unwrap_or(LEVELS - 1);
                let Some(start) = (first..LEVELS).find(|&l| held[l]) else {
                    continue;
                };
                starts |= 1 << start;
                for level in (start..LEVELS).filter(|&l| held[l]) {
                    let m = page_m(level);
                    let (x0, y0) = ((r[0] / m).floor() as i32, (r[1] / m).floor() as i32);
                    let (x1, y1) = ((r[2] / m).floor() as i32, (r[3] / m).floor() as i32);
                    for y in y0..=y1.min(y0 + 1) {
                        for x in x0..=x1.min(x0 + 1) {
                            marked.insert(PageKey {
                                level: level as u8,
                                page: [x, y],
                            });
                        }
                    }
                }
            }
            if marked.is_empty() {
                continue;
            }
            // A box marks its first level and every coarser one: a page is
            // under it where the page itself is marked, or its ancestor at
            // a coarser level some box starts at.
            for (key, &p) in &e.resident {
                let k = usize::from(key.level);
                let hit = std::iter::once(k)
                    .chain((k + 1..LEVELS).filter(|&l| starts >> l & 1 == 1))
                    .any(|l| {
                        let up = (l - k) as u32;
                        marked.contains(&PageKey {
                            level: l as u8,
                            page: key.page.map(|c| c >> up),
                        })
                    });
                if hit {
                    let stale = &mut self.physical[p as usize].stale;
                    if why == Stale::Changed || *stale == Stale::No {
                        *stale = why;
                    }
                    if e.asked.contains_key(key) {
                        e.todo.insert(*key);
                    }
                }
            }
        }
    }

    /// The pages to trace this frame, in views, with their physical pages
    /// (allocated, their table entries set): pages asked for and missing
    /// (coarse first: they back the finer ones' readers), pages whose
    /// terrain changed; then in the background ([`BACKGROUND_VIEWS`]) pages
    /// next to fresh requests (the view moving onto them) and, once the
    /// world has `settled`, pages of a coarser cut; at most
    /// [`TRACE_PAGES`] in [`MAX_VIEWS`] views. Physical pages come free, or
    /// from pages not asked for in [`KEEP_FRAMES`] (the last epoch's first,
    /// then the longest unused).
    pub fn plan(&mut self, frame: u64, settled: bool) -> Vec<Window> {
        let mut stats = SunPageStats {
            epochs: self.epochs.len(),
            ..Default::default()
        };
        let Some(e) = self.epochs.first_mut() else {
            self.stats = stats;
            return Vec::new();
        };
        e.asked.retain(|_, f| *f + ASKED_FRAMES >= frame);
        let physical = &self.physical;
        // (class, coarser first, key): 0 missing, 1 changed, 2 next to a
        // fresh request, 3 of a coarser cut.
        let mut wanted: Vec<(u8, std::cmp::Reverse<u8>, PageKey)> = Vec::new();
        let mut near: FxHashSet<PageKey> = FxHashSet::default();
        for &key in &std::mem::take(&mut e.newly) {
            for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                let n = PageKey {
                    level: key.level,
                    page: [key.page[0] + dx, key.page[1] + dy],
                };
                if !e.asked.contains_key(&n) && !e.resident.contains_key(&n) {
                    near.insert(n);
                }
            }
        }
        let (asked, resident) = (&e.asked, &e.resident);
        e.todo.retain(|&key| {
            if !asked.contains_key(&key) {
                return false;
            }
            match resident.get(&key).map(|&p| physical[p as usize].stale) {
                None => {
                    stats.missing += 1;
                    wanted.push((0, std::cmp::Reverse(key.level), key));
                }
                Some(Stale::Changed) => {
                    stats.stale += 1;
                    wanted.push((1, std::cmp::Reverse(key.level), key));
                }
                Some(Stale::Coarse) => {
                    if settled {
                        stats.coarse += 1;
                        wanted.push((3, std::cmp::Reverse(key.level), key));
                    }
                }
                Some(Stale::No) => return false,
            }
            true
        });
        stats.prefetch = near.len();
        wanted.extend(near.into_iter().map(|k| (2, std::cmp::Reverse(k.level), k)));
        // Background work goes by the block with the most of it: a view a
        // frame then carries as many pages as it can.
        let block = |k: &PageKey| (k.level, k.page.map(|p| p >> VIEW_SHIFT));
        let mut in_block: FxHashMap<(u8, [i32; 2]), usize> = FxHashMap::default();
        for (class, _, k) in &wanted {
            if *class >= 2 {
                *in_block.entry(block(k)).or_insert(0) += 1;
            }
        }
        // (Keys made once: a hash lookup a comparison was most of a plan.)
        let mut keyed: Vec<_> = wanted
            .iter()
            .map(|&(class, coarse, k)| {
                let crowd = if class >= 2 { in_block[&block(&k)] } else { 0 };
                (
                    (
                        class,
                        std::cmp::Reverse(crowd),
                        coarse,
                        k.page[1],
                        k.page[0],
                    ),
                    (class, coarse, k),
                )
            })
            .collect();
        stats.asked = e.asked.len();
        // Views: the block of the best page left a view, its pages in
        // order (a selection: sorting every page wanted was most of a plan
        // with thousands missing).
        let mut views: Vec<Window> = Vec::new();
        let (mut taken, mut background) = (0, 0);
        while views.len() < MAX_VIEWS && taken < TRACE_PAGES {
            let Some(&(_, (class, _, key))) = keyed
                .iter()
                .filter(|w| w.1 .0 < 2 || background < BACKGROUND_VIEWS)
                .min_by_key(|w| w.0)
            else {
                break;
            };
            let origin = key.page.map(|p| (p >> VIEW_SHIFT) << VIEW_SHIFT);
            let inside = |k: &PageKey| {
                k.level == key.level && k.page.map(|p| (p >> VIEW_SHIFT) << VIEW_SHIFT) == origin
            };
            let mut pages: Vec<_> = keyed
                .iter()
                .filter(|w| inside(&w.1 .2))
                .map(|w| (w.0, w.1 .2))
                .collect();
            pages.sort_unstable_by_key(|p| p.0);
            pages.truncate(TRACE_PAGES - taken);
            keyed.retain(|w| !inside(&w.1 .2));
            background += usize::from(class >= 2);
            taken += pages.len();
            views.push(Window {
                level: key.level,
                origin,
                pages: pages.into_iter().map(|p| (p.1, u16::MAX)).collect(),
            });
        }
        // Physical pages: those the pages hold, then free ones, then the
        // last epoch's, then the longest unused of the current's.
        let current = e.id;
        // Only as many as the pages need, by a partial selection (a sort of
        // the whole atlas a frame was a third of a plan).
        let needs = views
            .iter()
            .flat_map(|v| &v.pages)
            .filter(|(k, _)| !e.resident.contains_key(k))
            .count();
        let rank = |p: u16| {
            let ph = &physical[p as usize];
            let rank = match ph.owner {
                None => 2,
                Some((id, _)) if id != current => 1,
                _ => 0,
            };
            (rank, std::cmp::Reverse(ph.used))
        };
        let mut spare: Vec<u16> = if needs == 0 {
            Vec::new()
        } else {
            (0..SUN_OCCLUSION_PHYSICAL as u16)
                .filter(|&p| {
                    let ph = &physical[p as usize];
                    ph.owner.is_none_or(|(id, _)| id != current) || ph.used + KEEP_FRAMES < frame
                })
                .collect()
        };
        // Popped from the back: the best last (pages traced again in place
        // may take some of them: room for those too).
        let room = (needs + TRACE_PAGES).min(spare.len());
        if room < spare.len() {
            let cut = spare.len() - room;
            spare.select_nth_unstable_by_key(cut, |&p| rank(p));
            spare.drain(..cut);
        }
        spare.sort_unstable_by_key(|&p| rank(p));
        let last_slot = self.epochs.get(1).map(|l| (l.id, l.slot));
        let mut evicted: Vec<(Option<usize>, PageKey, u16)> = Vec::new();
        let e = &mut self.epochs[0];
        // Pages traced again in place keep theirs.
        let held: FxHashSet<u16> = views
            .iter()
            .flat_map(|v| &v.pages)
            .filter_map(|(key, _)| e.resident.get(key).copied())
            .collect();
        for v in &mut views {
            v.pages.retain_mut(|(key, p)| {
                if let Some(&own) = e.resident.get(key) {
                    *p = own;
                    return true;
                }
                while let Some(s) = spare.pop() {
                    if held.contains(&s) {
                        continue;
                    }
                    *p = s;
                    return true;
                }
                stats.out_of_pages = true;
                false
            });
        }
        views.retain(|v| !v.pages.is_empty());
        // Take the physical pages from their owners.
        for v in &views {
            for &(key, p) in &v.pages {
                if let Some((id, old)) = self.physical[p as usize].owner {
                    if (id, old) == (current, key) {
                        continue;
                    }
                    let slot = if id == current {
                        e.resident.remove(&old);
                        if e.asked.contains_key(&old) {
                            e.todo.insert(old);
                        }
                        Some(e.slot)
                    } else {
                        last_slot.filter(|(l, _)| *l == id).map(|(_, s)| s)
                    };
                    evicted.push((slot, old, p));
                }
            }
        }
        for (slot, old, p) in evicted {
            if let Some(slot) = slot {
                self.clear_entry(slot, old, p);
            }
            if let Some(last) = self.epochs.get_mut(1) {
                if self.physical[p as usize]
                    .owner
                    .is_some_and(|(id, _)| id == last.id)
                {
                    last.resident.remove(&old);
                }
            }
            self.physical[p as usize].owner = None;
        }
        // The views' pages resident, their table entries set: the plan is
        // made ahead of the frame that traces them (`SunPlanner`), whose
        // upload of these rows goes with the trace. (A cut that refines
        // under them later marks them [`Stale::Coarse`]: `invalidate`.)
        let (id, slot) = (self.epochs[0].id, self.epochs[0].slot);
        for v in &views {
            for &(key, p) in &v.pages {
                let e = &mut self.epochs[0];
                e.resident.insert(key, p);
                e.todo.remove(&key);
                // Pages traced ahead of a request are kept as if asked now.
                let used = e.asked.get(&key).copied().unwrap_or(0).max(frame);
                self.physical[p as usize] = Physical {
                    owner: Some((id, key)),
                    used,
                    stale: Stale::No,
                };
                let i = sun_occlusion_table_index(slot, key.level as usize, key.page);
                self.table[i] = sun_occlusion_entry(key.page, u32::from(p));
                self.dirty_rows.insert(i / SUN_OCCLUSION_WINDOW as usize);
            }
        }
        stats.traced = views.iter().map(|v| v.pages.len()).sum();
        stats.views = views.len();
        stats.resident = self.epochs.iter().map(|e| e.resident.len()).sum();
        self.stats = stats;
        views
    }
}

/// Level `level`'s texel (m) of pages whose finest is `finest`.
pub(super) fn texel(finest: f64, level: usize) -> f64 {
    finest * (level as f64).exp2()
}

/// The eye's page in each level of epoch `e` (none: all zero).
pub(super) fn eye_pages(finest: f64, e: Option<&EpochGrid>, eye: DVec3) -> [[i32; 2]; LEVELS] {
    let mut out = [[0; 2]; LEVELS];
    if let Some(e) = e {
        for (level, o) in out.iter_mut().enumerate() {
            let (t, _) = e.texel_of(eye, texel(finest, level));
            *o = t.map(|t| (t >> PAGE_SHIFT) as i32);
        }
    }
    out
}

/// The frame for the eye at `eye` (planet-centred): the epochs, the eye in
/// each level; and each physical page's near plane relative to the eye
/// (`nears`: the epoch it was traced for and its planet-centred distance
/// along that epoch's sun; a point `p` lies `dot(p, forward) - near` past
/// it). A page of an epoch gone reads 0: no table entry points at it.
pub(super) fn frame(
    finest: f64,
    epochs: &[EpochGrid],
    nears: &[(u64, f64)],
    eye: DVec3,
) -> (SunOcclusionFrame, Vec<f32>) {
    let mut frame = SunOcclusionFrame {
        info: [
            finest as f32,
            LEVELS as f32,
            SUN_OCCLUSION_PAGE as f32,
            SUN_OCCLUSION_WINDOW as f32,
        ],
        atlas: [SUN_OCCLUSION_ATLAS_PAGES, SUN_OCCLUSION_TABLE_WORDS, 0, 0],
        ..Default::default()
    };
    for (out, e) in frame.epochs.iter_mut().zip(epochs) {
        let [right, down, forward] = e.axes.map(|a| a.as_vec3());
        *out = SunOcclusionEpoch {
            right: right.extend((e.slot + 1) as f32).to_array(),
            down: down.extend(0.0).to_array(),
            forward: forward.extend(0.0).to_array(),
            levels: std::array::from_fn(|level| {
                let (t, f) = e.texel_of(eye, texel(finest, level));
                SunOcclusionLevel {
                    texel: t.map(|t| t as i32),
                    frac: f.map(|f| f as f32),
                }
            }),
        };
    }
    // Each epoch's sun once, not a search a page.
    let along: Vec<(u64, f64)> = epochs.iter().map(|e| (e.id, eye.dot(e.sun))).collect();
    let near = nears
        .iter()
        .map(|&(id, near)| {
            along
                .iter()
                .find(|a| a.0 == id)
                .map_or(0.0, |a| (a.1 - near) as f32)
        })
        .collect();
    (frame, near)
}

/// What the render thread hands the planner after a frame, for the next:
/// the frame planned for, the eye and the sun toward which, whether the
/// residency has settled, the requests read back (what each was asked
/// against, its words), the boxes whose terrain changed and those drawn
/// finer since.
pub(super) struct PlanJob {
    pub frame: u64,
    pub eye: DVec3,
    pub sun: DVec3,
    pub settled: bool,
    pub asked: Vec<(Asking, Vec<u32>)>,
    pub changes: Vec<[DVec3; 2]>,
    pub refined: Vec<[DVec3; 2]>,
}

/// A frame's plan from the planner's thread: the views to trace (their
/// pages resident and in the table already), the epochs, the table's
/// changed rows (runs: first word, words), the planner's stats and its time
/// (ms) by part (requests, invalidation, planning), and the request words'
/// buffers back for reuse.
pub(super) struct Planned {
    pub windows: Vec<Window>,
    pub epochs: Vec<EpochGrid>,
    pub rows: Vec<(usize, Vec<u32>)>,
    pub stats: SunPageStats,
    pub parts: [f64; 3],
    pub spent: Vec<Vec<u32>>,
}

/// The sun pages' planner on its own thread, a frame ahead: after frame N
/// records, the frame's requests and changed boxes go to it
/// ([`Self::queue`]) and frame N+1 takes the plan they made when it is done
/// ([`Self::try_take`]; the plan takes ~1 ms, the rest of a frame ~10). The
/// frame never waits for it: a plan not done yet is taken by a later frame,
/// and the inputs of the frames in between go to the next plan together
/// (a woken thread on a busy machine can wait a scheduling quantum for a
/// core). A plan is the same function of its inputs as on the render
/// thread, its table rows uploaded with the traces that fill them whichever
/// frame takes it; it sees each frame's requests a frame (or more) later,
/// which the requests' own latency (read back a frame or two late) already
/// covers. The render thread keeps what only it knows: each physical page's
/// near plane, set where it records the trace ([`Self::traced`]).
pub(super) struct SunPlanner {
    pub finest: f64,
    jobs: std::sync::mpsc::Sender<PlanJob>,
    plans: std::sync::Mutex<std::sync::mpsc::Receiver<Planned>>,
    /// Whether a job is on the thread (its plan not taken yet), and the
    /// inputs gathered for the next one while it is.
    in_flight: bool,
    next: Option<PlanJob>,
    /// The epochs of the plan taken last (current first).
    pub epochs: Vec<EpochGrid>,
    /// Each physical page's epoch and near plane, as last traced.
    nears: Vec<(u64, f64)>,
    pub stats: SunPageStats,
    /// Request word buffers to reuse.
    pub spare: Vec<Vec<u32>>,
}

impl SunPlanner {
    /// A planner for a world whose finest voxel is `voxel` m
    /// ([`SunPages::new`]), its thread started.
    pub fn spawn(voxel: f64) -> Self {
        let (jobs, inbox) = std::sync::mpsc::channel::<PlanJob>();
        let (outbox, plans) = std::sync::mpsc::channel();
        let pages = SunPages::new(voxel);
        let finest = pages.finest;
        std::thread::Builder::new()
            .name("sun pages planner".into())
            .spawn(move || {
                let mut pages = pages;
                while let Ok(mut job) = inbox.recv() {
                    let clock = std::time::Instant::now();
                    for (asking, words) in &job.asked {
                        pages.asked(asking, words);
                    }
                    let t0 = clock.elapsed().as_secs_f64() * 1e3;
                    pages.invalidate(&job.changes, Stale::Changed);
                    pages.invalidate(&job.refined, Stale::Coarse);
                    let t1 = clock.elapsed().as_secs_f64() * 1e3;
                    pages.begin(job.frame, job.eye, job.sun);
                    let windows = pages.plan(job.frame, job.settled);
                    let planned = Planned {
                        windows,
                        epochs: pages.epochs.iter().map(|e| e.grid).collect(),
                        rows: pages.take_rows(),
                        stats: pages.stats,
                        parts: [t0, t1 - t0, clock.elapsed().as_secs_f64() * 1e3 - t1],
                        spent: job.asked.drain(..).map(|(_, words)| words).collect(),
                    };
                    if outbox.send(planned).is_err() {
                        break;
                    }
                }
            })
            .expect("the sun pages planner thread starts");
        Self {
            finest,
            jobs,
            plans: std::sync::Mutex::new(plans),
            in_flight: false,
            next: None,
            epochs: Vec::new(),
            nears: vec![(u64::MAX, 0.0); SUN_OCCLUSION_PHYSICAL as usize],
            stats: SunPageStats::default(),
            spare: Vec::new(),
        }
    }

    /// A frame's inputs for the next plan: on the planner's thread at once
    /// when it is free, else with the frames' after it when it is (their
    /// requests and boxes together, the latest frame's eye, sun and
    /// settling).
    pub fn queue(&mut self, job: PlanJob) {
        let next = match self.next.take() {
            Some(mut next) => {
                next.asked.extend(job.asked);
                next.changes.extend(job.changes);
                next.refined.extend(job.refined);
                PlanJob {
                    asked: next.asked,
                    changes: next.changes,
                    refined: next.refined,
                    ..job
                }
            }
            None => job,
        };
        if self.in_flight {
            self.next = Some(next);
        } else {
            self.jobs.send(next).expect("the sun pages planner runs");
            self.in_flight = true;
        }
    }

    /// The plan on its way, if it is done (never waits): its epochs and
    /// stats kept; the inputs gathered since go to the planner.
    pub fn try_take(&mut self) -> Option<Planned> {
        if !self.in_flight {
            return None;
        }
        let mut planned = match self
            .plans
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .try_recv()
        {
            Ok(planned) => planned,
            Err(std::sync::mpsc::TryRecvError::Empty) => return None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                panic!("the sun pages planner stopped")
            }
        };
        self.in_flight = false;
        self.epochs.clone_from(&planned.epochs);
        self.stats = planned.stats;
        self.spare.append(&mut planned.spent);
        if let Some(next) = self.next.take() {
            self.queue(next);
        }
        Some(planned)
    }

    /// The current epoch of the plan taken last.
    pub fn current(&self) -> Option<&EpochGrid> {
        self.epochs.first()
    }

    /// Level `level`'s texel (m).
    pub fn texel(&self, level: usize) -> f64 {
        texel(self.finest, level)
    }

    /// The eye's page in each level of the current epoch.
    pub fn eye_pages(&self, eye: DVec3) -> [[i32; 2]; LEVELS] {
        eye_pages(self.finest, self.current(), eye)
    }

    /// `window`'s pages traced this frame through a near plane `near`
    /// (planet-centred distance along the current epoch's sun).
    pub fn traced(&mut self, window: &Window, near: f64) {
        let id = self.current().map_or(u64::MAX, |e| e.id);
        for &(_, p) in &window.pages {
            self.nears[p as usize] = (id, near);
        }
    }

    /// The frame for the eye at `eye` ([`frame`]).
    pub fn frame(&self, eye: DVec3) -> (SunOcclusionFrame, Vec<f32>) {
        frame(self.finest, &self.epochs, &self.nears, eye)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_asked_are_traced_coarse_first_and_kept() {
        let mut pages = SunPages::new(0.1);
        let eye = DVec3::new(0.0, 1000.0, 0.0);
        let sun = DVec3::new(0.3, 0.8, 0.4).normalize();
        pages.begin(0, eye, sun);
        // The pages around the eye only: coarse first, four a level.
        let views = pages.plan(0, true);
        let traced: usize = views.iter().map(|v| v.pages.len()).sum();
        assert!(traced > 0 && traced <= TRACE_PAGES);
        assert_eq!(usize::from(views[0].level), LEVELS - 1);
        // Planned pages are resident at once (traced by the frame the plan
        // is for), their table entries set.
        let e = pages.current().unwrap();
        for &(key, p) in &views[0].pages {
            assert_eq!(e.resident.get(&key), Some(&p));
            let i = sun_occlusion_table_index(e.slot, usize::from(key.level), key.page);
            assert_eq!(sun_occlusion_entry_physical(pages.table[i], key.page), Some(u32::from(p)));
        }
        assert!(!pages.take_rows().is_empty() && pages.take_rows().is_empty());
        // Everything asked traced within a few frames, then nothing.
        let mut frame = 1;
        loop {
            pages.begin(frame, eye, sun);
            let views = pages.plan(frame, true);
            if views.is_empty() {
                break;
            }
            frame += 1;
            assert!(frame < 160, "the pages around the eye took {frame} frames");
        }
        let asking = Asking {
            epoch: pages.current().unwrap().id,
            frame,
            eye_pages: pages.eye_pages(eye),
        };
        // A request: level 3, ten pages right of the eye's.
        let mut words = vec![0u32; LEVELS * (WINDOW * WINDOW) as usize / 32];
        let bit = 3 * (WINDOW * WINDOW) as usize + (WINDOW / 2 * WINDOW + WINDOW / 2 + 10) as usize;
        words[bit / 32] |= 1 << (bit % 32);
        pages.asked(&asking, &words);
        let views = pages.plan(frame, true);
        assert_eq!(views[0].level, 3);
        let eye_page = asking.eye_pages[3];
        assert_eq!(views[0].pages[0].0.page, [eye_page[0] + 10, eye_page[1]]);
    }

    #[test]
    fn changed_boxes_stale_the_pages_over_them() {
        let mut pages = SunPages::new(0.1);
        let eye = DVec3::new(0.0, 1000.0, 0.0);
        let sun = DVec3::Y;
        pages.begin(0, eye, sun);
        while !pages.plan(0, true).is_empty() {}
        let e = pages.current().unwrap();
        // A 1 m box at the eye's column: the coarse pages over it, nothing
        // far away.
        let near: Vec<PageKey> = e.resident.keys().copied().collect();
        pages.invalidate(
            &[[eye - DVec3::splat(0.5), eye + DVec3::splat(0.5)]],
            Stale::Changed,
        );
        let e = pages.current().unwrap();
        let stale = |k: &PageKey| pages.physical[e.resident[k] as usize].stale == Stale::Changed;
        let over: Vec<&PageKey> = near
            .iter()
            .filter(|k| {
                let m = texel(pages.finest, usize::from(k.level)) * f64::from(PAGE);
                let a = e.across(eye);
                (0..2).all(|i| (a[i] / m).floor() as i32 == k.page[i])
            })
            .collect();
        assert!(!over.is_empty() && over.iter().all(|k| stale(k)));
        assert!(near.iter().filter(|k| stale(k)).count() <= over.len() * 4);
    }

    #[test]
    fn a_turning_sun_hands_over_epochs() {
        let mut pages = SunPages::new(0.1);
        let eye = DVec3::new(0.0, 1000.0, 0.0);
        let mut sun = DVec3::new(0.3, 0.8, 0.4).normalize();
        for frame in 0..400 {
            pages.begin(frame, eye, sun);
            pages.plan(frame, true);
            let asking = Asking {
                epoch: pages.current().unwrap().id,
                frame,
                eye_pages: pages.eye_pages(eye),
            };
            pages.asked(&asking, &[]);
            // voxel_craft's day: a turn of the sun in 600 s, 60 frames a
            // second.
            sun = glam::DQuat::from_rotation_y(std::f64::consts::TAU / 36_000.0).mul_vec3(sun);
            assert!(pages.epochs.len() <= 2);
        }
        // The sun turned 0.07 rad: epochs followed it within a few turns.
        let c = pages.current().unwrap();
        assert!(
            c.sun.dot(sun).acos() < 4.0 * TURN,
            "{}",
            c.sun.dot(sun).acos()
        );
    }

    /// The planner's thread plans exactly what the planner does on the
    /// caller's: the same views, epochs and table rows for the same jobs
    /// (a flight with requests, changed and refined boxes and a turning
    /// sun), and its frame reads the near planes traced.
    #[test]
    fn the_planner_thread_plans_what_the_planner_does() {
        let mut thread = SunPlanner::spawn(0.1);
        let mut local = SunPages::new(0.1);
        let mut sun = DVec3::new(0.3, 0.8, 0.4).normalize();
        let job = |frame: u64, eye: DVec3, sun: DVec3, asking: Option<Asking>| {
            let mut words = vec![0u32; LEVELS * (WINDOW * WINDOW) as usize / 32];
            for level in [2usize, 5, 9] {
                for d in -3i32..=3 {
                    let bit = level * (WINDOW * WINDOW) as usize
                        + ((WINDOW / 2 + d) * WINDOW + WINDOW / 2 + (frame as i32 % 5) - 2) as usize;
                    words[bit / 32] |= 1 << (bit % 32);
                }
            }
            let c = eye - DVec3::Y * 1000.0;
            PlanJob {
                frame,
                eye,
                sun,
                settled: frame > 30,
                asked: asking.into_iter().map(|a| (a, words.clone())).collect(),
                changes: if frame % 7 == 0 { vec![[c, c + DVec3::splat(3.0)]] } else { Vec::new() },
                refined: vec![[c - DVec3::splat(20.0), c + DVec3::splat(6.4)]],
            }
        };
        let mut asking = None;
        for frame in 0..120u64 {
            let eye = DVec3::new(frame as f64 * 3.0, 1000.0, frame as f64 * 1.5);
            let j = job(frame, eye, sun, asking);
            for (a, words) in &j.asked {
                local.asked(a, words);
            }
            local.invalidate(&j.changes, Stale::Changed);
            local.invalidate(&j.refined, Stale::Coarse);
            local.begin(j.frame, j.eye, j.sun);
            let windows = local.plan(j.frame, j.settled);
            let rows = local.take_rows();
            thread.queue(j);
            let planned = loop {
                if let Some(planned) = thread.try_take() {
                    break planned;
                }
                std::thread::yield_now();
            };
            assert_eq!(planned.windows.len(), windows.len(), "frame {frame}");
            for (a, b) in planned.windows.iter().zip(&windows) {
                assert_eq!((a.level, a.origin, &a.pages), (b.level, b.origin, &b.pages));
            }
            assert_eq!(planned.rows, rows, "frame {frame}");
            assert_eq!(
                planned.epochs.iter().map(|e| (e.id, e.slot)).collect::<Vec<_>>(),
                local.epochs.iter().map(|e| (e.id, e.slot)).collect::<Vec<_>>()
            );
            for (n, w) in planned.windows.iter().enumerate() {
                thread.traced(w, 500.0 + n as f64);
            }
            asking = Some(Asking {
                epoch: thread.current().unwrap().id,
                frame,
                eye_pages: thread.eye_pages(eye),
            });
            assert_eq!(asking.unwrap().eye_pages, local.eye_pages(eye));
            sun = glam::DQuat::from_rotation_y(0.0005).mul_vec3(sun);
        }
        // A page traced last frame reads its near plane from the eye.
        let eye = DVec3::new(0.0, 1000.0, 0.0);
        let (_, nears) = thread.frame(eye);
        let e = *thread.current().unwrap();
        let resident = local.current().unwrap().resident.values().copied().collect::<Vec<_>>();
        assert!(resident
            .iter()
            .filter(|&&p| thread.nears[p as usize].0 == e.id)
            .all(|&p| nears[p as usize] == (eye.dot(e.sun) - thread.nears[p as usize].1) as f32));
    }

    /// The planner's CPU in a cruise-like flight (run with --release
    /// --ignored --nocapture): ~3000 pages asked, the atlas full, refined
    /// boxes every frame.
    #[test]
    #[ignore]
    fn planner_cpu_in_a_flight() {
        use std::time::Instant;
        let mut pages = SunPages::new(0.1);
        let sun = DVec3::new(0.3, 0.8, 0.4).normalize();
        let mut parts = [0.0f64; 4];
        let nears = vec![(0u64, 0.0f64); SUN_OCCLUSION_PHYSICAL as usize];
        let frames = 2000;
        for frame in 0..frames {
            let eye = DVec3::new(frame as f64 * 3.0, 1000.0, frame as f64 * 1.5);
            let t = Instant::now();
            pages.begin(frame, eye, sun);
            let asking = Asking {
                epoch: pages.current().unwrap().id,
                frame,
                eye_pages: pages.eye_pages(eye),
            };
            let mut words = vec![0u32; LEVELS * (WINDOW * WINDOW) as usize / 32];
            for level in 0..LEVELS {
                for dy in -7i32..=7 {
                    for dx in -7i32..=7 {
                        if dx * dx + dy * dy <= 49 {
                            let bit = level * (WINDOW * WINDOW) as usize
                                + ((WINDOW / 2 + dy) * WINDOW + WINDOW / 2 + dx) as usize;
                            words[bit / 32] |= 1 << (bit % 32);
                        }
                    }
                }
            }
            let t1 = Instant::now();
            pages.asked(&asking, &words);
            let t2 = Instant::now();
            let boxes: Vec<[DVec3; 2]> = (0..200)
                .map(|i| {
                    let c = eye
                        + DVec3::new(
                            (i % 20) as f64 * 7.0 - 70.0,
                            -1000.0,
                            (i / 20) as f64 * 7.0 - 35.0,
                        );
                    [c, c + DVec3::splat(6.4)]
                })
                .collect();
            pages.invalidate(&boxes, Stale::Coarse);
            let t3 = Instant::now();
            pages.plan(frame, true);
            let _ = pages.take_rows();
            let t4 = Instant::now();
            let grids: Vec<EpochGrid> = pages.epochs.iter().map(|e| e.grid).collect();
            let _ = super::frame(pages.finest, &grids, &nears, eye);
            let t5 = Instant::now();
            if frame >= frames / 2 {
                for (p, d) in parts
                    .iter_mut()
                    .zip([t2 - t1, t3 - t2, t4 - t3, t5 - t4 + (t1 - t)])
                {
                    *p += d.as_secs_f64() * 1e3 / (frames / 2) as f64;
                }
            }
        }
        let s = pages.stats;
        eprintln!(
            "planner ms a frame: asked {:.3} invalidate {:.3} plan {:.3} frame+begin {:.3}; resident {} asked {} missing {} coarse {}",
            parts[0], parts[1], parts[2], parts[3], s.resident, s.asked, s.missing, s.coarse
        );
    }
}
