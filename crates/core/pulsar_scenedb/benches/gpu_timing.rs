//! GPU timestamp harness (perf-val Task 3): real GPU-side nanosecond timing
//! for the boundary-sync delta path vs the legacy full-buffer re-upload
//! model, at N in {0, 1, 64, 1024} contiguous dirty rows.
//!
//! Run: `cargo bench -p pulsar_scenedb --features gpu --bench gpu_timing`
//!
//! No criterion here (task brief): timestamp queries make each iteration
//! self-measuring, so this is a plain `main()` (`harness = false`) that does
//! its own warm-up/sample-size/statistics and prints a table.
//!
//! ## Bracketing: single-submit vs two-submit (read this before touching the timer)
//!
//! The brief's caveat to investigate: does `queue.write_buffer`'s
//! queue-internal staging copy land strictly BETWEEN a `write_timestamp`
//! start-encoder and end-encoder submitted together in one `submit()` call,
//! or does it execute adjacent to (not inside) that bracket?
//!
//! Verified against the vendored `wgpu-core-30.0.0` source (not just
//! empirically): `Queue::submit_pending_submission`
//! (`wgpu-core-30.0.0/src/device/queue.rs`) does
//! `executions.insert(0, pending_execution)` — the queue's pending-writes
//! command buffer is ALWAYS spliced in at position 0 of a submission's
//! executions, i.e. before every user-supplied command buffer passed to that
//! same `submit()` call, regardless of the order those command buffers were
//! given. So a single-submit bracket — `submit([start_encoder,
//! end_encoder])` with the `write_buffer` calls issued in between (populating
//! the pending-writes belt before the call) — would run the copies BEFORE
//! *both* timestamps, not between them: `write_timestamp(start)` and
//! `write_timestamp(end)` would both execute after the copies land, and the
//! bracket would read ~0 regardless of N. This is exactly the failure mode
//! the brief warns about ("if your bracket shows no N-dependence, it's
//! wrong").
//!
//! This harness therefore uses the **two-submit** form instead
//! (`GpuTimer::measure_ns`):
//! 1. `submit([start_encoder])` alone — writes timestamp 0, executes first
//!    on this queue's timeline.
//! 2. Caller's `work()` runs, issuing `queue.write_buffer` calls — these are
//!    only enqueued into the pending-writes belt, NOT submitted yet.
//! 3. `submit([end_encoder])` alone — the pending-writes buffer is spliced in
//!    at position 0 of *this* submission (before `end_encoder`), so GPU
//!    order across the two submits is: [timestamp 0] -> [pending writes] ->
//!    [timestamp 1, resolve, copy-to-staging]. Same-queue submissions
//!    execute in submission order, so this ordering is guaranteed, not
//!    racy.
//!
//! Coarser than a single-submit bracket would have been (two driver
//! round-trips instead of one), but it is the only ordering wgpu actually
//! guarantees.
//!
//! ## Signal vs. submission-overhead noise floor (`REPEATS`)
//!
//! A first pass at this harness measured ONE payload repeat per bracket (one
//! `boundary.run`/one full `write_buffer` between the two submits). Result:
//! every N showed 15-70 us of noise with NO usable trend (N=0's mean was
//! *higher* than N=1024's) — the two-submit-plus-poll round trip itself
//! (driver + kernel + fence wait) costs tens of microseconds on this host,
//! which completely swamps the actual GPU copy time of a <=64 KB SSBO region
//! (expected to be sub-microsecond). A per-sample assert with any reasonable
//! tolerance could not have caught this — it's a signal-to-noise problem,
//! not an ordering bug.
//!
//! Fix: amortize the fixed per-bracket overhead across `REPEATS` payload
//! copies before dividing back down to a per-frame estimate — the harness
//! equivalent of criterion's own iteration-count scaling, done manually
//! because timestamp queries (not wall-clock) are the clock here.
//!
//! **A second pitfall on the way there, worth naming explicitly:** the
//! obvious way to get `REPEATS` copies is to loop the untimed
//! mark-dirty-then-`boundary.run` cycle `REPEATS` times *inside* the timed
//! closure (dirty marks must be redone every repeat — `sync` clears the
//! mask). That does produce a clean monotonic trend, but it is measuring the
//! wrong thing: because the two-submit bracket's `end` timestamp cannot fire
//! until the host actually calls the second `submit`, ANY host-side work
//! executed inside `work()` — including `write_transform`'s per-row column
//! write, dirty-bit mark, and generation stamp — delays that submit and gets
//! counted as "GPU time." At N=1024 this folded in 1024 `write_transform`
//! calls per repeat, so the number was dominated by CPU-side mutation
//! bookkeeping, not `write_buffer`'s copy cost — exactly the kind of
//! CPU/GPU-time conflation the task brief's bracketing caveat warned about,
//! just showing up one layer deeper than the submit-order question.
//!
//! This harness instead amplifies by holding **`REPEATS` INDEPENDENT
//! 1024-row cells** all resident at once (a `CellStorage`/page is hard-capped
//! at 1024 rows — `src/page.rs::MAX_PAGE_CAPACITY` — so "one bigger cell" is
//! not an option; see the "(a) Delta path" comment below for how the
//! multi-cell registration works). Every cell's first N rows are marked
//! dirty as untimed setup (exactly like
//! `scenedb_bench.rs::bench_region_sync_1024_dirty_rows` marks dirty before
//! timing), then a SINGLE `boundary.run` call over the whole `[CellSlot]`
//! slice is what's bracketed — it flushes all `REPEATS` cells' dirty ranges
//! as `REPEATS` real `write_buffer` submissions in one shot, with zero
//! `write_transform` calls inside the timed section. Dividing the bracket's
//! total ns by `REPEATS` gives a per-frame (per-cell) estimate that is now
//! dominated by actual `write_buffer` submission cost, not CPU mutation cost.
//! With `REPEATS = 256` the N-dependent trend is clearly visible and
//! monotonic (see the Task 3 report for the actual table), confirming the
//! two-submit bracket order from the previous section is correct — the
//! original flat/inverted numbers were a signal-to-noise problem, not a
//! bracket ordering problem.
//!
//! The full-upload measurement never had this pitfall: it has no dirty
//! tracking or per-row mutation to begin with, so its own `REPEATS`-copy loop
//! (bytes pre-built once, outside every bracket) was always just `REPEATS`
//! plain `write_buffer` calls with no CPU bookkeeping mixed in.
//!
//! ## Reuse note for T4/T6
//!
//! `GpuTimer` is written as a plain, dependency-light struct specifically so
//! later tasks can copy it wholesale into their own bench files — integration
//! benches cannot share a module across bench binaries (same reason this
//! crate's `test_context` helper is duplicated in every gpu-gated bench/test
//! file rather than factored out).

