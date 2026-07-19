//! Delta-sync vs legacy full-resync head-to-head (perf-val Task 4) — the
//! centerpiece measurement of the performance-validation campaign: does
//! `write_transform` + boundary-sync deliver work proportional to mutated
//! rows (contract claims #1/#2,
//! `.superpowers/sdd/stress-recon-perf-contract.md` §A.1), against
//! `sync_scene`'s O(total entities)-per-frame push model (same doc, §B)?
//!
//! Run: `cargo bench -p pulsar_scenedb --features gpu --bench legacy_model_bench`
//!
//! No criterion (task brief, same reasoning as `gpu_timing.rs`): this is a
//! plain `main()` (`harness = false`) with its own warm-up/iteration-count/
//! statistics, printing the matrix directly.
//!
//! ## Fixture shape
//!
//! `MAX_PAGE_CAPACITY` (`src/page.rs`) hard-caps a `CellStorage`/page at 1024
//! rows, so a scene of S rows is `⌈S/1024⌉` cells of capacity 1024 each (the
//! last cell partially filled) — the realistic multi-cell shape, not one
//! oversized cell. Both paths below build IDENTICAL content: the same
//! `rows_per_cell` split, the same per-row transform values (`mat(i)`).
//!
//! ## The two paths
//!
//! - **SceneDB (delta-sync):** `write_transform` on the mutated subset (a
//!   CONTIGUOUS prefix per touched cell — the base matrix; §5 below covers
//!   the scattered variant) then one `BoundaryPhase::run` (retire → compact →
//!   sync) — the same all-in-one call `scenedb_bench.rs`'s
//!   `region_sync_1024_dirty_rows` times, generalized to N cells via
//!   `CellSlot`. CPU wall time is `Instant`-bracketed around the ENTIRE
//!   mark-then-boundary sequence — deliberately NOT split the way
//!   `gpu_timing.rs`'s GPU-ns bracket excludes marking from its timed
//!   section. That split is right for isolating pure GPU copy cost; it would
//!   be dishonest here, where the point is "what does one SceneDB frame
//!   actually cost end to end," and `write_transform`'s bookkeeping is part
//!   of that cost, not overhead to subtract away.
//! - **Legacy model (`sync_scene`'s shape, LOWER-BOUND faithful):** every
//!   frame, build a FRESH `Vec<[f32; 16]>` covering every cell's full
//!   CAPACITY (not just occupied rows) — the DFS-clone-and-allocate analog of
//!   `scene/mod.rs::get_all_snapshots` — then issue ONE `write_buffer` per
//!   cell of that cell's full region, unconditionally, regardless of what (if
//!   anything) changed. This is deliberately NOT a caricature: no
//!   `serde_json`, no light-object destroy/recreate, no BVH rebuild in the
//!   loop, even though the real `sync_scene`
//!   (`engine_backend/.../renderer.rs:686-817`) pays for all three every
//!   frame on top of the DFS clone and the full re-upload modeled here. Our
//!   simulation is therefore a LOWER BOUND on legacy cost — the measured
//!   speedup below is conservative, not inflated.
//!
//! ## Honesty self-checks (in-bench, not just printed)
//!
//! 1. SceneDB bytes at M=0% == 0 (claim #2's sharpest edge — restated here as
//!    a bench-level sanity check; the actual TEST-level gate already exists:
//!    `tests/alloc_gate_gpu.rs::scene_gpu_store_boundary_sync_zero_dirty_rows_zero_alloc`
//!    asserts `(stats.ranges, stats.bytes) == (0, 0)` on a clean second
//!    boundary — cited here rather than duplicated, per the task brief).
//! 2. Legacy bytes are the SAME fixed `cells * 1024 * 64` every frame,
//!    independent of M (legacy has no mutation-awareness to begin with).
//! 3. SceneDB bytes at M=100% are within one cell's worth of padding of
//!    legacy bytes (the only difference is the last cell's shortfall between
//!    actual occupied rows and the full 1024-row capacity legacy always
//!    re-uploads).
//! 4. SceneDB CPU time is monotonic non-decreasing in M, per S, within a
//!    fixed per-S slack (same discipline as `gpu_timing.rs`'s
//!    `TREND_SLACK_NS` — a fixed tolerance band, not a measured-noise-derived
//!    one, so the check can't inflate itself away).
//!
//! ## Pump discipline (perf-val T1 lesson)
//!
//! Every iteration of every loop below ends its UNTIMED section with
//! `queue.submit(empty()) + device.poll(wait)`, exactly like
//! `scenedb_bench.rs::bench_region_sync_1024_dirty_rows` — otherwise the
//! `write_buffer` pending-writes staging belt grows unbounded across
//! iterations, especially dangerous for the legacy path at S=100k (6.4 MB of
//! full-region re-upload per frame; unpumped across dozens of iterations that
//! is hundreds of MB of un-reclaimed staging).

