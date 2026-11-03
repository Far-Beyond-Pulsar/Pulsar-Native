//! CPU-side residency for the foliage tile ring.
//!
//! Owns the mapping between world tile coordinates and slots in the GPU tile table, and
//! decides which tiles are placed on any given frame. Deliberately GPU-free: everything
//! here is testable in a headless container, and the O(perimeter) property that the whole
//! foliage design rests on is a property of *this* file, not of the shaders.
//!
//! # The cost model this file is defending
//!
//! A ring of tiles around the camera is resident. On camera motion, only tiles crossing
//! the ring boundary change state, so the work per frame is proportional to the ring's
//! **perimeter**. [`TileRing::update`] enumerates exactly the entering and leaving strips
//! and nothing else, and records [`TileRing::last_visited`] so a test can assert the
//! bound rather than trusting a comment. The obvious "simpler" implementation — rebuild
//! the resident set from scratch each frame and diff it — renders identically and is
//! O(area); it would make a 128 m Medium ring cost 1024 coordinate visits per frame
//! instead of ~64, and nothing on screen would tell you.
//!
//! The one case that is genuinely O(area) is a teleport, where the new window does not
//! overlap the old at all. That is unavoidable and it is why placement is budgeted: the
//! entering tiles queue up and drain at [`TileRing::max_tiles_per_frame`], so a teleport
//! degrades to a few frames of progressive fill-in rather than a hitch.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

/// What changed in the ring on one [`TileRing::update`] call.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RingUpdate {
    /// Slots handed a new tile coordinate this frame, i.e. scheduled for placement.
    pub placed: u32,
    /// Slots released back to the free list this frame.
    pub released: u32,
    /// Resident tiles evicted to make room, because the ring capacity is smaller than
    /// the window. Non-zero means the ring is thrashing and the capacity is misconfigured
    /// — the steady-state placement cost stops being zero.
    pub evicted: u32,
    /// Tile coordinates still waiting for a placement budget slot.
    pub pending: u32,
    /// The content generation changed and the whole ring was invalidated.
    pub invalidated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Slot {
    coord: Option<[i32; 2]>,
    touch: u64,
}

/// Residency ring: world tile coordinates ⟷ GPU tile-table slots.
pub struct TileRing {
    capacity: u32,
    tiles_across: u32,
    tile_size: f32,
    max_tiles_per_frame: u32,

    center: [i32; 2],
    has_center: bool,
    generation: u32,

    slots: Vec<Slot>,
    occupied: HashMap<[i32; 2], u32>,
    free_slots: Vec<u32>,
    /// `(touch, slot)` ordered so the front is the least recently admitted resident tile.
    /// A `BTreeSet` rather than a scan because eviction must not be O(capacity) — that
    /// would reintroduce per-frame area cost through the back door.
    lru: BTreeSet<(u64, u32)>,

    pending: VecDeque<[i32; 2]>,
    pending_set: HashSet<[i32; 2]>,

    place_queue: Vec<u32>,
    dirty: Vec<u32>,
    /// Resident tiles whose ground changed, to be placed again over the blades they
    /// keep meanwhile ([`TileRing::refresh`]), and those drained this frame.
    refresh: VecDeque<[i32; 2]>,
    refresh_set: HashSet<[i32; 2]>,
    refresh_queue: Vec<u32>,

    touch_counter: u64,
    last_visited: usize,
}

