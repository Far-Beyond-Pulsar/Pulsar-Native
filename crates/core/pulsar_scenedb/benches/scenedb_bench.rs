use criterion::{criterion_group, criterion_main, Criterion};
use pulsar_scenedb::{Aabb, Frustum, SpatialCell};
use std::hint::black_box;
use std::time::Duration;

#[cfg(feature = "gpu")]
use pulsar_scenedb::gpu::{
    CellSlot, EngineGpuContext, FrameDriver, HarvestPipeline, HarvestStaging, MeshClass,
    RegionClassConfig, SceneGpuConfig, SceneGpuStore, View,
};
#[cfg(feature = "gpu")]
use pulsar_scenedb::{CellStorage, CellType, Scratchpad, TypeToken};
#[cfg(feature = "gpu")]
use std::sync::Arc;
#[cfg(feature = "gpu")]
use std::time::Instant;

/// `scalar_aabb_scan_{256,1024}`: the TRUE scalar arm (perf-val T1 fix —
/// previously this called `query_aabb`, which routes through the runtime
/// SIMD dispatcher and on this host resolves to AVX2, i.e. it measured the
/// exact same path as `aabb_dispatch/dispatched_aabb_scan_*`). This now
/// calls `SpatialCell::query_aabb_scalar_for_bench` (bench-only `#[doc(hidden)]`
/// seam over `crate::simd::aabb_scan_scalar`), so the scalar/dispatched pair
/// shows a genuine SIMD delta.
fn bench_query(c: &mut Criterion) {
    let mut group = c.benchmark_group("spatial_query");
    for &n in &[256u32, 1024] {
        let mut cell = SpatialCell::new(n).unwrap();
        for i in 0..n {
            let f = i as f32;
            cell.alloc(Aabb {
                min: [f, 0.0, 0.0],
                max: [f + 1.0, 1.0, 1.0],
            })
            .unwrap();
        }
        let q = Aabb {
            min: [0.0, 0.0, 0.0],
            max: [n as f32 / 2.0, 1.0, 1.0],
        };
        let mut out = vec![0u32; n as usize];
        group.bench_function(format!("scalar_aabb_scan_{n}"), |b| {
            b.iter(|| black_box(cell.query_aabb_scalar_for_bench(black_box(&q), &mut out)))
        });
    }
    group.finish();
}

fn bench_churn(c: &mut Criterion) {
    c.bench_function("alloc_free_compact_256", |b| {
        b.iter(|| {
            let mut cell = SpatialCell::new(256).unwrap();
            let hs: Vec<_> = (0..256)
                .map(|i| {
                    cell.alloc(Aabb {
                        min: [i as f32; 3],
                        max: [i as f32 + 1.0; 3],
                    })
                    .unwrap()
                })
                .collect();
            for h in hs.iter().step_by(2) {
                cell.free(*h);
            }
            cell.compact();
            black_box(cell.rows_in_use())
        })
    });
}

fn bench_aabb_dispatch(c: &mut criterion::Criterion) {
    let mut group = c.benchmark_group("aabb_dispatch");
    for &n in &[256u32, 1024] {
        let mut cell = SpatialCell::new(n).unwrap();
        for i in 0..n {
            let f = i as f32;
            cell.alloc(Aabb { min: [f, 0.0, 0.0], max: [f + 1.0, 1.0, 1.0] }).unwrap();
        }
        let q = Aabb { min: [0.0, 0.0, 0.0], max: [n as f32 / 2.0, 1.0, 1.0] };
        let mut out = vec![0u32; n as usize];
        // This routes through the runtime dispatcher (AVX2 where available).
        group.bench_function(format!("dispatched_aabb_scan_{n}"), |b| {
            b.iter(|| black_box(cell.query_aabb(black_box(&q), &mut out)))
        });
    }
    group.finish();
}