use pulsar_scenedb::gpu::{
    CellId, CellSlot, EngineGpuContext, FrameDriver, RegionClassConfig, SceneGpuConfig,
    SceneGpuStore, SyncStats, GAP_MERGE_THRESHOLD,
};
use pulsar_scenedb::{CellStorage, CellType, Handle, TypeToken};
use std::sync::Arc;
use std::time::Instant;

/// `src/page.rs::MAX_PAGE_CAPACITY` — a page/cell never holds more than this
/// many rows.
const CELL_CAP: u32 = 1024;
/// `[f32; 16]` transform matrix stride in bytes.
const MAT_BYTES: u64 = 64;
const WARMUP: usize = 10;
const ITERS: usize = 50;
const S_VALUES: [u32; 3] = [1_000, 10_000, 100_000];
const M_VALUES: [f64; 5] = [0.0, 0.1, 1.0, 10.0, 100.0];
/// Amplification factor for the GPU-ns pair at 10k (T3's
/// "signal-vs-noise-floor" lesson: a single frame's write volume is too small
/// to rise above the two-submit bracket's tens-of-microseconds fixed
/// overhead; `gpu_timing.rs` used 256 independent 1024-row cells for the
/// same reason). Kept smaller here (32) than T3's 256 because each "copy" is
/// a full 10-cell/10k-row scene, not one cell — 256 copies would mean
/// building 2.56M handles just for fixture setup, blowing the bench's
/// wall-clock budget for a supplementary data point.
const GPU_REPEATS: u32 = 32;

fn test_context() -> EngineGpuContext {
    // Mirrors `gpu_timing.rs::test_context` — TIMESTAMP_QUERY features are
    // requested even though only the §6 GPU-ns pair uses them; every other
    // measurement in this file just ignores the extra capability.
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    }))
    .expect("no adapter — GPU benches need a local GPU");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("legacy-model-bench"),
        required_features: wgpu::Features::TIMESTAMP_QUERY
            | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS,
        ..Default::default()
    }))
    .expect("device — adapter must support TIMESTAMP_QUERY (verified present on this host's RTX 5080/Vulkan adapter, stress-recon-infrastructure.md §5)");
    EngineGpuContext::new(Arc::new(device), Arc::new(queue))
}

fn mat(seed: f32) -> [f32; 16] {
    core::array::from_fn(|i| seed + i as f32)
}

/// Reinterpret a `[[f32; 16]]` slice as bytes for a raw `queue.write_buffer`
/// call (bench-local equivalent of the crate's private `gpu::as_bytes`, same
/// as `gpu_timing.rs`).
fn as_bytes(s: &[[f32; 16]]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(s.as_ptr() as *const u8, std::mem::size_of_val(s)) }
}

fn bench_transform_cell(capacity: u32) -> CellStorage {
    let ct = CellType::new("legacy-model-bench-instance")
        .with(TypeToken::of::<[f32; 16]>())
        .build()
        .unwrap();
    CellStorage::from_cell_type(&ct, capacity).unwrap()
}

/// `⌈rows/1024⌉` cells, each holding `min(remaining, 1024)` rows — the last
/// cell is the only one ever partial.
fn rows_per_cell(rows: u32) -> Vec<u32> {
    let mut remaining = rows;
    let mut out = Vec::new();
    while remaining > 0 {
        let take = remaining.min(CELL_CAP);
        out.push(take);
        remaining -= take;
    }
    out
}

/// M% of the TOTAL row count, taken as a CONTIGUOUS run starting at global
/// row 0 and spilling cell-by-cell (each cell's share is itself a contiguous
/// `0..n` prefix — the base matrix's mutation pattern; §5's scattered variant
/// is the contrast case). Returns one count per cell.
fn contiguous_counts(rows_per_cell: &[u32], m_percent: f64) -> Vec<u32> {
    let total: u32 = rows_per_cell.iter().sum();
    let mut want = ((total as f64) * m_percent / 100.0).round() as u32;
    rows_per_cell
        .iter()
        .map(|&r| {
            let take = want.min(r);
            want -= take;
            take
        })
        .collect()
}

