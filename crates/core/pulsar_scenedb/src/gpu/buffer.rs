use crate::page::Pod;
use std::any::Any;
use std::marker::PhantomData;

/// Delta-sync instrumentation: how many `write_buffer` ranges and bytes the
/// last sync issued. The delta-minimality gates assert on this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncStats {
    pub ranges: u32,
    pub bytes: u64,
}

/// Gap-tolerant coalescing threshold for `SceneBuffer::sync_region`'s
/// run-detection loop (M3-b T3, R-PERF-1 measured decision — see
/// `.superpowers/sdd/m3b-task-3-report.md`): two dirty runs separated by
/// FEWER than `GAP_MERGE_THRESHOLD` clean rows merge into one upload range,
/// re-uploading the intervening clean rows' bytes alongside the dirty ones.
/// `GAP_MERGE_THRESHOLD == 0` reproduces today's strict-adjacency behavior
/// EXACTLY: a single clean row always splits a run (`gap_len` reaches the
/// threshold on the very first clean row after a run starts).
///
/// Safety of re-uploading a clean row's bytes: sound. The CPU column is the
/// sole source of truth for every buffer `sync_region` writes into (transform,
/// InstanceInfo, slot mirror) — nothing GPU-side ever writes back to these
/// SSBOs (every M3-b bind-group entry for them is `read_only: true`,
/// `crates/renderer/helio/crates/helio-scenedb/src/lib.rs`), and the
/// generation buffer's writes (`GenerationBuffer::write`/`rebuild`/
/// `rebuild_region`) never route through `sync_region` at all — a clean
/// row's dirty bit being false is exactly the invariant "VRAM already holds
/// this row's current CPU bytes," so re-uploading it is a byte-identical
/// no-op. See the M3-b T3 report for the full argument (write-path,
/// compaction, and recycled-tail cases).
///
/// `pub` (not `pub(crate)`): the M3-b T3 bench/tests live in `benches/` and
/// `tests/`, which compile as separate crates against `pulsar_scenedb`'s
/// public surface — they read this constant to self-document which G a given
/// sweep row/gate was built against, rather than hardcoding a duplicate copy
/// that could silently drift from the real value.
///
/// **Decision (M3-b T3, measured, REJECT): stays 0.** Swept G ∈ {0, 4, 16,
/// 64} — the required set — plus an exploratory G=128 to falsify the "accept"
/// hypothesis with data (`legacy_model_bench`, S=10k):
///
/// - The R-PERF-1 register's actual motivating case (T4: stride-100 scatter,
///   ~99-row gaps) is a NO-OP for every G in the required set — 99 > 64, so
///   nothing merges; ranges/bytes/CPU are identical to G=0 at G=4/16/64. G=128
///   (>99) DOES merge it: ranges 100→10, CPU 119µs→52µs (2.3x), but bytes
///   6,400→576,640 (**90x** inflation, ~88% of the full 655,360 B region) —
///   confirmed by the bench's own byte-volume honesty assertion, which fired
///   exactly as designed (`left: 6400, right: 576640`) because gap-merging
///   changes the total bytes moved at fixed M, invalidating that check's
///   precondition. A 90x bandwidth tax for a 2.3x CPU win, on the exact case
///   this remediation was chartered to fix, is not a trade worth taking —
///   and it directly undermines claim #1/#3 in the perf-validation report
///   (100-1000x fewer bytes than legacy; minimal coalesced ranges).
/// - A DIFFERENT, denser workload (stride-10 scatter, ~9-row gaps — not the
///   T4 case) DOES win at G=16/64: ranges 1000→10, CPU ~930µs→~68µs (13.7x),
///   but bytes 64,000→634,240 (~9.9x, ~97% of the region) — i.e. gap-merging
///   silently converts that pattern into a near-full-buffer reupload. Since
///   `GAP_MERGE_THRESHOLD` is one crate-wide compile-time constant with no
///   per-call opt-in, shipping it nonzero would impose that same silent
///   bandwidth tax on ANY future caller whose dirty pattern happens to have
///   small gaps, whether or not they want the CPU/range tradeoff — a footgun,
///   not a scoped accept.
///
/// Net: no G in the swept range closes R-PERF-1's motivating case without a
/// bandwidth cost the perf-validation report's own headline claims call
/// unacceptable; the G that WOULD close it inflates bytes worse still. The
/// knob is kept (tested, documented, `GAP_MERGE_THRESHOLD == 0` provably
/// reproduces strict adjacency — see `gpu_store.rs`'s
/// `sync_region_gap_of_one_row_splits_at_g0` gate) as a **measured, closed**
/// decision, not a live config surface. Full sweep table, byte-volume
/// arithmetic, and the correctness argument:
/// `.superpowers/sdd/m3b-task-3-report.md`.
pub const GAP_MERGE_THRESHOLD: u32 = 0;