/// `frustum_scan_1024`: routes through the runtime SIMD dispatcher
/// (`query_frustum` -> `crate::simd::frustum_scan`), same mislabel risk as
/// the AABB pair (perf-val T1) — on this host it resolves to AVX2. Paired
/// below with `scalar_frustum_scan_1024` (true scalar arm) so T7's scaling
/// study has both arms for both kernels. Bench ID kept stable (never
/// renamed — criterion `--baseline` comparability, see the campaign plan's
/// Global Constraints).
fn bench_frustum(c: &mut criterion::Criterion) {
    let mut cell = SpatialCell::new(1024).unwrap();
    for i in 0..1024u32 {
        let f = i as f32;
        cell.alloc(Aabb { min: [f, 0.0, 0.0], max: [f + 1.0, 1.0, 1.0] }).unwrap();
    }
    let f = Frustum { planes: [
        [1.0, 0.0, 0.0, 200.0], [-1.0, 0.0, 0.0, 800.0],
        [0.0, 1.0, 0.0, 10.0], [0.0, -1.0, 0.0, 10.0],
        [0.0, 0.0, 1.0, 10.0], [0.0, 0.0, -1.0, 10.0],
    ] };
    let mut out = vec![0u32; 1024];
    c.bench_function("frustum_scan_1024", |b| {
        b.iter(|| black_box(cell.query_frustum(black_box(&f), &mut out)))
    });
}

/// `scalar_frustum_scan_1024`: the TRUE scalar arm (perf-val T1/T7), added
/// alongside `frustum_scan_1024` — calls
/// `SpatialCell::query_frustum_scalar_for_bench` (bench-only `#[doc(hidden)]`
/// seam over `crate::simd::frustum_scan_scalar`), bypassing the runtime
/// dispatcher the same way `scalar_aabb_scan_*` bypasses it for AABB.
fn bench_scalar_frustum(c: &mut criterion::Criterion) {
    let mut cell = SpatialCell::new(1024).unwrap();
    for i in 0..1024u32 {
        let f = i as f32;
        cell.alloc(Aabb { min: [f, 0.0, 0.0], max: [f + 1.0, 1.0, 1.0] }).unwrap();
    }
    let f = Frustum { planes: [
        [1.0, 0.0, 0.0, 200.0], [-1.0, 0.0, 0.0, 800.0],
        [0.0, 1.0, 0.0, 10.0], [0.0, -1.0, 0.0, 10.0],
        [0.0, 0.0, 1.0, 10.0], [0.0, 0.0, -1.0, 10.0],
    ] };
    let mut out = vec![0u32; 1024];
    c.bench_function("scalar_frustum_scan_1024", |b| {
        b.iter(|| black_box(cell.query_frustum_scalar_for_bench(black_box(&f), &mut out)))
    });
}

// ---------------------------------------------------------------------------
// perf-val T7: query-scan scaling study (contract #19/#50) — scalar vs
// dispatched (AVX2), AABB + frustum, across N ∈ {1024, 16384, 256000,
// 1000448} rows. CPU-only (no `gpu` feature needed, same as the benches
// above).
//
// **Why K cells × 1024 rows, not one N-row cell:** perf-val T2 established
// that `SpatialCell` hard-caps at `MAX_PAGE_CAPACITY` = 1024 rows
// (`src/page.rs`) — a single cell CANNOT hold 16k+ rows. So each N here is
// K cells of exactly 1024 rows apiece (K = 1, 16, 250, 977), and the timed
// closure loops over all K cells per iteration, summing the per-cell hit
// count. This is also the realistic engine shape: the harvest pipeline
// queries per cell, never across a flattened N-row array.
//
// **No per-cell allocation in the timed loop:** one 1024-slot `out: Vec<u32>`
// is built once and reused across every cell in the K-loop (never
// reallocated per cell) — the multi-cell analog of the `_in` variants'
// scratchpad-reuse pattern. The seams here (`query_aabb_scalar_for_bench` /
// `query_frustum_scalar_for_bench`, and the dispatched `query_aabb` /
// `query_frustum`) do NOT take a liveness-words scratchpad parameter — they
// each capture their own `Vec<u64>` liveness snapshot internally (see their
// doc comments in `src/spatial.rs`), so every per-cell call in this bench
// pays that small internal allocation regardless of arm. This is a SHARED,
// non-differential confound: scalar and dispatched both call the same
// wrapper shape, so it inflates absolute ns/row at high K identically in
// both arms and cancels out of the scalar/dispatched RATIO — the quantity
// the contract questions (#19/#50) actually care about. Adding a
// scratchpad-taking scalar seam would be a `src/` change beyond a thin bench
// wrapper; not done here since the confound is provably symmetric (see the
// task report's analysis section for the instruction-level argument).
//
// **Selectivity fixed at the T1 fixture pattern (~50% hit rate) for every
// cell:** box `i` spans `[i, i+1)` on x (`y`/`z` pinned to `[0,1]`), the same
// construction as `bench_query`/`bench_aabb_dispatch` above. AABB query
// `min=[0,0,0], max=[512,1,1]` hits `i` in `0..=512` (513/1024 = 50.1%).
// Frustum plane `[-1,0,0,511.5]` (the only non-vacuous plane against this
// box population — see the task report) hits `i` in `0..=511` (512/1024 =
// 50.0% exactly). Every cell at every N uses the identical query, so
// hit-count scales linearly with N and the per-N honesty guard below can
// assert scalar/dispatched equality cheaply.
fn scan_scaling_fixture(k: u32) -> Vec<SpatialCell> {
    (0..k)
        .map(|_| {
            let mut cell = SpatialCell::new(1024).unwrap();
            for i in 0..1024u32 {
                let x = i as f32;
                cell.alloc(Aabb { min: [x, 0.0, 0.0], max: [x + 1.0, 1.0, 1.0] })
                    .unwrap();
            }
            cell
        })
        .collect()
}

