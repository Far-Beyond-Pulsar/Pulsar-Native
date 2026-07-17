//! Stress storms (perf-val Task 6, `docs/superpowers/plans/2026-07-17-scenedb20-perf-validation.md`
//! "### Task 6"): four load-bearing protective claims from
//! `.superpowers/sdd/stress-recon-perf-contract.md`, exercised under sustained
//! load rather than the single-frame precedent each already has elsewhere in
//! this crate. Real surfaceless wgpu device where a store is involved (same
//! headless harness as `gpu_store.rs`/`gpu_harvest.rs`); the test harness owns
//! the `device.poll` pump. Run serially: `cargo test -p pulsar_scenedb
//! --features gpu --test stress_gpu -- --test-threads=1` (GPU tests in this
//! crate are never run concurrently against one adapter).
//!
//! 1. **Hysteresis thrash-guard** (contract #27, `src/gpu/grid.rs`'s Test 11
//!    band machine): 600 simulated frames of camera jitter confined inside
//!    the hysteresis band -> zero domain transitions/region-allocs, then a
//!    wider-than-band oscillation -> real transitions (non-vacuous guard).
//! 2. **Eviction/recycle storm** (`src/gpu/scene_store.rs`'s `RegionPool`):
//!    64 logical cells over a 16-cell residency budget, round-robin
//!    promote/demote x500 -> region-pool footprint plateaus, VRAM survives
//!    the churn byte-exact, promote+demote latency distribution recorded.
//! 3. **Lease revocation latency** (contract #32, C4 2.0 ms budget,
//!    `src/lease.rs` + `src/gpu/harvest.rs`'s `revoke_overdue`): 1000
//!    revocations under concurrent harvest-workload contention, full
//!    histogram, p99 <= 2.0 ms.
//! 4. **DEI threshold straddle** (contract #22, `src/gpu/harvest.rs`'s 25%
//!    gate): 24%/25%/26% hit-rate cells straddling the strict `< 0.25` cut,
//!    bit-identical-vs-oracle output, byte-delta accounting.
//!
//! ## Documented interpretation calls (spec text under-determined; recorded
//! honestly rather than silently resolved)
//!
//! **Storm 1, "crossing the unpadded threshold" guard:** for the specific
//! Outer<->Margin geometry this storm reuses from `grid.rs`'s own Test 11
//! fixture (cell (5,0), base x in [500,600], cell_width 100 / margin_radius
//! 150 / pad_fraction 0.10 / hysteresis 20), the "unpadded" margin-promote
//! edge is 340 (grid.rs's own comment names this number), which sits ABOVE
//! the padded promote edge (330), which itself sits above the hysteresis band
//! under test ([310,330)). A jitter "strictly inside the hysteresis band" can
//! therefore never numerically reach 340 — the two requirements are
//! geometrically incompatible for this fixture. Rather than silently picking
//! a reading, this storm proves the more directly relevant edge instead: the
//! hysteresis-delayed DEMOTE FLOOR (310) that the jitter's own floor (312)
//! sits only 2 units above is a REAL, live, crossable decision boundary for
//! this exact config (a probe at 309 does demote). That closes the actual gap
//! the guard exists for — a jitter parked far from any real edge would
//! trivially show zero transitions without proving anything about hysteresis.
//! The unpadded-vs-padded threshold relationship (340 > 330) is asserted too,
//! as the literal textual nod to "unpadded threshold", though it plays a
//! supporting role, not the load-bearing one, in this storm's guard.
//!
//! **Storm 3, "request -> lease-actually-dropped" latency:** revocation in
//! this crate is advisory-flag semantics (M2b-b Test 10, `gpu_harvest.rs`):
//! `HarvestPipeline::revoke_overdue` synchronously sets `RevocationFlag` via
//! a `Release` store, observable by the SAME calling thread immediately via
//! `is_revoked()`'s `Acquire` load — there is no separate asynchronous
//! "notify the holder" channel in this crate today (that would be World-driver
//! scope, M4, not built). So "dropped" IS defined here as: the instant the
//! flag transitions inside that same call. The measured "request -> dropped"
//! latency is therefore the wall-clock cost of the `revoke_overdue` call
//! itself (invocation to observed-true), under concurrent harvest-workload
//! contention on the SAME `LeaseMask` — the only latency this API surface
//! can honestly expose without inventing a mechanism the crate doesn't have.
//!
//! **Storm 2, "region-pool size plateau":** `RegionPool` is a fixed-capacity
//! free-list constructed once with exactly `max_resident_cells` entries
//! (`region.rs`) — it can never structurally exceed that count, so a bare
//! "did it grow" check would be vacuously true regardless of whether
//! recycling actually works. The real regression this storm guards against —
//! a leak that silently stops returning regions to the free list — instead
//! surfaces as `register_cell` starting to fail with `RowsExhausted`/
//! `SlotsExhausted` well before cycle 500. This storm therefore asserts BOTH:
//! zero region-alloc failures across all 500 cycles (the load-bearing check),
//! AND that the set of distinct region bases actually touched plateaus at
//! exactly `max_resident_cells` from warm-up onward (cycle 50 vs cycle 500,
//! must be equal) — proving every physical region is genuinely being reused,
//! not merely that the fixed-size pool didn't (structurally couldn't) grow.
//! wgpu buffer sizes (never reallocated post-construction, contract #29) are
//! also snapshotted and confirmed unchanged as a third, independent guard.