use pulsar_scenedb::gpu::{
    CellSlot, EngineGpuContext, FrameDriver, RegionClassConfig, SceneGpuConfig, SceneGpuStore,
};
use pulsar_scenedb::{CellStorage, CellType, TypeToken};
use std::sync::Arc;

const ROWS: u32 = 1024;
const WARMUP: usize = 10;
const ITERS: usize = 100;
/// `[f32; 16]` transform matrix stride in bytes.
const MAT_BYTES: u64 = 64;
/// How many payload repeats are folded into ONE bracketed sample (see the
/// "Signal vs. submission-overhead noise floor" section of the file doc
/// comment). For the full-upload path this is a literal loop of `REPEATS`
/// `write_buffer` calls inside one bracket; for the delta path it is
/// `REPEATS` separate 1024-row cells synced by a single `boundary.run` call
/// (a page/cell cannot itself hold more than 1024 rows). Either way, one
/// bracket ends up carrying `REPEATS` real `write_buffer` submissions,
/// diluting the fixed two-submit overhead `REPEATS`-fold before dividing back
/// down to a per-frame estimate.
const REPEATS: u32 = 256;

/// Mirrors `scenedb_bench.rs::test_context`, with `TIMESTAMP_QUERY` (+
/// `TIMESTAMP_QUERY_INSIDE_ENCODERS`, needed because we call
/// `CommandEncoder::write_timestamp` directly, not inside a render/compute
/// pass) requested at `request_device` — every other harness in this crate
/// requests `DeviceDescriptor::default()` and gets zero GPU-timing
/// capability (stress-recon-infrastructure.md §5). The RTX 5080/Vulkan
/// adapter used in this workspace supports both.
fn test_context() -> EngineGpuContext {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    }))
    .expect("no adapter — GPU benches need a local GPU");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("gpu-timing-bench"),
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