impl TileRing {
    /// Create a ring covering `tiles_across` × `tiles_across` tiles with `capacity` GPU
    /// slots.
    ///
    /// `capacity` below `tiles_across²` is legal but means the ring cannot hold its own
    /// window, so tiles are evicted while still inside it and re-placed immediately —
    /// [`RingUpdate::evicted`] is the signal for that. It is allowed rather than asserted
    /// because a caller clamping a huge quality preset to a small arena should degrade,
    /// not panic mid-frame.
    pub fn new(capacity: u32, tiles_across: u32, tile_size: f32, max_tiles_per_frame: u32) -> Self {
        let capacity = capacity.max(1);
        Self {
            capacity,
            tiles_across: tiles_across.max(1),
            tile_size: if tile_size.is_finite() && tile_size > 0.0 {
                tile_size
            } else {
                crate::FOLIAGE_TILE_SIZE_METERS
            },
            max_tiles_per_frame: max_tiles_per_frame.max(1),
            center: [0, 0],
            has_center: false,
            generation: 0,
            slots: vec![
                Slot {
                    coord: None,
                    touch: 0
                };
                capacity as usize
            ],
            occupied: HashMap::with_capacity(capacity as usize),
            free_slots: (0..capacity).rev().collect(),
            lru: BTreeSet::new(),
            pending: VecDeque::new(),
            pending_set: HashSet::new(),
            place_queue: Vec::with_capacity(max_tiles_per_frame.max(1) as usize),
            dirty: Vec::new(),
            refresh: VecDeque::new(),
            refresh_set: HashSet::new(),
            refresh_queue: Vec::new(),
            touch_counter: 0,
            last_visited: 0,
        }
    }

    /// Number of GPU tile-table slots.
    #[inline]
    pub fn capacity(&self) -> u32 {
        self.capacity
    }

    /// Edge length of the square window, in tiles.
    #[inline]
    pub fn tiles_across(&self) -> u32 {
        self.tiles_across
    }

    /// Per-frame placement budget.
    #[inline]
    pub fn max_tiles_per_frame(&self) -> u32 {
        self.max_tiles_per_frame
    }

    /// Current content generation. See [`TileRing::update`].
    #[inline]
    pub fn generation(&self) -> u32 {
        self.generation
    }

    /// Number of coordinate visits performed by the last [`TileRing::update`].
    ///
    /// The instrument behind the O(perimeter) test. A one-tile camera step must visit
    /// roughly `2 × tiles_across` coordinates, not `tiles_across²`.
    #[inline]
    pub fn last_visited(&self) -> usize {
        self.last_visited
    }

    /// Slots scheduled for placement this frame, in queue order.
    #[inline]
    pub fn place_queue(&self) -> &[u32] {
        &self.place_queue
    }

    /// Resident slots placed again this frame ([`TileRing::refresh`]), after the place
    /// queue's within the same budget. Their headers stay: their blades draw until the
    /// new ones land.
    #[inline]
    pub fn refresh_queue(&self) -> &[u32] {
        &self.refresh_queue
    }

    /// Place resident tile `coord` again (its ground changed) when the budget allows,
    /// keeping its blades until then. Returns whether it is resident.
    pub fn refresh(&mut self, coord: [i32; 2]) -> bool {
        if !self.occupied.contains_key(&coord) {
            return false;
        }
        if self.refresh_set.insert(coord) {
            self.refresh.push_back(coord);
        }
        true
    }

    /// Resident tile coordinates within the inclusive tile bounds `lo..=hi`.
    pub fn resident_within(&self, lo: [i32; 2], hi: [i32; 2]) -> Vec<[i32; 2]> {
        let span = (i64::from(hi[0]) - i64::from(lo[0]) + 1).max(0)
            * (i64::from(hi[1]) - i64::from(lo[1]) + 1).max(0);
        if span > self.occupied.len() as i64 {
            return self
                .occupied
                .keys()
                .filter(|c| c[0] >= lo[0] && c[0] <= hi[0] && c[1] >= lo[1] && c[1] <= hi[1])
                .copied()
                .collect();
        }
        let mut out = Vec::new();
        for x in lo[0]..=hi[0] {
            for z in lo[1]..=hi[1] {
                if self.occupied.contains_key(&[x, z]) {
                    out.push([x, z]);
                }
            }
        }
        out
    }

    /// Slots whose GPU header must be re-uploaded this frame — newly placed tiles and
    /// newly freed ones. In queue order, so a slot that was released and then reassigned
    /// in the same frame ends up with the reassignment's header.
    #[inline]
    pub fn dirty_slots(&self) -> &[u32] {
        &self.dirty
    }

    /// Tile coordinate resident in `slot`, if any.
    #[inline]
    pub fn slot_coord(&self, slot: u32) -> Option<[i32; 2]> {
        self.slots.get(slot as usize).and_then(|s| s.coord)
    }

    /// Number of resident tiles.
    #[inline]
    pub fn resident_count(&self) -> u32 {
        self.occupied.len() as u32
    }