use pulsar_scenedb::gpu::{
    execute_transitions, CellCoord, CellId, CellSlot, Domain, EngineGpuContext, FrameDriver,
    GridConfig, HarvestPipeline, HarvestStaging, MeshClass, RegionClassConfig, SceneGpuConfig,
    SceneGpuStore, StreamingBudget, StreamingGrid, View,
};
use pulsar_scenedb::{Aabb, Handle, LeaseMask, Scratchpad, SpatialCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

fn test_context() -> EngineGpuContext {
    // Upstream wgpu 30: `Instance::new` still takes an owned
    // `InstanceDescriptor`, but the type no longer derives `Default` — use
    // the `new_without_display_handle()` constructor (headless, no window
    // system connection).
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    }))
    .expect("no adapter — GPU tests need a local GPU");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("scenedb-stress-test"),
        ..Default::default()
    }))
    .expect("device");
    EngineGpuContext::new(std::sync::Arc::new(device), std::sync::Arc::new(queue))
}

fn readback(ctx: &EngineGpuContext, buf: &wgpu::Buffer, bytes: u64) -> Vec<u8> {
    let staging = ctx.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: bytes,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut enc = ctx.device().create_command_encoder(&Default::default());
    enc.copy_buffer_to_buffer(buf, 0, &staging, 0, bytes);
    ctx.queue().submit([enc.finish()]);
    ctx.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    let slice = staging.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("map"));
    ctx.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    let data = slice.get_mapped_range().expect("mapped range").to_vec();
    staging.unmap();
    data
}

fn as_f32s(bytes: &[u8]) -> Vec<f32> {
    bytes.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect()
}

/// Observer AABB centered at `x`, half-width 10 — mirrors `grid.rs`'s own
/// Test 11 fixture helper exactly (same numbers, for continuity with the
/// documented 310/330 derivation).
fn observer_at(x: f32) -> Aabb {
    Aabb { min: [x - 10.0, -10.0, -10.0], max: [x + 10.0, 10.0, 10.0] }
}

/// `(min, mean, p50, p95, p99, max)` in nanoseconds, from an UNSORTED sample.
fn latency_histogram(samples: &mut [u128]) -> (u128, f64, u128, u128, u128, u128) {
    samples.sort_unstable();
    let n = samples.len();
    let pct = |p: f64| -> u128 {
        let idx = ((n as f64 - 1.0) * p).round() as usize;
        samples[idx.min(n - 1)]
    };
    let sum: u128 = samples.iter().sum();
    let mean = sum as f64 / n as f64;
    (samples[0], mean, pct(0.50), pct(0.95), pct(0.99), samples[n - 1])
}

// ============================================================================
// Storm 1 — Hysteresis thrash-guard (contract #27)
// ============================================================================