fn bench_transform_cell(capacity: u32) -> CellStorage {
    let ct = CellType::new("gpu-timing-instance")
        .with(TypeToken::of::<[f32; 16]>())
        .build()
        .unwrap();
    CellStorage::from_cell_type(&ct, capacity).unwrap()
}

/// Reinterpret a `[[f32; 16]]` slice as bytes for a raw `queue.write_buffer`
/// call. Bench-local equivalent of the crate's `pub(crate) gpu::as_bytes`
/// (not visible across the crate boundary from an integration bench) — safe
/// because `[f32; 16]` has no padding and no interior pointers.
fn as_bytes(s: &[[f32; 16]]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(s.as_ptr() as *const u8, std::mem::size_of_val(s)) }
}

/// A 2-slot GPU timestamp bracket: query set + resolve buffer + `MAP_READ`
/// staging buffer, `queue.get_timestamp_period()` cached once. See the file
/// doc comment for why `measure_ns` uses two submits, not one.
struct GpuTimer {
    query_set: wgpu::QuerySet,
    resolve_buf: wgpu::Buffer,
    staging_buf: wgpu::Buffer,
    period_ns: f32,
}

impl GpuTimer {
    fn new(ctx: &EngineGpuContext) -> Self {
        let query_set = ctx.device().create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("gpu-timing-query-set"),
            ty: wgpu::QueryType::Timestamp,
            count: 2,
        });
        let resolve_buf = ctx.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("gpu-timing-resolve"),
            size: 16,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let staging_buf = ctx.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("gpu-timing-staging"),
            size: 16,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let period_ns = ctx.queue().get_timestamp_period();
        Self { query_set, resolve_buf, staging_buf, period_ns }
    }

    /// Brackets `work` (which must issue `queue.write_buffer`/equivalent
    /// calls but must NOT itself call `queue.submit`) with GPU timestamps and
    /// returns the elapsed GPU time in nanoseconds. Two-submit form — see the
    /// file doc comment for why.
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
        ctx.device()
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll");
        let data = slice.get_mapped_range().expect("mapped range");
        let t0 = u64::from_le_bytes(data[0..8].try_into().unwrap());
        let t1 = u64::from_le_bytes(data[8..16].try_into().unwrap());
        drop(data);
        self.staging_buf.unmap();

        let ticks = t1.wrapping_sub(t0);
        (ticks as f64 * self.period_ns as f64).round() as u64
    }
}

/// mean / min / p95 (nearest-rank) over a sample set, ns-per-frame (already
/// divided by `REPEATS` by the caller, hence `f64` rather than `u64`).
fn stats(mut samples: Vec<f64>) -> (f64, f64, f64) {
    samples.sort_by(|a, b| a.total_cmp(b));
    let n = samples.len();
    let mean = samples.iter().copied().sum::<f64>() / n as f64;
    let min = samples[0];
    let p95_idx = (((n - 1) as f64) * 0.95).round() as usize;
    let p95 = samples[p95_idx];
    (mean, min, p95)
}