fn bench_scan_scaling(c: &mut Criterion) {
    let q = Aabb {
        min: [0.0, 0.0, 0.0],
        max: [512.0, 1.0, 1.0],
    };
    let f = Frustum {
        planes: [
            [1.0, 0.0, 0.0, 200.0],
            [-1.0, 0.0, 0.0, 511.5],
            [0.0, 1.0, 0.0, 10.0],
            [0.0, -1.0, 0.0, 10.0],
            [0.0, 0.0, 1.0, 10.0],
            [0.0, 0.0, -1.0, 10.0],
        ],
    };

    let mut group = c.benchmark_group("scan_scaling");
    // Reduced from criterion's default (100 samples / 5s measurement time):
    // 16 bench IDs × (10 warm-up-ish + N timed samples) over 4 row-count
    // tiers up to 977 cells must stay under the task's ~4-minute budget.
    // Each single iteration is sub-millisecond even at the largest tier (T1's
    // baseline extrapolates to well under 1 ms/iteration at 1M rows), so
    // criterion's auto-tuned iteration count per sample is not the risk —
    // the fixture-build cost per N tier (up to 977 cells × 1024 allocs) run
    // once outside the timed section is small (sub-second). 30 samples keeps
    // criterion's outlier/statistics machinery meaningful while bounding
    // total wall time comfortably inside budget (see the task report for the
    // measured wall-clock).
    group.sample_size(30);
    group.measurement_time(Duration::from_secs(2));

    for &k in &[1u32, 16, 250, 977] {
        let n = k * 1024;
        let cells = scan_scaling_fixture(k);
        let mut out = vec![0u32; 1024];

        group.bench_function(format!("aabb_scalar_{n}"), |b| {
            b.iter(|| {
                let mut hits = 0u32;
                for cell in &cells {
                    hits += cell.query_aabb_scalar_for_bench(black_box(&q), &mut out);
                }
                black_box(hits)
            })
        });
        group.bench_function(format!("aabb_dispatched_{n}"), |b| {
            b.iter(|| {
                let mut hits = 0u32;
                for cell in &cells {
                    hits += cell.query_aabb(black_box(&q), &mut out);
                }
                black_box(hits)
            })
        });
        group.bench_function(format!("frustum_scalar_{n}"), |b| {
            b.iter(|| {
                let mut hits = 0u32;
                for cell in &cells {
                    hits += cell.query_frustum_scalar_for_bench(black_box(&f), &mut out);
                }
                black_box(hits)
            })
        });
        group.bench_function(format!("frustum_dispatched_{n}"), |b| {
            b.iter(|| {
                let mut hits = 0u32;
                for cell in &cells {
                    hits += cell.query_frustum(black_box(&f), &mut out);
                }
                black_box(hits)
            })
        });

        // In-bench honesty guard (perf-val T7): scalar and dispatched must
        // produce the IDENTICAL total hit count at this N. This is a cheap
        // proxy at this bench's own fixture scale for the bit-identity
        // property tests already proven in `src/simd.rs`
        // (`avx2_matches_scalar_bit_for_bit` / `avx2_frustum_matches_scalar`)
        // — it would catch a regression in THIS bench's harness/fixture, not
        // just a kernel-level regression.
        let mut scratch = vec![0u32; 1024];
        let (mut aabb_s, mut aabb_d, mut frustum_s, mut frustum_d) = (0u32, 0u32, 0u32, 0u32);
        for cell in &cells {
            aabb_s += cell.query_aabb_scalar_for_bench(&q, &mut scratch);
            aabb_d += cell.query_aabb(&q, &mut scratch);
            frustum_s += cell.query_frustum_scalar_for_bench(&f, &mut scratch);
            frustum_d += cell.query_frustum(&f, &mut scratch);
        }
        assert_eq!(aabb_s, aabb_d, "AABB scalar/dispatched hit-count mismatch at N={n}");
        assert_eq!(
            frustum_s, frustum_d,
            "frustum scalar/dispatched hit-count mismatch at N={n}"
        );
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// M2b-β benches (gpu feature only — `SceneGpuStore`/`HarvestPipeline` live
// behind `cfg(feature = "gpu")` in the crate itself, so these benches cannot
// compile without it). Run with:
//   cargo bench -p pulsar_scenedb --features gpu --bench scenedb_bench
// These are numbers, not gates (no assert/regression thresholds) — see the
// M2b-β Task 10 report for the last captured sample set.
// ---------------------------------------------------------------------------

#[cfg(feature = "gpu")]
fn test_context() -> EngineGpuContext {
    // Mirrors `tests/gpu_store.rs::test_context` — upstream wgpu 30 (M3-α
    // Task 1 lineage decision): `InstanceDescriptor` no longer derives
    // `Default`; `new_without_display_handle()` is the headless equivalent,
    // and `apply_limit_buckets: false` preserves unbucketed adapter limits.
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    }))
    .expect("no adapter — GPU benches need a local GPU");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("scenedb-bench"),
        ..Default::default()
    }))
    .expect("device");
    EngineGpuContext::new(Arc::new(device), Arc::new(queue))
}

