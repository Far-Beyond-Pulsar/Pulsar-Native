//! GPU-gated allocation-counting gates (perf-val Task 2, spec §8.1) —
//! companion to `tests/alloc_gate.rs`'s CPU-only gates.
//!
//! `HarvestPipeline`/`HarvestStaging` (`src/gpu/harvest.rs`) and
//! `gpu::SceneGpuStore` are declared under `#[cfg(feature = "gpu")] pub mod
//! gpu;` in `src/lib.rs`, so any gate touching them needs a real headless
//! wgpu device and this file's `required-features = ["gpu"]` [[test]] entry
//! (Cargo.toml) — even though the harvest routing logic itself is pure
//! CPU-side (M2b-β T9's doc: "One cell, one view, one scan"). Verified: `grep
//! -n "mod gpu" src/lib.rs` shows the feature gate sits on the WHOLE `gpu`
//! module, not on individual items inside it, so `HarvestPipeline`/
//! `HarvestStaging` inherit the gate transitively despite being logically
//! CPU-only. A single `#[cfg(feature = "gpu")]` submodule inside
//! `alloc_gate.rs` would still force `required-features = ["gpu"]` onto that
//! WHOLE test target (Cargo's `required-features` is per-target, not
//! per-`#[cfg]`-module) and drop the CPU-only query gates out of the
//! featureless matrix — hence the two-file split.
//!
//! See `alloc_gate.rs`'s module doc for the counting-allocator design
//! rationale (thread-local arm flag AND thread-local counter, so concurrent
//! test threads never contaminate each other's measurement window). This
//! file's allocator is a separate instance — each `tests/*.rs` file compiles
//! to its own test binary, so each may declare its own `#[global_allocator]`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;

use pulsar_scenedb::gpu::{
    CellSlot, EngineGpuContext, FrameDriver, HarvestPipeline, HarvestStaging, MeshClass,
    RegionClassConfig, SceneGpuConfig, SceneGpuStore, View,
};
use pulsar_scenedb::{Aabb, Handle, Scratchpad, SpatialCell};

struct CountingAlloc;

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
    static COUNT: Cell<u64> = const { Cell::new(0) };
}

#[inline]
fn bump_if_armed() {
    if ARMED.with(Cell::get) {
        COUNT.with(|c| c.set(c.get() + 1));
    }
}

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        bump_if_armed();
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        bump_if_armed();
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        bump_if_armed();
        unsafe { System.alloc_zeroed(layout) }
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

fn counted<R>(f: impl FnOnce() -> R) -> (u64, R) {
    let before = COUNT.with(Cell::get);
    ARMED.with(|a| a.set(true));
    let out = f();
    ARMED.with(|a| a.set(false));
    let after = COUNT.with(Cell::get);
    (after - before, out)
}

/// Headless wgpu device (same pattern as `tests/gpu_store.rs`/`gpu_harvest.rs`
/// — the test harness owns the `device.poll` pump; no window system needed).
fn test_context() -> EngineGpuContext {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    }))
    .expect("no adapter — GPU tests need a local GPU");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("scenedb-alloc-gate-gpu-test"),
        ..Default::default()
    }))
    .expect("device");
    EngineGpuContext::new(Arc::new(device), Arc::new(queue))
}

/// A `SpatialCell::with_transform` cell populated with `count` unit boxes:
/// box `i` spans `[x_offset + i, x_offset + i + 1)` on x — mirrors
/// `tests/gpu_harvest.rs`'s `boxed_cell` fixture.
fn boxed_cell(capacity: u32, count: u32, x_offset: f32) -> SpatialCell {
    let mut cell = SpatialCell::with_transform(capacity).unwrap();
    for i in 0..count {
        let x = x_offset + i as f32;
        cell.alloc(Aabb { min: [x, 0.0, 0.0], max: [x + 1.0, 1.0, 1.0] }).unwrap();
    }
    cell
}