/// Every `stride`-th row in GLOBAL row order (spread across cells), as
/// per-cell LOCAL row indices — the scattered counterpart to
/// [`contiguous_counts`], quantifying the T2 range-count finding
/// (`tests/alloc_gate_gpu.rs`'s
/// `scene_gpu_store_boundary_sync_alloc_count_independent_of_dirty_row_count`
/// doc: scattered dirtiness scales range/alloc count with the run count, not
/// the row count) at the frame level.
fn scattered_local_indices(rows_per_cell: &[u32], stride: u32) -> Vec<Vec<u32>> {
    let mut out = Vec::with_capacity(rows_per_cell.len());
    let mut global_offset = 0u32;
    for &r in rows_per_cell {
        let end_global = global_offset + r;
        let mut g = global_offset.div_ceil(stride) * stride;
        let mut local = Vec::new();
        while g < end_global {
            local.push(g - global_offset);
            g += stride;
        }
        out.push(local);
        global_offset = end_global;
    }
    out
}

/// mean / min / p95 (nearest-rank) over a sample set.
fn stats(mut samples: Vec<f64>) -> (f64, f64, f64) {
    samples.sort_by(|a, b| a.total_cmp(b));
    let n = samples.len();
    let mean = samples.iter().copied().sum::<f64>() / n as f64;
    let min = samples[0];
    let p95_idx = (((n - 1) as f64) * 0.95).round() as usize;
    let p95 = samples[p95_idx];
    (mean, min, p95)
}

/// Build one SceneDB-side scene: `⌈rows/1024⌉` cells of capacity 1024,
/// registered under one region class sized to hold every cell resident at
/// once (never evicted — this bench measures steady-state sync cost, not
/// eviction).
fn build_scene(
    ctx: &EngineGpuContext,
    rows: u32,
) -> (SceneGpuStore, Vec<CellStorage>, Vec<CellId>, Vec<Vec<Handle>>, Vec<u32>) {
    let rpc = rows_per_cell(rows);
    let n = rpc.len() as u32;
    let cfg = SceneGpuConfig {
        classes: vec![RegionClassConfig { capacity: CELL_CAP, max_resident_cells: n }],
        tombstone_headroom: 64,
        max_cells_metadata: n,
    };
    let mut store = SceneGpuStore::new(ctx, cfg);
    let mut cells: Vec<CellStorage> = rpc.iter().map(|_| bench_transform_cell(CELL_CAP)).collect();
    let handles: Vec<Vec<Handle>> = cells
        .iter_mut()
        .zip(rpc.iter())
        .map(|(c, &r)| (0..r).map(|_| c.alloc().unwrap()).collect())
        .collect();
    let ids: Vec<CellId> = cells.iter().map(|c| store.register_cell(c, 0).unwrap()).collect();
    (store, cells, ids, handles, rpc)
}

/// Untimed: drains `register_cell`'s full-region warm-up dirty mark via one
/// boundary run + pump, so the first REAL measurement starts from a clean
/// dirty-mask baseline (mirrors `tests/alloc_gate_gpu.rs`'s warm-up-boundary
/// discipline).
fn drain_registration_warmup(
    ctx: &EngineGpuContext,
    store: &mut SceneGpuStore,
    cells: &mut [CellStorage],
    ids: &[CellId],
    frames: &mut FrameDriver,
) -> pulsar_scenedb::gpu::SimulateA {
    let sim = frames.begin();
    let boundary = sim.end().end().end();
    let mut slots: Vec<CellSlot> =
        cells.iter_mut().zip(ids.iter()).map(|(c, &id)| CellSlot { id, cell: c }).collect();
    let warm_stats = boundary.run(store, &mut slots);
    std::hint::black_box(warm_stats);
    ctx.queue().submit(std::iter::empty());
    ctx.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    frames.begin()
}

/// One SceneDB frame: mark `row_lists[cell]`'s rows dirty per cell, then one
/// boundary run. Slot-Vec construction AND the row-selection lists stay
/// OUTSIDE the timed bracket (T3/T4 review discipline: bench-harness
/// bookkeeping is not part of "the frame cost" — the lists are stable
/// fixture data per (S, M) config, precomputed once by the caller).
/// Returns (cpu_micros, stats) and refills `sim`.
#[allow(clippy::too_many_arguments)]
fn run_scenedb_frame(
    ctx: &EngineGpuContext,
    store: &mut SceneGpuStore,
    cells: &mut [CellStorage],
    ids: &[CellId],
    handles: &[Vec<Handle>],
    frames: &mut FrameDriver,
    sim: &mut Option<pulsar_scenedb::gpu::SimulateA>,
    row_lists: &[Vec<u32>],
) -> (f64, SyncStats) {
    let this_sim = sim.take().expect("witness refilled at the end of every iteration");
    let mut slots: Vec<CellSlot> =
        cells.iter_mut().zip(ids.iter()).map(|(c, &id)| CellSlot { id, cell: c }).collect();
    let start = Instant::now();
    for (cell_idx, (slot, hs)) in slots.iter_mut().zip(handles.iter()).enumerate() {
        for &li in &row_lists[cell_idx] {
            let h = hs[li as usize];
            store.write_transform(slot.id, slot.cell, h, &mat(li as f32), &this_sim);
        }
    }
    let boundary = this_sim.end().end().end();
    let sync_stats = boundary.run(store, &mut slots);
    let elapsed = start.elapsed();
    // Untimed pump (T1 lesson).
    ctx.queue().submit(std::iter::empty());
    ctx.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    *sim = Some(frames.begin());
    (elapsed.as_secs_f64() * 1e6, sync_stats)
}