fn main() {
    let ctx = test_context();
    let timer = GpuTimer::new(&ctx);
    println!(
        "timestamp_period_ns_per_tick = {:.4} (Vulkan/NV typically ~1.0)",
        timer.period_ns
    );

    // ---- (a) Delta path: boundary sync with N contiguous dirty rows ----
    //
    // A `CellStorage`/page is hard-capped at `MAX_PAGE_CAPACITY` (1024) rows
    // (`src/page.rs`) — there is no such thing as a single cell bigger than
    // 1024 rows, so amplification (see file doc comment) cannot be "one
    // bigger cell split into blocks." Instead: `REPEATS` INDEPENDENT
    // 1024-row cells, all registered resident in one class at once.
    // `BoundaryPhase::run` takes `cells: &mut [CellSlot<'_>]` — a slice of
    // multiple cells — so marking the first N rows of every one of the
    // `REPEATS` cells dirty (untimed) and then a SINGLE `boundary.run(...)`
    // call over all of them flushes `REPEATS` real `write_buffer`
    // submissions (one per cell's contiguous dirty run) in one bracket, with
    // zero `write_transform` CPU bookkeeping inside the timed section.
    let cfg = SceneGpuConfig {
        // `capacity` is PER-CELL region size (1024, matching every cell
        // registered below); the class's total row budget is
        // `capacity * max_resident_cells` (`scene_store.rs` `SceneGpuStore::new`
        // — `checked_mul`), computed internally. Do not pre-multiply here.
        classes: vec![RegionClassConfig { capacity: ROWS, max_resident_cells: REPEATS }],
        tombstone_headroom: 64,
        max_cells_metadata: REPEATS,
    };
    let mut store = SceneGpuStore::new(&ctx, cfg);
    let mut cells: Vec<CellStorage> = (0..REPEATS).map(|_| bench_transform_cell(ROWS)).collect();
    let ids: Vec<_> = cells.iter().map(|c| store.register_cell(c, 0).unwrap()).collect();
    let handles: Vec<Vec<_>> = cells
        .iter_mut()
        .map(|c| (0..ROWS).map(|_| c.alloc().unwrap()).collect())
        .collect();
    let mut frames = FrameDriver::new();
    let mut sim = Some(frames.begin());

    let ns_list = [0u32, 1, 64, 1024];
    let mut delta_results: Vec<(u32, f64, f64, f64)> = Vec::new();
    for &n in &ns_list {
        let mut samples = Vec::with_capacity(ITERS);
        for it in 0..(WARMUP + ITERS) {
            let this_sim = sim.take().expect("witness refilled at the end of every iteration");
            // Untimed: re-mark the first N rows of EVERY cell dirty (sync
            // clears the mask each boundary, so every sample needs a fresh
            // mark — same reasoning as `bench_region_sync_1024_dirty_rows`).
            for ((cell, hs), &id) in cells.iter_mut().zip(handles.iter()).zip(ids.iter()) {
                for (i, h) in hs.iter().enumerate().take(n as usize) {
                    store.write_transform(id, cell, *h, &mat(i as f32), &this_sim);
                }
            }
            let boundary = this_sim.end().end().end();
            // Slot list built OUTSIDE the bracket (T3 review minor: the Vec
            // alloc was uniform-across-N floor cost, but it doesn't belong
            // in the measurement at all).
            let mut slots: Vec<CellSlot> = cells
                .iter_mut()
                .zip(ids.iter())
                .map(|(cell, &id)| CellSlot { id, cell })
                .collect();
            let total_ns = timer.measure_ns(&ctx, || {
                let stats = boundary.run(&mut store, &mut slots);
                std::hint::black_box(stats);
            });
            if it >= WARMUP {
                samples.push(total_ns as f64 / REPEATS as f64);
            }
            sim = Some(frames.begin());
        }
        let (mean, min, p95) = stats(samples);
        delta_results.push((n, mean, min, p95));
    }

    // ---- (b) Full-upload model: one write_buffer of the whole 1024-row
    // region per frame, regardless of what changed — the legacy re-upload
    // shape. Single measurement (not swept over N): the legacy model has no
    // N-dependence to sweep.
    let full_bytes: Vec<[f32; 16]> = (0..ROWS).map(|i| mat(i as f32)).collect();
    let region_base = store.row_region_base(ids[0]) as u64;
    let mut full_samples = Vec::with_capacity(ITERS);
    for it in 0..(WARMUP + ITERS) {
        let total_ns = timer.measure_ns(&ctx, || {
            for _ in 0..REPEATS {
                ctx.queue().write_buffer(
                    store.transform_buffer(),
                    region_base * MAT_BYTES,
                    as_bytes(&full_bytes),
                );
            }
        });
        if it >= WARMUP {
            full_samples.push(total_ns as f64 / REPEATS as f64);
        }
    }
    let (full_mean, full_min, full_p95) = stats(full_samples);

    // ---- Report ----
    println!();
    // All numbers are PER-CELL AMORTIZED (raw 256-cell bracket / REPEATS):
    // one 1024-capacity cell with N contiguous dirty rows (one write range).
    println!("path\tN\tmean_ns(/cell)\tmin_ns(/cell)\tp95_ns(/cell)");
    for (n, mean, min, p95) in &delta_results {
        println!("delta\t{n}\t{mean:.1}\t{min:.1}\t{p95:.1}");
    }
    println!("full_upload\t{}\t{full_mean:.1}\t{full_min:.1}\t{full_p95:.1}", ROWS);

    let crossover = delta_results.iter().find(|(_, mean, _, _)| *mean >= full_mean).map(|(n, _, _, _)| *n);
    match crossover {
        Some(n) => println!(
            "crossover: delta GPU cost reaches full-upload cost ({full_mean:.1} ns) at N={n} \
             (caveat: delta rows carry boundary's O(rows) CPU slot-shadow scan in-bracket, \
             which full_upload skips — floor-subtracted pure-copy cost does NOT cross here)"
        ),
        None => println!(
            "crossover: delta GPU cost stays below full-upload cost ({full_mean:.1} ns) across every tested N in {ns_list:?} — no crossover observed"
        ),
    }

    // ---- Self-check: the harness's honesty gate ----
    // Two failure modes this must catch (T3 review): (a) a DEAD bracket
    // (e.g. single-submit form — pending writes splice before user encoders,
    // so everything reads ~flat ~0): caught by requiring the largest payload
    // to rise VISIBLY above the empty-payload floor; (b) a NOISE-DOMINATED
    // bracket (per-sample submission jitter swamping payload): caught by
    // FIXED absolute slack on the trend asserts — measured-floor-derived
    // slack would inflate with the noise it's meant to detect.
    let noise_floor = delta_results[0].1; // N=0 mean
    println!();
    println!("noise_floor_ns (N=0 delta mean) = {noise_floor:.1}");
    let (_, m1, _, _) = delta_results[1];
    let (_, m64, _, _) = delta_results[2];
    let (_, m1024, _, _) = delta_results[3];
    const TREND_SLACK_NS: f64 = 500.0;
    assert!(
        m1024 > noise_floor * 1.5,
        "payload invisible: N=1024 mean {m1024:.1} ns not clearly above the N=0 floor {noise_floor:.1} ns — dead or misplaced bracket, fix before reporting"
    );
    assert!(
        m1 <= m64 + TREND_SLACK_NS,
        "monotonicity broken: N=1 mean {m1:.1} ns > N=64 mean {m64:.1} ns + {TREND_SLACK_NS} ns — bracket is wrong or noise-dominated, fix before reporting"
    );
    assert!(
        m64 <= m1024 + TREND_SLACK_NS,
        "monotonicity broken: N=64 mean {m64:.1} ns > N=1024 mean {m1024:.1} ns + {TREND_SLACK_NS} ns — bracket is wrong or noise-dominated, fix before reporting"
    );
    println!("self-check OK: payload visible over floor and N=1 <= N=64 <= N=1024 within {TREND_SLACK_NS} ns fixed slack");
}