/// Same as [`boxed_cell`] but also returns every allocated handle, in
/// insertion (== row) order, for callers that need to drive
/// `write_transform` per-handle (the `SceneGpuStore` steady-state gates).
fn boxed_cell_with_handles(capacity: u32, count: u32) -> (SpatialCell, Vec<Handle>) {
    let mut cell = SpatialCell::with_transform(capacity).unwrap();
    let handles = (0..count)
        .map(|i| {
            let x = i as f32;
            cell.alloc(Aabb { min: [x, 0.0, 0.0], max: [x + 1.0, 1.0, 1.0] }).unwrap()
        })
        .collect();
    (cell, handles)
}

fn harvest_scene_cfg() -> SceneGpuConfig {
    SceneGpuConfig {
        classes: vec![RegionClassConfig { capacity: 64, max_resident_cells: 4 }],
        tombstone_headroom: 8,
        max_cells_metadata: 16,
    }
}

/// Gate (b), plain path: `HarvestPipeline::harvest_cell` on a >=25%-hit run
/// (the filter-and-offset branch, not DEI) makes zero allocations on the
/// SECOND call against the same cell/view after an explicit uncounted
/// warm-up — mirrors `tests/gpu_harvest.rs`'s
/// `harvest_makes_zero_new_allocations_after_warmup` capacity-based check,
/// strengthened here to a real allocation count (catches an alloc+free pair
/// that a capacity comparison alone would miss).
#[test]
fn harvest_cell_plain_path_zero_alloc_after_warmup() {
    let ctx = test_context();
    let mut store = SceneGpuStore::new(&ctx, harvest_scene_cfg());
    let cell = boxed_cell(64, 64, 0.0);
    let id = store.register_cell(cell.storage(), 0).unwrap();
    let base = store.row_region_base(id);

    let mut frames = FrameDriver::new();
    let h = frames.begin().end().end();
    let pipeline = HarvestPipeline::new();
    let mut pad = Scratchpad::new();
    let mut staging = HarvestStaging::new();

    // box i = [i, i+1); query [-0.5, 31.5] hits i in {0..=31} -> 32/64 = 50%
    // -> plain path (>= 25%).
    let view = View::Aabb(Aabb { min: [-0.5, 0.0, 0.0], max: [31.5, 1.0, 1.0] });
    let warm_n = pipeline.harvest_cell(&cell, base, MeshClass::Traditional, &view, &mut pad, &mut staging, &h);
    assert_eq!(warm_n, 32);
    assert_eq!(staging.stats.dei_compacted_runs, 0, "sanity: this run must take the plain path");

    // Clear WITHOUT freeing (§8.1) — a fresh `HarvestStaging::new()` here
    // would defeat the entire point of the gate.
    staging.clear();

    let (allocs, n2) = counted(|| {
        pipeline.harvest_cell(&cell, base, MeshClass::Traditional, &view, &mut pad, &mut staging, &h)
    });
    assert_eq!(n2, 32, "steady-state run reproduces the warm-up hit count");
    assert_eq!(allocs, 0, "§8.1: harvest_cell (plain path, incl. gens column) must make zero allocations after warm-up");
}