    /// Number of tiles still waiting on the placement budget.
    #[inline]
    pub fn pending_count(&self) -> u32 {
        self.pending_set.len() as u32
    }

    /// Whether a coordinate is currently resident.
    #[inline]
    pub fn is_resident(&self, coord: [i32; 2]) -> bool {
        self.occupied.contains_key(&coord)
    }

    /// Drop every resident tile and requeue the current window.
    ///
    /// Private because it appends to the per-frame `dirty` list, which
    /// [`TileRing::update`] clears at the top of the frame. Invalidating outside `update`
    /// would silently discard the "this slot is now free" header writes and leave the GPU
    /// drawing evicted tiles — which is exactly why the generation is an *argument* to
    /// `update` rather than a separate setter.
    fn invalidate(&mut self) {
        for slot in 0..self.capacity {
            self.release_slot(slot);
        }
        self.pending.clear();
        self.pending_set.clear();
        self.refresh.clear();
        self.refresh_set.clear();
        if self.has_center {
            let center = self.center;
            self.request_window(center);
        }
    }

    /// Advance the ring for this frame's camera position and content generation.
    ///
    /// Clears the previous frame's place queue and dirty list, applies a generation
    /// change, recentres the window, and drains up to
    /// [`TileRing::max_tiles_per_frame`] pending tiles into the place queue.
    ///
    /// Residency is keyed on `(tile_coord, generation)` — a density or terrain edit bumps
    /// the generation, which both invalidates the cached blades and feeds
    /// [`crate::blade_seed`] so the re-placed blades are a *different*
    /// deterministic set rather than the same one.
    pub fn update(&mut self, camera_xz: [f32; 2], generation: u32) -> RingUpdate {
        self.place_queue.clear();
        self.dirty.clear();
        self.refresh_queue.clear();
        self.last_visited = 0;

        let mut update = RingUpdate::default();

        if self.generation != generation {
            self.generation = generation;
            update.released += self.resident_count();
            self.invalidate();
            update.invalidated = true;
        }

        // A non-finite camera position freezes the window rather than teleporting it to
        // tile (0, 0): keeping the last good window on screen is strictly better than
        // dumping the player's surroundings and refilling from the origin. Pending work
        // still drains, so a single bad frame does not stall the fill-in.
        if camera_xz[0].is_finite() && camera_xz[1].is_finite() {
            let new_center = [
                (camera_xz[0] / self.tile_size).floor() as i32,
                (camera_xz[1] / self.tile_size).floor() as i32,
            ];

            if !self.has_center {
                self.has_center = true;
                self.center = new_center;
                self.request_window(new_center);
                self.last_visited += (self.tiles_across as usize).pow(2);
            } else if new_center != self.center {
                update.released += self.shift_to(new_center);
            }
        }

        let (placed, evicted) = self.drain_pending();
        // Tiles placed again share the budget, after the entering ones.
        while (self.place_queue.len() + self.refresh_queue.len())
            < self.max_tiles_per_frame as usize
        {
            let Some(coord) = self.refresh.pop_front() else {
                break;
            };
            self.refresh_set.remove(&coord);
            if let Some(&slot) = self.occupied.get(&coord) {
                if !self.place_queue.contains(&slot) {
                    self.refresh_queue.push(slot);
                }
            }
        }
        update.placed = placed;
        update.evicted = evicted;
        update.pending = self.pending_count();
        update
    }

    // ── Window arithmetic ───────────────────────────────────────────────────────

    /// Inclusive `(x0, x1, z0, z1)` tile bounds of the window centred on `center`.
    fn window(&self, center: [i32; 2]) -> (i32, i32, i32, i32) {
        let half = (self.tiles_across / 2) as i32;
        let span = self.tiles_across as i32 - 1;
        let x0 = center[0] - half;
        let z0 = center[1] - half;
        (x0, x0 + span, z0, z0 + span)
    }

    fn request_window(&mut self, center: [i32; 2]) {
        let (x0, x1, z0, z1) = self.window(center);
        for x in x0..=x1 {
            for z in z0..=z1 {
                self.request([x, z]);
            }
        }
    }

