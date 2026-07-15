//! The SceneDB-owned multi-cell device-side store (M2b-α §2, design Rev 2):
//! region-partitioned scene SSBOs shared across every registered CELL. Each
//! cell owns a disjoint `[row_base, row_base+capacity)` slice of the
//! transform and slot-mirror buffers and a disjoint
//! `[slot_base, slot_base+capacity+headroom)` slice of the generation buffer
//! (`RegionPool`, §7). Constructed on the engine-level device context (C0).
//!
//! Mirrored columns must be written via `SceneGpuStore::write_transform` and
//! compacted via `SceneGpuStore::compact_all`; raw column access bypasses
//! dirty tracking.

use super::{
    DirtyMask, EngineGpuContext, GenerationBuffer, RegionError, RegionPool, SceneBuffer,
    SimulateWitness, SubmissionTracker, SyncStats,
};
use crate::cell::{CellStorage, PendingRetire};
use crate::handle::Handle;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

/// One size class of region (design Rev 2 §2/§7): every cell registered
/// under this class gets a fixed-size `capacity`-row region, and at most
/// `max_resident_cells` such regions ever coexist.
#[derive(Debug, Clone, Copy)]
pub struct RegionClassConfig {
    pub capacity: u32,
    pub max_resident_cells: u32,
}

/// Fixed store capacities (SSBOs never reallocate — exceeding them is a hard
/// error at the call site, §8), expressed as size classes rather than one
/// flat cap (M2a's `GpuStoreConfig`).
#[derive(Debug, Clone)]
pub struct SceneGpuConfig {
    pub classes: Vec<RegionClassConfig>,
    /// Extra slots reserved per region beyond `capacity`, absorbing
    /// tombstoned (retired-but-not-yet-recycled) slots without stealing a
    /// neighbor's region (§4.1).
    pub tombstone_headroom: u32,
    /// Placeholder buffer; layout arrives in M3.
    pub max_materials: u32,
    /// Per-cell metadata SSBO entries (α: allocated, no writer).
    pub max_cells_metadata: u32,
}

impl SceneGpuConfig {
    /// The default tombstone headroom used across M2b-α fixtures.
    pub fn default_headroom() -> u32 {
        64
    }
}

/// Opaque handle to a registered cell's region assignment. Indexes
/// `SceneGpuStore`'s internal per-cell state; never crosses the FFI/shader
/// boundary (that's `row_region_base`'s job).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CellId(pub(crate) u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Write,
    Retired,
    Compacted,
}

struct QueuedRetire {
    pending: PendingRetire,
    serial: u64,
}

/// Per-cell GPU-side bookkeeping: the region assignment, the dirty state
/// that used to live on `GpuStore` directly (M2a), and the deferred-retire
/// queue (now one per cell rather than store-wide).
struct CellGpuState {
    /// Size class this cell was registered under (β reuses this for
    /// promotion/eviction; unread this task).
    #[allow(dead_code)]
    class: usize,
    row_base: u32,
    slot_base: u32,
    /// Class capacity + headroom; bounds every gen/slot write into this
    /// cell's region.
    slot_capacity: u32,
    dirty_transforms: DirtyMask,
    dirty_slots: DirtyMask,
    /// Per-row global-slot staging, refreshed by `sync_all` for every row
    /// `dirty_slots` marks, then uploaded into the shared slot-mirror SSBO
    /// (T4; C6 GPU handle validation).
    slot_scratch: Vec<u32>,
    /// Per-ROW shadow of the last LOCAL slot uploaded into the mirror for
    /// that row; `u32::MAX` = never uploaded. Read and written ONLY by
    /// `sync_all`'s self-healing boundary scan (`&mut self`), which compares
    /// it against the authoritative slot column and re-uploads every
    /// mismatch. Row-scoped on purpose: per-event triggers (gen-gate,
    /// write-path shadow check, compaction marks) each missed a staleness
    /// path — e.g. an alloc re-occupying a compaction-vacated row that is
    /// never written (Task 4 review + re-review); the boundary scan closes
    /// them all with one invariant.
    slot_shadow: Vec<u32>,
    /// Per-cell deferred-retire queue; nondecreasing serials (debug-asserted,
    /// T11).
    pending: VecDeque<QueuedRetire>,
    /// CPU-side shadow of the last generation uploaded per LOCAL slot (§4
    /// delta-minimality on the write path), seeded from the registry at
    /// `register_cell`. Atomic because `write_transform` takes `&self`.
    gen_shadow: Vec<AtomicU32>,
}

