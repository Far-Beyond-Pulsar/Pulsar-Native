use super::{EngineGpuContext, GenerationBuffer, SceneBuffer, SubmissionTracker, SyncStats};
use crate::cell::{CellStorage, PendingRetire};
use crate::handle::Handle;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

/// Fixed store capacities (SSBOs never reallocate — exceeding them is a hard
/// error at the call site, §8).
#[derive(Debug, Clone, Copy)]
pub struct GpuStoreConfig {
    pub max_rows: u32,
    pub max_slots: u32,
}

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

/// The SceneDB-owned device-side store (M2a §7): persistent scene SSBOs,
/// the mirrored-column writer, delta-sync, and the retirement drain.
/// Constructed on the engine-level device context (C0).
pub struct GpuStore {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    transforms: SceneBuffer<[f32; 16]>,
    generations: GenerationBuffer,
    tracker: SubmissionTracker,
    pending: VecDeque<QueuedRetire>,
    phase: Phase,
    /// CPU-side shadow of the last generation uploaded per slot (§4
    /// delta-minimality on the write path): `write_transform` only issues a
    /// generation-buffer write when the handle's generation differs from the
    /// shadow. Zero-initialized, matching VRAM's zero-init. Atomic because
    /// `write_transform` takes `&self` (the same Relaxed dirty-word pattern
    /// as `SceneBuffer`).
    gen_shadow: Vec<AtomicU32>,
    /// Instrumentation: total `generations.write(...)` calls issued, so tests
    /// can assert generation-upload minimality.
    gen_writes: AtomicU64,
}

impl GpuStore {
    pub fn new(ctx: &EngineGpuContext, cfg: GpuStoreConfig) -> Self {
        Self {
            device: Arc::clone(ctx.device()),
            queue: Arc::clone(ctx.queue()),
            transforms: SceneBuffer::new(ctx.device(), "scenedb-instances", cfg.max_rows),
            generations: GenerationBuffer::new(ctx.device(), cfg.max_slots),
            tracker: SubmissionTracker::new(),
            pending: VecDeque::new(),
            phase: Phase::Write,
            gen_shadow: (0..cfg.max_slots).map(|_| AtomicU32::new(0)).collect(),
            gen_writes: AtomicU64::new(0),
        }
    }

    pub fn tracker(&self) -> &SubmissionTracker {
        &self.tracker
    }

    /// Test instrument: how many generation-buffer writes this store has
    /// issued (asserting upload minimality, §4).
    #[doc(hidden)]
    pub fn generation_write_count(&self) -> u64 {
        self.gen_writes.load(Ordering::Relaxed)
    }

    /// Shadow-gated generation upload: writes VRAM (and the shadow) only when
    /// `generation` differs from the last value uploaded for `slot`.
    fn write_generation(&self, slot: u32, generation: u32) {
        if self.gen_shadow[slot as usize].load(Ordering::Relaxed) == generation {
            return;
        }
        self.generations.write(&self.queue, slot, generation);
        self.gen_shadow[slot as usize].store(generation, Ordering::Relaxed);
        self.gen_writes.fetch_add(1, Ordering::Relaxed);
    }

    /// The single mutation path for the GPU-mirrored transform column (§4):
    /// writes the core column AND sets the row's dirty bit in one operation.
    /// False for stale/invalid handles.
    ///
    /// Also stamps the handle's generation into the slot-indexed generation
    /// buffer (adaptation to §5/§7): the design's "written by retirement"
    /// trigger only ever bumps a slot's entry on retire, so a slot's *first*
    /// generation (assigned at `alloc`, which does not pass through
    /// `GpuStore` at all) would otherwise never reach VRAM until that slot
    /// is later retired. The stamp is shadow-gated (`gen_shadow`): a
    /// generation reaches VRAM on the first write after alloc and on
    /// retirement — NOT per `write_transform` call — so repeat writes to a
    /// live handle (the §4 hot path) issue zero generation-buffer traffic
    /// while the buffer still mirrors `HandleRegistry::generations()` for
    /// every allocated slot (C6).
    pub fn write_transform(&self, cell: &mut CellStorage, handle: Handle, m: &[f32; 16]) -> bool {
        debug_assert_eq!(self.phase, Phase::Write, "mutation outside the write window");
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
        self.transforms.mark_row_dirty(row);
        self.write_generation(handle.index(), handle.generation());
        true
    }