#[test]
fn storm1_hysteresis_thrash_guard_under_60hz_equivalent_jitter() {
    // Reuse grid.rs's own Test 11 fixture numbers verbatim: cell (5,0), base
    // x in [500,600]; cell_width 100, margin_radius 150, pad_fraction 0.10
    // (Δpad = 10), hysteresis 20 -> padded promote edge 330, demote-hold
    // floor 310 (band [310,330)), unpadded/naive promote edge 340 (module
    // doc: "the UNPADDED threshold 340").
    const PADDED_PROMOTE: f32 = 330.0;
    const DEMOTE_FLOOR: f32 = 310.0;
    const UNPADDED_PROMOTE: f32 = 340.0;
    const BAND_WIDTH: f32 = PADDED_PROMOTE - DEMOTE_FLOOR; // 20.0

    let cfg = GridConfig { cell_width: 100.0, margin_radius: 150.0, pad_fraction: 0.10, hysteresis: 20.0 };
    let budget = StreamingBudget {
        vram_hlod_budget: u64::MAX,
        vram_geometry_budget: u64::MAX,
        max_materialized_cells: 16,
        proxy_mesh_bytes: 1,
        mean_cell_geometry_bytes: 1,
    };
    let classes = [RegionClassConfig { capacity: 64, max_resident_cells: 4 }];
    let far = CellCoord { x: 5, z: 0 };

    // ---- GUARD: prove this is not a vacuous jitter -------------------------
    // (a) the unpadded/naive threshold genuinely sits above the padded one —
    // Δpad is doing real work here, not a no-op:
    assert!(
        UNPADDED_PROMOTE > PADDED_PROMOTE,
        "GUARD: Δpad must genuinely advance the boundary (unpadded {UNPADDED_PROMOTE} > padded {PADDED_PROMOTE}), \
         or the whole hysteresis-padding claim is moot for this config"
    );
    // (b) the demote floor (310) — the edge this storm's jitter actually
    // hugs (jitter floor 312 sits only 2 units above it) — is a REAL, live,
    // crossable boundary for this exact config, not some unreachable number.
    // Disposable grid so the main run below starts from clean state.
    {
        let mut probe = StreamingGrid::new(cfg, budget, &classes).unwrap();
        probe.materialize(far);
        probe.classify(&[observer_at(480.0)]); // decisive settle: Outer -> Margin
        let ts = probe.take_transitions();
        assert_eq!(ts.len(), 1);
        probe.commit_transition(ts[0]);
        assert_eq!(probe.domain(far), Some(Domain::Margin));
        probe.classify(&[observer_at(DEMOTE_FLOOR - 1.0)]); // 309: 1 unit below the floor
        let ts2 = probe.take_transitions();
        assert_eq!(
            ts2.len(),
            1,
            "GUARD: a probe 1 unit below the demote floor (309) must demote — \
             otherwise this jitter's floor (312) proves nothing about hysteresis"
        );
        assert_eq!(ts2[0].to, Domain::Outer);
    }

    // ---- Real run: grid + store + execute_transitions ----------------------
    let ctx = test_context();
    let mut store = SceneGpuStore::new(
        &ctx,
        SceneGpuConfig {
            classes: vec![RegionClassConfig { capacity: 64, max_resident_cells: 4 }],
            tombstone_headroom: 8,
            max_cells_metadata: 16,
        },
    );
    let mut grid = StreamingGrid::new(cfg, budget, &classes).unwrap();
    grid.materialize(far);
    let mut cells: HashMap<CellCoord, SpatialCell> = HashMap::new();
    cells.insert(far, SpatialCell::with_transform(64).unwrap());
    let mut frames = FrameDriver::new();

    // Settle: decisive move into Margin (matches grid.rs's own precedent).
    grid.classify(&[observer_at(480.0)]);
    let sim = frames.begin();
    let b = sim.end().end().end();
    let (retired, _) = b.retire(&mut store, &mut []);
    let serial = store.tracker().next_serial();
    let settle_stats = execute_transitions(&mut grid, &mut store, &mut cells, &|_| 0, serial, &retired);
    assert_eq!(settle_stats.promoted, 1, "settle: exactly one Outer->Margin promotion");
    assert_eq!(settle_stats.demoted, 0);
    retired.compact(&mut store, &mut []).sync(&mut store, &mut []);
    assert_eq!(grid.domain(far), Some(Domain::Margin), "cell resident before the jitter phase begins");

    // ---- Phase 1: 600 frames of in-band jitter, amplitude strictly INSIDE
    // the hysteresis band [310,330) — same waveform as grid.rs's Test 11
    // (center in {312,314,...,328}, cycling every 9 steps). Zero transitions
    // expected AFTER settle. Counted via `StreamingGrid::take_transitions`
    // (the grid's own transition-drain API), every single frame — not a
    // once-at-the-end check.
    //
    // Since `execute_transitions` acts ONLY on drained transitions (module
    // contract, grid.rs), an empty drain on every one of these 600 frames is
    // definitionally equivalent to zero promotions/evictions/region-allocs —
    // wiring the full store/execute_transitions machinery through every
    // frame here would be redundant with that guarantee (phase 2 below does
    // exercise it, to show the contrast).
    let mut phase1_transitions = 0u32;
    for i in 0..600u32 {
        let center = 320.0 + ((i % 9) as f32 - 4.0) * 2.0; // in [312, 328]
        assert!(
            (312.0..=328.0).contains(&center) && center < PADDED_PROMOTE && center > DEMOTE_FLOOR,
            "frame {i}: jitter center {center} escaped the intended in-band range"
        );
        grid.classify(&[observer_at(center)]);
        let ts = grid.take_transitions();
        phase1_transitions += ts.len() as u32;
        assert!(ts.is_empty(), "frame {i} (center {center}): unexpected transition inside the hysteresis band");
    }
    assert_eq!(phase1_transitions, 0);
    assert_eq!(grid.domain(far), Some(Domain::Margin), "held Margin through all 600 in-band frames");

    // ---- Phase 2: amplitude > band width (50 > 20) DOES produce
    // transitions — proves phase 1's zero-transition assert isn't vacuous.
    let phase2_values = [295.0f32, 345.0f32]; // range 50, straddling both 310 and 330
    let phase2_amplitude = phase2_values[1] - phase2_values[0];
    assert!(phase2_amplitude > BAND_WIDTH, "phase 2 amplitude ({phase2_amplitude}) must exceed the band width ({BAND_WIDTH})");
    let mut total_promoted = 0u32;
    let mut total_demoted = 0u32;
    for i in 0..40usize {
        let center = phase2_values[i % 2];
        grid.classify(&[observer_at(center)]);
        let sim = frames.begin();
        let b = sim.end().end().end();
        let (retired, _) = b.retire(&mut store, &mut []);
        let serial = store.tracker().next_serial();
        let stats = execute_transitions(&mut grid, &mut store, &mut cells, &|_| 0, serial, &retired);
        store.tracker().force_complete(serial); // test hook: recycle promptly, matching the M2b-b eviction-timing precedent
        total_promoted += stats.promoted;
        total_demoted += stats.demoted;
        retired.compact(&mut store, &mut []).sync(&mut store, &mut []);
    }
    assert!(
        total_promoted + total_demoted > 0,
        "GUARD: amplitude > band width must produce real transitions — the phase-1 zero-transition \
         assert would otherwise be checking a config that never transitions regardless of amplitude"
    );

    println!(
        "[storm1 hysteresis] phase1: 600 in-band frames (center in [312,328], band [310,330)) -> {phase1_transitions} transitions (expect 0)"
    );
    println!(
        "[storm1 hysteresis] phase2: 40 frames, amplitude {phase2_amplitude} > band width {BAND_WIDTH} -> promoted={total_promoted} demoted={total_demoted} (expect >0)"
    );
    println!(
        "[storm1 hysteresis] guard: unpadded threshold {UNPADDED_PROMOTE} > padded threshold {PADDED_PROMOTE}; demote floor {DEMOTE_FLOOR} proven live via a -1 unit probe"
    );
}