/// One cell's region assignment paired with its mutable storage, for the
/// bulk `*_all` frame-boundary stages.
///
/// The (id, cell) pairing is TRUSTED: the store cannot verify that `cell` is
/// the storage `id` was registered with, and a mismatched pair commits
/// retires and dirty marks into the wrong cell's regions.
pub struct CellSlot<'a> {
    pub id: CellId,
    pub cell: &'a mut CellStorage,
}

/// The SceneDB-owned multi-cell device-side store (M2b-α §2): persistent
/// region-partitioned scene SSBOs, the mirrored-column writer, delta-sync,
/// and the retirement drain — generalizing M2a's single-cell `GpuStore` to
/// N cells sharing one set of buffers via `RegionPool` (§7).
pub struct SceneGpuStore {
    #[allow(dead_code)]
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    transforms: SceneBuffer<[f32; 16]>,
    slot_mirror: SceneBuffer<u32>,
    generations: GenerationBuffer,
    material: wgpu::Buffer,
    cell_metadata: wgpu::Buffer,
    tracker: SubmissionTracker,
    phase: Phase,
    /// One row pool and one slot pool per size class, base offsets laid end
    /// to end in class order (§7).
    row_pools: Vec<RegionPool>,
    slot_pools: Vec<RegionPool>,
    cells: Vec<CellGpuState>,
    /// Instrumentation: total generation-buffer writes issued across every
    /// cell, so tests can assert generation-upload minimality.
    gen_writes: AtomicU64,
}