    /// §5 flow step 1: liveness-dead + pinned + enqueued against `serial`.
    /// Registry and GPU buffers unchanged until the serial completes.
    pub fn free_deferred(&mut self, cell: &mut CellStorage, handle: Handle, serial: u64) -> bool {
        debug_assert_eq!(self.phase, Phase::Write, "free_deferred outside the write window");
        let Some(pending) = cell.mark_pending_retire(handle) else {
            return false;
        };
        self.pending.push_back(QueuedRetire { pending, serial });
        true
    }

    /// §5 flow step 3, frame boundary, runs FIRST: for every queued entry
    /// whose serial is complete, write the new generation to VRAM, then
    /// commit in the registry — the gen bump reaches the GPU before the slot
    /// can be re-allocated (C6). Returns the number of slots retired.
    pub fn retire(&mut self, cell: &mut CellStorage) -> u32 {
        debug_assert_eq!(self.phase, Phase::Write, "retire must open the frame boundary");
        self.phase = Phase::Retired;
        let done = self.tracker.completed();
        let mut drained = 0;
        while let Some(front) = self.pending.front() {
            if front.serial > done {
                break; // FIFO serials: everything behind is also incomplete
            }
            let QueuedRetire { pending, .. } = self.pending.pop_front().unwrap();
            // Retirement always bumps the generation, so the shadow-gated
            // write always lands in VRAM (and updates the shadow) before the
            // registry commit can recycle the slot (C6).
            self.write_generation(pending.slot, pending.next_gen);
            cell.commit_retire(pending);
            drained += 1;
        }
        drained
    }

    /// Frame-boundary compaction (§4): every moved row's destination is
    /// marked dirty so the next sync re-uploads it.
    pub fn compact(&mut self, cell: &mut CellStorage) {
        debug_assert_eq!(self.phase, Phase::Retired, "compact must follow retire");
        self.phase = Phase::Compacted;
        cell.compact_report(|_from, to| self.transforms.mark_row_dirty(to));
    }

    /// Frame-boundary upload (§4): coalesced dirty-row write of the transform
    /// column; closes the boundary (next phase is the write window).
    pub fn sync(&mut self, cell: &CellStorage) -> SyncStats {
        debug_assert_eq!(self.phase, Phase::Compacted, "sync must follow compact");
        self.phase = Phase::Write;
        let col = cell
            .column_for::<[f32; 16]>()
            .expect("cell has no [f32; 16] transform column");
        self.transforms.sync(&self.queue, &col[..cell.rows_in_use() as usize])
    }

    /// Test 14: build a fresh store on a fresh device purely from the
    /// CPU-authoritative columns (no GPU-only state exists to lose).
    ///
    /// Precondition: no rows may be pinned (all pending retires drained via
    /// `retire()`) — recovery of in-flight retirement across a device loss is
    /// M4 scope; rebuilding while a pin is outstanding would strand it
    /// permanently (the pin bit lives only in `CellStorage`, and this fresh
    /// store has no queued `PendingRetire` to eventually unpin it).
    pub fn rebuild_from(ctx: &EngineGpuContext, cfg: GpuStoreConfig, cell: &CellStorage) -> Self {
        debug_assert!(
            (0..cell.rows_in_use()).all(|r| !cell.is_row_pinned(r)),
            "rebuild_from with in-flight retirement: drain retire() before device-loss rebuild — pins would be permanently stranded"
        );
        let mut store = Self::new(ctx, cfg);
        let rows = cell.rows_in_use();
        for row in 0..rows {
            store.transforms.mark_row_dirty(row);
        }
        let col = cell
            .column_for::<[f32; 16]>()
            .expect("cell has no [f32; 16] transform column");
        store.transforms.sync(&store.queue, &col[..rows as usize]);
        let gens = cell.registry().generations();
        store.generations.rebuild(&store.queue, gens);
        // Seed the shadow with every uploaded generation so it never drifts
        // from VRAM (slots beyond `gens.len()` stay 0 == VRAM's zero-init).
        for (slot, &generation) in gens.iter().enumerate() {
            store.gen_shadow[slot].store(generation, Ordering::Relaxed);
        }
        store
    }

    pub fn transform_buffer(&self) -> &wgpu::Buffer {
        self.transforms.buffer()
    }

    pub fn generation_buffer(&self) -> &wgpu::Buffer {
        self.generations.buffer()
    }
}
