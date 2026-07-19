//! Allocation-counting gates (perf-val Task 2, spec §8.1 "no per-query
//! allocations" / M2b-β T2's `query_*_in` no-alloc seam) — CPU-only half.
//!
//! GPU-gated counterparts (harvest warm path, `SceneGpuStore` boundary sync)
//! live in `tests/alloc_gate_gpu.rs`: `HarvestPipeline`/`HarvestStaging` and
//! `gpu::SceneGpuStore` are declared under `#[cfg(feature = "gpu")] pub mod
//! gpu;` in `src/lib.rs` even though the harvest CPU-side logic itself is
//! feature-agnostic (M2b-β T9 doc), so any gate touching them requires a
//! `[[test]] required-features = ["gpu"]` target — a single gpu-gated
//! `#[cfg]` module inside this file would still force `required-features =
//! ["gpu"]` on the WHOLE target (Cargo's `required-features` is per-target,
//! not per-`#[cfg]`-module), which would drop gate (a) out of the featureless
//! matrix. Splitting into two files is what keeps gate (a) running by
//! default.
//!
//! # The counting allocator
//!
//! `CountingAlloc` wraps `System` and counts every `alloc`/`realloc`/
//! `alloc_zeroed` call — the allocation-shaped calls; `dealloc` is not
//! counted (freeing capacity that steady-state code never touches is not a
//! §8.1 violation). Two pieces of state, BOTH thread-local:
//!
//! - `ARMED`: the "bracket" flag — only a call made by the thread that
//!   called [`counted`], while inside its closure, counts at all.
//! - the counter itself.
//!
//! Both must be thread-local, not merely the arm flag guarding a single
//! global counter: `cargo test`'s default harness runs many tests
//! concurrently on separate OS threads, each doing its own (unmeasured, and
//! often allocation-heavy — `Vec`/`String`/test-harness bookkeeping)
//! background work for the ENTIRE process lifetime. A shared global counter
//! would let one thread's armed window observe allocation traffic from a
//! completely unrelated concurrently-running test, making the gate flaky by
//! construction. A thread-local counter makes each gate observe only its own
//! thread's allocations, armed or not — exactly the "must not assert global
//! quiet" requirement.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use pulsar_scenedb::{Aabb, Frustum, LivenessSnapshot, SpatialCell};

struct CountingAlloc;

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
    static COUNT: Cell<u64> = const { Cell::new(0) };
}