/// Type-erased GPU buffer dispatch: allows `SceneGpuStore` to sync any
/// column's byte data through a matching `SceneBuffer<T>` without knowing
/// `T` at compile time.
pub trait GpuBufferDispatch: Send + Sync {
    /// Coalescing delta-sync from a byte slice (reinterpreted as `&[T]`
    /// inside the implementation).  Clears the dirty mask.
    fn sync_region(
        &self,
        queue: &wgpu::Queue,
        data: &[u8],
        row_base: u32,
        dirty: &super::DirtyMask,
    ) -> SyncStats;

    fn element_size(&self) -> usize;
    fn buffer(&self) -> &wgpu::Buffer;
    fn capacity(&self) -> u32;
    fn as_any(&self) -> &dyn Any;

    /// Unconditional bulk write from raw bytes (reinterpreted as `&[T]`).
    fn write_rows_raw(&self, queue: &wgpu::Queue, data: &[u8], row_base: u32);
}

/// One persistent **row-indexed** scene SSBO (M2a §3/§4; M2b-α §2: dirty
/// state now lives beside the cell, in a caller-supplied `DirtyMask`).
/// Generic over the C5 element type. Allocated once at capacity; never
/// reallocates.
pub struct SceneBuffer<T: Pod> {
    buf: wgpu::Buffer,
    capacity: u32,
    _elem: PhantomData<T>,
}

impl<T: Pod> SceneBuffer<T> {
    pub fn new(device: &wgpu::Device, label: &str, capacity: u32) -> Self {
        let size = capacity as u64 * std::mem::size_of::<T>() as u64;
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        Self {
            buf,
            capacity,
            _elem: PhantomData,
        }
    }

