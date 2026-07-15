# SceneDB 2.0 — M2b-α Implementation Plan (Asset Store, Region Reshape, Phase Machine)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reshape the M2a single-cell `GpuStore` into the multi-cell, region-partitioned `SceneGpuStore`, add the load-time asset store (geometry/mesh-metadata/cluster), the global-slot mirror, and the compile-time phase machine — per design Rev 2 §1.1 (M2b-α scope).

**Architecture:** Global SSBOs partitioned into per-cell regions from size-class pools (`global_row = region_base + local_row`); per-cell GPU state (dirty masks, pending retires, gen-shadow slice) lives in `CellGpuState` inside the scene-wide `SceneGpuStore`. Assets are write-once-at-load stores beside it. Phase order becomes zero-size witness types; the runtime `Phase` enum stays as a debug backstop.

**Tech Stack:** Rust 2021, wgpu 28 fork (workspace dep, rev `fce5b80…`), pollster/naga dev-deps (already present), trybuild NOT used — `compile_fail` doc-tests instead.

**Design of record:** `docs/superpowers/specs/2026-07-14-scenedb20-m2b-streaming-orchestration-design.md` (Rev 2). Contracts: C0–C6. Predecessor code: `src/gpu/{store,buffer,generation,tracker,context}.rs` as of commit `4bce0cb8` (+NEON rebase).

## Global Constraints