    /// Move the window, touching only the strips that entered or left.
    ///
    /// The two-loop shape per direction is what keeps this O(perimeter): the first loop
    /// walks whole columns that changed, the second walks whole rows *restricted to the
    /// overlapping columns*, so no coordinate is visited twice. When the shift exceeds
    /// the window width the overlap is empty, the second loop does nothing, and the first
    /// covers the entire window — the teleport case, which is O(area) by necessity.
    fn shift_to(&mut self, new_center: [i32; 2]) -> u32 {
        let (ox0, ox1, oz0, oz1) = self.window(self.center);
        let (nx0, nx1, nz0, nz1) = self.window(new_center);
        let mut released = 0u32;

        // Leaving: old \ new.
        for x in ox0..=ox1 {
            if x < nx0 || x > nx1 {
                for z in oz0..=oz1 {
                    self.last_visited += 1;
                    if self.release_coord([x, z]) {
                        released += 1;
                    }
                }
            }
        }
        let overlap_x0 = ox0.max(nx0);
        let overlap_x1 = ox1.min(nx1);
        if overlap_x0 <= overlap_x1 {
            for z in oz0..=oz1 {
                if z < nz0 || z > nz1 {
                    for x in overlap_x0..=overlap_x1 {
                        self.last_visited += 1;
                        if self.release_coord([x, z]) {
                            released += 1;
                        }
                    }
                }
            }
        }

        // Entering: new \ old.
        for x in nx0..=nx1 {
            if x < ox0 || x > ox1 {
                for z in nz0..=nz1 {
                    self.last_visited += 1;
                    self.request([x, z]);
                }
            }
        }
        if overlap_x0 <= overlap_x1 {
            for z in nz0..=nz1 {
                if z < oz0 || z > oz1 {
                    for x in overlap_x0..=overlap_x1 {
                        self.last_visited += 1;
                        self.request([x, z]);
                    }
                }
            }
        }

        self.center = new_center;
        released
    }

    fn contains(&self, coord: [i32; 2]) -> bool {
        if !self.has_center {
            return false;
        }
        let (x0, x1, z0, z1) = self.window(self.center);
        coord[0] >= x0 && coord[0] <= x1 && coord[1] >= z0 && coord[1] <= z1
    }

    // ── Slot bookkeeping ────────────────────────────────────────────────────────

    fn request(&mut self, coord: [i32; 2]) {
        if self.occupied.contains_key(&coord) {
            return;
        }
        if self.pending_set.insert(coord) {
            self.pending.push_back(coord);
        }
    }

    /// Release whatever slot holds `coord`. Returns whether anything was released.
    fn release_coord(&mut self, coord: [i32; 2]) -> bool {
        // A tile can leave the ring before its placement budget ever came up; drop the
        // request rather than placing a tile that is already gone.
        if self.pending_set.remove(&coord) {
            // The `pending` deque entry is left behind and skipped on drain — removing
            // from the middle of a VecDeque is O(n) and this is the hot path.
        }
        let Some(slot) = self.occupied.remove(&coord) else {
            return false;
        };
        self.free_slot(slot);
        true
    }

    fn release_slot(&mut self, slot: u32) {
        let Some(entry) = self.slots.get(slot as usize).copied() else {
            return;
        };
        let Some(coord) = entry.coord else {
            return;
        };
        self.occupied.remove(&coord);
        self.free_slot(slot);
    }

    fn free_slot(&mut self, slot: u32) {
        let Some(entry) = self.slots.get_mut(slot as usize) else {
            return;
        };
        let touch = entry.touch;
        entry.coord = None;
        entry.touch = 0;
        self.lru.remove(&(touch, slot));
        self.free_slots.push(slot);
        self.dirty.push(slot);
    }