#[inline]
fn bump_if_armed() {
    // `Cell::get`/`set` on a `const`-initialized thread_local never touches
    // the global allocator itself (no lazy `Option` init to allocate) — safe
    // to call from inside `alloc`/`realloc` without reentering this same
    // allocator.
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
        // Deliberately NOT counted — see module doc.
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

/// Arm the counting allocator for the CALLING THREAD ONLY, run `f`, disarm,
/// and return `(allocs_during_f, f()'s result)`. Nesting is not supported
/// (the flag is a plain bool, not a depth counter) — none of this file's
/// gates nest `counted` calls.
fn counted<R>(f: impl FnOnce() -> R) -> (u64, R) {
    let before = COUNT.with(Cell::get);
    ARMED.with(|a| a.set(true));
    let out = f();
    ARMED.with(|a| a.set(false));
    let after = COUNT.with(Cell::get);
    (after - before, out)
}

fn boxed_cell(n: u32) -> SpatialCell {
    let mut c = SpatialCell::new(n).unwrap();
    for i in 0..n {
        let x = i as f32;
        c.alloc(Aabb { min: [x, 0.0, 0.0], max: [x + 1.0, 1.0, 1.0] }).unwrap();
    }
    c
}

/// Gate (a), AABB half: `query_aabb_in` against caller-provided scratch
/// (a plain `Vec` sized once before the bracket — the M2b-β no-alloc seam
/// doesn't require the crate's `Scratchpad` type specifically, any
/// pre-sized caller buffer satisfies the contract) makes zero allocations in
/// steady state.
#[test]
fn query_aabb_in_zero_alloc_steady_state() {
    // `SpatialCell::new` (`CellStorage`/`PageLayout`) caps a single cell's
    // capacity at `page::MAX_PAGE_CAPACITY` (1024) — use the max.
    let cell = boxed_cell(1024);
    let len = cell.rows_in_use() as usize;
    let mut out = vec![0u32; len];
    let mut words = vec![0u64; len.div_ceil(64)];
    let q = Aabb { min: [100.0, 0.0, 0.0], max: [900.0, 1.0, 1.0] };

    // Explicit warm-up (uncounted): sizes `out`/`words` to their steady-state
    // capacity and exercises the exact code path once before measurement.
    let nw = LivenessSnapshot::capture_words(cell.storage().liveness(), len as u32, &mut words);
    let warm_hits = cell.query_aabb_in(&q, &words[..nw], &mut out);
    assert!(warm_hits > 0, "sanity: warm-up query must hit something real");

    let (allocs, hits) = counted(|| {
        let nw = LivenessSnapshot::capture_words(cell.storage().liveness(), len as u32, &mut words);
        cell.query_aabb_in(&q, &words[..nw], &mut out)
    });
    assert_eq!(hits, warm_hits, "steady-state query reproduces the warm-up hit count");
    assert_eq!(allocs, 0, "§8.1: query_aabb_in must make zero allocations in steady state");
}

/// Gate (a), frustum half: identical shape for `query_frustum_in`.
#[test]
fn query_frustum_in_zero_alloc_steady_state() {
    let cell = boxed_cell(1024);
    let len = cell.rows_in_use() as usize;
    let mut out = vec![0u32; len];
    let mut words = vec![0u64; len.div_ceil(64)];
    // Axis-aligned box [100, 900] on x, wide open on y/z — same hit shape as
    // the AABB gate above, expressed as six inward-normal planes.
    let f = Frustum {
        planes: [
            [1.0, 0.0, 0.0, -100.0],
            [-1.0, 0.0, 0.0, 900.0],
            [0.0, 1.0, 0.0, 1000.0],
            [0.0, -1.0, 0.0, 1000.0],
            [0.0, 0.0, 1.0, 1000.0],
            [0.0, 0.0, -1.0, 1000.0],
        ],
    };

    let nw = LivenessSnapshot::capture_words(cell.storage().liveness(), len as u32, &mut words);
    let warm_hits = cell.query_frustum_in(&f, &words[..nw], &mut out);
    assert!(warm_hits > 0, "sanity: warm-up query must hit something real");

    let (allocs, hits) = counted(|| {
        let nw = LivenessSnapshot::capture_words(cell.storage().liveness(), len as u32, &mut words);
        cell.query_frustum_in(&f, &words[..nw], &mut out)
    });
    assert_eq!(hits, warm_hits, "steady-state query reproduces the warm-up hit count");
    assert_eq!(allocs, 0, "§8.1: query_frustum_in must make zero allocations in steady state");
}

/// FOLDED T1 review minor: a seam == dispatched pinning test. The bench-only
/// scalar seams (`query_aabb_scalar_for_bench`/`query_frustum_scalar_for_bench`,
/// `src/spatial.rs:219`/`:248`) exist purely so the benches can force the
/// non-dispatched scalar backend; nothing previously pinned their output
/// against the runtime-dispatched public API on the SAME data, so a future
/// edit to either kernel silently diverging from the other would only show
/// up as a bench-number curiosity, never a test failure. This drives both
/// query shapes over a randomized cell and asserts bit-identical
/// `(hit_count, out_buffer)` pairs between the dispatched and scalar-forced
/// paths, killing that drift class outright.
#[test]
fn scalar_seam_pins_to_dispatched_output_aabb_and_frustum() {
    use rand::{Rng, SeedableRng};
    let mut rng = rand::rngs::StdRng::seed_from_u64(0xA10C_5EAD);
    let mut c = SpatialCell::new(512).unwrap();
    for _ in 0..300 {
        let min: [f32; 3] = std::array::from_fn(|_| rng.gen_range(-50.0..50.0));
        let ext: [f32; 3] = std::array::from_fn(|_| rng.gen_range(0.0..8.0));
        let max = [min[0] + ext[0], min[1] + ext[1], min[2] + ext[2]];
        c.alloc(Aabb { min, max }).unwrap();
    }
    let len = c.rows_in_use() as usize;

    for _ in 0..20 {
        let qmin: [f32; 3] = std::array::from_fn(|_| rng.gen_range(-50.0..50.0));
        let qext: [f32; 3] = std::array::from_fn(|_| rng.gen_range(0.0..40.0));
        let q = Aabb { min: qmin, max: [qmin[0] + qext[0], qmin[1] + qext[1], qmin[2] + qext[2]] };
        let mut out_dispatched = vec![0u32; len];
        let mut out_scalar = vec![0u32; len];
        let n_d = c.query_aabb(&q, &mut out_dispatched);
        let n_s = c.query_aabb_scalar_for_bench(&q, &mut out_scalar);
        assert_eq!(
            (n_d, &out_dispatched),
            (n_s, &out_scalar),
            "dispatched vs scalar-forced AABB seam drift — src/spatial.rs:219"
        );
    }

    // Six inward-normal planes of an axis-aligned box centered at
    // (cx,cy,cz) with half-extent `half`, randomized per iteration.
    for _ in 0..20 {
        let half: f32 = rng.gen_range(5.0..60.0);
        let cx: f32 = rng.gen_range(-30.0..30.0);
        let cy: f32 = rng.gen_range(-30.0..30.0);
        let cz: f32 = rng.gen_range(-30.0..30.0);
        let f = Frustum {
            planes: [
                [1.0, 0.0, 0.0, half - cx],
                [-1.0, 0.0, 0.0, half + cx],
                [0.0, 1.0, 0.0, half - cy],
                [0.0, -1.0, 0.0, half + cy],
                [0.0, 0.0, 1.0, half - cz],
                [0.0, 0.0, -1.0, half + cz],
            ],
        };
        let mut out_dispatched = vec![0u32; len];
        let mut out_scalar = vec![0u32; len];
        let n_d = c.query_frustum(&f, &mut out_dispatched);
        let n_s = c.query_frustum_scalar_for_bench(&f, &mut out_scalar);
        assert_eq!(
            (n_d, &out_dispatched),
            (n_s, &out_scalar),
            "dispatched vs scalar-forced frustum seam drift — src/spatial.rs:248"
        );
    }
}
