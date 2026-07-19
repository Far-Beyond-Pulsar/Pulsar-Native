# SceneDB 2.0 — M2b-β Implementation Plan (Streaming Grid, Residency, Harvest)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement design Rev 2's β scope — the concentric streaming grid with region-backed residency (promotion/eviction over M2b-α's `SceneGpuStore`), and the zero-alloc multi-view harvest pipeline (partition, DEI compaction, lease/snapshot wiring) — closing the §11 β carry-forwards (recycled slot-region tail scrub, exhaustion-as-graceful-degradation).

**Architecture:** Two serialized waves. **Wave 1 (Tasks 1–5):** the cell shape β needs (bounds + token-registered transform), the no-alloc query seams, the pure-logic `StreamingGrid` (Test 11), and residency (`unregister_cell` eviction + tail-scrubbed promotion + serial-pinned region recycling, Test D2-tail). **Wave 2 (Tasks 6–10):** `HarvestPipeline` (per-view single-scan partition emitting global-row tokens), DEI dense compaction (scalar → AVX2 bit-identical), lease timeout/revocation (Test 10), multi-view, benches + docs.

**Tech Stack:** Rust 2021, wgpu 28 fork (workspace), existing M1b types (`LeaseMask`, `Scratchpad`, `LivenessSnapshot`, `RevocationFlag`), M2b-α `SceneGpuStore`/phase machine.

**Design of record:** `docs/superpowers/specs/2026-07-14-scenedb20-m2b-streaming-orchestration-design.md` (Rev 2) §4, §5, §5.1, §6.1, §8, §9 β-gates, **§11 carry-forwards**. Spec: §5 (streaming), §5.3 (budget), §5.5 (hysteresis), §8.3–8.5 (tokens/DEI), §9 (leases). Contracts C3, C4.

## Global Constraints