// ============================================================================
// Storm 2 — Eviction/recycle storm
// ============================================================================

fn known_matrix(cell_idx: usize, row: u32) -> [f32; 16] {
    // Injective in (cell_idx, row) for cell_idx < 64, row < 64 — 10_000
    // spacing per cell dwarfs the 10-per-row and 0.01-per-component terms,
    // so no two (cell_idx, row) pairs can coincidentally collide.
    let base = (cell_idx as f32) * 10_000.0 + (row as f32) * 10.0;
    core::array::from_fn(|i| base + i as f32 * 0.01)
}

#[test]
fn storm2_eviction_recycle_storm_500_cycles() {
    const TOTAL_LOGICAL: usize = 64;
    const MAX_RESIDENT: usize = 16;
    const CYCLES: usize = 500;
    const CAPACITY: u32 = 64;

    let ctx = test_context();
    let mut store = SceneGpuStore::new(
        &ctx,
        SceneGpuConfig {
            classes: vec![RegionClassConfig { capacity: CAPACITY, max_resident_cells: MAX_RESIDENT as u32 }],
            tombstone_headroom: 8,
            max_cells_metadata: 4,
        },
    );
    let mut frames = FrameDriver::new();

    // wgpu buffers are allocated once at construction and never reallocated
    // (contract #29) — snapshot sizes now, re-check identical after the storm.
    let transform_bytes_before = store.transform_buffer().size();
    let gen_bytes_before = store.generation_buffer().size();
    let mirror_bytes_before = store.slot_mirror_buffer().size();
    let info_bytes_before = store.instance_info_buffer().size();

    // 64 logical cells, each FULLY packed to the class capacity (64 rows) so
    // both boundary rows (0 and CAPACITY-1) get exercised by the VRAM
    // integrity check below.
    let mut logical_cells: Vec<SpatialCell> = Vec::with_capacity(TOTAL_LOGICAL);
    let mut handles: Vec<Vec<Handle>> = Vec::with_capacity(TOTAL_LOGICAL);
    for idx in 0..TOTAL_LOGICAL {
        let mut cell = SpatialCell::with_transform(CAPACITY).unwrap();
        let mut hs = Vec::with_capacity(CAPACITY as usize);
        for row in 0..CAPACITY {
            let x = (idx as f32) * 1000.0 + row as f32;
            hs.push(cell.alloc(Aabb { min: [x, 0.0, 0.0], max: [x + 1.0, 1.0, 1.0] }).unwrap());
        }
        logical_cells.push(cell);
        handles.push(hs);
    }

    let mut resident: VecDeque<(usize, CellId)> = VecDeque::new();
    let mut distinct_bases: HashSet<u32> = HashSet::new();
    let mut cycle_latencies_ns: Vec<u128> = Vec::with_capacity(CYCLES);
    let mut hwm_at_50 = 0usize;
    let mut register_failures = 0u32;

    for cycle in 0..CYCLES {
        let t0 = Instant::now();
        let next_idx = cycle % TOTAL_LOGICAL;

        if resident.len() >= MAX_RESIDENT {
            let (evict_idx, evict_id) = resident.pop_front().unwrap();
            let serial = store.tracker().next_serial();
            store.unregister_cell(evict_id, logical_cells[evict_idx].storage_mut(), serial);
            store.tracker().force_complete(serial); // test hook — see gpu_store.rs precedent
            let sim = frames.begin();
            let b = sim.end().end().end();
            b.run(&mut store, &mut []); // drains the just-freed region back to the pool
        }

        match store.register_cell(logical_cells[next_idx].storage(), 0) {
            Ok(id) => {
                distinct_bases.insert(store.row_region_base(id));
                resident.push_back((next_idx, id));
            }
            Err(_) => register_failures += 1,
        }

        let t1 = Instant::now();
        cycle_latencies_ns.push((t1 - t0).as_nanos());

        if cycle == 49 {
            hwm_at_50 = distinct_bases.len();
        }
    }
    let hwm_at_500 = distinct_bases.len();

    assert_eq!(register_failures, 0, "MISS: register_cell must never fail across {CYCLES} round-robin cycles — a failure means recycling leaked");
    assert_eq!(resident.len(), MAX_RESIDENT, "steady-state residency must sit exactly at the budget");
    assert_eq!(
        hwm_at_50, hwm_at_500,
        "MISS: region-pool footprint (distinct bases touched) grew after warm-up — unbounded growth"
    );
    assert_eq!(hwm_at_50, MAX_RESIDENT, "expected exactly max_resident_cells distinct physical regions in steady state");

    assert_eq!(store.transform_buffer().size(), transform_bytes_before, "transform SSBO reallocated during the storm");
    assert_eq!(store.generation_buffer().size(), gen_bytes_before, "generation SSBO reallocated during the storm");
    assert_eq!(store.slot_mirror_buffer().size(), mirror_bytes_before, "slot-mirror SSBO reallocated during the storm");
    assert_eq!(store.instance_info_buffer().size(), info_bytes_before, "instance-info SSBO reallocated during the storm");

    // ---- VRAM integrity spot-check: write known, per-(cell,row)-unique
    // transforms into EVERY row (including both boundary rows 0 and 63) of
    // every currently-resident cell, drive the boundary, and read back
    // byte-exact — 500 cycles of recycling must not have corrupted the
    // row->region bookkeeping or bled data across a region boundary.
    for &(idx, id) in resident.iter() {
        let sim = frames.begin();
        for row in 0..CAPACITY {
            let val = known_matrix(idx, row);
            let ok = store.write_transform(id, logical_cells[idx].storage_mut(), handles[idx][row as usize], &val, &sim);
            assert!(ok, "write_transform must succeed for a live row in a currently-resident cell");
        }
        let b = sim.end().end().end();
        let mut slots = [CellSlot { id, cell: logical_cells[idx].storage_mut() }];
        b.run(&mut store, &mut slots);
    }
    let total_rows = MAX_RESIDENT as u64 * CAPACITY as u64;
    let data = readback(&ctx, store.transform_buffer(), total_rows * 64);
    let mut rows_checked = 0u32;
    for &(idx, id) in resident.iter() {
        let base = store.row_region_base(id) as usize;
        for row in 0..CAPACITY {
            let expect = known_matrix(idx, row);
            let off = (base + row as usize) * 64;
            let got = as_f32s(&data[off..off + 64]);
            assert_eq!(got, expect, "cell {idx} row {row} (region base {base}) corrupted after {CYCLES}-cycle recycle storm");
            rows_checked += 1;
        }
    }
    assert_eq!(rows_checked, MAX_RESIDENT as u32 * CAPACITY, "every row of every resident cell was spot-checked, including boundary rows 0 and 63");

    let (min, mean, p50, p95, p99, max) = latency_histogram(&mut cycle_latencies_ns);
    println!(
        "[storm2 eviction] {CYCLES} cycles, {TOTAL_LOGICAL} logical cells / {MAX_RESIDENT} residency budget: \
         region-pool distinct bases: cycle50={hwm_at_50} cycle500={hwm_at_500} (equal, == budget)"
    );
    println!("[storm2 eviction] register_cell failures: {register_failures} (expect 0)");
    println!(
        "[storm2 eviction] promote+demote per-cycle latency (ns): min={min} mean={mean:.1} p50={p50} p95={p95} p99={p99} max={max}"
    );
    println!("[storm2 eviction] VRAM spot-check: {rows_checked} rows byte-exact across {MAX_RESIDENT} resident cells (incl. boundary rows 0/{})", CAPACITY - 1);
    println!("[storm2 eviction] SSBO sizes unchanged: transform={transform_bytes_before}B gen={gen_bytes_before}B mirror={mirror_bytes_before}B info={info_bytes_before}B");
}