    /// Move up to the per-frame budget of pending tiles into the place queue.
    ///
    /// Returns `(placed, evicted)`.
    fn drain_pending(&mut self) -> (u32, u32) {
        let mut placed = 0u32;
        let mut evicted = 0u32;

        while placed < self.max_tiles_per_frame {
            let Some(coord) = self.pending.pop_front() else {
                break;
            };
            // Stale entry left behind by `release_coord`: it costs nothing and must not
            // consume budget.
            if !self.pending_set.remove(&coord) {
                continue;
            }
            if self.occupied.contains_key(&coord) || !self.contains(coord) {
                continue;
            }

            let slot = match self.free_slots.pop() {
                Some(slot) => slot,
                None => {
                    // Ring capacity is smaller than the window. Evict the least recently
                    // admitted resident; every resident is inside the window by
                    // construction, so there is no "outside the ring" candidate to prefer
                    // and admission order is the only ranking available.
                    let Some(&(touch, victim)) = self.lru.iter().next() else {
                        // Capacity 0 in practice: nothing to evict and nothing to give.
                        self.pending.push_front(coord);
                        self.pending_set.insert(coord);
                        break;
                    };
                    self.lru.remove(&(touch, victim));
                    if let Some(entry) = self.slots.get_mut(victim as usize) {
                        if let Some(old_coord) = entry.coord.take() {
                            self.occupied.remove(&old_coord);
                        }
                        entry.touch = 0;
                    }
                    self.dirty.push(victim);
                    evicted += 1;
                    victim
                }
            };

            self.touch_counter += 1;
            let touch = self.touch_counter;
            if let Some(entry) = self.slots.get_mut(slot as usize) {
                entry.coord = Some(coord);
                entry.touch = touch;
            }
            self.lru.insert((touch, slot));
            self.occupied.insert(coord, slot);
            self.place_queue.push(slot);
            self.dirty.push(slot);
            placed += 1;
        }

        (placed, evicted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TILE: f32 = crate::FOLIAGE_TILE_SIZE_METERS;

    fn ring(across: u32, budget: u32) -> TileRing {
        TileRing::new(across * across, across, TILE, budget)
    }

    /// Run enough frames for the ring to reach steady state at the given position.
    fn settle(ring: &mut TileRing, xz: [f32; 2]) {
        let generation = ring.generation();
        for _ in 0..4096 {
            let update = ring.update(xz, generation);
            if update.pending == 0 {
                return;
            }
        }
        panic!("ring never converged");
    }

    #[test]
    fn first_update_requests_the_whole_window_then_drains_at_the_budget() {
        let mut ring = ring(9, 4);
        let update = ring.update([0.0, 0.0], 0);
        assert_eq!(update.placed, 4, "the per-frame budget is a hard cap");
        assert_eq!(update.pending, 81 - 4);

        // 81 tiles at 4 per frame is 21 frames, and not one more.
        let mut frames = 1;
        while ring.pending_count() > 0 {
            ring.update([0.0, 0.0], 0);
            frames += 1;
            assert!(frames <= 21, "budgeted fill took {frames} frames");
        }
        assert_eq!(ring.resident_count(), 81);
    }

    #[test]
    fn a_settled_ring_does_no_work_when_the_camera_does_not_cross_a_tile_boundary() {
        let mut ring = ring(9, 8);
        settle(&mut ring, [0.0, 0.0]);

        // Move within the same tile: nothing enters, nothing leaves, nothing is visited.
        let update = ring.update([TILE * 0.4, TILE * 0.4], 0);
        assert_eq!(update, RingUpdate::default());
        assert_eq!(ring.last_visited(), 0, "steady state must be free");
        assert!(ring.place_queue().is_empty());
        assert!(ring.dirty_slots().is_empty());
    }

    #[test]
    fn a_one_tile_step_touches_a_perimeter_not_an_area() {
        const ACROSS: u32 = 33;
        let mut ring = ring(ACROSS, 4096);
        settle(&mut ring, [0.0, 0.0]);

        let update = ring.update([TILE * 1.5, 0.0], 0);
        assert_eq!(update.released, ACROSS, "one column leaves");
        assert_eq!(update.placed, ACROSS, "one column enters");

        // The bound that matters: 2 × the perimeter strip, never the 1089-tile area.
        assert_eq!(ring.last_visited(), 2 * ACROSS as usize);
        assert!(
            ring.last_visited() < (ACROSS as usize).pow(2) / 4,
            "a one-tile step visited {} coordinates on a {ACROSS}x{ACROSS} ring — \
             the residency cache has become O(area)",
            ring.last_visited()
        );
    }

    #[test]
    fn a_diagonal_step_touches_two_perimeters_without_double_counting() {
        const ACROSS: u32 = 17;
        let mut ring = ring(ACROSS, 4096);
        settle(&mut ring, [0.0, 0.0]);

        ring.update([TILE * 1.5, TILE * 1.5], 0);
        // An L-shaped strip: a full column plus a row restricted to the overlap, on each
        // of the entering and leaving sides. Double-visiting the corner would push this
        // over the bound.
        let expected = 2 * (ACROSS as usize + (ACROSS as usize - 1));
        assert_eq!(ring.last_visited(), expected);
    }

    #[test]
    fn entering_and_leaving_sets_are_exact() {
        const ACROSS: u32 = 5;
        let mut ring = ring(ACROSS, 4096);
        settle(&mut ring, [0.0, 0.0]);
        // Window is x,z in -2..=2.
        assert!(ring.is_resident([-2, 0]));
        assert!(ring.is_resident([2, 0]));
        assert!(!ring.is_resident([3, 0]));

        ring.update([TILE * 1.5, 0.0], 0);
        // Window is now x in -1..=3.
        assert!(!ring.is_resident([-2, 0]), "the far column must have left");
        assert!(
            ring.is_resident([3, 0]),
            "the near column must have entered"
        );
        for z in -2..=2 {
            assert!(ring.is_resident([-1, z]));
            assert!(ring.is_resident([3, z]));
            assert!(!ring.is_resident([-2, z]));
        }
        assert_eq!(ring.resident_count(), (ACROSS * ACROSS) as u32);
    }

    #[test]
    fn a_teleport_degrades_to_progressive_fill_in_not_a_hitch() {
        const ACROSS: u32 = 17;
        const BUDGET: u32 = 24;
        let mut ring = ring(ACROSS, BUDGET);
        settle(&mut ring, [0.0, 0.0]);

        // Somewhere with no window overlap at all.
        let update = ring.update([TILE * 10_000.0, TILE * 10_000.0], 0);
        assert_eq!(
            update.released,
            ACROSS * ACROSS,
            "the old window is fully dropped"
        );
        assert_eq!(
            update.placed, BUDGET,
            "a teleport must not place more than the per-frame budget"
        );
        assert!(update.pending > 0);

        let mut frames = 1;
        while ring.pending_count() > 0 {
            let update = ring.update([TILE * 10_000.0, TILE * 10_000.0], 0);
            assert!(update.placed <= BUDGET);
            frames += 1;
        }
        assert_eq!(ring.resident_count(), ACROSS * ACROSS);
        assert_eq!(frames, (ACROSS * ACROSS).div_ceil(BUDGET));
    }

    #[test]
    fn tiles_that_leave_before_their_budget_slot_are_never_placed() {
        const ACROSS: u32 = 5;
        // A budget of one, so almost everything sits pending.
        let mut ring = ring(ACROSS, 1);
        ring.update([0.0, 0.0], 0);
        assert_eq!(ring.pending_count(), 24);

        // Sprint far enough that the whole first window leaves while still pending.
        ring.update([TILE * 100.0, 0.0], 0);
        for _ in 0..64 {
            ring.update([TILE * 100.0, 0.0], 0);
        }
        // Nothing from the original window survived.
        for x in -2..=2i32 {
            for z in -2..=2i32 {
                assert!(
                    !ring.is_resident([x, z]),
                    "stale request placed tile {x},{z}"
                );
            }
        }
    }

    #[test]
    fn an_undersized_ring_evicts_lru_instead_of_failing() {
        // 25 tiles of window, 8 slots: the ring cannot hold its own footprint.
        let mut ring = TileRing::new(8, 5, TILE, 4);
        let mut evicted_total = 0u32;
        for _ in 0..16 {
            let update = ring.update([0.0, 0.0], 0);
            evicted_total += update.evicted;
        }
        assert!(
            evicted_total > 0,
            "an undersized ring must report thrashing"
        );
        assert!(
            ring.resident_count() <= 8,
            "residency exceeded the ring capacity"
        );
        // Every occupied slot is still self-consistent after the churn.
        for slot in 0..8 {
            if let Some(coord) = ring.slot_coord(slot) {
                assert!(ring.is_resident(coord));
            }
        }
    }

    #[test]
    fn a_generation_bump_invalidates_residency_and_requeues_the_window() {
        const ACROSS: u32 = 7;
        const AREA: u32 = ACROSS * ACROSS;
        let mut ring = ring(ACROSS, 4);
        settle(&mut ring, [0.0, 0.0]);
        assert_eq!(ring.resident_count(), AREA);

        let update = ring.update([0.0, 0.0], 1);
        assert!(update.invalidated, "a new generation must invalidate");
        assert_eq!(update.released, AREA);
        // Every freed slot must appear in the dirty list in the *same* frame, or the GPU
        // keeps drawing tiles whose blades belong to the previous generation.
        assert!(update.placed <= 4);
        for slot in 0..AREA {
            assert!(
                ring.dirty_slots().contains(&slot),
                "slot {slot} was invalidated without a header write"
            );
        }
        assert_eq!(ring.resident_count(), update.placed);
        assert_eq!(ring.pending_count(), AREA - update.placed);

        // An unchanged generation is free.
        let update = ring.update([0.0, 0.0], 1);
        assert!(!update.invalidated);
        settle(&mut ring, [0.0, 0.0]);
        assert_eq!(ring.resident_count(), AREA);
    }

    #[test]
    fn dirty_slots_cover_every_placement_and_release() {
        const ACROSS: u32 = 9;
        let mut ring = ring(ACROSS, 4096);
        settle(&mut ring, [0.0, 0.0]);

        ring.update([TILE * 1.5, 0.0], 0);
        // One column out, one column in — both need a GPU header write, and the placed
        // slots must be a subset of the dirty ones or the GPU reads a stale tile coord.
        assert!(ring.dirty_slots().len() >= 2 * ACROSS as usize);
        for slot in ring.place_queue() {
            assert!(ring.dirty_slots().contains(slot));
        }
    }

    #[test]
    fn a_non_finite_camera_freezes_the_ring_rather_than_dumping_it() {
        const ACROSS: u32 = 5;
        let mut ring = ring(ACROSS, 4096);
        settle(&mut ring, [0.0, 0.0]);
        let before = ring.resident_count();

        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let update = ring.update([bad, 0.0], 0);
            assert_eq!(update.placed, 0);
            assert_eq!(update.released, 0);
            assert_eq!(ring.resident_count(), before);
            assert_eq!(ring.last_visited(), 0);
        }
    }

    /// A resident tile whose ground changed is placed again within the budget, after
    /// the entering tiles, without a new header (its blades draw until then); a tile not
    /// resident cannot be.
    #[test]
    fn a_refreshed_tile_is_placed_again_without_a_new_header() {
        let mut r = ring(4, 64);
        settle(&mut r, [0.0, 0.0]);
        let coord = [0, 0];
        assert!(r.is_resident(coord));
        assert!(r.refresh(coord));
        assert!(!r.refresh([100, 100]));
        r.update([0.0, 0.0], 0);
        assert_eq!(r.refresh_queue().len(), 1);
        assert!(r.place_queue().is_empty());
        assert!(r.dirty_slots().is_empty(), "a refresh rewrote a header");
        assert_eq!(r.slot_coord(r.refresh_queue()[0]), Some(coord));
        // Done: not again.
        r.update([0.0, 0.0], 0);
        assert!(r.refresh_queue().is_empty());
        // Within a box: only the resident ones.
        assert_eq!(r.resident_within([-1, -1], [0, 0]).len(), 4);
        assert!(r.resident_within([50, 50], [60, 60]).is_empty());
    }

    #[test]
    fn slots_are_never_double_booked() {
        const ACROSS: u32 = 7;
        let mut ring = ring(ACROSS, 6);
        // Wander around, forcing constant churn against a tight budget.
        for step in 0..200 {
            let x = ((step % 17) as f32 - 8.0) * TILE;
            let z = ((step % 11) as f32 - 5.0) * TILE;
            ring.update([x, z], 0);

            let mut seen = HashSet::new();
            for slot in 0..ring.capacity() {
                if let Some(coord) = ring.slot_coord(slot) {
                    assert!(seen.insert(coord), "two slots hold tile {coord:?}");
                    assert!(ring.is_resident(coord));
                }
            }
            assert_eq!(seen.len() as u32, ring.resident_count());
        }
    }
}