    /// Coalescing delta-upload of one CELL REGION (design Rev 2 §2): identical
    /// to the M2a streaming coalescer but offset by `region_base` rows, with
    /// the dirty mask supplied by the cell's `CellGpuState`. Clears the mask.
    pub fn sync_region(
        &self,
        queue: &wgpu::Queue,
        cpu: &[T],
        region_base: u32,
        dirty: &super::DirtyMask,
    ) -> SyncStats {
        assert!(
            region_base as u64 + cpu.len() as u64 <= self.capacity as u64,
            "region [{region_base}, +{}) exceeds SSBO capacity {} — scene buffers never reallocate",
            cpu.len(),
            self.capacity
        );
        assert!(
            dirty.capacity() as u64 >= cpu.len() as u64,
            "dirty mask smaller than the CPU slice — wrong mask for this cell"
        );
        let stride = std::mem::size_of::<T>() as u64;
        let n = cpu.len() as u32;
        let mut stats = SyncStats { ranges: 0, bytes: 0 };
        // Gap-tolerant run detection (M3-b T3, R-PERF-1): `run_start` is the
        // start of the current (possibly gap-bridged) range; `run_end` is the
        // EXCLUSIVE end of the last row known to be dirty within it (never
        // extended into an un-bridged gap); `gap_len` counts consecutive
        // clean rows seen since the last dirty row. A gap is bridged (the
        // range keeps accumulating) while `gap_len < GAP_MERGE_THRESHOLD`;
        // once `gap_len` reaches the threshold the gap can never shrink
        // again (no dirty row seen since), so the range flushes immediately
        // rather than waiting for lookahead. `GAP_MERGE_THRESHOLD == 0`
        // collapses this to the original strict-adjacency loop exactly: the
        // very first clean row after a run makes `gap_len == 1 >= 0`, so it
        // flushes on that row precisely like the old `(false, Some(start))`
        // arm did.
        let mut run_start: Option<u32> = None;
        let mut run_end: u32 = 0;
        let mut gap_len: u32 = 0;
        for row in 0..n {
            if dirty.is_marked(row) {
                if run_start.is_none() {
                    run_start = Some(row);
                }
                run_end = row + 1;
                gap_len = 0;
            } else if let Some(start) = run_start {
                gap_len += 1;
                if gap_len >= GAP_MERGE_THRESHOLD {
                    self.flush(queue, cpu, region_base, start, run_end, stride, &mut stats);
                    run_start = None;
                    gap_len = 0;
                }
            }
        }
        if let Some(start) = run_start {
            self.flush(queue, cpu, region_base, start, run_end, stride, &mut stats);
        }
        dirty.clear_all();
        stats
    }

    /// Unconditional bulk write of a region prefix (registration warm-up /
    /// device-loss rebuild). Not delta-tracked.
    pub fn write_rows(&self, queue: &wgpu::Queue, cpu: &[T], region_base: u32) {
        assert!(region_base as u64 + cpu.len() as u64 <= self.capacity as u64);
        if !cpu.is_empty() {
            queue.write_buffer(&self.buf, region_base as u64 * std::mem::size_of::<T>() as u64, super::as_bytes(cpu));
        }
    }

    fn flush(
        &self,
        queue: &wgpu::Queue,
        cpu: &[T],
        region_base: u32,
        start: u32,
        end: u32,
        stride: u64,
        stats: &mut SyncStats,
    ) {
        let bytes = super::as_bytes(&cpu[start as usize..end as usize]);
        queue.write_buffer(&self.buf, (region_base as u64 + start as u64) * stride, bytes);
        stats.ranges += 1;
        stats.bytes += bytes.len() as u64;
    }

    pub fn buffer(&self) -> &wgpu::Buffer {
        &self.buf
    }

    pub fn capacity(&self) -> u32 {
        self.capacity
    }
}

impl<T: Pod + Send + Sync + 'static> GpuBufferDispatch for SceneBuffer<T> {
    fn sync_region(
        &self,
        queue: &wgpu::Queue,
        data: &[u8],
        row_base: u32,
        dirty: &super::DirtyMask,
    ) -> SyncStats {
        // Reinterpret the byte slice as &[T]
        assert_eq!(
            data.len() % std::mem::size_of::<T>(),
            0,
            "byte slice length not a multiple of element size"
        );
        let typed: &[T] =
            unsafe { std::slice::from_raw_parts(data.as_ptr() as *const T, data.len() / std::mem::size_of::<T>()) };
        SceneBuffer::sync_region(self, queue, typed, row_base, dirty)
    }

    fn element_size(&self) -> usize {
        std::mem::size_of::<T>()
    }

    fn buffer(&self) -> &wgpu::Buffer {
        &self.buf
    }

    fn capacity(&self) -> u32 {
        self.capacity
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn write_rows_raw(&self, queue: &wgpu::Queue, data: &[u8], row_base: u32) {
        assert_eq!(data.len() % std::mem::size_of::<T>(), 0);
        let typed: &[T] = unsafe {
            std::slice::from_raw_parts(data.as_ptr() as *const T, data.len() / std::mem::size_of::<T>())
        };
        self.write_rows(queue, typed, row_base);
    }
}