/// The legacy-model path for one scene size: `⌈rows/1024⌉ * 1024 * 64`
/// bytes re-uploaded EVERY frame regardless of mutation, via a fresh
/// `Vec<[f32; 16]>` snapshot (the DFS-clone analog) and one `write_buffer`
/// per cell's full region. Returns (mean_us, p95_us, bytes_per_frame).
fn measure_legacy(ctx: &EngineGpuContext, rows: u32) -> (f64, f64, u64) {
    let rpc = rows_per_cell(rows);
    let n_cells = rpc.len() as u32;
    let buf_rows = n_cells * CELL_CAP;
    let buffer = ctx.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("legacy-full-upload"),
        size: buf_rows as u64 * MAT_BYTES,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });

    let mut samples = Vec::with_capacity(ITERS);
    let mut last_bytes = 0u64;
    for it in 0..(WARMUP + ITERS) {
        let start = Instant::now();
        // Fresh full-scene snapshot allocation every frame — the DFS-clone
        // analog of `scene/mod.rs::get_all_snapshots` (recursive DFS +
        // per-level Vec append). Sized to full CELL capacity per cell (not
        // just occupied rows): `sync_scene` re-uploads the whole region it
        // owns every frame, independent of occupancy.
        let snapshot: Vec<[f32; 16]> = (0..buf_rows).map(|i| mat(i as f32)).collect();
        let mut bytes = 0u64;
        for c in 0..n_cells {
            let lo = (c * CELL_CAP) as usize;
            let hi = lo + CELL_CAP as usize;
            ctx.queue().write_buffer(&buffer, lo as u64 * MAT_BYTES, as_bytes(&snapshot[lo..hi]));
            bytes += CELL_CAP as u64 * MAT_BYTES;
        }
        let elapsed = start.elapsed();
        // Untimed pump (T1 lesson) — critical here: 100k rows = 6.4 MB/frame
        // of full re-upload, unbounded staging growth would be severe.
        ctx.queue().submit(std::iter::empty());
        ctx.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
        if it >= WARMUP {
            samples.push(elapsed.as_secs_f64() * 1e6);
            last_bytes = bytes;
        }
    }
    let (mean, _min, p95) = stats(samples);
    (mean, p95, last_bytes)
}

/// A 2-slot GPU timestamp bracket — copied wholesale from `gpu_timing.rs`
/// (T3's reuse note: `GpuTimer` is deliberately dependency-light so later
/// tasks can copy it; integration benches cannot share a module across bench
/// binaries).
struct GpuTimer {
    query_set: wgpu::QuerySet,
    resolve_buf: wgpu::Buffer,
    staging_buf: wgpu::Buffer,
    period_ns: f32,
}

impl GpuTimer {
    fn new(ctx: &EngineGpuContext) -> Self {
        let query_set = ctx.device().create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("legacy-bench-gpu-timing-query-set"),
            ty: wgpu::QueryType::Timestamp,
            count: 2,
        });
        let resolve_buf = ctx.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("legacy-bench-gpu-timing-resolve"),
            size: 16,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let staging_buf = ctx.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("legacy-bench-gpu-timing-staging"),
            size: 16,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let period_ns = ctx.queue().get_timestamp_period();
        Self { query_set, resolve_buf, staging_buf, period_ns }
    }

    /// Two-submit bracket — see `gpu_timing.rs`'s file doc comment for why a
    /// single-submit bracket reads flat/dead (wgpu-core splices the pending-
    /// writes belt at position 0 of every submission, before both timestamps
    /// if they share one `submit()` call).
    fn measure_ns(&self, ctx: &EngineGpuContext, work: impl FnOnce()) -> u64 {
        let mut enc_start = ctx.device().create_command_encoder(&Default::default());
        enc_start.write_timestamp(&self.query_set, 0);
        ctx.queue().submit([enc_start.finish()]);

        work();

        let mut enc_end = ctx.device().create_command_encoder(&Default::default());
        enc_end.write_timestamp(&self.query_set, 1);
        enc_end.resolve_query_set(&self.query_set, 0..2, &self.resolve_buf, 0);
        enc_end.copy_buffer_to_buffer(&self.resolve_buf, 0, &self.staging_buf, 0, 16);
        ctx.queue().submit([enc_end.finish()]);

        let slice = self.staging_buf.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.expect("map"));
        ctx.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
        let data = slice.get_mapped_range().expect("mapped range");
        let t0 = u64::from_le_bytes(data[0..8].try_into().unwrap());
        let t1 = u64::from_le_bytes(data[8..16].try_into().unwrap());
        drop(data);
        self.staging_buf.unmap();

        let ticks = t1.wrapping_sub(t0);
        (ticks as f64 * self.period_ns as f64).round() as u64
    }
}

