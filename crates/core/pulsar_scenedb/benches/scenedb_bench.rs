use criterion::{criterion_group, criterion_main, Criterion};
use pulsar_scenedb::{Aabb, Frustum, SpatialCell};
use std::hint::black_box;

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
use std::time::{Duration, Instant};

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
            b.iter(|| black_box(cell.query_aabb(black_box(&q), &mut out)))
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
/// **Why `iter_custom`, not `iter_batched`:** the brief's suggested shape
/// (`iter_batched(setup = mark all dirty, routine = boundary sync)`) needs
/// `setup` and `routine` to share the same live `store`/`cell` — but
/// `Bencher::iter_batched` takes two independent `FnMut` closures, and both
/// would need to capture `&mut store`/`&mut cell` simultaneously (they are
/// constructed together and both stay alive for the whole `iter_batched`
/// call), which the borrow checker rejects. `iter_custom` gives the same
/// timing isolation — mark-dirty runs untimed inside the loop body, only the
/// boundary run is bracketed by `Instant::now()` — from a single closure
/// with ordinary sequential borrows.
#[cfg(feature = "gpu")]
fn bench_region_sync_1024_dirty_rows(c: &mut Criterion) {
    let ctx = test_context();
    let cfg = SceneGpuConfig {
        classes: vec![RegionClassConfig { capacity: 1024, max_resident_cells: 1 }],
        tombstone_headroom: 64,
        max_materials: 16,
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
#[cfg(feature = "gpu")]
fn bench_promotion_demotion_cycle(c: &mut Criterion) {
    let ctx = test_context();
    let cfg = SceneGpuConfig {
        classes: vec![RegionClassConfig { capacity: 64, max_resident_cells: 2 }],
        tombstone_headroom: 8,
        max_materials: 4,
        max_cells_metadata: 4,
    };
    let mut store = SceneGpuStore::new(&ctx, cfg);
    let mut cell = bench_transform_cell(64);
    let mut frames = FrameDriver::new();

    c.bench_function("promotion_demotion_cycle", |b| {
        b.iter(|| {
            let id = store.register_cell(&cell, 0).unwrap();
            let serial = store.tracker().next_serial();
            store.unregister_cell(id, &mut cell, serial);
            store.tracker().force_complete(serial);
            let boundary = frames.begin().end().end().end();
            let (retired, _drained) = boundary.retire(&mut store, &mut []);
            let compacted = retired.compact(&mut store, &mut []);
            let stats = compacted.sync(&mut store, &mut []);
            black_box(stats);
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
    bench_region_sync_1024_dirty_rows,
    bench_harvest_partition_1024,
    bench_dei_compact_1024_sparse,
    bench_promotion_demotion_cycle
);
#[cfg(not(feature = "gpu"))]
criterion_group!(benches, bench_query, bench_churn, bench_aabb_dispatch, bench_frustum);

criterion_main!(benches);