#[cfg(feature = "gpu")]
fn bench_mat(seed: f32) -> [f32; 16] {
    core::array::from_fn(|i| seed + i as f32)
}

#[cfg(feature = "gpu")]
fn bench_transform_cell(capacity: u32) -> CellStorage {
    let ct = CellType::new("bench-instance")
        .with(TypeToken::of::<[f32; 16]>())
        .build()
        .unwrap();
    CellStorage::from_cell_type(&ct, capacity).unwrap()
}

/// A 1024-box `SpatialCell` for the harvest/DEI benches: box `i` spans
/// `[i, i+1)` on x (y/z pinned to `[0,1]`), so a query's hit set is exactly
/// predictable from its x-range alone (same construction as
/// `tests/gpu_harvest.rs::boxed_cell`, minus the transform column those
/// tests need for `SceneGpuStore` registration — harvest never touches it).
#[cfg(feature = "gpu")]
fn bench_boxed_cell(capacity: u32) -> SpatialCell {
    let mut cell = SpatialCell::new(capacity).unwrap();
    for i in 0..capacity {
        let x = i as f32;
        cell.alloc(Aabb { min: [x, 0.0, 0.0], max: [x + 1.0, 1.0, 1.0] }).unwrap();
    }
    cell
}

/// `region_sync_1024_dirty_rows`: CPU-side cost of syncing a fully-dirty
/// 1024-row region — the transform SSBO delta-upload plus the slot-mirror
/// self-healing boundary scan (`BoundaryPhase::run` = retire → compact →
/// sync; with nothing pending/freed here, retire and compact are no-ops and
/// sync dominates).
///
/// **What this measures:** `queue.write_buffer` calls are asynchronous —
/// this times the CPU-side encode + `write_buffer` submission cost only, NOT
/// GPU execution time. There is no GPU-side timestamp query in this harness.
///
/// **Honest steady-state (perf-val T1 fix):** every iteration now ends its
/// UNTIMED section with `queue.submit(empty()) + device.poll(wait)`, so the
/// `write_buffer` pending-writes staging belt is flushed and reclaimed before
/// the next iteration runs. Previously nothing in this crate ever submitted
/// or polled the device, so the staging belt grew without bound across
/// criterion's iteration-count warm-up (recon measured 17+ GB private bytes
/// and a statistical run that never converged within the default sample
/// count — see `.superpowers/sdd/stress-recon-infrastructure.md` §2). The
/// number recorded here is therefore now a genuine steady-state per-iteration
/// cost, safe to sample statistically at criterion defaults; the earlier
/// smoke-mode number (13.08 µs, `--sample-size 10 --measurement-time 1`) rode
/// an ever-growing staging pool and was not comparable across iterations.
///
/// **Why `iter_custom`, not `iter_batched`:** the brief's suggested shape
/// (`iter_batched(setup = mark all dirty, routine = boundary sync)`) needs
/// `setup` and `routine` to share the same live `store`/`cell` — but
/// `Bencher::iter_batched` takes two independent `FnMut` closures, and both
/// would need to capture `&mut store`/`&mut cell` simultaneously (they are
/// constructed together and both stay alive for the whole `iter_batched`
/// call), which the borrow checker rejects. `iter_custom` gives the same
/// timing isolation — mark-dirty runs untimed inside the loop body, only the
/// boundary run is bracketed by `Instant::now()`, and the submit/poll pump
/// runs untimed after that bracket — from a single closure with ordinary
/// sequential borrows.
#[cfg(feature = "gpu")]
fn bench_region_sync_1024_dirty_rows(c: &mut Criterion) {
    let ctx = test_context();
    let cfg = SceneGpuConfig {
        classes: vec![RegionClassConfig { capacity: 1024, max_resident_cells: 1 }],
        tombstone_headroom: 64,
        max_cells_metadata: 16,
    };
    let mut store = SceneGpuStore::new(&ctx, cfg);
    let mut cell = bench_transform_cell(1024);
    let id = store.register_cell(&cell, 0).unwrap();
    let handles: Vec<_> = (0..1024).map(|_| cell.alloc().unwrap()).collect();
    let mut frames = FrameDriver::new();
    // `Option` + `take()` rather than a bare `SimulateA` local: the witness
    // is consumed by `.end()` each iteration, and an `FnMut` closure cannot
    // move a captured-by-reference variable out of itself directly — only
    // out of a `&mut Option<T>` via `take`, refilled with the next frame's
    // witness before the closure returns.
    let mut sim = Some(frames.begin());

    c.bench_function("region_sync_1024_dirty_rows", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let this_sim = sim.take().expect("witness refilled at the end of every iteration");
                // Untimed: re-mark every row dirty (sync clears the dirty
                // mask each boundary, so a clean second iteration would time
                // an empty sync otherwise).
                for (i, &h) in handles.iter().enumerate() {
                    store.write_transform(id, &mut cell, h, &bench_mat(i as f32), &this_sim);
                }
                let boundary = this_sim.end().end().end();
                let start = Instant::now();
                let stats = {
                    let mut slots = [CellSlot { id, cell: &mut cell }];
                    boundary.run(&mut store, &mut slots)
                };
                total += start.elapsed();
                black_box(stats);
                // Untimed: flush the `write_buffer` pending-writes staging
                // belt every iteration so it doesn't grow unbounded across
                // criterion's iteration-count warm-up (perf-val T1 fix).
                ctx.queue().submit(std::iter::empty());
                ctx.device()
                    .poll(wgpu::PollType::wait_indefinitely())
                    .expect("poll");
                sim = Some(frames.begin());
            }
            total
        });
    });
}