// ============================================================================
// Storm 3 — Lease revocation latency (C4 2.0 ms budget)
// ============================================================================

#[test]
fn storm3_lease_revocation_latency_1000_requests_under_harvest_load() {
    const BUDGET_MS: f64 = 2.0;
    const N: usize = 1000;
    const WORKERS: usize = 4;

    let mask = LeaseMask::new();
    let stop = AtomicBool::new(false);
    let pipeline = HarvestPipeline::new();

    // Shared read-only workload cell: 512 boxes, queried repeatedly by
    // background threads to generate REAL contention on the same
    // `LeaseMask` the measured lease is drawn from (an active harvest
    // workload, not an idle pool).
    let mut cell = SpatialCell::new(512).unwrap();
    for i in 0..512u32 {
        let x = i as f32;
        cell.alloc(Aabb { min: [x, 0.0, 0.0], max: [x + 1.0, 1.0, 1.0] }).unwrap();
    }

    let mut frames = FrameDriver::new();
    let h = frames.begin().end().end(); // HarvestPhase witness, read-only for the whole test

    let mut latencies_ns: Vec<u128> = Vec::with_capacity(N);

    std::thread::scope(|scope| {
        for w in 0..WORKERS {
            let mask = &mask;
            let stop = &stop;
            let pipeline = &pipeline;
            let cell = &cell;
            let h = &h;
            scope.spawn(move || {
                let mut pad = Scratchpad::new();
                let mut staging = HarvestStaging::new();
                let view = View::Aabb(Aabb { min: [0.0, 0.0, 0.0], max: [512.0, 1.0, 1.0] });
                let client: &'static str = match w {
                    0 => "workload-0",
                    1 => "workload-1",
                    2 => "workload-2",
                    _ => "workload-3",
                };
                while !stop.load(Ordering::Relaxed) {
                    if let Some(lease) = pipeline.acquire_lease(mask, 0.0, client) {
                        pipeline.harvest_cell(cell, 0, MeshClass::Traditional, &view, &mut pad, &mut staging, h);
                        staging.clear();
                        drop(lease);
                    }
                }
            });
        }

        for i in 0..N {
            let lease = loop {
                if let Some(l) = pipeline.acquire_lease(&mask, 0.0, "measured") {
                    break l;
                }
                std::hint::spin_loop();
            };
            // Synthetic clock (this crate never reads real wall time, by
            // design — see harvest.rs's HarvestLease doc): held since 0.0,
            // "now" past the budget, so this lease is unconditionally
            // overdue by construction.
            let now_ms = BUDGET_MS + 1.0;

            let t0 = Instant::now();
            let revoked = pipeline.revoke_overdue(&[&lease], now_ms, BUDGET_MS);
            let observed_revoked = lease.revocation.is_revoked();
            let t1 = Instant::now();

            assert_eq!(revoked, 1, "iteration {i}: overdue lease must be revoked");
            assert!(observed_revoked, "iteration {i}: revocation flag must be observably set by the time the call returns");
            latencies_ns.push((t1 - t0).as_nanos());
            drop(lease);
        }

        stop.store(true, Ordering::Relaxed);
    });

    let (min, mean, p50, p95, p99, max) = latency_histogram(&mut latencies_ns);
    let budget_ns = (BUDGET_MS * 1_000_000.0) as u128;

    println!(
        "[storm3 lease] {N} revocations under {WORKERS}-thread concurrent harvest workload (shared LeaseMask, {} slots)",
        pulsar_scenedb::LEASE_SLOTS
    );
    println!(
        "[storm3 lease] request->dropped latency (ns, synchronous flag-set under contention): min={min} mean={mean:.1} p50={p50} p95={p95} p99={p99} max={max}"
    );
    println!("[storm3 lease] budget: {budget_ns} ns (2.0 ms); p99/budget = {:.4}", p99 as f64 / budget_ns as f64);

    if p99 > budget_ns {
        // Honest-violation protocol: do NOT weaken the assert below. If this
        // branch is ever hit, the storm must be re-run under
        // `#[ignore = "perf-val T6.3: lease revocation p99 busted the 2.0ms \
        // C4 budget — see .superpowers/sdd/perfval-task-6-report.md"]` and
        // the report must carry the full histogram prominently.
        eprintln!("[storm3 lease] HONEST VIOLATION: p99 ({p99} ns) exceeds the 2.0ms C4 budget ({budget_ns} ns)");
    }
    assert!(
        p99 <= budget_ns,
        "C4 budget violated: p99 revocation latency {p99} ns > {budget_ns} ns (2.0 ms) — see the honest-violation protocol note above this assert"
    );
}