/// Gate (b), DEI path: identical shape, but a <25%-hit run so `harvest_cell`
/// takes the `compress_tokens` dense-compaction branch both times — the
/// `remap` column growth path has its own allocation surface distinct from
/// the plain path's.
#[test]
fn harvest_cell_dei_path_zero_alloc_after_warmup() {
    let ctx = test_context();
    let mut store = SceneGpuStore::new(&ctx, harvest_scene_cfg());
    let cell = boxed_cell(64, 64, 0.0);
    let id = store.register_cell(cell.storage(), 0).unwrap();
    let base = store.row_region_base(id);

    let mut frames = FrameDriver::new();
    let h = frames.begin().end().end();
    let pipeline = HarvestPipeline::new();
    let mut pad = Scratchpad::new();
    let mut staging = HarvestStaging::new();

    // box i = [i, i+1); query [10.5, 17.5] hits i in {10..=17} -> 8/64 =
    // 12.5% < 25% -> DEI dense-compaction path.
    let view = View::Aabb(Aabb { min: [10.5, 0.0, 0.0], max: [17.5, 1.0, 1.0] });
    let warm_n = pipeline.harvest_cell(&cell, base, MeshClass::Traditional, &view, &mut pad, &mut staging, &h);
    assert_eq!(warm_n, 8, "12.5% hit ratio");
    assert_eq!(staging.stats.dei_compacted_runs, 1, "sanity: this run must take the DEI path");

    staging.clear();

    let (allocs, n2) = counted(|| {
        pipeline.harvest_cell(&cell, base, MeshClass::Traditional, &view, &mut pad, &mut staging, &h)
    });
    assert_eq!(n2, 8, "steady-state run reproduces the warm-up hit count");
    assert_eq!(staging.stats.dei_compacted_runs, 1, "steady-state run also takes the DEI path");
    assert_eq!(allocs, 0, "§8.1: harvest_cell (DEI path, incl. remap + gens columns) must make zero allocations after warm-up");
}

fn store_scene_cfg() -> SceneGpuConfig {
    SceneGpuConfig {
        classes: vec![RegionClassConfig { capacity: 512, max_resident_cells: 2 }],
        tombstone_headroom: 8,
        max_cells_metadata: 4,
    }
}

/// Gate (c), zero-dirty half: a frame boundary (retire -> compact -> sync)
/// with ZERO `write_transform`/`write_instance_info` calls since the
/// previous boundary makes zero allocations. `register_cell` marks the
/// entire occupied region dirty as its warm-up (design §4.1) — the FIRST
/// boundary drains that warm-up sync (uncounted, and asserted non-empty as a
/// sanity check that the warm-up mark was real); the SECOND boundary, with
/// nothing mutated in between, is the actual gate.
#[test]
fn scene_gpu_store_boundary_sync_zero_dirty_rows_zero_alloc() {
    let ctx = test_context();
    let mut store = SceneGpuStore::new(&ctx, store_scene_cfg());
    let (mut cell, _handles) = boxed_cell_with_handles(512, 256);
    let id = store.register_cell(cell.storage(), 0).unwrap();

    let mut frames = FrameDriver::new();

    // Warm-up boundary (uncounted): drains register_cell's full-region mark.
    let sim = frames.begin();
    let mut warm_slots = [CellSlot { id, cell: cell.storage_mut() }];
    let warm_stats = sim.end().end().end().run(&mut store, &mut warm_slots);
    assert!(warm_stats.bytes > 0, "sanity: warm-up boundary actually uploaded the registered region");

    // Steady state: a second boundary, nothing dirtied in between.
    let sim2 = frames.begin();
    let mut slots2 = [CellSlot { id, cell: cell.storage_mut() }];
    let (allocs, stats2) = counted(|| sim2.end().end().end().run(&mut store, &mut slots2));
    assert_eq!((stats2.ranges, stats2.bytes), (0, 0), "sanity: clean frame uploads nothing");
    assert_eq!(allocs, 0, "§8.1: zero-dirty-row boundary sync must make zero allocations");
}