/// `harvest_partition_1024`: pure-CPU `harvest_cell` plain-path cost — one
/// 1024-row cell, a query hitting exactly 512 rows (50%, well above the 25%
/// DEI threshold). No GPU device involved (`harvest_cell` only reads the
/// cell's CPU-side spatial/liveness columns).
#[cfg(feature = "gpu")]
fn bench_harvest_partition_1024(c: &mut Criterion) {
    let cell = bench_boxed_cell(1024);
    let mut frames = FrameDriver::new();
    let h = frames.begin().end().end();
    let pipeline = HarvestPipeline::new();
    let mut pad = Scratchpad::new();
    let mut staging = HarvestStaging::new();
    // box i = [i, i+1); query [-0.5, 511.5] hits i in 0..=511 -> 512/1024 = 50%.
    let view = View::Aabb(Aabb { min: [-0.5, 0.0, 0.0], max: [511.5, 1.0, 1.0] });

    c.bench_function("harvest_partition_1024", |b| {
        b.iter(|| {
            staging.clear();
            let n = pipeline.harvest_cell(
                &cell,
                0,
                MeshClass::Traditional,
                &view,
                &mut pad,
                &mut staging,
                &h,
            );
            black_box(n)
        });
    });
}

/// `dei_compact_1024_sparse`: pure-CPU `harvest_cell` DEI-compaction cost —
/// one 1024-row cell, a query hitting exactly 128 rows (12.5%, below the 25%
/// threshold), forcing `crate::simd::compress_tokens` dense compaction.
#[cfg(feature = "gpu")]
fn bench_dei_compact_1024_sparse(c: &mut Criterion) {
    let cell = bench_boxed_cell(1024);
    let mut frames = FrameDriver::new();
    let h = frames.begin().end().end();
    let pipeline = HarvestPipeline::new();
    let mut pad = Scratchpad::new();
    let mut staging = HarvestStaging::new();
    // box i = [i, i+1); query [-0.5, 127.5] hits i in 0..=127 -> 128/1024 = 12.5%.
    let view = View::Aabb(Aabb { min: [-0.5, 0.0, 0.0], max: [127.5, 1.0, 1.0] });

    c.bench_function("dei_compact_1024_sparse", |b| {
        b.iter(|| {
            staging.clear();
            let n = pipeline.harvest_cell(
                &cell,
                0,
                MeshClass::Traditional,
                &view,
                &mut pad,
                &mut staging,
                &h,
            );
            black_box(n)
        });
    });
}