- **C0:** `cargo check -p pulsar_scenedb --no-default-features` stays green. Grid/harvest GPU-side code under `#[cfg(feature = "gpu")]`; pure-logic grid classification and DEI kernels live where they need no wgpu (grid classification is gpu-module anyway since it drives `SceneGpuStore`; DEI compress kernels go in core `simd.rs` — they see only `u32` tokens).
- **C4:** null token `0xFFFF_FFFF` (`NULL_ROW`); row tokens valid for the issuing frame only; DEI threshold **< 25%** triggers dense compaction; remap layout `remap[dense_i] = original_run_index: u32` is **M3-frozen**.
- **§5.5:** promotion boundary = cell bounds + `Δpad` (default **10% of cell width**); demotion boundary = promotion boundary + `δhyst`. Sub-pad jitter must cause **zero** transitions (Test 11).
- **§6.1:** NO row-granularity harvest pins. The only serial pinning is region-granularity (eviction). Queue-FIFO ordering is the safety invariant.
- **§11 carry-forwards closed here:** recycled slot-region **tail scrub** at promotion; slot/row-region **exhaustion as graceful degradation** (cell stays in current domain + telemetry, never a panic on the residency path).
- **SIMD discipline (M1b):** scalar reference first; AVX2 arm must be **bit-for-bit identical** across ≥200 randomized cases; runtime dispatch like `simd.rs`.
- **Transitions ordering (C3/§4.1):** classification during Simulate; transitions execute at the boundary **between retire and compact**, witnessed by `&RetiredPhase`; never mid-frame.
- **Eviction refinement of design §4.1 (record in design when landing Task 4):** pending retires are committed **CPU-side immediately at eviction** (zero VRAM writes — the region pin alone protects VRAM until the serial completes; §4.1's "at region-free completion" wording existed only to prevent VRAM writes into freed regions, and none happen).
- **GPU test suites:** run sequentially, `-- --test-threads=1` (device contention).
- **Windows:** author `.rs` files ONLY via Write/Edit tools (BOM hazard). Commit style `type(scenedb): …` + trailer `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.

## File Structure

```
crates/core/pulsar_scenedb/
  src/cell.rs           # + pub(crate) register_token_column::<T>(user_col)      [T1]
  src/spatial.rs        # + with_transform(), TRANSFORM_COLUMN, query_*_in()     [T1,T2]
  src/lease.rs          # + Scratchpad::get_u32_u64 split borrow                 [T2]
  src/snapshot.rs       # + LivenessSnapshot::capture_words (into caller scratch)[T2]
  src/simd.rs           # + compress_tokens (DEI) scalar + AVX2 arms             [T6,T7]
  src/gpu/grid.rs       # StreamingGrid, GridConfig, Domain, CellCoord,
                        # Transition, StreamingBudget, execute_transitions       [T3,T4,T5]
  src/gpu/harvest.rs    # View, MeshClass, HarvestStaging, HarvestPipeline,
                        # lease/revocation wiring, revalidate_run                [T6,T8,T9]
  src/gpu/scene_store.rs# + unregister_cell, tail scrub in register_cell,
                        #   region-pool drain in retire_all, cell_metadata write [T4,T5]
  src/gpu/mod.rs        # + mod grid; mod harvest; re-exports                    [T3,T6]
  tests/gpu_store.rs    # residency/eviction/D2-tail gates                       [T4,T5]
  tests/gpu_harvest.rs  # NEW [[test]] target: harvest/DEI/lease gates (10/12)   [T6+]
  benches/scenedb_bench.rs # region sync, partition+DEI, promotion/demotion      [T10]
  Cargo.toml            # + [[test]] gpu_harvest                                 [T6]
```

---

## Wave 1 — Grid & Residency

### Task 1: GPU-mirrorable spatial cells (`SpatialCell::with_transform`)

**Files:**
- Modify: `crates/core/pulsar_scenedb/src/cell.rs` (one new `pub(crate)` method)
- Modify: `crates/core/pulsar_scenedb/src/spatial.rs`
- Test: inline in `spatial.rs` + one integration test appended to `tests/gpu_store.rs`

**Why:** β cells need BOTH the six bounds columns (queries) and the `[f32; 16]` transform column (`SceneGpuStore::write_transform` resolves it **token-keyed** via `column_for_mut::<[f32;16]>`). `SpatialCell::new` builds positionally (empty token index) and `CellType` can't hold six same-type `f32` tokens — so the transform column is registered into the token index explicitly.

**Interfaces:**
- Produces:
  - `CellStorage::register_token_column::<T: Pod + 'static>(&mut self, user_col: usize)` (`pub(crate)`) — appends `(TypeToken::of::<T>().id(), user_col)` to `token_index`; debug_asserts the id isn't already present and `size_of::<T>() == column desc size`.
  - `SpatialCell::with_transform(capacity: u32) -> Result<Self, LayoutError>` — seven user columns: six `f32` bounds + `[f32; 16]` at index `TRANSFORM_COLUMN`.
  - `pub const TRANSFORM_COLUMN: usize = SPATIAL_COLUMNS;` (= 6) in `spatial.rs`.

- [ ] **Step 1: Failing tests**

`spatial.rs` tests:

```rust
#[test]
fn with_transform_exposes_token_keyed_mat4_column() {
    let mut c = SpatialCell::with_transform(64).unwrap();
    let h = c.alloc(aabb([0.0; 3], [1.0; 3])).unwrap();
    let row = c.row_of(h).unwrap() as usize;
    c.storage_mut().column_for_mut::<[f32; 16]>().unwrap()[row] = [7.0; 16];
    assert_eq!(c.storage().column_for::<[f32; 16]>().unwrap()[row], [7.0; 16]);
    // Bounds columns unaffected and still positional:
    assert_eq!(c.storage().user_column::<f32>(0)[row], 0.0);
}
```

`tests/gpu_store.rs` (append; proves the store accepts these cells end-to-end):

```rust
#[test]
fn spatial_cell_with_transform_registers_and_syncs() {
    let ctx = test_context();
    let mut store = SceneGpuStore::new(&ctx, scene_cfg());
    let mut frames = FrameDriver::new();
    let mut sc = pulsar_scenedb::SpatialCell::with_transform(64).unwrap();
    let id = store.register_cell(sc.storage(), 0).unwrap();
    let h = sc.alloc(pulsar_scenedb::Aabb { min: [0.0; 3], max: [1.0; 3] }).unwrap();
    let sim = frames.begin();
    assert!(store.write_transform(id, sc.storage_mut(), h, &mat(5.0), &sim));
    let b = sim.end().end().end();
    let mut slots = [CellSlot { id, cell: sc.storage_mut() }];
    b.run(&mut store, &mut slots);
    let base = store.row_region_base(id) as usize;
    let gpu = as_f32s(&readback(&ctx, store.transform_buffer(), (64 * 4 * 64) as u64));
    assert_eq!(&gpu[base * 16..base * 16 + 16], &mat(5.0));
}
```

- [ ] **Step 2: verify FAIL** (`with_transform`/`register_token_column` unresolved).
- [ ] **Step 3: Implement**

`cell.rs`:

```rust
    /// Register a token→user-column mapping on a positionally-constructed
    /// cell so `column_for::<T>()` resolves (M2b-β: `SpatialCell` carries six
    /// same-type bounds columns that CellType's type-keyed tokens cannot
    /// express, plus one token-keyed transform column for the GPU mirror).
    pub(crate) fn register_token_column<T: crate::page::Pod + 'static>(&mut self, user_col: usize) {
        let id = TypeToken::of::<T>().id();
        debug_assert!(
            !self.token_index.iter().any(|(tid, _)| *tid == id),
            "token already registered on this cell"
        );
        debug_assert_eq!(
            self.page.layout().column_descs()[user_col + 1].size as usize,
            std::mem::size_of::<T>(),
            "token type size does not match the column stride"
        );
        self.token_index.push((id, user_col));
    }
```

`spatial.rs`:

```rust
/// User-column index of the GPU-mirrored transform column on cells built
/// with [`SpatialCell::with_transform`].
pub const TRANSFORM_COLUMN: usize = SPATIAL_COLUMNS;

    /// A spatial cell that also carries a token-registered `[f32; 16]`
    /// transform column (user column [`TRANSFORM_COLUMN`]) so it can be
    /// registered with `gpu::SceneGpuStore` (which resolves the mirrored
    /// column token-keyed). Stride: 6×4 + 64 = 88 B ≤ the C2 128 B ceiling.
    pub fn with_transform(capacity: u32) -> Result<Self, LayoutError> {
        let mut columns = [ColumnDesc::of::<f32>(); SPATIAL_COLUMNS + 1];
        columns[TRANSFORM_COLUMN] = ColumnDesc::of::<[f32; 16]>();
        let mut storage = CellStorage::new(&columns, capacity)?;
        storage.register_token_column::<[f32; 16]>(TRANSFORM_COLUMN);
        Ok(Self { storage })
    }
```

(Note the array type: build a 7-element `ColumnDesc` array — six `f32` then the mat4. If `[ColumnDesc; 7]` from-fn syntax fights you, use a `Vec<ColumnDesc>`.)

- [ ] **Step 4: run** — `cargo test -p pulsar_scenedb --lib spatial` PASS; `cargo test -p pulsar_scenedb --features gpu --test gpu_store -- --test-threads=1` (19) PASS; `--no-default-features` green.
- [ ] **Step 5: Commit** — `feat(scenedb): SpatialCell::with_transform — GPU-mirrorable spatial cells (M2b-b T1)`

---

### Task 2: No-alloc query seams (§5.1 — the §8.1 carry-forward)

**Files:**
- Modify: `src/lease.rs`, `src/snapshot.rs`, `src/spatial.rs`
- Test: inline in each

**Interfaces:**
- Produces:
  - `Scratchpad::get_u32_u64(&mut self, len32: usize, len64: usize) -> (&mut [u32], &mut [u64])` — simultaneous split borrow of both buffers (the existing `get_u32`/`get_u64` are exclusive borrows; harvest needs token scratch AND snapshot words at once). Updates both peaks.
  - `LivenessSnapshot::capture_words(mask: &LivenessMask, len: u32, out: &mut [u64]) -> usize` (associated fn) — fills the `ceil(len/64)` words covering rows `0..len` into caller scratch, returns the word count; same Relaxed-load + phase-barrier contract as `capture` (copy that doc verbatim).
  - `SpatialCell::query_aabb_in(&self, q: &Aabb, liveness_words: &[u64], out: &mut [u32]) -> u32` and `query_frustum_in(...)` — identical to the existing functions but scanning against caller-provided words (the kernels already take `&[u64]`); `debug_assert_eq!(liveness_words.len(), len.div_ceil(64))`. The existing `query_aabb`/`query_frustum` become thin wrappers that capture into a local `Vec` and delegate (doc updated: "allocates; harvest uses `query_aabb_in` with `Scratchpad` words — §8.1").

- [ ] **Step 1: Failing tests**

```rust
// lease.rs
#[test]
fn split_borrow_returns_both_buffers() {
    let mut pad = Scratchpad::new();
    let (t, w) = pad.get_u32_u64(100, 4);
    t[0] = 7;
    w[0] = 0xFF;
    assert!(t.len() >= 100 && w.len() >= 4);
    assert!(pad.buf_len_u32() >= 100 && pad.buf_len_u64() >= 4);
}

// snapshot.rs
#[test]
fn capture_words_matches_owned_capture() {
    let mask = LivenessMask::new(128);
    for i in 0..70 { mask.set_live(i); }
    mask.set_dead(3);
    let owned = LivenessSnapshot::capture(&mask, 70);
    let mut scratch = [0u64; 2];
    let n = LivenessSnapshot::capture_words(&mask, 70, &mut scratch);
    assert_eq!(n, 2);
    assert_eq!(&scratch[..n], owned.words());
}

// spatial.rs — bit-identity of the _in variants against the allocating path
#[test]
fn query_in_variants_match_allocating_queries() {
    let mut c = SpatialCell::new(256).unwrap();
    for i in 0..40 {
        let f = i as f32;
        c.alloc(aabb([f, 0.0, 0.0], [f + 0.5, 1.0, 1.0])).unwrap();
    }
    let q = aabb([3.0, 0.0, 0.0], [20.0, 1.0, 1.0]);
    let len = c.rows_in_use() as usize;
    let mut out_a = vec![0u32; len];
    let mut out_b = vec![0u32; len];
    let n_a = c.query_aabb(&q, &mut out_a);
    let mut words = vec![0u64; len.div_ceil(64)];
    let nw = LivenessSnapshot::capture_words(c.storage().liveness(), len as u32, &mut words);
    let n_b = c.query_aabb_in(&q, &words[..nw], &mut out_b);
    assert_eq!((n_a, &out_a), (n_b, &out_b), "in-variant is bit-identical");
}
```

- [ ] **Step 2: verify FAIL.**
- [ ] **Step 3: Implement.** `get_u32_u64` grows both `Vec`s then returns `(&mut self.u32_buf[..len32], &mut self.u64_buf[..len64])` — two disjoint fields borrowed simultaneously from `&mut self` is legal (field-level split borrow); update both `peak_*_this_window`. `capture_words`: same body as `capture` but writing into `out` (assert `out.len() >= n_words`). `query_*_in`: extract the current column-slicing + kernel call from `query_aabb`/`query_frustum` into the `_in` form; wrappers capture into a local Vec and call `_in` (behavior unchanged — the existing spatial test suite is the regression oracle).
- [ ] **Step 4: run** — full `cargo test -p pulsar_scenedb --lib --tests` (126+) PASS.
- [ ] **Step 5: Commit** — `feat(scenedb): no-alloc query seams — split-borrow scratchpad, capture_words, query_*_in (M2b-b T2, closes §8.1 carry-forward)`

---

### Task 3: `StreamingGrid` — pure-logic classification, hysteresis, cross-fade (Test 11)

**Files:**
- Create: `src/gpu/grid.rs`; Modify: `src/gpu/mod.rs` (`mod grid;` + `pub use grid::{CellCoord, Domain, GridConfig, StreamingBudget, BudgetError, StreamingGrid, Transition};`)
- Test: inline in `grid.rs` (pure logic — Test 11 lives here)

**Interfaces:**
- Produces (exact):

```rust
#[derive(Clone, Copy, Debug)]
pub struct GridConfig {
    pub cell_width: f32,
    pub margin_radius: f32,   // world units beyond the inner union
    pub pad_fraction: f32,    // §5.5 Δpad, default 0.10
    pub hysteresis: f32,      // §5.5 δhyst, world units beyond the pad
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)] pub struct CellCoord { pub x: i32, pub z: i32 }
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum Domain { Inner, Margin, Outer }
#[derive(Clone, Copy, Debug, PartialEq)] pub struct Transition { pub coord: CellCoord, pub from: Domain, pub to: Domain }
pub struct StreamingBudget {
    pub vram_hlod_budget: u64, pub vram_geometry_budget: u64,
    pub max_materialized_cells: u32,          // bounded world extent (α final-review M1)
    pub proxy_mesh_bytes: u64, pub mean_cell_geometry_bytes: u64,
}
#[derive(Debug, PartialEq)] pub enum BudgetError { HlodOverBudget, GeometryOverBudget }

impl StreamingGrid {
    pub fn new(cfg: GridConfig, budget: StreamingBudget, inner_classes: &[RegionClassConfig]) -> Result<Self, BudgetError>;
    /// Track a content-bearing cell (assigns a dense id, starts Outer).
    pub fn materialize(&mut self, coord: CellCoord) -> u32; // dense id
    pub fn domain(&self, coord: CellCoord) -> Option<Domain>;
    pub fn alpha(&self, coord: CellCoord) -> Option<f32>;
    pub fn dense_id(&self, coord: CellCoord) -> Option<u32>;
    /// §5 classification with §5.5 hysteresis. Call during Simulate; queues
    /// transitions, applies NO state change to domains yet.
    pub fn classify(&mut self, observer_aabbs: &[Aabb]);
    /// Drain queued transitions (caller executes them at the boundary).
    pub fn take_transitions(&mut self) -> Vec<Transition>;
    /// Confirm an executed transition (caller reports success/decline).
    pub fn commit_transition(&mut self, t: Transition);
    /// §5.2: advance cross-fade by observer world-distance travelled.
    pub fn advance_crossfade(&mut self, distance: f32, fade_distance: f32);
}
```

Classification semantics (write exactly): a cell's **base bounds** are its world AABB from `coord × cell_width`. Inner target: base bounds **intersect** the union of observer AABBs (test each observer AABB, any-of). Margin target: within `margin_radius` of any observer AABB (grow the observer AABB by `margin_radius` and test intersection). Hysteresis (§5.5): to PROMOTE (Outer→Margin, Margin→Inner) the observer must intersect the cell bounds **shrunk** by nothing — promotion boundary = bounds + `pad` where `pad = pad_fraction × cell_width` (i.e. grow cell bounds by `pad` before testing); to DEMOTE, the observer must be outside bounds + `pad` + `hysteresis` (grow by `pad + hysteresis`; if still intersecting at that size, no demotion). One domain step per boundary (Outer→Margin→Inner across two boundaries is fine and simpler; document). α: promotion toward resident raises α target to 1, demotion lowers to 0; `advance_crossfade` moves α linearly by `distance / fade_distance`, clamped.

- [ ] **Step 1: Failing tests** — the Test 11 gate plus basics:

```rust
fn cfg() -> GridConfig { GridConfig { cell_width: 100.0, margin_radius: 150.0, pad_fraction: 0.10, hysteresis: 20.0 } }
fn budget() -> StreamingBudget {
    StreamingBudget { vram_hlod_budget: u64::MAX, vram_geometry_budget: u64::MAX,
        max_materialized_cells: 1024, proxy_mesh_bytes: 1024, mean_cell_geometry_bytes: 1 << 20 }
}
fn observer_at(x: f32) -> Aabb { Aabb { min: [x - 10.0, -10.0, -10.0], max: [x + 10.0, 10.0, 10.0] } }

#[test]
fn test11_subpad_jitter_causes_zero_transitions() {
    let mut g = StreamingGrid::new(cfg(), budget(), &[]).unwrap();
    g.materialize(CellCoord { x: 0, z: 0 });
    g.materialize(CellCoord { x: 1, z: 0 });
    // Park the observer just past cell 0's edge toward cell 1, then jitter
    // within the 10-unit pad (10% of 100).
    g.classify(&[observer_at(95.0)]);
    for t in g.take_transitions() { g.commit_transition(t); }
    let settled: Vec<_> = [CellCoord { x: 0, z: 0 }, CellCoord { x: 1, z: 0 }]
        .iter().map(|c| g.domain(*c).unwrap()).collect();
    for i in 0..200 {
        let jitter = ((i % 7) as f32 - 3.0) * 1.0; // ±3 units — sub-pad
        g.classify(&[observer_at(95.0 + jitter)]);
        assert!(g.take_transitions().is_empty(), "jitter frame {i} caused a transition");
    }
    let after: Vec<_> = [CellCoord { x: 0, z: 0 }, CellCoord { x: 1, z: 0 }]
        .iter().map(|c| g.domain(*c).unwrap()).collect();
    assert_eq!(settled, after, "domains unchanged under sub-pad jitter");
}

#[test]
fn test11_decisive_crossing_promotes_exactly_once_and_demotion_lags_by_hysteresis() {
    let mut g = StreamingGrid::new(cfg(), budget(), &[]).unwrap();
    let far = CellCoord { x: 5, z: 0 }; // cell spanning x ∈ [500, 600)
    g.materialize(far);
    g.classify(&[observer_at(0.0)]);
    for t in g.take_transitions() { g.commit_transition(t); }
    assert_eq!(g.domain(far), Some(Domain::Outer));
    // Decisive move into margin range of the far cell:
    g.classify(&[observer_at(480.0)]); // 150-unit margin reach + pad covers [500,600)
    let ts = g.take_transitions();
    assert_eq!(ts.len(), 1, "exactly one transition");
    assert_eq!(ts[0], Transition { coord: far, from: Domain::Outer, to: Domain::Margin });
    g.commit_transition(ts[0]);
    // Retreat to just inside the demotion boundary → NO demotion (hysteresis):
    g.classify(&[observer_at(480.0 - cfg().hysteresis + 1.0)]);
    assert!(g.take_transitions().is_empty(), "inside hysteresis band: no demotion");
    // Retreat past it → demotion:
    g.classify(&[observer_at(300.0)]);
    let ts = g.take_transitions();
    assert_eq!(ts.len(), 1);
    assert_eq!(ts[0].to, Domain::Outer);
}

#[test]
fn budget_violation_fails_construction() {
    let mut b = budget();
    b.vram_hlod_budget = 10; // 1024 cells × 1 KiB proxies ≫ 10 bytes
    assert_eq!(StreamingGrid::new(cfg(), b, &[]).unwrap_err(), BudgetError::HlodOverBudget);
}

#[test]
fn crossfade_advances_by_world_distance_and_clamps() {
    let mut g = StreamingGrid::new(cfg(), budget(), &[]).unwrap();
    let c = CellCoord { x: 0, z: 0 };
    g.materialize(c);
    g.classify(&[observer_at(50.0)]);
    for t in g.take_transitions() { g.commit_transition(t); }
    // Now heading resident: α target 1.
    g.advance_crossfade(25.0, 100.0);
    assert!((g.alpha(c).unwrap() - 0.25).abs() < 1e-6);
    g.advance_crossfade(1000.0, 100.0);
    assert_eq!(g.alpha(c).unwrap(), 1.0, "clamped");
}
```

- [ ] **Step 2: verify FAIL.** **Step 3: Implement** per the semantics block (pure logic: `HashMap<CellCoord, GridCellState { domain, dense_id, alpha, alpha_target }>`, `Vec<Transition>` queue; budget check per spec §5.3: `max_materialized_cells × proxy_mesh_bytes ≤ vram_hlod_budget` and `Σ inner_classes(max_resident_cells × mean_cell_geometry_bytes) ≤ vram_geometry_budget` — with the α-recorded permanent-proxy term folded into the HLOD side; classification computes a per-cell TARGET domain then emits a transition only when target differs from current AND the hysteresis test for the direction passes; one step per classify).
- [ ] **Step 4: run** — `cargo test -p pulsar_scenedb --features gpu --lib gpu::grid` PASS; C0 guard green.
- [ ] **Step 5: Commit** — `feat(scenedb): StreamingGrid — domains, hysteresis, cross-fade, budget (M2b-b T3, Test 11)`

---

### Task 4: Residency — eviction, tail-scrubbed promotion, region recycling (D2-tail gate)

**Files:**
- Modify: `src/gpu/scene_store.rs`
- Test: append to `tests/gpu_store.rs`

**Interfaces:**
- Produces:
  - `SceneGpuStore::unregister_cell(&mut self, id: CellId, cell: &mut CellStorage, last_serial: u64)` — the §4.1 eviction: (1) commit every queued pending retire **CPU-side immediately** (`cell.commit_retire(pending)` — zero VRAM writes; see Global Constraints for why this refines §4.1); (2) `free_pinned(row_base/slot_base, last_serial)` on both class pools; (3) drop the `CellGpuState` (a `cells: Vec<Option<CellGpuState>>` slot becomes `None`; `CellId`s are NOT recycled — document). Panics (debug) if `id` already unregistered.
  - `retire_all` additionally drains both pool families: `pool.drain_completed(self.tracker.completed())` for every class (regions become reallocatable).
  - **Tail scrub in `register_cell`** (§11 carry-forward): after `rebuild_region(slot_base, gens)`, also write the tail `[gens.len()..slot_capacity)`: upload a zero-fill (`vec![0u32; slot_capacity - gens.len()]` via `write_buffer` at `(slot_base + gens.len()) * 4`) and zero the corresponding shadow entries — a recycled region must never expose the prior tenant's generations (fail-open). Use a persistent zero-buffer or chunked writes; document the cost (once per promotion, cold path).
  - `generation_write_count` unchanged (scrub writes bypass the counter — they're region-lifecycle, not per-slot; document on the counter).

- [ ] **Step 1: Failing tests** (the gates):

```rust
#[test]
fn eviction_returns_region_only_after_serial_completes() {
    let ctx = test_context();
    let mut store = SceneGpuStore::new(&ctx, SceneGpuConfig {
        classes: vec![RegionClassConfig { capacity: 64, max_resident_cells: 1 }],
        tombstone_headroom: 8, max_materials: 1, max_cells_metadata: 4,
    });
    let mut frames = FrameDriver::new();
    let mut cell_a = transform_cell(64);
    let id_a = store.register_cell(&cell_a, 0).unwrap();
    let serial = store.tracker().next_serial();
    store.unregister_cell(id_a, &mut cell_a, serial);
    // Region still pinned: a new registration must fail.
    let cell_b = transform_cell(64);
    assert!(store.register_cell(&cell_b, 0).is_err(), "region pinned until serial completes");
    // Complete the serial; the drain happens in retire (frame boundary):
    store.tracker().force_complete(serial);
    let sim = frames.begin();
    let b = sim.end().end().end();
    let (retired, _) = b.retire(&mut store, &mut []);
    let compacted = retired.compact(&mut store, &mut []);
    compacted.sync(&mut store, &mut []);
    assert!(store.register_cell(&cell_b, 0).is_ok(), "region recycled after drain");
}

#[test]
fn eviction_commits_pending_retires_cpu_side_with_zero_vram_writes() {
    let ctx = test_context();
    let mut store = SceneGpuStore::new(&ctx, scene_cfg());
    let mut frames = FrameDriver::new();
    let mut cell = transform_cell(64);
    let id = store.register_cell(&cell, 0).unwrap();
    let h = cell.alloc().unwrap();
    let sim = frames.begin();
    store.write_transform(id, &mut cell, h, &mat(1.0), &sim);
    let b = sim.end().end().end();
    { let mut slots = [CellSlot { id, cell: &mut cell }]; b.run(&mut store, &mut slots); }
    // Deferred-free h, then evict BEFORE its serial completes:
    let sim = frames.begin();
    let s = store.tracker().next_serial();
    store.free_deferred(id, &mut cell, h, s, &sim);
    drop(sim);
    let writes_before = store.generation_write_count();
    store.unregister_cell(id, &mut cell, s);
    assert_eq!(store.generation_write_count(), writes_before, "zero VRAM writes at eviction");
    // CPU-side: handle stale, slot recycled, row unpinned+compactable:
    assert_eq!(cell.row_of(h), None, "pending retire committed CPU-side");
    let h2 = cell.alloc().unwrap();
    assert_eq!(h2.index(), h.index(), "slot recycled");
}

#[test]
fn d2_tail_recycled_region_never_exposes_prior_generations() {
    let ctx = test_context();
    let mut store = SceneGpuStore::new(&ctx, SceneGpuConfig {
        classes: vec![RegionClassConfig { capacity: 64, max_resident_cells: 1 }],
        tombstone_headroom: 8, max_materials: 1, max_cells_metadata: 4,
    });
    let mut frames = FrameDriver::new();
    // Tenant A: churn 3 slots to gen 2 so the region holds non-zero gens.
    let mut cell_a = transform_cell(64);
    for _ in 0..3 { let h = cell_a.alloc().unwrap(); cell_a.free(h); }
    for _ in 0..3 { cell_a.alloc().unwrap(); } // slots 0..3 at gen 2
    let id_a = store.register_cell(&cell_a, 0).unwrap();
    let serial = store.tracker().next_serial();
    store.unregister_cell(id_a, &mut cell_a, serial);
    store.tracker().force_complete(serial);
    { let sim = frames.begin(); let b = sim.end().end().end();
      let (r, _) = b.retire(&mut store, &mut []); r.compact(&mut store, &mut []).sync(&mut store, &mut []); }
    // Tenant B: ONE slot only — the region tail must not show A's gen-2 values.
    let mut cell_b = transform_cell(64);
    cell_b.alloc().unwrap();
    let _id_b = store.register_cell(&cell_b, 0).unwrap();
    let gens = as_u32s(&readback(&ctx, store.generation_buffer(), (72 * 4) as u64));
    assert_eq!(gens[0], 1, "B's slot 0");
    assert!(gens[1..72].iter().all(|&g| g == 0),
        "tail scrubbed — no prior-tenant generations survive (found {:?})",
        gens[1..72].iter().enumerate().filter(|(_, &g)| g != 0).take(4).collect::<Vec<_>>());
}
```

(Adjust `free_deferred`/`write_transform` calls to the witness-bearing signatures as they exist post-α — check `tests/gpu_store.rs` for the exact current form and match it.)

- [ ] **Step 2: verify FAIL** (`unregister_cell` unresolved). **Step 3: Implement** per Interfaces. `cells: Vec<CellGpuState>` becomes `Vec<Option<CellGpuState>>` — update every `self.cells[id.0 as usize]` accessor to `.as_ref().expect("cell unregistered")` (grep them all; the phase stage methods too). **Step 4: run** — gpu_store suite (22) + core suites PASS. **Step 5: Commit** — `feat(scenedb): residency — serial-pinned eviction + recycled-region tail scrub (M2b-b T4, closes §11 carry-forwards)`

---

### Task 5: Grid↔store transition executor + per-cell metadata writer

**Files:**
- Modify: `src/gpu/grid.rs` (executor + metadata packing), `src/gpu/phase.rs` (nothing structural — the executor takes `&RetiredPhase`)
- Test: append to `tests/gpu_store.rs`

**Interfaces:**
- Produces:
  - `pub fn execute_transitions(grid: &mut StreamingGrid, store: &mut SceneGpuStore, cells: &mut HashMap<CellCoord, SpatialCell>, class_of: &dyn Fn(CellCoord) -> usize, eviction_serial: u64, _w: &RetiredPhase) -> TransitionStats` (free fn in `grid.rs`): drains `take_transitions()`; **Outer→Margin** = `store.register_cell(cell.storage(), class)` — on `Err(RegionError)` the transition is DECLINED (cell stays in its current domain, `stats.declined += 1`, telemetry via `tracing::warn!`; §8 graceful degradation, closes the exhaustion carry-forward); **Margin→Inner / Inner→Margin** = `commit_transition` only (domain flag); **Margin→Outer** = `store.unregister_cell(id, cell.storage_mut(), eviction_serial)`. Successful transitions are `commit_transition`ed; grid records the `CellId` for resident cells (`grid.set_gpu_id(coord, Some(id))` — add that internal state + `pub fn gpu_id(&self, coord) -> Option<CellId>`).
  - `pub struct TransitionStats { pub promoted: u32, pub demoted: u32, pub declined: u32 }`
  - `StreamingGrid::write_cell_metadata(&self, queue: &wgpu::Queue, buf: &wgpu::Buffer)` — packs `(f32 α, u32 domain)` per materialized cell at `dense_id * 8` (domain encoding: Inner=2, Margin=1, Outer=0; document as the M3 stipple-pass contract). Simple full rewrite of materialized entries per boundary (≤ `max_cells_metadata`; delta-tracking is a recorded future optimization, not built).

- [ ] **Step 1: Failing test:**

```rust
#[test]
fn transitions_execute_at_boundary_and_metadata_mirrors_state() {
    use pulsar_scenedb::gpu::{execute_transitions, CellCoord, Domain, GridConfig, StreamingBudget, StreamingGrid};
    use std::collections::HashMap;
    let ctx = test_context();
    let mut store = SceneGpuStore::new(&ctx, scene_cfg());
    let mut frames = FrameDriver::new();
    let mut grid = StreamingGrid::new(
        GridConfig { cell_width: 100.0, margin_radius: 150.0, pad_fraction: 0.10, hysteresis: 20.0 },
        StreamingBudget { vram_hlod_budget: u64::MAX, vram_geometry_budget: u64::MAX,
            max_materialized_cells: 16, proxy_mesh_bytes: 1, mean_cell_geometry_bytes: 1 },
        &[RegionClassConfig { capacity: 64, max_resident_cells: 4 }],
    ).unwrap();
    let c0 = CellCoord { x: 0, z: 0 };
    grid.materialize(c0);
    let mut cells = HashMap::new();
    cells.insert(c0, pulsar_scenedb::SpatialCell::with_transform(64).unwrap());
    // Observer inside cell 0 → Outer→Margin queued:
    grid.classify(&[pulsar_scenedb::Aabb { min: [40.0, -1.0, -1.0], max: [60.0, 1.0, 1.0] }]);
    let sim = frames.begin();
    let b = sim.end().end().end();
    let (retired, _) = b.retire(&mut store, &mut []);
    let serial = store.tracker().next_serial();
    let stats = execute_transitions(&mut grid, &mut store, &mut cells, &|_| 0, serial, &retired);
    assert_eq!(stats.promoted, 1);
    assert_eq!(grid.domain(c0), Some(Domain::Margin));
    assert!(grid.gpu_id(c0).is_some(), "resident cell has a region");
    retired.compact(&mut store, &mut []).sync(&mut store, &mut []);
    grid.advance_crossfade(50.0, 100.0);
    grid.write_cell_metadata(&ctx.queue(), store.cell_metadata_buffer());
    let meta = readback(&ctx, store.cell_metadata_buffer(), 8);
    let alpha = f32::from_le_bytes(meta[0..4].try_into().unwrap());
    let domain = u32::from_le_bytes(meta[4..8].try_into().unwrap());
    assert!((alpha - 0.5).abs() < 1e-6);
    assert_eq!(domain, 1, "Margin encodes as 1");
}
```

- [ ] **Step 2: FAIL → Step 3: Implement → Step 4: run (gpu_store 23; core green; C0 green) → Step 5: Commit** — `feat(scenedb): boundary transition executor + per-cell metadata writer (M2b-b T5)`

---

## Wave 2 — Harvest

### Task 6: `HarvestPipeline` — single-scan partition emitting global-row tokens

**Files:**
- Create: `src/gpu/harvest.rs`; Modify: `src/gpu/mod.rs` (`mod harvest;` + `pub use harvest::{HarvestStaging, HarvestPipeline, MeshClass, View};`)
- Modify: `Cargo.toml` (`[[test]] name = "gpu_harvest" required-features = ["gpu"]`)
- Create: `tests/gpu_harvest.rs` (copy `test_context`/`readback` helpers verbatim from `tests/gpu_store.rs`, plus local `transform-spatial` cell builders)

**Interfaces:**

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum MeshClass { Traditional, VirtualGeometry, HlodProxy }
pub enum View { Aabb(crate::spatial::Aabb), Frustum(crate::spatial::Frustum) }
/// Per-view staging arrays (§5.2). Persistent — cleared, not reallocated,
/// each frame (capacity survives; §8.1 after warm-up).
pub struct HarvestStaging {
    pub traditional: Vec<u32>, pub vg: Vec<u32>, pub hlod: Vec<u32>,
    pub remap: Vec<u32>,           // M3-frozen: remap[dense_i] = original_run_index
    pub stats: HarvestStats,
}
#[derive(Default, Clone, Copy, Debug)] pub struct HarvestStats {
    pub cells: u32, pub tokens_valid: u32, pub tokens_total: u32, pub dei_compacted_runs: u32,
}
pub struct HarvestPipeline(());   // stateless in β single-thread form; methods take scratch
impl HarvestPipeline {
    pub fn new() -> Self;
    /// Query one resident inner cell for one view and route its run into the
    /// staging arrays, adding `region_base` to every valid token (§2 — the
    /// sentinel is never offset, it is DROPPED here). DEI (§8.5): if
    /// valid/total < 0.25 the run is dense-compacted via the compress kernel
    /// with a remap-table segment appended to `staging.remap`.
    pub fn harvest_cell(
        &self, cell: &SpatialCell, region_base: u32, class: MeshClass, view: &View,
        pad: &mut Scratchpad, staging: &mut HarvestStaging, _h: &HarvestPhase,
    ) -> u32 /* valid tokens routed */;
}
```

Body shape (write exactly): `let len = cell.rows_in_use() as usize;` → `let (tokens, words) = pad.get_u32_u64(len, len.div_ceil(64));` → `let nw = LivenessSnapshot::capture_words(cell.storage().liveness(), len as u32, words);` → `let n = match view { View::Aabb(q) => cell.query_aabb_in(q, &words[..nw], tokens), View::Frustum(f) => cell.query_frustum_in(f, &words[..nw], tokens) };` → target array by `class` → if `len > 0 && (n as f32 / len as f32) < 0.25` { dense-compact via `crate::simd::compress_tokens(&tokens[..len], region_base, dest, &mut staging.remap)`; `staging.stats.dei_compacted_runs += 1` } else { plain scan: `for t in &tokens[..len] { if *t != NULL_ROW { dest.push(region_base + *t); } }` } → update stats → return n. (T6 ships with `compress_tokens` scalar from the same commit — see below; AVX2 is T7.)

`compress_tokens` (in `src/simd.rs`, `pub(crate)`, scalar reference):

```rust
/// §8.5 dense compaction (scalar reference): strip NULL_ROW sentinels from a
/// positional token run, pushing `base + token` into `dense` and the ORIGINAL
/// run index into `remap` (C4 M3-frozen layout: remap[dense_i] = run index).
/// Returns the dense count. AVX2 arm (M2b-b T7) must match bit-for-bit.
pub(crate) fn compress_tokens(run: &[u32], base: u32, dense: &mut Vec<u32>, remap: &mut Vec<u32>) -> u32 {
    let mut count = 0;
    for (i, &t) in run.iter().enumerate() {
        if t != crate::registry::NULL_ROW {
            dense.push(base + t);
            remap.push(i as u32);
            count += 1;
        }
    }
    count
}
```

- [ ] **Step 1: Failing tests** (`tests/gpu_harvest.rs`):

```rust
#[test]
fn harvest_routes_global_tokens_by_class_and_never_offsets_sentinels() {
    // Two cells in disjoint regions, one Traditional one VG; a query that hits
    // a strict subset; assert every routed token == region_base + local row,
    // classes routed to their arrays, and no 0xFFFF_FFFF-derived value appears.
}
#[test]
fn dei_below_quarter_compacts_with_roundtrip_remap() {
    // 64-row cell, 8 hits (12.5% < 25%): dense array has 8 global tokens,
    // remap has 8 original run indices; for each i: dense[i] == base + run[remap[i]].
    // Also: a 50%-hit cell takes the plain path (dei_compacted_runs unchanged).
}
#[test]
fn harvest_makes_zero_new_allocations_after_warmup() {
    // Run harvest twice with the same Scratchpad + HarvestStaging; assert
    // scratch/staging capacities unchanged between run 1 and run 2
    // (Vec::capacity comparison — the observable §8.1 proxy).
}
```

(Write these three tests in full in the implementation — construct cells with `SpatialCell::with_transform`, register with the store for real region bases, use the `HarvestPhase` witness from `FrameDriver` — `frames.begin().end().end()` yields it.)

- [ ] **Step 2: FAIL → Step 3: Implement → Step 4: run** (`cargo test -p pulsar_scenedb --features gpu --test gpu_harvest -- --test-threads=1` 3/3; gpu_store no regression; core + C0 green). **Step 5: Commit** — `feat(scenedb): HarvestPipeline — single-scan partition, global tokens, scalar DEI (M2b-b T6, Test 12 scalar)`

---

### Task 7: DEI compress AVX2 arm (bit-for-bit)

**Files:** `src/simd.rs` (+ dispatch), tests inline (property tests, M1b style).

Runtime-dispatched `compress_tokens` (scalar arm = T6's; AVX2 arm using the same dispatch pattern as `aabb_scan` — check `simd.rs`'s existing `is_x86_feature_detected!` idiom and mirror it). Property test: ≥200 randomized runs (varying len 0..=1024, hit densities 0–100%, random base) — AVX2 output `(dense, remap)` byte-identical to scalar. If AVX2 offers no clean compress-store primitive pre-AVX-512, a lookup-table/`_mm256_permutevar8x32_epi32` approach is standard; if after investigation the arm can't beat scalar cleanly, document and keep dispatch scalar-only with a recorded decision (the design's "AVX2 after" is a target, not a suicide pact — but the bit-identity harness must exist either way).

- [ ] Steps: property test first (against a second, naive reference) → implement arm + dispatch → 200-case bit-identity green → commit `feat(scenedb): AVX2 compress-store DEI arm, bit-identical (M2b-b T7)`.

---

### Task 8: Lease timeout, revocation, stale-lane revalidation (Test 10)

**Files:** `src/gpu/harvest.rs` (+ re-exports), tests in `tests/gpu_harvest.rs`.

**Interfaces:**

```rust
/// A held harvest lease: cell lease slot + revocation flag + capture-time
/// snapshot words (spec §9.2.1 — a revoked holder keeps reading its pinned
/// snapshot; results are revalidated against live state on use).
pub struct HarvestLease<'a> {
    lease: crate::lease::Lease<'a>,
    pub revocation: std::sync::Arc<crate::snapshot::RevocationFlag>,
    pub held_since_ms: f64,
}
impl HarvestPipeline {
    pub fn acquire_lease<'a>(&self, mask: &'a LeaseMask, now_ms: f64) -> Option<HarvestLease<'a>>;
    /// §9.2.1 isolation check (2.0 ms budget, injectable clock): revoke every
    /// lease held past the deadline. Returns revoked count (logged).
    pub fn revoke_overdue(&self, leases: &[&HarvestLease<'_>], now_ms: f64, budget_ms: f64) -> u32;
}
/// Stale-validation lane: re-validate a token run against LIVE liveness —
/// rows that died since the snapshot become NULL_ROW in place. Returns the
/// surviving count.
pub fn revalidate_run(cell: &SpatialCell, run: &mut [u32]) -> u32;
```

Test 10 gate (write in full): acquire a lease at t=0 against a snapshot; mutate the cell (free a row) after capture; at t=2.5 ms `revoke_overdue(..., 2.0)` returns 1 and the flag reads revoked; the holder's snapshot STILL shows the old liveness (pinned — `LivenessSnapshot` semantics); `revalidate_run` then strips the dead row (NULL_ROW) and returns n−1; `LeaseMask::any_held` goes false after the guard drops so compaction may proceed; a second revocation sweep with no overdue leases returns 0. Plus: pool-exhaustion behavior (65th acquire returns None — spec §9.2 blocking is the World driver's loop, document).

- [ ] Steps: failing gate → implement → green (gpu_harvest 5+) → commit `feat(scenedb): lease timeout + revocation + stale-lane revalidation (M2b-b T8, Test 10)`.

---

### Task 9: Multi-view harvest + concurrency smoke

**Files:** `src/gpu/harvest.rs`, tests in `tests/gpu_harvest.rs`.

`pub fn harvest_views(&self, cells: &[(&SpatialCell, u32 /*region_base*/, MeshClass)], views: &[View], pads: &mut [Scratchpad], stagings: &mut [HarvestStaging], _h: &HarvestPhase)` — one scratch + staging per view (spec §8.4); iterate views × cells calling `harvest_cell`. Concurrency smoke test: two views harvested from two `std::thread::scope` threads over the SAME cells (read-only queries + per-thread scratch — spec §8.4 safety claim), asserting identical results to the sequential run. (`harvest_cell` takes `&self` and only `&SpatialCell` — compile-time proof of no shared mutation; the test is a behavioral double-check.)

- [ ] Steps: failing tests → implement → green → commit `feat(scenedb): multi-view harvest + §8.4 concurrency smoke (M2b-b T9)`.

---

### Task 10: Benches + docs wrap + full matrix

**Files:** `benches/scenedb_bench.rs` (extend), `src/lib.rs` + `README.md` (M2b-β sections), design doc status line → "M2b implemented (α+β)".

Benches (criterion, no gates — numbers land in the report): `region_sync_1024_dirty_rows` (SceneBuffer::sync_region over a full region), `harvest_partition_1024` (harvest_cell plain path), `dei_compact_1024_sparse` (12.5% density), `promotion_demotion_cycle` (register→evict→drain→register). Use the existing `scenedb_bench.rs` harness conventions (`std::hint::black_box`).

Full acceptance matrix (all green, GPU suites sequential):
```
cargo check -p pulsar_scenedb --no-default-features
cargo test  -p pulsar_scenedb --lib --tests
cargo test  -p pulsar_scenedb --features gpu --test gpu_store   -- --test-threads=1
cargo test  -p pulsar_scenedb --features gpu --test gpu_harvest -- --test-threads=1
cargo test  -p pulsar_scenedb --features gpu --test gpu_assets  -- --test-threads=1
cargo test  -p pulsar_scenedb --features gpu --test gpu_layout
cargo test  -p pulsar_scenedb --features gpu --doc
cargo bench -p pulsar_scenedb --bench scenedb_bench --features gpu -- --test   # benches compile+smoke
```

- [ ] Commit — `docs(scenedb): M2b-beta docs — streaming grid, residency, harvest complete`

---

## Deferred (β does NOT build)

Threaded write-window + phase-machine lifetime-witness hardening (M4 §11); disk/compressed outer frames (M4); HLOD proxy authoring/stipple shader + remap consumer + cull passes (M3); World-level driver integration incl. lease-exhaustion blocking loop and `alloc` gating (M4); metadata delta-tracking (recorded optimization).

## Verification (end-to-end)

Task 10's matrix. Named gates: **Test 10** (T8), **Test 11** (T3), **Test 12** (T6 scalar + T7 bit-identity), **D2-tail** (T4), eviction serial-pinning (T4), §8.1 no-alloc warm-up proxy (T6). Every β §11 carry-forward has a closing task: tail scrub (T4), exhaustion degradation (T5), get_u64/harvest wiring (T2/T6).