impl SceneGpuStore {
    pub fn new(ctx: &EngineGpuContext, cfg: SceneGpuConfig) -> Self {
        let mut row_pools = Vec::with_capacity(cfg.classes.len());
        let mut slot_pools = Vec::with_capacity(cfg.classes.len());
        let mut row_offset = 0u32;
        let mut slot_offset = 0u32;
        for class in &cfg.classes {
            let slot_region_size = class.capacity + cfg.tombstone_headroom;
            row_pools.push(RegionPool::new(row_offset, class.capacity, class.max_resident_cells));
            slot_pools.push(RegionPool::new(slot_offset, slot_region_size, class.max_resident_cells));
            // Checked accumulation: a pathological config must fail loudly at
            // construction, not wrap into silently-undersized SSBOs.
            row_offset = row_offset
                .checked_add(
                    class
                        .capacity
                        .checked_mul(class.max_resident_cells)
                        .expect("row capacity overflow"),
                )
                .expect("row capacity overflow");
            slot_offset = slot_offset
                .checked_add(
                    slot_region_size
                        .checked_mul(class.max_resident_cells)
                        .expect("slot capacity overflow"),
                )
                .expect("slot capacity overflow");
        }
        Self {
            device: Arc::clone(ctx.device()),
            queue: Arc::clone(ctx.queue()),
            transforms: SceneBuffer::new(ctx.device(), "scenedb-instances", row_offset),
            slot_mirror: SceneBuffer::new(ctx.device(), "scenedb-slot-mirror", row_offset),
            generations: GenerationBuffer::new(ctx.device(), slot_offset),
            // Material stride is 32 bytes per entry (C5); only the field
            // LAYOUT is M3-deferred. Sizing at the final stride now keeps the
            // §10 allocate-once contract — M3 fills the layout in place, no
            // buffer recreation.
            material: ctx.device().create_buffer(&wgpu::BufferDescriptor {
                label: Some("scenedb-materials"),
                size: cfg.max_materials as u64 * 32,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
            // Per-cell metadata stride is 8 bytes (design §4.1: f32 alpha +
            // u32 domain). Allocated at final stride now (§10); α has no
            // writer.
            cell_metadata: ctx.device().create_buffer(&wgpu::BufferDescriptor {
                label: Some("scenedb-cell-metadata"),
                size: cfg.max_cells_metadata as u64 * 8,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
            tracker: SubmissionTracker::new(),
            phase: Phase::Write,
            row_pools,
            slot_pools,
            cells: Vec::new(),
            gen_writes: AtomicU64::new(0),
        }
    }

    pub fn tracker(&self) -> &SubmissionTracker {
        &self.tracker
    }

    /// Test instrument: how many generation-buffer writes this store has
    /// issued across every cell (asserting upload minimality, §4).
    #[doc(hidden)]
    pub fn generation_write_count(&self) -> u64 {
        self.gen_writes.load(Ordering::Relaxed)
    }

    /// Shadow-gated generation upload: writes VRAM (and the shadow) only when
    /// `generation` differs from the last value uploaded for `local_slot`,
    /// translated to the cell's global slot via `state.slot_base`.
    ///
    /// Deliberately NOT the slot-mirror dirty trigger: this gate is
    /// SLOT-scoped, but mirror staleness is ROW-scoped — a retired slot
    /// recycled into a different row arrives with its generation already
    /// shadowed (the retire stamped it), so the gate stays silent while the
    /// new row's mirror entry is stale (fail-open C6, Task 4 review). The
    /// mirror trigger is `sync_all`'s self-healing boundary scan.
    fn write_generation(&self, state: &CellGpuState, local_slot: u32, generation: u32) {
        assert!(
            local_slot < state.slot_capacity,
            "slot {local_slot} beyond region capacity {} — write must never land in a neighbor's region",
            state.slot_capacity
        );
        if state.gen_shadow[local_slot as usize].load(Ordering::Relaxed) == generation {
            return;
        }
        self.generations.write(&self.queue, state.slot_base + local_slot, generation);
        state.gen_shadow[local_slot as usize].store(generation, Ordering::Relaxed);
        self.gen_writes.fetch_add(1, Ordering::Relaxed);
    }

    /// §4.1 promotion primitive (α: registration; β reuses it for promotion):
    /// allocates row+slot regions, bulk-rebuilds the generation region from
    /// the registry, seeds the gen-shadow, marks all occupied rows dirty in
    /// the transform mask. The slot mirror needs no warm-up — the first
    /// `sync_all` boundary scan uploads every occupied row's slot entry.
    pub fn register_cell(&mut self, cell: &CellStorage, class: usize) -> Result<CellId, RegionError> {
        if self.row_pools[class].free_count() == 0 {
            return Err(RegionError::RowsExhausted);
        }
        if self.slot_pools[class].free_count() == 0 {
            return Err(RegionError::SlotsExhausted);
        }
        let row_base = self.row_pools[class].alloc().expect("checked free_count above");
        let slot_base = self.slot_pools[class].alloc().expect("checked free_count above");
        let row_capacity = self.row_pools[class].region_size();
        let slot_capacity = self.slot_pools[class].region_size();

        assert!(
            cell.rows_in_use() <= row_capacity,
            "cell occupies {} rows but class capacity is {row_capacity}",
            cell.rows_in_use()
        );
        let gens = cell.registry().generations();
        assert!(gens.len() as u32 <= slot_capacity, "cell has more slots ({}) than its region capacity {slot_capacity}", gens.len());
        self.generations.rebuild_region(&self.queue, slot_base, gens);

        let gen_shadow: Vec<AtomicU32> = (0..slot_capacity).map(|_| AtomicU32::new(0)).collect();
        for (slot, &generation) in gens.iter().enumerate() {
            gen_shadow[slot].store(generation, Ordering::Relaxed);
        }

        let dirty_transforms = DirtyMask::new(row_capacity);
        dirty_transforms.mark_range(cell.rows_in_use());
        // No slot-mirror warm-up: `sync_all`'s self-healing boundary scan is
        // the SOLE dirty_slots marker and scratch/shadow writer. The shadow
        // starts all-MAX (= never uploaded; real local slots are always
        // < slot_capacity < u32::MAX), so the first boundary marks and
        // uploads every occupied row on its own. Keeping mark/fill paired in
        // exactly one place removes the "mark without a scratch fill uploads
        // stale bytes" footgun.
        let dirty_slots = DirtyMask::new(row_capacity);

        self.cells.push(CellGpuState {
            class,
            row_base,
            slot_base,
            slot_capacity,
            dirty_transforms,
            dirty_slots,
            slot_scratch: vec![0u32; row_capacity as usize],
            slot_shadow: vec![u32::MAX; row_capacity as usize],
            pending: VecDeque::new(),
            gen_shadow,
        });
        Ok(CellId(self.cells.len() as u32 - 1))
    }

    /// Test 14 (C0 companion gate): build a fresh multi-cell store on a fresh
    /// device purely from every cell's CPU-authoritative columns (no GPU-only
    /// state exists to lose, design §3 "derived data is not stored"). Returns
    /// the rebuilt store paired with each input cell's freshly assigned
    /// `CellId`, in the same order as `cells`.
    ///
    /// Precondition per cell: no rows may be pinned (all pending retires
    /// drained via `retire_all`) — recovery of in-flight retirement across a
    /// device loss is M4 scope; rebuilding while a pin is outstanding would
    /// strand it permanently (the pin bit lives only in `CellStorage`, and
    /// this fresh store has no queued `PendingRetire` to eventually unpin
    /// it). Verbatim message carried over from M2a's `GpuStore::rebuild_from`.
    ///
    /// For each cell: `register_cell` already rebuilds that cell's generation
    /// region, seeds the gen shadow, and marks every occupied row dirty in
    /// the transform mask (§4.1 warm-up) — but `write_rows` below is an
    /// UNCONDITIONAL bulk write, so those warm-up marks are cleared right
    /// after to avoid double-uploading the same bytes at the first boundary.
    /// The slot mirror has no warm-up marker of its own (its sole dirty
    /// trigger is `sync_all`'s self-healing boundary scan, which hasn't run
    /// yet for a freshly rebuilt store), so it is bulk-filled here too —
    /// scratch and shadow alike — matching exactly what that boundary scan
    /// would otherwise produce on its first pass.
    pub fn rebuild(
        ctx: &EngineGpuContext,
        cfg: SceneGpuConfig,
        cells: &[(usize, &CellStorage)],
    ) -> (Self, Vec<CellId>) {
        let mut store = Self::new(ctx, cfg);
        let mut ids = Vec::with_capacity(cells.len());
        for &(class, cell) in cells {
            debug_assert!(
                (0..cell.rows_in_use()).all(|r| !cell.is_row_pinned(r)),
                "rebuild_from with in-flight retirement: drain retire() before device-loss rebuild — pins would be permanently stranded"
            );
            let id = store.register_cell(cell, class).expect("rebuild: cell must fit its class region");
            let rows = cell.rows_in_use();
            let row_base = store.cells[id.0 as usize].row_base;
            let slot_base = store.cells[id.0 as usize].slot_base;

            let col = cell
                .column_for::<[f32; 16]>()
                .expect("cell has no [f32; 16] transform column");
            store.transforms.write_rows(&store.queue, &col[..rows as usize], row_base);
            store.cells[id.0 as usize].dirty_transforms.clear_all();

            let col0 = cell.slot_column();
            {
                let state = &mut store.cells[id.0 as usize];
                for row in 0..rows {
                    let local_slot = col0[row as usize];
                    state.slot_scratch[row as usize] = slot_base + local_slot;
                    state.slot_shadow[row as usize] = local_slot;
                }
            }
            store.slot_mirror.write_rows(&store.queue, &store.cells[id.0 as usize].slot_scratch[..rows as usize], row_base);

            ids.push(id);
        }
        (store, ids)
    }

    /// The single mutation path for the GPU-mirrored transform column (§4):
    /// writes the core column AND sets the row's dirty bit in one operation.
    /// False for stale/invalid handles.
    ///
    /// Also stamps the handle's generation into the slot-indexed generation
    /// buffer (adaptation to §5/§7): the design's "written by retirement"
    /// trigger only ever bumps a slot's entry on retire, so a slot's *first*
    /// generation (assigned at `alloc`, which does not pass through
    /// `SceneGpuStore` at all) would otherwise never reach VRAM until that
    /// slot is later retired. The stamp is shadow-gated (`gen_shadow`): a
    /// generation reaches VRAM on the first write after alloc and on
    /// retirement — NOT per `write_transform` call — so repeat writes to a
    /// live handle (the §4 hot path) issue zero generation-buffer traffic
    /// while the buffer still mirrors `HandleRegistry::generations()` for
    /// every allocated slot (C6).
    pub fn write_transform(
        &self,
        id: CellId,
        cell: &mut CellStorage,
        handle: Handle,
        m: &[f32; 16],
        _sim: &impl SimulateWitness,
    ) -> bool {
        debug_assert_eq!(self.phase, Phase::Write, "mutation outside the write window");
        let state = &self.cells[id.0 as usize];
        let Some(row) = cell.row_of(handle) else {
            return false;
        };
        if cell.is_row_pinned(row) {
            return false; // in-flight retirement: logically deleted (§8) — no further mutation
        }
        let col = cell
            .column_for_mut::<[f32; 16]>()
            .expect("cell has no [f32; 16] transform column");
        col[row as usize] = *m;
        state.dirty_transforms.mark(row);
        self.write_generation(state, handle.index(), handle.generation());
        true
    }

    /// §5 flow step 1: liveness-dead + pinned + enqueued against `serial`.
    /// Registry and GPU buffers unchanged until the serial completes.
    pub fn free_deferred(
        &mut self,
        id: CellId,
        cell: &mut CellStorage,
        handle: Handle,
        serial: u64,
        _sim: &impl SimulateWitness,
    ) -> bool {
        debug_assert_eq!(self.phase, Phase::Write, "free_deferred outside the write window");
        let Some(pending) = cell.mark_pending_retire(handle) else {
            return false;
        };
        let state = &mut self.cells[id.0 as usize];
        debug_assert!(
            state.pending.back().map_or(true, |q| q.serial <= serial),
            "free_deferred serials must be nondecreasing per cell — the retire \
             drain's FIFO early-break would silently stall retirement behind an \
             out-of-order serial"
        );
        state.pending.push_back(QueuedRetire { pending, serial });
        true
    }

    /// §5 flow step 3, frame boundary, runs FIRST: for every cell, drain that
    /// cell's queue (FIFO, early-break on the first incomplete serial)
    /// against `tracker.completed()`; for every drained entry write the new
    /// generation to VRAM, then commit in the registry — the gen bump
    /// reaches the GPU before the slot can be re-allocated (C6). Returns the
    /// total number of slots retired across every cell.
    pub(crate) fn retire_all(&mut self, cells: &mut [CellSlot<'_>]) -> u32 {
        debug_assert_eq!(self.phase, Phase::Write, "retire_all must open the frame boundary");
        self.phase = Phase::Retired;
        let done = self.tracker.completed();
        let mut drained = 0u32;
        for slot in cells.iter_mut() {
            let idx = slot.id.0 as usize;
            loop {
                let ready = matches!(self.cells[idx].pending.front(), Some(front) if front.serial <= done);
                if !ready {
                    break; // FIFO serials: everything behind is also incomplete
                }
                let QueuedRetire { pending, .. } = self.cells[idx].pending.pop_front().unwrap();
                // Retirement always bumps the generation, so the shadow-gated
                // write always lands in VRAM (and updates the shadow) before
                // the registry commit can recycle the slot (C6).
                self.write_generation(&self.cells[idx], pending.slot, pending.next_gen);
                slot.cell.commit_retire(pending);
                drained += 1;
            }
        }
        drained
    }

    /// Frame-boundary compaction (§4): every moved row's destination is
    /// marked dirty in the transform mask so the next sync re-uploads it.
    /// The slot mirror is NOT marked here — `sync_all`'s boundary scan
    /// detects moved slots on its own.
    pub(crate) fn compact_all(&mut self, cells: &mut [CellSlot<'_>]) {
        debug_assert_eq!(self.phase, Phase::Retired, "compact_all must follow retire_all");
        self.phase = Phase::Compacted;
        for slot in cells.iter_mut() {
            let state = &self.cells[slot.id.0 as usize];
            slot.cell.compact_report(|_from, to| {
                // Only the TRANSFORM mark: the slot mirror needs no
                // per-move trigger — `sync_all`'s self-healing boundary
                // scan compares every occupied row's slot shadow against
                // the slot column and catches swap destinations itself.
                state.dirty_transforms.mark(to);
            });
        }
    }

    /// Frame-boundary upload (§4): coalesced dirty-row write of each cell's
    /// transform-column region into its disjoint slice of the shared SSBO,
    /// then the slot mirror via the self-healing boundary scan — every
    /// occupied row whose shadow disagrees with the slot column is marked,
    /// staged, and uploaded, regardless of HOW the slot got there. Closes
    /// the boundary (next phase is the write window). Post-condition: mirror
    /// entries `[row_base, row_base + rows_in_use)` equal
    /// `slot_base + slot_column()[row]` exactly.
    pub(crate) fn sync_all(&mut self, cells: &mut [CellSlot<'_>]) -> SyncStats {
        debug_assert_eq!(self.phase, Phase::Compacted, "sync_all must follow compact_all");
        self.phase = Phase::Write;
        let mut total = SyncStats { ranges: 0, bytes: 0 };
        for slot in cells.iter_mut() {
            let rows = slot.cell.rows_in_use() as usize;
            let col = slot
                .cell
                .column_for::<[f32; 16]>()
                .expect("cell has no [f32; 16] transform column");
            let state = &self.cells[slot.id.0 as usize];
            let stats = self.transforms.sync_region(&self.queue, &col[..rows], state.row_base, &state.dirty_transforms);
            total.ranges += stats.ranges;
            total.bytes += stats.bytes;

            // Self-healing boundary scan — the ONLY slot-mirror dirty
            // trigger (Task 4 re-review): compare every occupied row's
            // shadow against the authoritative slot column and mark exactly
            // the mismatches, whatever moved the slot there (write after
            // alloc, compaction swap, or an alloc re-occupying a vacated row
            // that is never written — the ghost-duplicate case no per-event
            // trigger caught). O(rows) u32 compares per cell per boundary;
            // uploads only actual mismatches.
            let col0 = slot.cell.slot_column();
            let state = &mut self.cells[slot.id.0 as usize];
            for row in 0..rows as u32 {
                let expect = col0[row as usize];
                if state.slot_shadow[row as usize] != expect {
                    state.dirty_slots.mark(row);
                    state.slot_scratch[row as usize] = state.slot_base + expect;
                    state.slot_shadow[row as usize] = expect;
                }
            }
            let state = &self.cells[slot.id.0 as usize];
            let stats =
                self.slot_mirror.sync_region(&self.queue, &state.slot_scratch[..rows], state.row_base, &state.dirty_slots);
            total.ranges += stats.ranges;
            total.bytes += stats.bytes;
        }
        total
    }

    pub fn row_region_base(&self, id: CellId) -> u32 {
        self.cells[id.0 as usize].row_base
    }

    pub fn transform_buffer(&self) -> &wgpu::Buffer {
        self.transforms.buffer()
    }

    /// Row-indexed global-slot mirror (T4; C6 GPU handle validation).
    ///
    /// Guarantee: after every `sync_all`, entries
    /// `[row_base, row_base + rows_in_use)` of each registered cell mirror
    /// `slot_base + slot_column()[row]` EXACTLY — the boundary scan
    /// self-heals any row whose slot changed by any mechanism (write after
    /// alloc, compaction swap, or an alloc into a previously vacated row
    /// that is never written).
    ///
    /// Mirror entries beyond a cell's `rows_in_use` are stale-but-inert:
    /// compaction shrinks the row count without erasing the mirror tail, so
    /// those entries may hold old slot IDs. Nothing may index the mirror
    /// past the harvested row count (M3 contract) — consumers dispatch over
    /// `rows_in_use`, never region capacity.
    pub fn slot_mirror_buffer(&self) -> &wgpu::Buffer {
        self.slot_mirror.buffer()
    }

    pub fn generation_buffer(&self) -> &wgpu::Buffer {
        self.generations.buffer()
    }

    /// Placeholder buffer; layout arrives in M3.
    pub fn material_buffer(&self) -> &wgpu::Buffer {
        &self.material
    }

    /// Per-cell metadata SSBO (α: allocated, no writer).
    pub fn cell_metadata_buffer(&self) -> &wgpu::Buffer {
        &self.cell_metadata
    }
}