/// The GPU-ns pair at S=10k: SceneDB delta (M=1%, contiguous) vs legacy
/// full-upload, both amplified `GPU_REPEATS`-fold (T3's signal-vs-noise-floor
/// lesson — see `GPU_REPEATS`'s doc comment). Returns
/// (delta_mean_ns, delta_p95_ns, full_mean_ns, full_p95_ns).
fn measure_gpu_ns_pair(ctx: &EngineGpuContext) -> (f64, f64, f64, f64) {
    let timer = GpuTimer::new(ctx);
    const ROWS: u32 = 10_000;
    const M: f64 = 1.0;

    // ---- (a) SceneDB delta: GPU_REPEATS independent 10k-row scenes, all
    // resident at once, each with the same contiguous M% subset dirtied
    // (untimed) — one boundary.run over the COMBINED cell slice is what's
    // bracketed, exactly `gpu_timing.rs`'s multi-cell amplification trick.
    let rpc = rows_per_cell(ROWS);
    let cells_per_copy = rpc.len() as u32;
    let total_cells = cells_per_copy * GPU_REPEATS;
    let cfg = SceneGpuConfig {
        classes: vec![RegionClassConfig { capacity: CELL_CAP, max_resident_cells: total_cells }],
        tombstone_headroom: 64,
        max_cells_metadata: total_cells,
    };
    let mut store = SceneGpuStore::new(ctx, cfg);
    let rpc_all: Vec<u32> = (0..GPU_REPEATS).flat_map(|_| rpc.iter().copied()).collect();
    let mut cells: Vec<CellStorage> =
        rpc_all.iter().map(|_| bench_transform_cell(CELL_CAP)).collect();
    let handles: Vec<Vec<Handle>> = cells
        .iter_mut()
        .zip(rpc_all.iter())
        .map(|(c, &r)| (0..r).map(|_| c.alloc().unwrap()).collect())
        .collect();
    let ids: Vec<CellId> = cells.iter().map(|c| store.register_cell(c, 0).unwrap()).collect();
    let mut frames = FrameDriver::new();
    let mut sim = Some(drain_registration_warmup(ctx, &mut store, &mut cells, &ids, &mut frames));

    let counts_one = contiguous_counts(&rpc, M);
    let counts_all: Vec<u32> = (0..GPU_REPEATS).flat_map(|_| counts_one.iter().copied()).collect();

    let mut delta_samples = Vec::with_capacity(ITERS);
    for it in 0..(WARMUP + ITERS) {
        let this_sim = sim.take().expect("witness refilled at the end of every iteration");
        // Untimed: mark every copy's dirty subset.
        for (((cell, &id), hs), &n) in
            cells.iter_mut().zip(ids.iter()).zip(handles.iter()).zip(counts_all.iter())
        {
            for (i, h) in hs.iter().enumerate().take(n as usize) {
                store.write_transform(id, cell, *h, &mat(i as f32), &this_sim);
            }
        }
        let boundary = this_sim.end().end().end();
        let mut slots: Vec<CellSlot> =
            cells.iter_mut().zip(ids.iter()).map(|(c, &id)| CellSlot { id, cell: c }).collect();
        let total_ns = timer.measure_ns(ctx, || {
            let stats = boundary.run(&mut store, &mut slots);
            std::hint::black_box(stats);
        });
        if it >= WARMUP {
            delta_samples.push(total_ns as f64 / GPU_REPEATS as f64);
        }
        sim = Some(frames.begin());
    }
    let (delta_mean, _delta_min, delta_p95) = stats(delta_samples);

    // ---- (b) Legacy full-upload: GPU_REPEATS * cells_per_copy write_buffer
    // calls of one cell's full region each, inside one bracket — the same
    // amplification applied to the legacy shape (one call per cell, every
    // frame, `cells_per_copy` calls == one "frame" of legacy work; looping
    // that `GPU_REPEATS` times inside the bracket is the amortization).
    let legacy_buffer = ctx.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("legacy-bench-gpu-timing-full-upload"),
        size: cells_per_copy as u64 * CELL_CAP as u64 * MAT_BYTES,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let full_bytes: Vec<[f32; 16]> = (0..CELL_CAP).map(|i| mat(i as f32)).collect();
    let mut full_samples = Vec::with_capacity(ITERS);
    for it in 0..(WARMUP + ITERS) {
        let total_ns = timer.measure_ns(ctx, || {
            for _ in 0..GPU_REPEATS {
                for c in 0..cells_per_copy {
                    ctx.queue().write_buffer(
                        &legacy_buffer,
                        c as u64 * CELL_CAP as u64 * MAT_BYTES,
                        as_bytes(&full_bytes),
                    );
                }
            }
        });
        if it >= WARMUP {
            full_samples.push(total_ns as f64 / GPU_REPEATS as f64);
        }
    }
    let (full_mean, _full_min, full_p95) = stats(full_samples);

    (delta_mean, delta_p95, full_mean, full_p95)
}