/// Gate (c), N-dirty-rows half: the boundary-sync allocation count with N
/// CONTIGUOUS dirty rows (`write_transform`'d over handles `0..N`, uncounted
/// — only the boundary sync itself is measured) must be INDEPENDENT of N.
/// Run at N=64 and N=256 and assert equal counts.
///
/// What the constant covers (measured: **4 allocations per boundary**, both
/// at N=64 and N=256, once the process-global one-time warm-up below is
/// primed away): a contiguous `0..N` dirty run always coalesces to exactly
/// ONE `write_buffer` range in the transform-column sync
/// (`SceneBuffer::sync_region`'s run-length coalescing, verified
/// independently by `tests/gpu_store.rs`'s
/// `delta_minimality_clean_frame_writes_nothing_and_scattered_rows_coalesce`)
/// regardless of how large that one run is; the instance-info sync and the
/// self-healing slot-mirror boundary scan both walk `rows_in_use()` (fixed at
/// 256 for this fixture, independent of N) and find nothing to upload either
/// way (no compaction/eviction occurred, so `slot_shadow` already matches
/// `slot_column()` from the warm-up boundary). So SceneDB's OWN code issues
/// exactly the same fixed sequence of calls per boundary regardless of N —
/// one `write_transform` loop (uncounted, outside the bracket) and one
/// boundary sync inside it. The measured "4" is wgpu's Rust-side
/// `queue.write_buffer` plumbing cost for that one call (buffer/tracker
/// bookkeeping inside wgpu-core), not SceneDB heap use — this crate holds no
/// scratch Vec anywhere in the sync path that could grow with N. It scales
/// with byte volume (`stats.bytes`, asserted below) but not with the
/// allocation COUNT, which is what §8.1 is about.
#[test]
fn scene_gpu_store_boundary_sync_alloc_count_independent_of_dirty_row_count() {
    let ctx = test_context();

    let run_with_n_dirty = |n: u32| -> u64 {
        let mut store = SceneGpuStore::new(&ctx, store_scene_cfg());
        let (mut cell, handles) = boxed_cell_with_handles(512, 256);
        let id = store.register_cell(cell.storage(), 0).unwrap();
        let mut frames = FrameDriver::new();

        // Warm-up boundary (uncounted): drains register_cell's full-region mark.
        let sim = frames.begin();
        let mut warm_slots = [CellSlot { id, cell: cell.storage_mut() }];
        sim.end().end().end().run(&mut store, &mut warm_slots);

        // Dirty exactly rows [0, n) via write_transform — one contiguous run,
        // uncounted (only the boundary sync below is measured).
        let sim2 = frames.begin();
        for h in handles.iter().take(n as usize) {
            assert!(store.write_transform(id, cell.storage_mut(), *h, &[9.0; 16], &sim2));
        }

        let mut slots2 = [CellSlot { id, cell: cell.storage_mut() }];
        let (allocs, stats) = counted(|| sim2.end().end().end().run(&mut store, &mut slots2));
        assert_eq!(stats.ranges, 1, "N contiguous dirty rows [0,{n}) coalesce into exactly one write_buffer range");
        assert_eq!(stats.bytes, n as u64 * 64, "transform column: n rows * 64 bytes (mat4) uploaded");
        allocs
    };

    // Priming call (uncounted, result discarded): empirically, the very
    // FIRST `write_transform` + boundary-sync issued anywhere in this
    // process costs exactly one extra allocation versus every subsequent
    // one, REGARDLESS of N (verified during investigation by swapping which
    // of N=64/N=256 ran first — the extra alloc always followed whichever
    // call ran FIRST, never a specific N) — a process/device-
    // global one-time warm-up cost inside wgpu's Rust-side plumbing (e.g. a
    // lazily-grown tracker/slotmap reaching steady capacity), not a §8.1
    // violation in SceneDB's own code (which issues the identical, fixed
    // sequence of calls every time — one `write_transform` loop + one
    // `queue.write_buffer`-shaped boundary sync — regardless of N). Priming
    // it here, before either measured closure, is the same "explicit warm-up
    // pass before the counted bracket" discipline the other gates in this
    // file already apply per-buffer; this just extends it to the process-
    // global cost this particular measurement happens to be sensitive to.
    run_with_n_dirty(1);

    let allocs_64 = run_with_n_dirty(64);
    let allocs_256 = run_with_n_dirty(256);
    assert_eq!(
        allocs_64, allocs_256,
        "§8.1: boundary-sync allocation count must be independent of the dirty row count N \
         (N=64 -> {allocs_64} allocs, N=256 -> {allocs_256} allocs)"
    );
}
