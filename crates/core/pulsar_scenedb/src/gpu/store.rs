use super::{EngineGpuContext, GenerationBuffer, SceneBuffer, SubmissionTracker, SyncStats};
use crate::cell::{CellStorage, PendingRetire};
use crate::handle::Handle;
use std::collections::VecDeque;
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
        }
    }

    pub fn tracker(&self) -> &SubmissionTracker {
        &self.tracker
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
    /// is later retired. Writing it here — the only call site that ever
    /// observes a live handle — keeps the generation buffer a true mirror of
    /// `HandleRegistry::generations()` for every allocated slot, not just
    /// retired ones (C6).
    pub fn write_transform(&self, cell: &mut CellStorage, handle: Handle, m: &[f32; 16]) -> bool {
        debug_assert_eq!(self.phase, Phase::Write, "mutation outside the write window");
        let Some(row) = cell.row_of(handle) else {
            return false;
        };
        let col = cell
            .column_for_mut::<[f32; 16]>()
            .expect("cell has no [f32; 16] transform column");
        col[row as usize] = *m;
        self.transforms.mark_row_dirty(row);
        self.generations.write(&self.queue, handle.index(), handle.generation());
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
            self.generations.write(&self.queue, pending.slot, pending.next_gen);
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
    pub fn rebuild_from(ctx: &EngineGpuContext, cfg: GpuStoreConfig, cell: &CellStorage) -> Self {
        let mut store = Self::new(ctx, cfg);
        let rows = cell.rows_in_use();
        for row in 0..rows {
            store.transforms.mark_row_dirty(row);
        }
        let col = cell
            .column_for::<[f32; 16]>()
            .expect("cell has no [f32; 16] transform column");
        store.transforms.sync(&store.queue, &col[..rows as usize]);
        store.generations.rebuild(&store.queue, cell.registry().generations());
        store
    }

    pub fn transform_buffer(&self) -> &wgpu::Buffer {
        self.transforms.buffer()
    }

    pub fn generation_buffer(&self) -> &wgpu::Buffer {
        self.generations.buffer()
    }
}