fn main() {
    let ctx = test_context();

    struct Row {
        s: u32,
        m: f64,
        scenedb_cpu_mean: f64,
        scenedb_cpu_p95: f64,
        legacy_cpu_mean: f64,
        legacy_cpu_p95: f64,
        scenedb_bytes: u64,
        legacy_bytes: u64,
        ranges: u32,
    }
    let mut rows: Vec<Row> = Vec::new();

    // For §5's scattered-vs-contiguous pair.
    let mut contiguous_10k_1pct: Option<(f64, u32, u64)> = None; // (cpu_mean, ranges, bytes)

    for &s in &S_VALUES {
        println!("building scene S={s} ...");
        let (mut store, mut cells, ids, handles, rpc) = build_scene(&ctx, s);
        let mut frames = FrameDriver::new();
        let mut sim =
            Some(drain_registration_warmup(&ctx, &mut store, &mut cells, &ids, &mut frames));

        let (legacy_mean, legacy_p95, legacy_bytes) = measure_legacy(&ctx, s);
        let n_cells = rpc.len() as u32;
        assert_eq!(
            legacy_bytes,
            n_cells as u64 * CELL_CAP as u64 * MAT_BYTES,
            "honesty check: legacy bytes must be the fixed full-capacity size every frame"
        );

        let mut cpu_means_by_m: Vec<f64> = Vec::new();
        for &m in &M_VALUES {
            let counts = contiguous_counts(&rpc, m);
            // Precomputed once per (S, M) — fixture data, never in-bracket.
            let row_lists: Vec<Vec<u32>> =
                counts.iter().map(|&c| (0..c).collect()).collect();
            let mut cpu_samples = Vec::with_capacity(ITERS);
            let mut last_stats = SyncStats { ranges: 0, bytes: 0 };
            for it in 0..(WARMUP + ITERS) {
                let (us, stats_out) = run_scenedb_frame(
                    &ctx,
                    &mut store,
                    &mut cells,
                    &ids,
                    &handles,
                    &mut frames,
                    &mut sim,
                    &row_lists,
                );
                if it >= WARMUP {
                    cpu_samples.push(us);
                    last_stats = stats_out;
                }
            }
            let (mean, _min, p95) = stats(cpu_samples);

            if m == 0.0 {
                assert_eq!(
                    (last_stats.ranges, last_stats.bytes),
                    (0, 0),
                    "honesty check: zero-mutation frame must upload zero ranges/bytes \
                     (already gated as a TEST, not just a bench, by \
                     tests/alloc_gate_gpu.rs::scene_gpu_store_boundary_sync_zero_dirty_rows_zero_alloc)"
                );
            }
            if m == 100.0 {
                assert!(
                    legacy_bytes >= last_stats.bytes,
                    "honesty check: legacy (full-capacity re-upload) must be >= scenedb's \
                     actual-occupied-rows upload at M=100%"
                );
                let pad = legacy_bytes - last_stats.bytes;
                assert!(
                    pad < CELL_CAP as u64 * MAT_BYTES,
                    "honesty check: M=100% scenedb bytes ({}) should be within one cell's \
                     worth of padding ({} B) of legacy bytes ({legacy_bytes}) — got a {pad} B gap",
                    last_stats.bytes,
                    CELL_CAP as u64 * MAT_BYTES
                );
            }

            if s == 10_000 && m == 1.0 {
                contiguous_10k_1pct = Some((mean, last_stats.ranges, last_stats.bytes));
            }

            cpu_means_by_m.push(mean);
            rows.push(Row {
                s,
                m,
                scenedb_cpu_mean: mean,
                scenedb_cpu_p95: p95,
                legacy_cpu_mean: legacy_mean,
                legacy_cpu_p95: legacy_p95,
                scenedb_bytes: last_stats.bytes,
                legacy_bytes,
                ranges: last_stats.ranges,
            });
        }

        // Honesty check: SceneDB CPU time is monotonic non-decreasing in M,
        // within a FIXED per-S slack (T3-review style: a constant tolerance
        // band, not one derived from the measured noise itself, which would
        // let the check inflate its own bar). Slack: 10% of the M=100% mean,
        // floored at 20 us — scales with the S-dependent magnitude of the
        // measurement while staying a single fixed number per S, computed
        // once before the comparison (not adaptively per pair).
        let slack = (cpu_means_by_m.last().copied().unwrap_or(0.0) * 0.10).max(20.0);
        for w in cpu_means_by_m.windows(2) {
            assert!(
                w[0] <= w[1] + slack,
                "honesty check: scenedb CPU time not monotonic in M at S={s} \
                 ({:?} vs {:?}, slack={slack:.1} us)",
                w[0],
                w[1]
            );
        }
        // Pairwise slack alone permits a cumulative decline of
        // (windows × slack) while "passing" (T4 review nit) — pin the
        // endpoints too: full mutation must cost strictly more than none.
        let (first_m, last_m) =
            (cpu_means_by_m.first().copied().unwrap(), cpu_means_by_m.last().copied().unwrap());
        assert!(
            last_m > first_m,
            "honesty check: M=100% mean ({last_m:.1} us) must exceed M=0 mean ({first_m:.1} us) at S={s}"
        );
        println!("  S={s}: monotonic-in-M check OK (slack={slack:.1} us, endpoints {first_m:.1} -> {last_m:.1})");
    }

    // ---- §5: scattered vs contiguous at S=10k, M=1% (stride ~100, T4's
    // case), plus a denser scatter (stride 10) for the M3-b T3 gap-tolerant
    // coalescing sweep (R-PERF-1). At stride 100 the gaps between dirty rows
    // are ~99 clean rows (only G > 99 could ever merge anything there); at
    // stride 10 the gaps are ~9 clean rows (G >= 10 collapses the whole
    // scatter into one range) — printing both at whatever GAP_MERGE_THRESHOLD
    // this binary was built with is what the T3 sweep reruns across G values.
    println!();
    println!("GAP_MERGE_THRESHOLD = {GAP_MERGE_THRESHOLD}");
    println!("scattered-vs-contiguous @ S=10000, M=1%:");

    /// Runs the scattered-mutation shape at a fixed GLOBAL row stride on a
    /// fresh S=10k scene (isolated from the M-sweep's accumulated dirty-mask
    /// history) and returns (cpu_mean_us, cpu_p95_us, ranges, bytes) of the
    /// last measured frame.
    fn measure_scattered_stride(ctx: &EngineGpuContext, stride: u32) -> (f64, f64, u32, u64) {
        let (mut store, mut cells, ids, handles, rpc) = build_scene(ctx, 10_000);
        let mut frames = FrameDriver::new();
        let mut sim =
            Some(drain_registration_warmup(ctx, &mut store, &mut cells, &ids, &mut frames));
        let sel = scattered_local_indices(&rpc, stride);

        let mut cpu_samples = Vec::with_capacity(ITERS);
        let mut last_stats = SyncStats { ranges: 0, bytes: 0 };
        for it in 0..(WARMUP + ITERS) {
            let (us, stats_out) = run_scenedb_frame(
                ctx,
                &mut store,
                &mut cells,
                &ids,
                &handles,
                &mut frames,
                &mut sim,
                &sel,
            );
            if it >= WARMUP {
                cpu_samples.push(us);
                last_stats = stats_out;
            }
        }
        let (mean, _min, p95) = stats(cpu_samples);
        (mean, p95, last_stats.ranges, last_stats.bytes)
    }

    let (scattered_cpu_mean, scattered_cpu_p95, scattered_ranges, scattered_bytes) =
        measure_scattered_stride(&ctx, 100);
    let (dense_cpu_mean, dense_cpu_p95, dense_ranges, dense_bytes) =
        measure_scattered_stride(&ctx, 10);

    if let Some((c_mean, c_ranges, c_bytes)) = contiguous_10k_1pct {
        println!("  contiguous (stride 1, the base M=1% run): cpu_mean_us={c_mean:.2} ranges={c_ranges} bytes={c_bytes}");
        println!(
            "  scattered  (stride 100, ~99-row gaps): cpu_mean_us={scattered_cpu_mean:.2} cpu_p95_us={scattered_cpu_p95:.2} ranges={scattered_ranges} bytes={scattered_bytes}"
        );
        println!(
            "  dense      (stride  10, ~9-row gaps):  cpu_mean_us={dense_cpu_mean:.2} cpu_p95_us={dense_cpu_p95:.2} ranges={dense_ranges} bytes={dense_bytes}"
        );
        assert_eq!(
            c_bytes, scattered_bytes,
            "honesty check: contiguous and stride-100 scattered mutate the same TOTAL byte volume at M=1%"
        );
        assert!(
            scattered_ranges >= c_ranges,
            "honesty check: scattered mutation must coalesce into AT LEAST as many ranges as \
             contiguous (T2 finding; strictly more at G below the stride's gap size) — got \
             scattered={scattered_ranges} contiguous={c_ranges}"
        );
        println!(
            "  T2 range-count finding at frame level (stride 100): scattered {:.1}x the ranges of \
             contiguous ({scattered_ranges} vs {c_ranges}), cpu_mean {:.2}x ({scattered_cpu_mean:.2} us vs {c_mean:.2} us)",
            scattered_ranges as f64 / c_ranges.max(1) as f64,
            scattered_cpu_mean / c_mean.max(0.001)
        );
        println!(
            "  dense (stride 10) vs contiguous: ranges {dense_ranges} vs {c_ranges}, cpu_mean {:.2}x \
             ({dense_cpu_mean:.2} us vs {c_mean:.2} us), bytes {dense_bytes} (vs {c_bytes} contiguous — \
             10x the mutated rows, so 10x the true dirty bytes; only bytes ABOVE that 10x baseline are \
             gap-merge inflation)",
            dense_cpu_mean / c_mean.max(0.001)
        );
    }

    // ---- §6: GPU-ns pair at 10k ----
    println!();
    println!("GPU-ns pair @ S=10000, M=1% (delta vs legacy full-upload, GPU_REPEATS={GPU_REPEATS} amortized):");
    let (delta_ns_mean, delta_ns_p95, full_ns_mean, full_ns_p95) = measure_gpu_ns_pair(&ctx);
    println!("  delta:       mean_ns={delta_ns_mean:.1} p95_ns={delta_ns_p95:.1}");
    println!("  full_upload: mean_ns={full_ns_mean:.1} p95_ns={full_ns_p95:.1}");
    assert!(delta_ns_mean > 0.0 && full_ns_mean > 0.0, "GPU-ns pair: both means must be positive");
    println!(
        "  (caveat, T3 lesson: at these payload sizes the two-submit bracket's fixed \
         driver/kernel round-trip overhead — tens of microseconds on this host — may still \
         dominate over pure copy-time differences; treat this pair as directional, not a \
         precision measurement)"
    );

    // ---- The matrix ----
    println!();
    println!(
        "S\tM%\tscenedb_cpu_us(mean/p95)\tlegacy_cpu_us(mean/p95)\tspeedup\tscenedb_bytes\tlegacy_bytes\tbyte_ratio\tranges"
    );
    for r in &rows {
        let speedup = r.legacy_cpu_mean / r.scenedb_cpu_mean.max(0.001);
        let byte_ratio = if r.scenedb_bytes == 0 {
            f64::INFINITY
        } else {
            r.legacy_bytes as f64 / r.scenedb_bytes as f64
        };
        println!(
            "{}\t{}\t{:.2}/{:.2}\t{:.2}/{:.2}\t{:.2}x\t{}\t{}\t{}\t{}",
            r.s,
            r.m,
            r.scenedb_cpu_mean,
            r.scenedb_cpu_p95,
            r.legacy_cpu_mean,
            r.legacy_cpu_p95,
            speedup,
            r.scenedb_bytes,
            r.legacy_bytes,
            if byte_ratio.is_finite() { format!("{byte_ratio:.2}x") } else { "inf".to_string() },
            r.ranges
        );
    }

    // ---- Crossover analysis ----
    println!();
    for &s in &S_VALUES {
        let s_rows: Vec<&Row> = rows.iter().filter(|r| r.s == s).collect();
        let crossover = s_rows.iter().find(|r| r.scenedb_cpu_mean >= r.legacy_cpu_mean);
        match crossover {
            Some(r) => println!(
                "crossover @ S={s}: delta stops winning on CPU time at M={}% \
                 (scenedb {:.2} us >= legacy {:.2} us)",
                r.m, r.scenedb_cpu_mean, r.legacy_cpu_mean
            ),
            None => println!(
                "crossover @ S={s}: delta wins on CPU time across every tested M in {M_VALUES:?} — no crossover observed"
            ),
        }
    }
}