- **C0:** `cargo check -p pulsar_scenedb --no-default-features` stays green; all GPU code under `#[cfg(feature = "gpu")]`; no `pulsar_scenedb` → Helio edge.
- **Regions:** SSBOs allocated once from config; region-bounds assert on every generation and slot-mirror write (a write must never land in a neighbor's region); slot regions sized `capacity + tombstone_headroom` (default 64); row/slot-region exhaustion = hard error, never realloc.
- **C5 layouts:** MeshMetadata = 72 B exactly (field table in spec §6.1); ClusterNode = 48 B (C5); `#[repr(C)]`, scalar fields only; XOR rule (`lod_count` vs `cluster_table_offset` — exactly one non-zero) and `self_error < parent_error` are hard registration errors; Test 3 rows assert **storage** address-space layout.
- **C6:** generation reaches VRAM before slot re-pools; shadow-gated writes; retire drain assumes nondecreasing serials per queue (debug-asserted from this milestone).
- **§6.1 (design):** NO row-granularity harvest pins; the only serial pinning is region-granularity (pool free path).
- **Frame boundary order:** retire → (transitions: β no-op) → compact → sync; phase witnesses in Task 11.
- **Windows/encoding:** author `.rs` files ONLY via Write/Edit tools (PowerShell redirection adds a BOM that breaks rustc).
- **Fork API forms:** `wgpu::Instance::new(owned InstanceDescriptor)`; `device.poll(wgpu::PollType::wait_indefinitely())`.
- **Test commands:** core `cargo test -p pulsar_scenedb --lib --tests`; GPU `cargo test -p pulsar_scenedb --features gpu --test gpu_store --test gpu_assets --test gpu_layout`; guard `cargo check -p pulsar_scenedb --no-default-features`; doc-gates `cargo test -p pulsar_scenedb --features gpu --doc`.
- **Commit style:** `type(scenedb): summary` + trailer `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.

## File Structure

```
crates/core/pulsar_scenedb/
  src/gpu/mod.rs          # + mod region/dirty/scene_store/assets/phase; re-exports; store.rs REMOVED (Task 5)
  src/gpu/region.rs       # RegionPool (size-class free list, serial-pinned free)     [Task 1]
  src/gpu/dirty.rs        # DirtyMask (extracted per-cell dirty words)                [Task 2]
  src/gpu/buffer.rs       # SceneBuffer loses its internal mask; gains sync_region    [Task 2]
  src/gpu/generation.rs   # + rebuild_region                                          [Task 3]
  src/gpu/scene_store.rs  # SceneGpuStore, CellGpuState, CellId, configs              [Tasks 3-5]
  src/gpu/assets.rs       # RangeList, GeometryArena, MeshMetadata, MeshRegistry,
                          # ClusterNode, ClusterBuffer                                [Tasks 6-8]
  src/gpu/phase.rs        # witness types + FrameDriver + compile_fail doc-tests      [Task 11]
  src/cell.rs             # + pub(crate) slot_column()                                [Task 4]
  tests/gpu_store.rs      # migrated to SceneGpuStore                                 [Task 5]
  tests/gpu_assets.rs     # NEW [[test]] target, required-features gpu               [Task 6]
  tests/gpu_layout.rs     # + MeshMetadata/ClusterNode/slot-mirror rows              [Task 9]
  Cargo.toml              # + [[test]] gpu_assets                                     [Task 6]
```

**Reconciliation note (design §7):** the design lists mesh-configurator/cluster buffers both under `SceneGpuStore` and as standalone components. Resolution here: **assets own their SSBOs** (constructed from `&EngineGpuContext`); `SceneGpuStore` owns instance, slot-mirror, generation, material-placeholder, and per-cell-metadata buffers. One store per concern; Helio binds both in M3.

---

### Task 1: `RegionPool` — size-class free list with serial-pinned free

**Files:**
- Create: `crates/core/pulsar_scenedb/src/gpu/region.rs`
- Modify: `crates/core/pulsar_scenedb/src/gpu/mod.rs` (add `mod region;` + `pub use region::{RegionPool, RegionError};`)
- Test: inline `#[cfg(test)]` in `region.rs` (pure logic, no device)

**Interfaces:**
- Produces:
  - `RegionPool::new(base_offset: u32, region_size: u32, count: u32) -> Self` — regions at `base_offset + i*region_size` for `i in 0..count`, all initially free.
  - `alloc(&mut self) -> Option<u32>` — pops a free region base; `None` = exhausted (caller turns this into the §8 hard error).
  - `free_pinned(&mut self, base: u32, serial: u64)` — queues the region for reuse once `serial` completes (the §4.1 eviction path; unused until β but the pool is the α deliverable). Debug-asserts `base` belongs to this pool and is not already free/pinned.
  - `drain_completed(&mut self, completed: u64) -> u32` — moves every pinned region with `serial <= completed` to the free list; returns count.
  - `region_size(&self) -> u32`, `free_count(&self) -> u32`.
  - `enum RegionError { RowsExhausted, SlotsExhausted }` (used by Task 3).

- [ ] **Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_exhausts_then_none() {
        let mut p = RegionPool::new(1000, 256, 2);
        let a = p.alloc().unwrap();
        let b = p.alloc().unwrap();
        assert_ne!(a, b);
        for base in [a, b] {
            assert!(base == 1000 || base == 1256, "bases offset by region_size from base_offset");
        }
        assert_eq!(p.alloc(), None, "exhausted pool");
    }

    #[test]
    fn pinned_free_returns_only_after_serial_completes() {
        let mut p = RegionPool::new(0, 256, 1);
        let a = p.alloc().unwrap();
        p.free_pinned(a, 5);
        assert_eq!(p.alloc(), None, "pinned region not reusable");
        assert_eq!(p.drain_completed(4), 0, "serial incomplete");
        assert_eq!(p.drain_completed(5), 1);
        assert_eq!(p.alloc(), Some(a), "region recycled after completion");
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p pulsar_scenedb --features gpu --lib gpu::region 2>&1 | tail -4`
Expected: compile FAIL — `RegionPool` not found

- [ ] **Step 3: Implement**

```rust
//! Size-class region pools (design Rev 2 §2/§7): fixed-size regions of the
//! global row/slot spaces, O(1) alloc/free, with serial-pinned free — the
//! ONLY serial pinning in M2b (§6.1; row-granularity harvest pins are
//! forbidden by design).

use std::collections::VecDeque;

/// Hard region-exhaustion errors (§8): surfaced to the caller, never a realloc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionError {
    RowsExhausted,
    SlotsExhausted,
}

pub struct RegionPool {
    base_offset: u32,
    region_size: u32,
    count: u32,
    free: Vec<u32>,
    /// (region_base, submission serial) — reusable once the serial completes.
    pinned: VecDeque<(u32, u64)>,
}

impl RegionPool {
    pub fn new(base_offset: u32, region_size: u32, count: u32) -> Self {
        // LIFO free list; reverse so the first alloc returns the lowest base
        // (deterministic tests, better locality).
        let free = (0..count).rev().map(|i| base_offset + i * region_size).collect();
        Self { base_offset, region_size, count, free, pinned: VecDeque::new() }
    }

    pub fn alloc(&mut self) -> Option<u32> {
        self.free.pop()
    }

    /// Queue a region for reuse once `serial` completes (§4.1 eviction).
    pub fn free_pinned(&mut self, base: u32, serial: u64) {
        debug_assert!(
            base >= self.base_offset
                && (base - self.base_offset) % self.region_size == 0
                && (base - self.base_offset) / self.region_size < self.count,
            "region base {base} does not belong to this pool"
        );
        debug_assert!(
            !self.free.contains(&base) && !self.pinned.iter().any(|&(b, _)| b == base),
            "double free of region {base}"
        );
        self.pinned.push_back((base, serial));
    }

    /// Recycle every pinned region whose serial is complete. Returns count.
    pub fn drain_completed(&mut self, completed: u64) -> u32 {
        let mut drained = 0;
        // Serials are not guaranteed monotone across cells; scan the whole queue.
        let mut i = 0;
        while i < self.pinned.len() {
            if self.pinned[i].1 <= completed {
                let (base, _) = self.pinned.remove(i).unwrap();
                self.free.push(base);
                drained += 1;
            } else {
                i += 1;
            }
        }
        drained
    }

    pub fn region_size(&self) -> u32 {
        self.region_size
    }

    pub fn free_count(&self) -> u32 {
        self.free.len() as u32
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p pulsar_scenedb --features gpu --lib gpu::region 2>&1 | tail -3`
Expected: PASS (2 tests). Also `cargo check -p pulsar_scenedb --no-default-features` stays green.

- [ ] **Step 5: Commit**

```bash
git add crates/core/pulsar_scenedb/src/gpu/region.rs crates/core/pulsar_scenedb/src/gpu/mod.rs
git commit -m "feat(scenedb): RegionPool — size-class regions with serial-pinned free (M2b-a)"
```

---

### Task 2: Extract `DirtyMask`; `SceneBuffer::sync_region`

**Files:**
- Create: `crates/core/pulsar_scenedb/src/gpu/dirty.rs`
- Modify: `crates/core/pulsar_scenedb/src/gpu/buffer.rs`
- Modify: `crates/core/pulsar_scenedb/src/gpu/store.rs` (mechanical: `GpuStore` gains a `dirty_transforms: DirtyMask` field and passes it through — keeps the M2a suite green until Task 5 deletes it)
- Modify: `crates/core/pulsar_scenedb/src/gpu/mod.rs` (add `mod dirty;` + `pub use dirty::DirtyMask;`)
- Test: inline in `dirty.rs`; existing `tests/gpu_store.rs` is the regression oracle

**Interfaces:**
- Produces:
  - `DirtyMask::new(capacity: u32) -> Self` — `Vec<AtomicU64>` words, `capacity.div_ceil(64)`.
  - `mark(&self, row: u32)` (relaxed `fetch_or`, debug bound check), `is_marked(&self, row: u32) -> bool`, `clear_all(&self)`, `mark_range(&self, rows: u32)` (mark `0..rows` — promotion warm-up), `capacity(&self) -> u32`.
  - `SceneBuffer<T>` LOSES its internal dirty words, `mark_row_dirty`, and the old `sync`; GAINS
    `sync_region(&self, queue: &wgpu::Queue, cpu: &[T], region_base: u32, dirty: &DirtyMask) -> SyncStats` — the identical streaming coalescer, byte offset `(region_base + row) * stride`, clears the mask at the end. Panics if `region_base + cpu.len() as u32 > self.capacity`.
  - `SceneBuffer::write_rows(&self, queue, cpu: &[T], region_base: u32)` — unconditional bulk region write (registration warm-up / Test 14 rebuild).

- [ ] **Step 1: Write failing test** (in `dirty.rs`)

```rust
#[test]
fn mask_marks_and_clears() {
    let m = DirtyMask::new(130);
    m.mark(0);
    m.mark(129);
    assert!(m.is_marked(0) && m.is_marked(129) && !m.is_marked(64));
    m.clear_all();
    assert!(!m.is_marked(0) && !m.is_marked(129));
    m.mark_range(65);
    assert!(m.is_marked(64) && !m.is_marked(65));
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p pulsar_scenedb --features gpu --lib gpu::dirty` → compile FAIL.

- [ ] **Step 3: Implement**

`dirty.rs` — move the word-mask logic out of `SceneBuffer` verbatim (same orderings):

```rust
//! Per-cell row dirty bitmask (design Rev 2 §2): dirty state lives beside the
//! CELL, not inside the global buffer — the same atomic-word shape as M1's
//! LivenessMask. Relaxed ordering per the M2a contract: the frame-boundary
//! join provides the happens-before edge between column writes and sync.

use std::sync::atomic::{AtomicU64, Ordering};

pub struct DirtyMask {
    words: Vec<AtomicU64>,
    capacity: u32,
}

impl DirtyMask {
    pub fn new(capacity: u32) -> Self {
        let words = (0..capacity.div_ceil(64)).map(|_| AtomicU64::new(0)).collect();
        Self { words, capacity }
    }

    #[inline]
    pub fn mark(&self, row: u32) {
        debug_assert!(row < self.capacity, "row {row} beyond mask capacity {}", self.capacity);
        self.words[(row / 64) as usize].fetch_or(1u64 << (row % 64), Ordering::Relaxed);
    }

    #[inline]
    pub fn is_marked(&self, row: u32) -> bool {
        self.words[(row / 64) as usize].load(Ordering::Relaxed) & (1u64 << (row % 64)) != 0
    }

    /// Mark rows `0..rows` (promotion warm-up: full-region resync, §4.1).
    pub fn mark_range(&self, rows: u32) {
        for row in 0..rows {
            self.mark(row);
        }
    }

    pub fn clear_all(&self) {
        for w in &self.words {
            w.store(0, Ordering::Relaxed);
        }
    }

    pub fn capacity(&self) -> u32 {
        self.capacity
    }
}
```

`buffer.rs` — delete `dirty`, `mark_row_dirty`, `is_dirty`, `sync`; replace with:

```rust
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
        let stride = std::mem::size_of::<T>() as u64;
        let n = cpu.len() as u32;
        let mut stats = SyncStats { ranges: 0, bytes: 0 };
        let mut run_start: Option<u32> = None;
        for row in 0..n {
            match (dirty.is_marked(row), run_start) {
                (true, None) => run_start = Some(row),
                (false, Some(start)) => {
                    self.flush(queue, cpu, region_base, start, row, stride, &mut stats);
                    run_start = None;
                }
                _ => {}
            }
        }
        if let Some(start) = run_start {
            self.flush(queue, cpu, region_base, start, n, stride, &mut stats);
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
```

`store.rs` (mechanical bridge, deleted in Task 5): add `dirty_transforms: DirtyMask::new(cfg.max_rows)` field; replace every `self.transforms.mark_row_dirty(r)` with `self.dirty_transforms.mark(r)` and both `self.transforms.sync(&self.queue, cpu)` calls with `self.transforms.sync_region(&self.queue, cpu, 0, &self.dirty_transforms)`.

- [ ] **Step 4: Run the FULL matrix** — `cargo test -p pulsar_scenedb --features gpu --test gpu_store` (11/11 — the M2a gates are the regression oracle for the refactor), `--lib gpu::dirty` PASS, `--no-default-features` green.

- [ ] **Step 5: Commit**

```bash
git add crates/core/pulsar_scenedb/src/gpu/
git commit -m "refactor(scenedb): extract DirtyMask; SceneBuffer::sync_region for per-cell regions (M2b-a)"
```

---

### Task 3: `SceneGpuStore` core — configs, `register_cell`, write/retire/compact/sync

**Files:**
- Create: `crates/core/pulsar_scenedb/src/gpu/scene_store.rs`
- Modify: `crates/core/pulsar_scenedb/src/gpu/generation.rs` (add `rebuild_region`)
- Modify: `crates/core/pulsar_scenedb/src/gpu/mod.rs` (add `mod scene_store;` + `pub use scene_store::{CellId, RegionClassConfig, SceneGpuConfig, SceneGpuStore};`)
- Test: new tests appended to `tests/gpu_store.rs` (old `GpuStore` tests stay until Task 5)

**Interfaces:**
- Consumes: `RegionPool`/`RegionError` (T1), `DirtyMask`/`sync_region`/`write_rows` (T2), `GenerationBuffer`, `SubmissionTracker`, `EngineGpuContext`, and the core `pub(crate)` seams (`mark_pending_retire`, `commit_retire`, `compact_report`, `is_row_pinned`).
- Produces (exact signatures later tasks and β rely on):

```rust
pub struct RegionClassConfig { pub capacity: u32, pub max_resident_cells: u32 }
pub struct SceneGpuConfig {
    pub classes: Vec<RegionClassConfig>,
    pub tombstone_headroom: u32,   // default 64 via SceneGpuConfig::default_headroom()
    pub max_materials: u32,        // placeholder buffer, layout M3
    pub max_cells_metadata: u32,   // per-cell metadata SSBO entries (α: allocated, no writer)
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)] pub struct CellId(pub(crate) u32);

impl SceneGpuStore {
    pub fn new(ctx: &EngineGpuContext, cfg: SceneGpuConfig) -> Self;
    /// §4.1 promotion primitive (α: registration; β reuses it for promotion):
    /// allocates row+slot regions, bulk-rebuilds the generation region from
    /// the registry, seeds the gen-shadow, marks all occupied rows dirty
    /// (transforms + slot mirror).
    pub fn register_cell(&mut self, cell: &CellStorage, class: usize) -> Result<CellId, RegionError>;
    pub fn tracker(&self) -> &SubmissionTracker;
    #[doc(hidden)] pub fn generation_write_count(&self) -> u64;
    pub fn write_transform(&self, id: CellId, cell: &mut CellStorage, handle: Handle, m: &[f32; 16]) -> bool;
    pub fn free_deferred(&mut self, id: CellId, cell: &mut CellStorage, handle: Handle, serial: u64) -> bool;
    pub fn retire_all(&mut self, cells: &mut [CellSlot<'_>]) -> u32;   // stage 1
    pub fn compact_all(&mut self, cells: &mut [CellSlot<'_>]);         // stage 2 (β inserts transitions between 1 and 2)
    pub fn sync_all(&mut self, cells: &mut [CellSlot<'_>]) -> SyncStats; // stage 3 (summed)
    pub fn row_region_base(&self, id: CellId) -> u32;
    pub fn transform_buffer(&self) -> &wgpu::Buffer;
    pub fn slot_mirror_buffer(&self) -> &wgpu::Buffer;   // buffer exists from this task; maintenance in T4
    pub fn generation_buffer(&self) -> &wgpu::Buffer;
    pub fn material_buffer(&self) -> &wgpu::Buffer;
    pub fn cell_metadata_buffer(&self) -> &wgpu::Buffer;
}
pub struct CellSlot<'a> { pub id: CellId, pub cell: &'a mut CellStorage }
```

Key internals (write exactly):

```rust
struct CellGpuState {
    class: usize,
    row_base: u32,
    slot_base: u32,
    slot_capacity: u32, // class capacity + headroom; bounds every gen/slot write
    dirty_transforms: DirtyMask,
    dirty_slots: DirtyMask,
    slot_scratch: Vec<u32>,          // per-row global-slot staging (T4)
    pending: VecDeque<QueuedRetire>, // per-cell; nondecreasing serials (debug-asserted T11)
    gen_shadow: Vec<AtomicU32>,      // sized slot_capacity; seeded at register
}
```

- Global buffer capacities: rows = `Σ classes[i].capacity * classes[i].max_resident_cells` (transforms + slot mirror); generation slots = `Σ (capacity + headroom) * max_resident_cells`. Pools: per class one row pool and one slot pool, with running `base_offset`s laid end to end in class order.
- `write_generation(&self, state: &CellGpuState, local_slot: u32, generation: u32)` — `assert!(local_slot < state.slot_capacity, "slot {local_slot} beyond region capacity {} — write must never land in a neighbor's region", state.slot_capacity)`, shadow-gate against `state.gen_shadow[local_slot]`, write global slot `state.slot_base + local_slot`, bump the shared `gen_writes` counter.
- `register_cell`: pools alloc (map `None` → `RegionError::{Rows,Slots}Exhausted`); `let gens = cell.registry().generations()`; `assert!(gens.len() as u32 <= slot_capacity)`; `self.generations.rebuild_region(&self.queue, slot_base, gens)`; seed shadow from `gens`; `dirty_transforms.mark_range(cell.rows_in_use())`; `dirty_slots.mark_range(cell.rows_in_use())`; push state; return `CellId(index)`.
- `retire_all`: phase `Write→Retired`; for each slot, drain that cell's queue against `tracker.completed()` (FIFO early-break per queue), gen-write-then-`commit_retire` exactly as M2a.
- `compact_all`: phase `Retired→Compacted`; per cell `cell.compact_report(|_f, to| { state.dirty_transforms.mark(to); state.dirty_slots.mark(to); })`.
- `sync_all`: phase `Compacted→Write`; per cell: transforms `sync_region(queue, &col[..rows_in_use], row_base, &dirty_transforms)`; slot-mirror sync is T4 (this task leaves `dirty_slots` cleared without upload via `clear_all` + a `// T4` comment is FORBIDDEN — instead T3 simply does not touch `dirty_slots` in sync and T4 adds the upload; the T3 test only asserts transform behavior).
- `GenerationBuffer::rebuild_region(&self, queue, region_base: u32, generations: &[u32])` — `assert!(region_base as u64 + generations.len() as u64 <= self.max_slots as u64)`; `write_buffer` at `region_base * 4`.

- [ ] **Step 1: Write failing test** (append to `tests/gpu_store.rs`; helpers `test_context`, `mat`, `as_f32s`, `transform_cell` already exist)

```rust
use pulsar_scenedb::gpu::{CellSlot, RegionClassConfig, SceneGpuConfig, SceneGpuStore};

fn scene_cfg() -> SceneGpuConfig {
    SceneGpuConfig {
        classes: vec![RegionClassConfig { capacity: 64, max_resident_cells: 4 }],
        tombstone_headroom: 8,
        max_materials: 16,
        max_cells_metadata: 16,
    }
}

fn scene_boundary(store: &mut SceneGpuStore, slots: &mut [CellSlot<'_>]) -> pulsar_scenedb::gpu::SyncStats {
    store.retire_all(slots);
    store.compact_all(slots);
    store.sync_all(slots)
}

#[test]
fn two_cells_write_into_disjoint_regions() {
    let ctx = test_context();
    let mut store = SceneGpuStore::new(&ctx, scene_cfg());
    let mut cell_a = transform_cell(64);
    let mut cell_b = transform_cell(64);
    let ida = store.register_cell(&cell_a, 0).unwrap();
    let idb = store.register_cell(&cell_b, 0).unwrap();
    assert_ne!(store.row_region_base(ida), store.row_region_base(idb));
    let ha = cell_a.alloc().unwrap();
    let hb = cell_b.alloc().unwrap();
    assert!(store.write_transform(ida, &mut cell_a, ha, &mat(1.0)));
    assert!(store.write_transform(idb, &mut cell_b, hb, &mat(2.0)));
    {
        let mut slots = [CellSlot { id: ida, cell: &mut cell_a }, CellSlot { id: idb, cell: &mut cell_b }];
        scene_boundary(&mut store, &mut slots);
    }
    let gpu = as_f32s(&readback(&ctx, store.transform_buffer(), (64 * 4 * 64) as u64));
    let base_a = store.row_region_base(ida) as usize;
    let base_b = store.row_region_base(idb) as usize;
    assert_eq!(&gpu[base_a * 16..base_a * 16 + 16], &mat(1.0), "cell A row 0 in region A");
    assert_eq!(&gpu[base_b * 16..base_b * 16 + 16], &mat(2.0), "cell B row 0 in region B");
}

#[test]
fn region_exhaustion_is_a_hard_error() {
    let ctx = test_context();
    let mut store = SceneGpuStore::new(
        &ctx,
        SceneGpuConfig {
            classes: vec![RegionClassConfig { capacity: 64, max_resident_cells: 1 }],
            tombstone_headroom: 8,
            max_materials: 1,
            max_cells_metadata: 1,
        },
    );
    let c1 = transform_cell(64);
    let c2 = transform_cell(64);
    assert!(store.register_cell(&c1, 0).is_ok());
    assert!(store.register_cell(&c2, 0).is_err(), "second cell exceeds max_resident_cells");
}

#[test]
fn registration_rebuilds_generation_region_and_shadow() {
    // The D2 regression shape (single-region form; recycled-region form is β):
    // a cell with churned generations registers; its region must mirror the
    // registry immediately, with zero per-write stamps needed afterwards.
    let ctx = test_context();
    let mut store = SceneGpuStore::new(&ctx, scene_cfg());
    let mut cell = transform_cell(64);
    let h1 = cell.alloc().unwrap();
    cell.free(h1); // immediate-free churn BEFORE registration: gen bumped to 2 in registry
    let h2 = cell.alloc().unwrap(); // recycles slot 0 at gen 2
    let id = store.register_cell(&cell, 0).unwrap();
    let gens = as_u32s(&readback(&ctx, store.generation_buffer(), 8));
    let sb = 0usize; // first slot region starts at 0
    assert_eq!(gens[sb], 2, "registration uploaded the churned generation");
    // Shadow seeded: writing the transform must NOT re-stamp the generation.
    let before = store.generation_write_count();
    assert!(store.write_transform(id, &mut cell, h2, &mat(3.0)));
    assert_eq!(store.generation_write_count(), before, "shadow already knows gen 2");
}
```

- [ ] **Step 2: Run to verify failure** — compile FAIL (`SceneGpuStore` unresolved).

- [ ] **Step 3: Implement** `scene_store.rs` per the Interfaces block above, porting `write_transform`/`free_deferred`/retire/compact logic from `store.rs` with these mechanical substitutions: `self.transforms.mark_row_dirty(r)` → `state.dirty_transforms.mark(r)`; `self.generations.write(...)` → `self.write_generation(state, slot, gen)` (global = `state.slot_base + slot`); phase enum unchanged (`Write/Retired/Compacted`, transitions once per `*_all` stage). Doc comments carry over, updated for regions (see the M2a `store.rs` for the exact contract language — keep C6/§4/§6.1 references).

- [ ] **Step 4: Run** — new tests PASS; old GpuStore suite still 11/11; `--no-default-features` green.

- [ ] **Step 5: Commit**

```bash
git add crates/core/pulsar_scenedb/src/gpu/ crates/core/pulsar_scenedb/tests/gpu_store.rs
git commit -m "feat(scenedb): SceneGpuStore — multi-cell region-partitioned store (M2b-a §2)"
```

---

### Task 4: Global-slot mirror maintenance

**Files:**
- Modify: `crates/core/pulsar_scenedb/src/cell.rs` (add accessor)
- Modify: `crates/core/pulsar_scenedb/src/gpu/scene_store.rs`
- Test: append to `tests/gpu_store.rs`

**Interfaces:**
- Consumes: T3's `CellGpuState.{dirty_slots, slot_scratch}`, `SceneBuffer<u32>` slot mirror.
- Produces:
  - `CellStorage::slot_column(&self) -> &[u32]` (`pub(crate)`) — physical column 0 (the slot-ID column), full capacity slice.
  - Slot-mirror upkeep inside `SceneGpuStore`: `write_transform` marks `dirty_slots` **when the gen-shadow gate fires** (first write after alloc — the row's slot is new to the GPU then); `compact_all` marks moved destinations in `dirty_slots` (T3 already does); `sync_all` refreshes `slot_scratch[row] = state.slot_base + cell.slot_column()[row]` for every marked row, then `self.slot_mirror.sync_region(&self.queue, &state.slot_scratch[..rows], state.row_base, &state.dirty_slots)`; `register_cell` fills the scratch for `0..rows_in_use` (mask already set by T3).
  - Rows alloc'd but never written stay un-uploaded (zero-init slot 0 / gen mismatch) — **fails closed** on GPU validation; documented on `slot_mirror_buffer()`.

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn slot_mirror_tracks_alloc_and_compaction_moves() {
    let ctx = test_context();
    let mut store = SceneGpuStore::new(&ctx, scene_cfg());
    let mut cell = transform_cell(64);
    let id = store.register_cell(&cell, 0).unwrap();
    let ha = cell.alloc().unwrap();
    let hb = cell.alloc().unwrap();
    let hc = cell.alloc().unwrap();
    for (h, s) in [(ha, 1.0f32), (hb, 2.0), (hc, 3.0)] {
        store.write_transform(id, &mut cell, h, &mat(s));
    }
    {
        let mut slots = [CellSlot { id, cell: &mut cell }];
        scene_boundary(&mut store, &mut slots);
    }
    let base = store.row_region_base(id) as usize;
    let mirror = as_u32s(&readback(&ctx, store.slot_mirror_buffer(), (64 * 4 * 4) as u64));
    // slot region base for class-0 cell 0 is 0; global_slot == local slot here.
    assert_eq!(&mirror[base..base + 3], &[ha.index(), hb.index(), hc.index()]);
    // Retire hb; hc swaps into its row; the mirror must follow the move.
    let serial = store.tracker().next_serial();
    store.free_deferred(id, &mut cell, hb, serial);
    store.tracker().force_complete(serial);
    {
        let mut slots = [CellSlot { id, cell: &mut cell }];
        scene_boundary(&mut store, &mut slots);
    }
    let hc_row = cell.row_of(hc).unwrap() as usize;
    let mirror = as_u32s(&readback(&ctx, store.slot_mirror_buffer(), (64 * 4 * 4) as u64));
    assert_eq!(mirror[base + hc_row], hc.index(), "moved row's mirror entry updated");
}
```

- [ ] **Step 2: Run to verify failure** — `slot_column` unresolved / mirror bytes zero.

- [ ] **Step 3: Implement.** `cell.rs`:

```rust
    /// Physical column 0 — the slot-ID column (one owning slot per row).
    /// Read by the GPU layer to maintain the row-indexed global-slot mirror
    /// (design Rev 2 §2; C6 GPU handle validation).
    pub(crate) fn slot_column(&self) -> &[u32] {
        self.page.column_slice::<u32>(0)
    }
```

`scene_store.rs`: in `write_transform`, move the gen-stamp call to capture whether it wrote: change `write_generation` to return `bool` (wrote), and on `true` also `state.dirty_slots.mark(row)`. In `sync_all`, before the mirror sync: `let col0 = cell.slot_column(); for row in 0..rows { if state.dirty_slots.is_marked(row) { state.slot_scratch[row as usize] = state.slot_base + col0[row as usize]; } }` then `self.slot_mirror.sync_region(...)`. In `register_cell`, fill scratch for `0..rows_in_use` unconditionally.

- [ ] **Step 4: Run** — new test PASS, everything else green.
- [ ] **Step 5: Commit** — `feat(scenedb): global-slot mirror maintenance (C6 GPU validation data path)`

---

### Task 5: Migrate the M2a gates to `SceneGpuStore`; delete `GpuStore`

**Files:**
- Modify: `crates/core/pulsar_scenedb/tests/gpu_store.rs` (port `test6_retirement_invariant`, `test14_device_loss_rematerialization`, the shadow-gate minimality test, `write_transform_is_the_single_mutation_path`, `compaction_move_is_resynced_...`, delta correctness/minimality to the `SceneGpuStore` API; delete `store_and_cell`/`frame_boundary` old helpers and the `GpuStore` import)
- Delete: `crates/core/pulsar_scenedb/src/gpu/store.rs`; remove `mod store;`/`pub use store::...` from `mod.rs`
- Modify: `scene_store.rs` — add `SceneGpuStore::rebuild(ctx, cfg, cells: &[(usize /*class*/, &CellStorage)]) -> (Self, Vec<CellId>)` (Test 14): constructs the store, `register_cell`s each (which already rebuilds gen regions + marks everything dirty), then bulk-writes transforms + mirror via `write_rows` per region and clears masks. Same drained-pins debug_assert per cell as `rebuild_from` had (verbatim message).

**Porting rules (exact):** every `GpuStore::new(&ctx, GpuStoreConfig{..})` becomes `SceneGpuStore::new(&ctx, scene_cfg())` + `register_cell`; every `store.retire(&mut cell)` / `compact` / `sync` becomes the `*_all` form with a one-element `CellSlot` array; buffer readbacks add `row_region_base(id)` offsets (region base is 0 for the first class-0 cell, so most byte math is unchanged — keep the explicit base add anyway). Delta-minimality assertions (`SyncStats`) and generation-write-count assertions carry over untouched. Test 14 uses `SceneGpuStore::rebuild` and additionally asserts the slot-mirror region is byte-identical.

- [ ] **Step 1: Port tests** (mechanical per rules above — every original assertion preserved).
- [ ] **Step 2: Delete `store.rs`** + mod entries; remove `GpuStoreConfig` re-export.
- [ ] **Step 3: Run FULL matrix** — `gpu_store` (now ~15 tests) PASS; core suites PASS; guard green. The gates are the acceptance bar: test6 and test14 must pass unmodified in their assertions.
- [ ] **Step 4: Commit** — `refactor(scenedb): migrate M2a gates to SceneGpuStore; retire single-cell GpuStore`

---

### Task 6: `RangeList` + `GeometryArena`

**Files:**
- Create: `crates/core/pulsar_scenedb/src/gpu/assets.rs` (starts with RangeList + arena; T7/T8 extend the same file)
- Modify: `mod.rs` (`mod assets;` + `pub use assets::{GeometryArena, ArenaError};`)
- Modify: `Cargo.toml` (add `[[test]] name = "gpu_assets" required-features = ["gpu"]`)
- Create: `tests/gpu_assets.rs` (copies the `test_context`/`readback` helpers from `tests/gpu_store.rs` — integration tests cannot share modules without a common mod file; a shared `tests/common/mod.rs` refactor is deliberately out of scope)

**Interfaces:**
- Produces:
  - `RangeList::new(total: u64)`, `alloc(&mut self, len: u64, align: u64) -> Option<u64>` (first-fit), `free(&mut self, offset: u64, len: u64)` (coalescing) — private helper, unit-tested.
  - `GeometryArena::new(ctx: &EngineGpuContext, vertex_bytes: u64, index_bytes: u64) -> Self` — two SSBO-usage buffers (`STORAGE | COPY_DST | COPY_SRC` — index buffer is consumed by compute in M3's GPU-driven path, plus `INDEX` usage).
  - `upload_vertices(&mut self, queue, bytes: &[u8]) -> Result<u32, ArenaError>` / `upload_indices(...)` — 4-byte-aligned first-fit alloc + `write_buffer`; returns byte offset (the §6.1 `vertex_offset`/`index_offset` values). `ArenaError::Exhausted` on failure (§8 hard error).
  - `free_vertices(&mut self, offset: u32, len: u32)` / `free_indices` — asset-unload path.
  - `vertex_buffer()`, `index_buffer()`.

- [ ] **Step 1: failing tests** — `tests/gpu_assets.rs`: upload two vertex blobs, assert disjoint offsets and byte-exact readback of both; exhaust a tiny arena → `Err(Exhausted)`; free + realloc reuses the space (offset equality after coalescing). RangeList unit tests inline in `assets.rs`: first-fit, alignment padding, coalescing of adjacent frees.
- [ ] **Step 2: verify failure** — compile FAIL.
- [ ] **Step 3: implement.** RangeList, complete:

```rust
/// First-fit byte-range suballocator over one buffer (design Rev 2 §3):
/// whole-mesh allocations at load, frees only on asset unload — no per-frame
/// churn, so first-fit with free-span coalescing is sufficient.
struct RangeList {
    /// Sorted, non-adjacent free spans: (offset, len).
    free: Vec<(u64, u64)>,
}

impl RangeList {
    fn new(total: u64) -> Self {
        Self { free: vec![(0, total)] }
    }

    fn alloc(&mut self, len: u64, align: u64) -> Option<u64> {
        debug_assert!(align.is_power_of_two());
        for i in 0..self.free.len() {
            let (off, span) = self.free[i];
            let aligned = (off + align - 1) & !(align - 1);
            let pad = aligned - off;
            if pad + len <= span {
                // Split: [off, aligned) stays free (alignment pad),
                // [aligned+len, off+span) stays free (tail).
                let tail = span - pad - len;
                self.free.remove(i);
                if tail > 0 {
                    self.free.insert(i, (aligned + len, tail));
                }
                if pad > 0 {
                    self.free.insert(i, (off, pad));
                }
                return Some(aligned);
            }
        }
        None
    }

    fn free(&mut self, offset: u64, len: u64) {
        let idx = self.free.partition_point(|&(o, _)| o < offset);
        self.free.insert(idx, (offset, len));
        // Coalesce with next, then with previous.
        if idx + 1 < self.free.len() && self.free[idx].0 + self.free[idx].1 == self.free[idx + 1].0 {
            self.free[idx].1 += self.free[idx + 1].1;
            self.free.remove(idx + 1);
        }
        if idx > 0 && self.free[idx - 1].0 + self.free[idx - 1].1 == self.free[idx].0 {
            self.free[idx - 1].1 += self.free[idx].1;
            self.free.remove(idx);
        }
    }
}
```

`GeometryArena` wraps two `(wgpu::Buffer, RangeList)` pairs; `upload_vertices` = `self.vfree.alloc(bytes.len() as u64, 4).ok_or(ArenaError::Exhausted)` then `queue.write_buffer(&self.vertex, offset, bytes)`, returning `offset as u32`; `free_vertices(offset, len)` delegates to `RangeList::free`. Vertex/index buffer usages: `STORAGE | COPY_DST | COPY_SRC` plus `INDEX` on the index buffer. The arena retains no CPU copy of geometry — Test 14's asset half re-uploads from the caller's retained blobs (asset system owns source data; the arena is residency only).
- [ ] **Step 4: run** — `cargo test -p pulsar_scenedb --features gpu --test gpu_assets` PASS.
- [ ] **Step 5: Commit** — `feat(scenedb): GeometryArena — global vertex/index buffers with range suballocation (M2b-a 2b.0)`

---

### Task 7: `MeshMetadata` (72 B) + `MeshRegistry`

**Files:** extend `src/gpu/assets.rs`; re-export `{MeshMetadata, MeshRegistry, MeshError}`; tests in `tests/gpu_assets.rs`.

**Interfaces:**

```rust
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeshMetadata {
    pub vertex_offset: u32,        // 0
    pub index_offset: u32,         // 4
    pub index_count: u32,          // 8
    pub base_vertex: i32,          // 12
    pub material_index: u32,       // 16
    pub lod_count: u32,            // 20
    pub lod_distances: [f32; 4],   // 24
    pub local_aabb_center: [f32; 3], // 40
    pub cluster_table_offset: u32, // 52
    pub local_aabb_extents: [f32; 3], // 56
    pub meshlet_count: u32,        // 68
}                                  // = 72 bytes (C5/§6.1)
const _: () = assert!(std::mem::size_of::<MeshMetadata>() == 72);
unsafe impl crate::page::Pod for MeshMetadata {}   // upload via as_bytes

pub enum MeshError { XorRule, RegistryFull }
impl MeshRegistry {
    pub fn new(ctx: &EngineGpuContext, max_meshes: u32) -> Self;  // SSBO 72*max
    /// C5 XOR rule: exactly one of {lod_count, cluster_table_offset} non-zero.
    pub fn register(&mut self, queue: &wgpu::Queue, m: MeshMetadata) -> Result<u32, MeshError>;
    pub fn get(&self, mesh_index: u32) -> &MeshMetadata;
    pub fn len(&self) -> u32;
    pub fn entries(&self) -> &[MeshMetadata];   // Test 14 rebuild source
    pub fn buffer(&self) -> &wgpu::Buffer;
    pub fn rebuild(&self, queue: &wgpu::Queue); // bulk re-upload (Test 14)
}
```

Note: `unsafe impl Pod for MeshMetadata` lives in **assets.rs** (gpu-gated) — it must NOT go into the graphics-free core; `Pod` is public so a foreign impl in the same crate is fine.

- [ ] **Steps:** failing tests (register a traditional mesh (`lod_count=2, cluster_table_offset=0`) and a VG mesh (`0, 100`); readback the SSBO and assert both 72 B records byte-exact against `as_bytes(entries())`; `register` with both fields non-zero → `Err(XorRule)`; both zero → `Err(XorRule)`; fill to `max_meshes` → `Err(RegistryFull)`) → verify FAIL → implement → PASS → commit `feat(scenedb): MeshRegistry — C5 72B mesh metadata with XOR validation (M2b-a 2b.0)`.

---

### Task 8: `ClusterNode` (48 B) + `ClusterBuffer`

**Files:** extend `src/gpu/assets.rs`; re-export `{ClusterNode, ClusterBuffer, ClusterError}`; tests in `tests/gpu_assets.rs`.

**Interfaces:**

```rust
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClusterNode {
    pub meshlet_offset: u32,      // 0
    pub meshlet_count: u32,       // 4
    pub parent_error: f32,        // 8
    pub self_error: f32,          // 12  invariant: self_error < parent_error
    pub group_id: u32,            // 16
    pub child_offset: u32,        // 20
    pub child_count: u32,         // 24
    pub padding: u32,             // 28  must be 0
    pub bounding_sphere: [f32; 4],// 32  xyz center, w radius
}                                 // = 48 bytes (C5)
const _: () = assert!(std::mem::size_of::<ClusterNode>() == 48);

pub enum ClusterError { ErrorMonotonicity, PaddingNonZero, BufferFull }
impl ClusterBuffer {
    pub fn new(ctx: &EngineGpuContext, max_nodes: u32) -> Self;
    /// Appends a mesh's DAG nodes; returns the node offset (the C5
    /// `cluster_table_offset` unit). Validates self_error < parent_error and
    /// padding == 0 for every node.
    pub fn append(&mut self, queue: &wgpu::Queue, nodes: &[ClusterNode]) -> Result<u32, ClusterError>;
    pub fn len(&self) -> u32;
    pub fn nodes(&self) -> &[ClusterNode];
    pub fn buffer(&self) -> &wgpu::Buffer;
    pub fn rebuild(&self, queue: &wgpu::Queue);
}
```

- [ ] **Steps:** failing tests (append 2 valid nodes → offset 0, then 1 more → offset 2; readback byte-exact; `self_error >= parent_error` → `Err(ErrorMonotonicity)`; `padding != 0` → `Err(PaddingNonZero)`; overflow → `Err(BufferFull)`) → FAIL → implement → PASS → commit `feat(scenedb): ClusterBuffer — C5 48B cluster DAG nodes with monotonicity validation`.

---

### Task 9: Test 3 extension — MeshMetadata / ClusterNode / slot-mirror WGSL layouts

**Files:** modify `tests/gpu_layout.rs` only.

WGSL to reflect (append to the existing source string or a second one):

```wgsl
struct MeshMetadata {
    vertex_offset: u32, index_offset: u32, index_count: u32, base_vertex: i32,
    material_index: u32, lod_count: u32,
    lod_d0: f32, lod_d1: f32, lod_d2: f32, lod_d3: f32,
    aabb_cx: f32, aabb_cy: f32, aabb_cz: f32,
    cluster_table_offset: u32,
    aabb_ex: f32, aabb_ey: f32, aabb_ez: f32,
    meshlet_count: u32,
}
struct ClusterNode {
    meshlet_offset: u32, meshlet_count: u32, parent_error: f32, self_error: f32,
    group_id: u32, child_offset: u32, child_count: u32, padding: u32,
    bs_x: f32, bs_y: f32, bs_z: f32, bs_w: f32,
}
@group(0) @binding(2) var<storage, read> mesh_meta: array<MeshMetadata>;
@group(0) @binding(3) var<storage, read> clusters: array<ClusterNode>;
@group(0) @binding(4) var<storage, read> slot_mirror: array<u32>;
```

Assertions (via the existing `wgsl_struct_layout` harness): `MeshMetadata` size == 72 == `size_of::<MeshMetadata>()`, offsets `[0,4,8,12,16,20,24,28,32,36,40,44,48,52,56,60,64,68]` matching field order; `ClusterNode` size == 48, `bs_x` at 32; slot-mirror element u32 size 4. Add the one-line comment the Task 12 (M2a) ledger asked for: naga's `Layouter` computes address-space-agnostic base layout — these structs are scalar-only precisely so uniform/storage divergence cannot bite; the `var<storage>` declarations make the intended address space explicit.

- [ ] **Steps:** write tests → FAIL (structs absent from WGSL) → add WGSL + assertions → `cargo test -p pulsar_scenedb --features gpu --test gpu_layout` PASS (5 tests) → commit `test(scenedb): Test 3 extension — MeshMetadata/ClusterNode/slot-mirror byte layouts (C5)`.

---

### Task 10: Test 14 extension — multi-cell + assets device-loss rebuild

**Files:** append to `tests/gpu_store.rs` (scene rebuild) and `tests/gpu_assets.rs` (asset rebuild). Test-only.

Scene half: two registered cells with churn (alloc 8 / deferred-retire 2 each, serials force-completed, boundaries run), snapshot transform + slot-mirror + generation buffers; drop store; drop context; fresh context; `SceneGpuStore::rebuild(&ctx2, cfg, &[(0, &cell_a), (0, &cell_b)])`; assert all three buffers byte-identical over every occupied region (transforms over `rows_in_use*64` per region; mirror over `rows_in_use*4`; generations over `generations().len()*4` per slot region). Asset half: arena with two meshes + registry + cluster nodes; drop; fresh context; `GeometryArena` re-upload from retained CPU blobs + `MeshRegistry::rebuild` + `ClusterBuffer::rebuild`; byte-identical readback.

- [ ] **Steps:** write both tests → run (should PASS if Tasks 5-8 are correct; a failure here is a bug in those tasks — report BLOCKED, do not weaken) → commit `test(scenedb): Test 14 extension — multi-cell + asset re-materialization (C0 companion)`.

---

### Task 11: Phase machine — witness types, `FrameDriver`, compile-fail gates, tracker hardening

**Files:**
- Create: `crates/core/pulsar_scenedb/src/gpu/phase.rs`; `mod.rs` re-exports `{FrameDriver, SimulateA, SimulateB, HarvestPhase, BoundaryPhase, SimulateWitness}`.
- Modify: `scene_store.rs` — `write_transform` and `free_deferred` gain a final `_sim: &impl SimulateWitness` parameter; `retire_all`/`compact_all`/`sync_all` become `pub(crate)` (callable only through `BoundaryPhase`); `free_deferred` gains the S6 debug_assert.
- Modify: `tests/gpu_store.rs` — mechanical: every mutation call adds the witness argument; boundaries go through the driver.

**Interfaces (exact — design §6):**

```rust
/// C3 frame phases as zero-size witnesses (design Rev 2 §6): holding a phase
/// value IS the permission. Misuse is a compile error; the runtime Phase enum
/// inside SceneGpuStore stays as a debug backstop for untyped callers (FFI).
pub struct FrameDriver(());
impl FrameDriver {
    pub fn new() -> Self;
    pub fn begin(&mut self) -> SimulateA;
}
pub struct SimulateA(());
impl SimulateA { pub fn end(self) -> SimulateB; }
pub struct SimulateB(());
impl SimulateB { pub fn end(self) -> HarvestPhase; }
pub struct HarvestPhase(());
impl HarvestPhase { pub fn end(self) -> BoundaryPhase; }
pub struct BoundaryPhase(());
impl BoundaryPhase {
    /// retire → (transitions: β) → compact → sync, consuming the witness.
    pub fn run(self, store: &mut SceneGpuStore, cells: &mut [CellSlot<'_>]) -> SyncStats;
}
/// Sealed: mutation APIs accept either simulate sub-phase (C3 A=gameplay,
/// B=physics writeback — the distinction gains teeth when physics lands, M4).
pub trait SimulateWitness: private::Sealed {}
impl SimulateWitness for SimulateA {}
impl SimulateWitness for SimulateB {}
mod private { pub trait Sealed {} impl Sealed for super::SimulateA {} impl Sealed for super::SimulateB {} }
```

Compile-fail doc-tests on `phase.rs` (run with `cargo test -p pulsar_scenedb --features gpu --doc`):

```rust
/// Mutation requires a Simulate witness — a Harvest witness does not compile:
/// ```compile_fail
/// use pulsar_scenedb::gpu::*;
/// fn f(store: &SceneGpuStore, id: CellId, cell: &mut pulsar_scenedb::CellStorage,
///      h: pulsar_scenedb::Handle, harvest: &HarvestPhase) {
///     store.write_transform(id, cell, h, &[0.0; 16], harvest); // not a SimulateWitness
/// }
/// ```
/// Boundary stages cannot be reordered — `retire_all` is pub(crate):
/// ```compile_fail
/// use pulsar_scenedb::gpu::*;
/// fn f(store: &mut SceneGpuStore, cells: &mut [CellSlot<'_>]) {
///     store.retire_all(cells); // private outside the crate
/// }
/// ```
```

S6 hardening in `free_deferred` (after the queue push site, exact):

```rust
        debug_assert!(
            state.pending.back().map_or(true, |q| q.serial <= serial),
            "free_deferred serials must be nondecreasing per cell — the retire \
             drain's FIFO early-break would silently stall retirement behind an \
             out-of-order serial"
        );
```

(placed BEFORE the push, comparing against the current back). `SubmissionTracker::signal_submitted` doc gains: "must be called only after the work for `serial` has been submitted — signaling first completes the watermark early and breaks C6."

- [ ] **Steps:** write a runtime test (drive one full frame through `FrameDriver` and assert the store behaves identically to the manual sequence) + the compile_fail doc-tests → verify doc-tests FAIL before implementation (they *pass* trivially while the API doesn't exist — so implement first, then confirm `cargo test --doc` runs them and they hold) → migrate test call sites → FULL matrix incl. `--doc` → commit `feat(scenedb): compile-time frame-phase witnesses + tracker hardening (M2b-a 2b.3)`.

---

### Task 12: Docs wrap-up + full verification matrix

**Files:** `src/lib.rs` (crate doc: M2b-α status), `README.md` (SceneGpuStore/assets/phase sections + gpu_assets test command), design doc status line (`Rev 2 — α implemented`), memory of record untouched (session artifact).

- [ ] **Verification matrix (acceptance gate):**

```
cargo check -p pulsar_scenedb --no-default-features        → green
cargo test  -p pulsar_scenedb --lib --tests                → green (126+)
cargo test  -p pulsar_scenedb --features gpu --test gpu_store --test gpu_assets --test gpu_layout  → green
cargo test  -p pulsar_scenedb --features gpu --doc         → green (compile_fail gates)
cargo check -p pulsar_scenedb --all-targets --features gpu → green
```

- [ ] **Commit** — `docs(scenedb): M2b-alpha docs — SceneGpuStore, asset store, phase machine complete`

---

## Deferred to M2b-β (do NOT implement here)

StreamingGrid/domains/hysteresis/α-cross-fade writer, HarvestPipeline, DEI compress-store, Scratchpad split-borrow + `capture_into` + words-parameterized queries, per-cell metadata writer, region eviction flow (the pool's `free_pinned`/`drain_completed` ship in α, unused), Tests 10/11/12, benches.

**Deferred to M4 (design §6 note):** demoting `CellStorage::free` below `pub` — the design's "only public deletion on GPU-backed worlds is the deferred path" applies at the World integration level; `free` stays public in α for CPU-only cells (per the M2a design's "immediate-free retained for non-GPU use"), still guarded by the pinned-row debug_assert.

## Verification (end-to-end)

Task 12's matrix. Gate identity: the migrated Test 6 / Test 14 keep every original assertion; the D2-regression (registration gen-rebuild) test and the slot-mirror move test are the new α-specific gates; compile_fail doc-tests prove the phase machine actually forbids what it claims to forbid.