/// `promotion_demotion_cycle`: one register_cell → unregister_cell →
/// boundary-drain cycle per iteration, with the eviction serial force-
/// completed so the region is actually recycled (drained by `retire`) before
/// the next iteration's `register_cell` — otherwise every iteration after the
/// first would hit `RegionError::RowsExhausted`/`SlotsExhausted` against a
/// still-pinned region. Same `retire`/`compact`/`sync` split-stage pattern as
/// `tests/gpu_store.rs::eviction_returns_region_only_after_serial_completes`.
///
/// **Honest steady-state (perf-val T1 fix):** `compacted.sync` issues
/// `queue.write_buffer` calls the same as `region_sync_1024_dirty_rows`
/// (smaller per-iteration volume here — a 64-row region — but the same
/// unbounded-staging exposure: nothing in this crate ever submits/polls the
/// device). Converted from plain `b.iter` to `iter_custom` so the
/// submit/poll pump can run in an UNTIMED section after each iteration,
/// outside the `Instant::now()` bracket — the timed bracket covers exactly
/// what the old `b.iter` closure covered (register → unregister →
/// force-complete → retire → compact → sync). The recorded number is now a
/// genuine steady-state cost rather than one measured against an
/// ever-growing staging pool.
#[cfg(feature = "gpu")]
fn bench_promotion_demotion_cycle(c: &mut Criterion) {
    let ctx = test_context();
    let cfg = SceneGpuConfig {
        classes: vec![RegionClassConfig { capacity: 64, max_resident_cells: 2 }],
        tombstone_headroom: 8,
        max_cells_metadata: 4,
    };
    let mut store = SceneGpuStore::new(&ctx, cfg);
    let mut cell = bench_transform_cell(64);
    let mut frames = FrameDriver::new();

    c.bench_function("promotion_demotion_cycle", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let start = Instant::now();
                let id = store.register_cell(&cell, 0).unwrap();
                let serial = store.tracker().next_serial();
                store.unregister_cell(id, &mut cell, serial);
                store.tracker().force_complete(serial);
                let boundary = frames.begin().end().end().end();
                let (retired, _drained) = boundary.retire(&mut store, &mut []);
                let compacted = retired.compact(&mut store, &mut []);
                let stats = compacted.sync(&mut store, &mut []);
                total += start.elapsed();
                black_box(stats);
                // Untimed: same pump as `region_sync_1024_dirty_rows` —
                // flush the pending-writes staging belt every iteration.
                ctx.queue().submit(std::iter::empty());
                ctx.device()
                    .poll(wgpu::PollType::wait_indefinitely())
                    .expect("poll");
            }
            total
        });
    });
}

#[cfg(feature = "gpu")]
criterion_group!(
    benches,
    bench_query,
    bench_churn,
    bench_aabb_dispatch,
    bench_frustum,
    bench_scalar_frustum,
    bench_scan_scaling,
    bench_region_sync_1024_dirty_rows,
    bench_harvest_partition_1024,
    bench_dei_compact_1024_sparse,
    bench_promotion_demotion_cycle
);
#[cfg(not(feature = "gpu"))]
criterion_group!(
    benches,
    bench_query,
    bench_churn,
    bench_aabb_dispatch,
    bench_frustum,
    bench_scalar_frustum,
    bench_scan_scaling
);

criterion_main!(benches);