// ============================================================================
// Storm 4 — DEI threshold straddle (contract #22)
// ============================================================================

#[test]
fn storm4_dei_threshold_straddle_24_25_26_percent() {
    const LEN: u32 = 1000;
    const REGION_BASE: u32 = 424_242;

    fn boxed_1000() -> SpatialCell {
        let mut cell = SpatialCell::new(LEN).unwrap();
        for i in 0..LEN {
            let x = i as f32;
            cell.alloc(Aabb { min: [x, 0.0, 0.0], max: [x + 1.0, 1.0, 1.0] }).unwrap();
        }
        cell
    }

    let mut frames = FrameDriver::new();
    let h = frames.begin().end().end();
    let pipeline = HarvestPipeline::new();
    let mut pad = Scratchpad::new();

    // ---- 24% (240/1000): strictly below 25% -> must take the DEI branch ---
    let cell_24 = boxed_1000();
    let mut staging_24 = HarvestStaging::new();
    // box i = [i, i+1); query [100.5, 339.5] hits i in {100..=339} -> 240 hits.
    let query_24 = View::Aabb(Aabb { min: [100.5, 0.0, 0.0], max: [339.5, 1.0, 1.0] });
    let n24 = pipeline.harvest_cell(&cell_24, REGION_BASE, MeshClass::Traditional, &query_24, &mut pad, &mut staging_24, &h);
    assert_eq!(n24, 240);
    let ratio_24 = n24 as f64 / LEN as f64;
    assert!((ratio_24 - 0.24).abs() < 1e-9, "self-check: 24% fixture must be EXACTLY 24.0% ({ratio_24})");
    assert!(ratio_24 < 0.25, "GUARD: 24% fixture must genuinely be < 25% ({ratio_24})");
    assert_eq!(staging_24.stats.dei_compacted_runs, 1, "24% hit ratio must take the DEI branch");
    assert_eq!(staging_24.remap.len(), 240, "remap grew by exactly the hit count");

    // Oracle: the plain-path-equivalent output for THIS SAME query,
    // independently computed from the box construction alone (never via
    // harvest_cell/compress_tokens) — the existing oracle pattern
    // (`expected_gens` in gpu_harvest.rs) applied to token identity instead
    // of generation.
    let expected_plain: Vec<u32> = (100..=339u32).map(|i| REGION_BASE + i).collect();
    assert_eq!(
        staging_24.traditional, expected_plain,
        "DEI dense output must be bit-identical to the independently-computed plain-path oracle for the SAME query"
    );
    let dense_bytes_24 = staging_24.traditional.len() as u64 * 4;
    let remap_bytes_24 = staging_24.remap.len() as u64 * 4;
    let dei_total_bytes = dense_bytes_24 + remap_bytes_24;

    // ---- exactly 25% (250/1000): the STRICT `< 0.25` boundary — must NOT
    // take DEI (checks the comparison is strict-less-than, not <=). --------
    let cell_25 = boxed_1000();
    let mut staging_25 = HarvestStaging::new();
    let query_25 = View::Aabb(Aabb { min: [100.5, 0.0, 0.0], max: [349.5, 1.0, 1.0] }); // {100..=349} -> 250
    let n25 = pipeline.harvest_cell(&cell_25, REGION_BASE, MeshClass::Traditional, &query_25, &mut pad, &mut staging_25, &h);
    assert_eq!(n25, 250);
    let ratio_25 = n25 as f64 / LEN as f64;
    assert!((ratio_25 - 0.25).abs() < 1e-9, "self-check: fixture must be EXACTLY 25.0% ({ratio_25})");
    assert_eq!(staging_25.stats.dei_compacted_runs, 0, "exactly 25% must take the PLAIN path — the gate is strict `<`, not `<=`");
    assert_eq!(staging_25.remap.len(), 0, "plain path never touches remap");

    // ---- 26% (260/1000): above threshold -> plain path --------------------
    let cell_26 = boxed_1000();
    let mut staging_26 = HarvestStaging::new();
    let query_26 = View::Aabb(Aabb { min: [100.5, 0.0, 0.0], max: [359.5, 1.0, 1.0] }); // {100..=359} -> 260
    let n26 = pipeline.harvest_cell(&cell_26, REGION_BASE, MeshClass::Traditional, &query_26, &mut pad, &mut staging_26, &h);
    assert_eq!(n26, 260);
    let ratio_26 = n26 as f64 / LEN as f64;
    assert!((ratio_26 - 0.26).abs() < 1e-9, "self-check: 26% fixture must be EXACTLY 26.0% ({ratio_26})");
    assert!(ratio_26 >= 0.25, "GUARD: 26% fixture must genuinely be >= 25% ({ratio_26})");
    assert_eq!(staging_26.stats.dei_compacted_runs, 0, "26% hit ratio must take the PLAIN path");
    assert_eq!(staging_26.remap.len(), 0, "plain path never touches remap");
    let plain_bytes_26 = staging_26.traditional.len() as u64 * 4;

    // ---- Byte-delta accounting at the boundary -----------------------------
    // Three distinct, honestly-separated quantities (see the module doc's
    // "documented interpretation calls" for why this crate's plain path does
    // NOT itself carry a raw-sentinel positional payload today):
    //   (a) DEI's own dense-vs-remap overhead: the cost of carrying the
    //       remap table alongside the dense token array, paid ONLY on the
    //       DEI side.
    //   (b) DEI's total upload vs the plain path's total upload, each at its
    //       own natural operating point (24% vs 26%).
    //   (c) the hypothetical fully-positional/lockstep alternative (every
    //       row incl. sentinels, `len * 4`) neither branch actually uploads
    //       in this crate today — included for contract-#22 context only.
    let dense_vs_remap_overhead = remap_bytes_24; // (a)
    let dei_vs_plain_delta = dei_total_bytes as i64 - plain_bytes_26 as i64; // (b)
    let hypothetical_positional_bytes = LEN as u64 * 4; // (c)

    println!("[storm4 DEI] 24% (240/1000): dei_compacted_runs={} dense={dense_bytes_24}B remap={remap_bytes_24}B total={dei_total_bytes}B", staging_24.stats.dei_compacted_runs);
    println!("[storm4 DEI] 25% (250/1000, strict-< boundary): dei_compacted_runs={} (expect 0)", staging_25.stats.dei_compacted_runs);
    println!("[storm4 DEI] 26% (260/1000): dei_compacted_runs={} dense={plain_bytes_26}B remap=0B total={plain_bytes_26}B", staging_26.stats.dei_compacted_runs);
    println!("[storm4 DEI] byte delta (a) DEI's own dense-vs-remap overhead: {dense_vs_remap_overhead}B");
    println!("[storm4 DEI] byte delta (b) DEI total (24%) vs plain total (26%): {dei_vs_plain_delta}B");
    println!("[storm4 DEI] byte delta (c) hypothetical fully-positional/lockstep upload (len*4, neither branch does this today): {hypothetical_positional_bytes}B");
}
