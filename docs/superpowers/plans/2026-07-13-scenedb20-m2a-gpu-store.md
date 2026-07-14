# SceneDB 2.0 — M2a GPU-Resident Store Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the feature-gated `pulsar_scenedb::gpu` layer — persistent scene SSBOs, CPU→GPU delta-sync, and pin-by-serial retirement — verified headless per the M2a design Rev 3.

**Architecture:** One crate (`crates/core/pulsar_scenedb`): the graphics-free core gains `pub(crate)` pin/retire primitives and `Scratchpad::get_u64`; a new `src/gpu/` module behind the `gpu` cargo feature owns `EngineGpuContext`, `SceneBuffer<T>` (row-indexed, dirty-bitmask delta-sync), `GenerationBuffer` (slot-indexed), `SubmissionTracker`, and `GpuStore` orchestrating the `retire → compact → sync` frame boundary. No separate GPU crate (CONTRACTS C0).

**Tech Stack:** Rust 2021, wgpu 28 (Far-Beyond-Pulsar fork, workspace dep pinned `fce5b80…`), pollster 1.0 (tests), naga path-dep from the fork (Test 3).

**Design of record:** `docs/superpowers/specs/2026-06-13-scenedb20-m2a-gpu-store-design.md` (Rev 3). Contracts: `docs/superpowers/specs/CONTRACTS.md` C0, C1, C5, C6.

## Global Constraints

- **C0:** no edge from `pulsar_scenedb` to Helio; `cargo check -p pulsar_scenedb --no-default-features` must stay green (graphics-free core).
- **Feature gate:** every GPU item sits under `#[cfg(feature = "gpu")]`; `default = []`; `gpu = ["dep:wgpu"]`.
- **C5:** instance element = 64 B row-major mat4 (`[f32; 16]`); generation buffer = `u32` per slot. Scene buffers **row-indexed**; generation buffer is the lone **slot-indexed** buffer.
- **C6:** a slot is recycled only after `Queue::on_submitted_work_done` confirms its serial; the new generation reaches the VRAM generation buffer **before** the slot returns to the free pool. Frame-counter arithmetic is forbidden.
- **SSBOs never reallocate:** capacity fixed at creation; exceeding it is a hard error at the call site, never a mid-frame realloc.
- **Frame-boundary order:** `retire()` → `compact()` → `sync()`, debug-asserted.
- **Windows/encoding:** author `.rs` files ONLY via the Write/Edit tools — PowerShell `Set-Content`/`Out-File`/redirection adds a UTF-8 BOM that breaks rustc.
- **Test command:** `cargo test -p pulsar_scenedb --lib --tests` (core), `cargo test -p pulsar_scenedb --features gpu --test gpu_store` / `--test gpu_layout` (GPU; needs a local GPU — CI runs core only).
- **Commit style:** `type(scenedb): summary`, end body with `Co-Authored-By:` trailer per repo convention.

## File Structure

```
crates/core/pulsar_scenedb/
  Cargo.toml                 # + [features], optional wgpu, dev-deps, [[test]] entries
  src/lib.rs                 # + #[cfg(feature = "gpu")] pub mod gpu; re-exports
  src/page.rs                # + unsafe impl Pod for [f32; 16]
  src/lease.rs               # + Scratchpad::get_u64 / buf_len_u64, dual-buffer decay
  src/registry.rs            # + pub(crate) commit_retire(slot) -> u32
  src/cell.rs                # + PinSet, mark_pending_retire/commit_retire, compact_report
  src/gpu/mod.rs             # module root, as_bytes helper, re-exports
  src/gpu/context.rs         # EngineGpuContext
  src/gpu/tracker.rs         # SubmissionTracker
  src/gpu/buffer.rs          # SceneBuffer<T>, SyncStats
  src/gpu/generation.rs      # GenerationBuffer
  src/gpu/store.rs           # GpuStore, GpuStoreConfig, phase guard
  tests/gpu_store.rs         # device+readback helpers, delta/minimality/compaction/Test 6/Test 14
  tests/gpu_layout.rs        # Test 3 — naga WGSL byte-layout harness
.github/workflows/ci.yml     # + graphics-free guard step
```

---

### Task 1: Feature scaffold + CI graphics-free guard

**Files:**
- Modify: `crates/core/pulsar_scenedb/Cargo.toml`
- Modify: `crates/core/pulsar_scenedb/src/lib.rs`
- Create: `crates/core/pulsar_scenedb/src/gpu/mod.rs`
- Modify: `.github/workflows/ci.yml` (after the `cargo check` step, line ~48)

**Interfaces:**
- Produces: `gpu` cargo feature; `pulsar_scenedb::gpu` module path; CI guard. Later tasks add files under `src/gpu/` and test targets `gpu_store`/`gpu_layout`.

- [ ] **Step 1: Add feature, optional wgpu, dev-deps, test targets to Cargo.toml**

Append/merge into `crates/core/pulsar_scenedb/Cargo.toml` (keep existing content):

```toml
[features]
default = []
gpu = ["dep:wgpu"]

# (add to existing [dependencies])
# wgpu = { workspace = true, optional = true }

# (add to existing [dev-dependencies])
# pollster = "1.0"
# naga = { git = "https://github.com/Far-Beyond-Pulsar/wgpu", rev = "fce5b80e8017304449124b12637ec324417e40c8", features = ["wgsl-in"] }
# (AMENDED during Task 1: a path dep into crates/graphics/wgpu/naga cannot
#  build — that submodule is its own cargo workspace, so naga's
#  `version.workspace = true` fields can't resolve against our root. The git
#  dep is pinned to the SAME rev as the workspace wgpu dep and must be bumped
#  in lockstep with it.)

[[test]]
name = "gpu_store"
required-features = ["gpu"]

[[test]]
name = "gpu_layout"
required-features = ["gpu"]
```

(`naga` is a path dev-dep into the `crates/graphics/wgpu` submodule — same rev as the workspace `wgpu` git dep by construction. Dev-deps can't be optional; the graphics-free guard checks the **lib** only, so this does not violate C0.)

- [ ] **Step 2: Create the module root**

`src/gpu/mod.rs`:

```rust
//! SceneDB GPU layer (M2a, design Rev 3): persistent scene SSBOs, CPU→GPU
//! delta-sync, and pin-by-serial retirement. Feature-gated (`gpu`); the core
//! crate stays graphics-free (CONTRACTS C0).

mod buffer;
mod context;
mod generation;
mod store;
mod tracker;

pub use buffer::{SceneBuffer, SyncStats};
pub use context::EngineGpuContext;
pub use generation::GenerationBuffer;
pub use store::{GpuStore, GpuStoreConfig};
pub use tracker::SubmissionTracker;

/// Reinterpret a Pod slice as bytes for `queue.write_buffer`.
pub(crate) fn as_bytes<T: crate::page::Pod>(s: &[T]) -> &[u8] {
    // SAFETY: T: Pod guarantees no padding-UB and no invalid bit patterns.
    unsafe { std::slice::from_raw_parts(s.as_ptr() as *const u8, std::mem::size_of_val(s)) }
}
```

For this task only, create the five submodule files as empty stubs (`// M2a Task N`) so the module compiles; each later task fills its own file. Alternatively comment out the `mod`/`pub use` lines and re-enable per task — pick the stub approach so `--features gpu` always builds.

- [ ] **Step 3: Gate the module in lib.rs**

Add after the existing `pub mod` block in `src/lib.rs`:

```rust
#[cfg(feature = "gpu")]
pub mod gpu;
```

- [ ] **Step 4: Verify both build configurations**

Run: `cargo check -p pulsar_scenedb --no-default-features`
Expected: green (graphics-free core, no wgpu in the tree)

Run: `cargo check -p pulsar_scenedb --features gpu`
Expected: green (wgpu resolves from the workspace dep)

- [ ] **Step 5: Add the CI guard**

In `.github/workflows/ci.yml`, after the `cargo check` step:

```yaml
      - name: SceneDB graphics-free core guard (CONTRACTS C0)
        run: cargo check -p pulsar_scenedb --no-default-features
```

- [ ] **Step 6: Commit**

```bash
git add crates/core/pulsar_scenedb/Cargo.toml crates/core/pulsar_scenedb/src/lib.rs crates/core/pulsar_scenedb/src/gpu/ .github/workflows/ci.yml
git commit -m "feat(scenedb): gpu feature scaffold + CI graphics-free guard (C0)"
```

---

### Task 2: Core — `Pod` for `[f32; 16]` and `Scratchpad::get_u64`

**Files:**
- Modify: `crates/core/pulsar_scenedb/src/page.rs` (the `impl_pod!` line, ~line 28)
- Modify: `crates/core/pulsar_scenedb/src/lease.rs`
- Test: inline `#[cfg(test)]` in `lease.rs`; page test inline in `page.rs`

**Interfaces:**
- Produces: `[f32; 16]` usable as a column element type (the C5 instance mat4); `Scratchpad::get_u64(len) -> &mut [u64]`, `Scratchpad::buf_len_u64() -> usize`. `end_frame()` decays u32 and u64 buffers independently under the same 8-frame/50% policy.

- [ ] **Step 1: Write failing tests**

In `page.rs` tests:

```rust
#[test]
fn mat4_array_is_a_column_type() {
    // Compile-time: ColumnDesc::of requires T: Pod.
    let d = ColumnDesc::of::<[f32; 16]>();
    assert_eq!(d.size, 64);
}
```

In `lease.rs` tests:

```rust
#[test]
fn scratchpad_u64_grows_and_decays_independently() {
    let mut pad = Scratchpad::new();
    {
        let b = pad.get_u64(500);
        assert!(b.len() >= 500);
    }
    let cap = pad.buf_len_u64();
    assert!(cap >= 500);
    // u32 buffer untouched by u64 usage:
    assert_eq!(pad.buf_len_u32(), 0);
    for _ in 0..(2 * DECAY_FRAMES) {
        let _ = pad.get_u64(8);
        pad.end_frame();
    }
    assert!(pad.buf_len_u64() < cap, "u64 buffer decayed");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p pulsar_scenedb --lib mat4_array scratchpad_u64 2>&1 | tail -5`
Expected: compile FAIL — `[f32; 16]: Pod` not satisfied; `get_u64` not found

- [ ] **Step 3: Implement**

`page.rs` — extend the existing macro invocation:

```rust
impl_pod!(u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize, f32, f64);
// C5 instance element: 64-byte row-major mat4. Kept in the graphics-free core
// so the transform column exists independent of the gpu feature.
unsafe impl Pod for [f32; 16] {}
```

`lease.rs` — add to `Scratchpad` (mirror the u32 fields; per-buffer peaks share one frame window):

```rust
pub struct Scratchpad {
    u32_buf: Vec<u32>,
    u64_buf: Vec<u64>,
    peak_this_window: usize,
    peak_u64_this_window: usize,
    frames_in_window: u32,
}
```

```rust
    /// Borrow a u64 buffer of at least `len` (liveness words / dirty words;
    /// the M1b §8.1 carry-forward). Not zeroed.
    pub fn get_u64(&mut self, len: usize) -> &mut [u64] {
        if self.u64_buf.len() < len {
            self.u64_buf.resize(len, 0);
        }
        self.peak_u64_this_window = self.peak_u64_this_window.max(len);
        &mut self.u64_buf[..len]
    }

    #[must_use]
    pub fn buf_len_u64(&self) -> usize {
        self.u64_buf.len()
    }
```

In `end_frame()`, apply the identical halving rule to `u64_buf`/`peak_u64_this_window` inside the same `frames_in_window >= DECAY_FRAMES` block, and reset both peaks. Update `new()` for the new fields.

- [ ] **Step 4: Run tests**

Run: `cargo test -p pulsar_scenedb --lib 2>&1 | tail -3`
Expected: PASS (all lib tests; no regressions in the existing decay test)

- [ ] **Step 5: Commit**

```bash
git add crates/core/pulsar_scenedb/src/page.rs crates/core/pulsar_scenedb/src/lease.rs
git commit -m "feat(scenedb): Pod mat4 column element + Scratchpad::get_u64 (M2a §4, §8.1 carry-forward)"
```

---

### Task 3: Core — pin-by-serial retirement primitives (`pub(crate)`)

**Files:**
- Modify: `crates/core/pulsar_scenedb/src/registry.rs`
- Modify: `crates/core/pulsar_scenedb/src/cell.rs`
- Test: inline `#[cfg(test)]` in both files

**Interfaces:**
- Consumes: `HandleRegistry` fields, `LivenessMask`, `Handle` (Task-independent, existing M1 code).
- Produces (all `pub(crate)`, design Rev 3 §5):
  - `HandleRegistry::commit_retire(&mut self, slot: u32) -> u32` — deferred tail of `free`: nulls `slot_to_row`, bumps generation (retiring permanently at `u32::MAX`), pools the slot; returns the new generation.
  - `struct PendingRetire { pub slot: u32, pub row: u32, pub next_gen: u32 }` in `cell.rs`.
  - `CellStorage::mark_pending_retire(&mut self, handle: Handle) -> Option<PendingRetire>` — liveness-dead + pinned; registry untouched; handle still resolves by row.
  - `CellStorage::commit_retire(&mut self, pending: PendingRetire)` — unpin + registry commit.
  - `CellStorage::is_row_pinned(&self, row: u32) -> bool`.

- [ ] **Step 1: Write failing tests**

In `cell.rs` tests:

```rust
#[test]
fn pending_retire_keeps_handle_resolvable_but_not_live() {
    let mut c = cell();
    let h = c.alloc().unwrap();
    let p = c.mark_pending_retire(h).unwrap();
    assert_eq!(p.slot, h.index());
    assert_eq!(p.next_gen, h.generation() + 1);
    // In-flight window: row still resolvable (GPU's last harvest is valid)…
    assert_eq!(c.row_of(h), Some(p.row));
    assert!(c.is_row_pinned(p.row));
    // …but excluded from liveness (won't appear in new harvests).
    assert_eq!(c.live_count(), 0);
    // Double-mark is rejected.
    assert!(c.mark_pending_retire(h).is_none());
}

#[test]
fn commit_retire_rejects_stale_handle_and_recycles_slot() {
    let mut c = cell();
    let h = c.alloc().unwrap();
    let p = c.mark_pending_retire(h).unwrap();
    let row = p.row;
    c.commit_retire(p);
    assert!(!c.is_row_pinned(row));
    assert_eq!(c.row_of(h), None, "stale after commit");
    let h2 = c.alloc().unwrap();
    assert_eq!(h2.index(), h.index(), "slot recycled only after commit");
    assert_eq!(h2.generation(), h.generation() + 1);
}
```

In `registry.rs` tests:

```rust
#[test]
fn commit_retire_is_the_deferred_tail_of_free() {
    let mut reg = HandleRegistry::new();
    let h = reg.allocate(3);
    let new_gen = reg.commit_retire(h.index());
    assert_eq!(new_gen, h.generation() + 1);
    assert_eq!(reg.row_of(h), None);
    let h2 = reg.allocate(0);
    assert_eq!(h2.index(), h.index());
    assert_eq!(h2.generation(), new_gen);
}

#[test]
fn commit_retire_permanently_retires_at_gen_max() {
    let mut reg = HandleRegistry::new();
    let h = reg.allocate(0);
    reg.force_generation(h.index(), u32::MAX - 1);
    assert_eq!(reg.commit_retire(h.index()), u32::MAX);
    assert_eq!(reg.retired_count(), 1);
    let h2 = reg.allocate(0);
    assert_ne!(h2.index(), h.index(), "retired slot never reissued");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p pulsar_scenedb --lib pending_retire commit_retire 2>&1 | tail -5`
Expected: compile FAIL — methods not found

- [ ] **Step 3: Implement**

`registry.rs`:

```rust
    /// Deferred tail of [`free`](Self::free) for the pin-by-serial retirement
    /// path (M2a §5): nulls the row mapping, bumps the generation (permanent
    /// retirement at u32::MAX), pools the slot. Returns the new generation.
    /// Caller (CellStorage) guarantees the slot is live-pending — the handle
    /// was validated at mark time and the slot cannot be freed twice because
    /// the row stays pinned until this call.
    pub(crate) fn commit_retire(&mut self, slot: u32) -> u32 {
        let s = slot as usize;
        debug_assert!(
            self.slot_to_row[s] != NULL_ROW,
            "commit_retire: slot {slot} is not allocated"
        );
        self.slot_to_row[s] = NULL_ROW;
        debug_assert!(self.generations[s] < u32::MAX);
        let next = self.generations[s] + 1;
        self.generations[s] = next;
        if next == u32::MAX {
            self.retired_count += 1;
        } else {
            self.free.push(slot);
        }
        next
    }
```

`cell.rs` — add a pin bitmask and the two primitives:

```rust
/// In-flight retirement record (M2a §5). Produced by `mark_pending_retire`,
/// consumed by `commit_retire` once the submission serial completes.
pub(crate) struct PendingRetire {
    pub slot: u32,
    pub row: u32,
    pub next_gen: u32,
}
```

Add field `pins: Vec<u64>` to `CellStorage` (sized `capacity.div_ceil(64)` in both constructors, zero-initialized) plus:

```rust
    #[inline]
    pub(crate) fn is_row_pinned(&self, row: u32) -> bool {
        self.pins[(row / 64) as usize] & (1u64 << (row % 64)) != 0
    }

    fn pin_row(&mut self, row: u32) {
        self.pins[(row / 64) as usize] |= 1u64 << (row % 64);
    }

    fn unpin_row(&mut self, row: u32) {
        self.pins[(row / 64) as usize] &= !(1u64 << (row % 64));
    }

    /// Begin deferred retirement (M2a §5): liveness-dead (excluded from new
    /// harvests) and pinned (excluded from compaction), but the registry is
    /// untouched — the handle intentionally still resolves by row during the
    /// in-flight window. None for stale handles or already-pending rows.
    pub(crate) fn mark_pending_retire(&mut self, handle: Handle) -> Option<PendingRetire> {
        let row = self.registry.row_of(handle)?;
        if self.is_row_pinned(row) {
            return None;
        }
        self.liveness.set_dead(row);
        self.pin_row(row);
        Some(PendingRetire { slot: handle.index(), row, next_gen: handle.generation() + 1 })
    }

    /// Complete deferred retirement: unpin the row (compactable) and run the
    /// registry tail (generation bump + slot pooling). The caller must have
    /// written `pending.next_gen` to the VRAM generation buffer FIRST (C6).
    pub(crate) fn commit_retire(&mut self, pending: PendingRetire) {
        self.unpin_row(pending.row);
        let new_gen = self.registry.commit_retire(pending.slot);
        debug_assert_eq!(new_gen, pending.next_gen, "generation drift between mark and commit");
    }
```

Guard the immediate-free path against mixing (in `free()`, before `set_dead`):

```rust
        debug_assert!(
            !self.is_row_pinned(row),
            "free() on a pending-retire row — use the deferred path end-to-end"
        );
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p pulsar_scenedb --lib 2>&1 | tail -3`
Expected: PASS, zero regressions

- [ ] **Step 5: Commit**

```bash
git add crates/core/pulsar_scenedb/src/registry.rs crates/core/pulsar_scenedb/src/cell.rs
git commit -m "feat(scenedb): pub(crate) pin-by-serial retirement primitives (M2a design Rev 3 §5)"
```

---

### Task 4: Core — `compact_report` with skip-pinned semantics

**Files:**
- Modify: `crates/core/pulsar_scenedb/src/cell.rs`
- Test: inline `#[cfg(test)]` in `cell.rs`

**Interfaces:**
- Consumes: Task 3's `pins` / `is_row_pinned`.
- Produces: `CellStorage::compact_report(&mut self, on_move: impl FnMut(u32, u32))` (`pub(crate)`) — swap-and-pop that (a) treats pinned rows as immovable-in-place, (b) stops popping at a pinned tail (holes behind a pinned tail persist until unpin — best-effort under pins, complete after `retire()` runs first at the boundary), (c) reports every `(from_row, to_row)` move. Public `compact()` becomes `self.compact_report(|_, _| {})`.

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn compact_reports_moves() {
    let mut c = cell();
    let hs: Vec<_> = (0..4).map(|_| c.alloc().unwrap()).collect();
    c.free(hs[1]);
    let mut moves = Vec::new();
    c.compact_report(|from, to| moves.push((from, to)));
    assert_eq!(moves, vec![(3, 1)], "last live row fills the hole");
    assert_eq!(c.rows_in_use(), 3);
}

#[test]
fn pinned_row_survives_compaction_in_place() {
    let mut c = cell();
    let ha = c.alloc().unwrap();
    let hb = c.alloc().unwrap();
    let hc = c.alloc().unwrap();
    let row_b = c.row_of(hb).unwrap();
    let p = c.mark_pending_retire(hb).unwrap(); // dead but pinned
    c.free(ha); // dead, unpinned → compactable hole at row 0
    c.compact();
    // Pinned row untouched at its original index; its bytes are preserved.
    assert!(c.is_row_pinned(row_b));
    assert_eq!(c.row_of(hb), Some(row_b), "pinned row not moved");
    // hc filled ha's hole:
    assert_eq!(c.row_of(hc), Some(0));
    // After commit, a second compact reclaims the row.
    c.commit_retire(p);
    c.compact();
    assert_eq!(c.rows_in_use(), 1);
}

#[test]
fn pinned_tail_blocks_pop_leaving_hole() {
    let mut c = cell();
    let ha = c.alloc().unwrap();
    let hb = c.alloc().unwrap(); // tail row 1
    let _p = c.mark_pending_retire(hb).unwrap(); // pinned tail
    c.free(ha); // hole at row 0
    c.compact();
    // Neither the pinned tail nor the hole can move this frame.
    assert_eq!(c.rows_in_use(), 2, "hole persists behind a pinned tail");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p pulsar_scenedb --lib compact_reports pinned_ 2>&1 | tail -5`
Expected: compile FAIL — `compact_report` not found

- [ ] **Step 3: Implement**

Rewrite `compact` as a delegating wrapper and port the existing loop into `compact_report`, with three changes (pinned-skip in the scan, pinned-stop in the trailing-pop loop, pinned-stop before the swap) and the move report:

```rust
    /// Frame-boundary swap-and-pop compaction (spec §4.4). Public form of
    /// [`compact_report`] without move observation.
    pub fn compact(&mut self) {
        self.compact_report(|_, _| {});
    }

    /// Compaction that reports every `(from_row, to_row)` move so the GPU
    /// layer can mark destination rows dirty (M2a §4). Pinned rows (in-flight
    /// retirement, M2a §5) are neither swapped away nor filled into, and a
    /// pinned tail stops the pop frontier: holes behind it persist until the
    /// pin clears — `retire()` runs before `compact()` at the boundary, so
    /// steady-state pins are already drained.
    pub(crate) fn compact_report(&mut self, mut on_move: impl FnMut(u32, u32)) {
        let mut len = self.page.len();
        let mut row = 0u32;
        while row < len {
            if self.liveness.is_live(row) || self.is_row_pinned(row) {
                row += 1;
                continue;
            }
            // Shrink trailing dead rows first (stop at pinned rows — they
            // cannot pop). set_dead keeps the ≥len-all-dead invariant that
            // M1b's GPU liveness upload relies on.
            while len > row + 1 && !self.liveness.is_live(len - 1) && !self.is_row_pinned(len - 1) {
                len -= 1;
                self.liveness.set_dead(len);
                self.page.pop_row();
            }
            if len == row + 1 {
                self.page.pop_row();
                break;
            }
            let last = len - 1;
            if !self.liveness.is_live(last) {
                // `last` is pinned (the shrink loop above consumed every
                // unpinned-dead tail row). Nothing past it can move or pop
                // this frame; the hole at `row` persists until unpin.
                break;
            }
            self.swap_rows(row, last);
            let moved_slot = self.page.column_slice::<u32>(0)[row as usize];
            self.registry.set_row(moved_slot, row);
            self.liveness.set_live(row);
            self.liveness.set_dead(last);
            on_move(last, row);
            len -= 1;
            self.page.pop_row();
            row += 1;
        }
    }
```

- [ ] **Step 4: Run tests (including all existing compaction tests)**

Run: `cargo test -p pulsar_scenedb --lib --tests 2>&1 | tail -4`
Expected: PASS — the full 117-test suite plus new tests; existing `compact_handles_multiple_holes_including_tail`, stress, and property tests are the regression oracle for the rewrite

- [ ] **Step 5: Commit**

```bash
git add crates/core/pulsar_scenedb/src/cell.rs
git commit -m "feat(scenedb): compact_report with skip-pinned rows + move reporting (M2a §4/§5)"
```

---

### Task 5: GPU — `EngineGpuContext` + headless device/readback test helpers

**Files:**
- Create: `crates/core/pulsar_scenedb/src/gpu/context.rs` (replace stub)
- Create: `crates/core/pulsar_scenedb/tests/gpu_store.rs` (helpers + smoke test)

**Interfaces:**
- Produces:
  - `EngineGpuContext::new(device: Arc<wgpu::Device>, queue: Arc<wgpu::Queue>) -> Self`, `.device() -> &Arc<wgpu::Device>`, `.queue() -> &Arc<wgpu::Queue>`.
  - Test helpers in `tests/gpu_store.rs`: `fn test_context() -> EngineGpuContext` and `fn readback(ctx: &EngineGpuContext, buf: &wgpu::Buffer, bytes: u64) -> Vec<u8>` — used by every subsequent GPU test.

- [ ] **Step 1: Implement `context.rs`**

```rust
use std::sync::Arc;

/// Engine-level owner of the wgpu device/queue (C0: the device outlives any
/// renderer). M2a defines the type and constructs it in tests; M4 wires the
/// engine (`engine_backend`) as the single runtime owner, above both SceneDB's
/// GPU layer and any renderer.
pub struct EngineGpuContext {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
}

impl EngineGpuContext {
    pub fn new(device: Arc<wgpu::Device>, queue: Arc<wgpu::Queue>) -> Self {
        Self { device, queue }
    }

    pub fn device(&self) -> &Arc<wgpu::Device> {
        &self.device
    }

    pub fn queue(&self) -> &Arc<wgpu::Queue> {
        &self.queue
    }
}
```

- [ ] **Step 2: Write the test harness + smoke test**

`tests/gpu_store.rs`:

```rust
//! M2a headless verification (design Rev 3 §9): real surfaceless wgpu device;
//! the test harness owns the `device.poll` pump.

use pulsar_scenedb::gpu::EngineGpuContext;
use std::sync::Arc;

fn test_context() -> EngineGpuContext {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .expect("no adapter — GPU tests need a local GPU");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("scenedb-m2a-test"),
        ..Default::default()
    }))
    .expect("device");
    EngineGpuContext::new(Arc::new(device), Arc::new(queue))
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
    let slice = staging.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("map"));
    ctx.device().poll(wgpu::PollType::Wait).expect("poll");
    let data = slice.get_mapped_range().to_vec();
    staging.unmap();
    data
}

#[test]
fn smoke_device_and_readback() {
    let ctx = test_context();
    let buf = ctx.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("smoke"),
        size: 16,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    ctx.queue().write_buffer(&buf, 0, &[7u8; 16]);
    assert_eq!(readback(&ctx, &buf, 16), vec![7u8; 16]);
}
```

(If the fork's wgpu 28 API differs on `PollType`/`request_device` arity, check `crates/core/pulsar_game/src/windowed_app.rs:170-195` — the in-tree reference for this exact fork rev.)

- [ ] **Step 3: Run**

Run: `cargo test -p pulsar_scenedb --features gpu --test gpu_store 2>&1 | tail -3`
Expected: PASS (1 test)

- [ ] **Step 4: Commit**

```bash
git add crates/core/pulsar_scenedb/src/gpu/context.rs crates/core/pulsar_scenedb/tests/gpu_store.rs
git commit -m "feat(scenedb): EngineGpuContext + headless wgpu test harness (M2a §2/§9)"
```

---

### Task 6: GPU — `SubmissionTracker`

**Files:**
- Create: `crates/core/pulsar_scenedb/src/gpu/tracker.rs` (replace stub)
- Test: add to `tests/gpu_store.rs`

**Interfaces:**
- Produces:
  - `SubmissionTracker::new() -> Self`
  - `next_serial(&self) -> u64` — monotonic, first call returns 1
  - `signal_submitted(&self, queue: &wgpu::Queue, serial: u64)` — registers `on_submitted_work_done`; when it fires, the completion watermark rises to ≥ serial
  - `completed(&self) -> u64` — highest confirmed-complete serial
  - `force_complete(&self, serial: u64)` — `#[doc(hidden)]` test hook (the "controllable submission-completion signal" of design §1/§9)

- [ ] **Step 1: Write failing tests** (append to `tests/gpu_store.rs`)

```rust
use pulsar_scenedb::gpu::SubmissionTracker;

#[test]
fn tracker_serials_are_monotonic_and_start_incomplete() {
    let t = SubmissionTracker::new();
    let s1 = t.next_serial();
    let s2 = t.next_serial();
    assert_eq!((s1, s2), (1, 2));
    assert_eq!(t.completed(), 0, "nothing complete before any signal");
    t.force_complete(s1);
    assert_eq!(t.completed(), 1);
    t.force_complete(0); // watermark never regresses
    assert_eq!(t.completed(), 1);
}

#[test]
fn tracker_real_gpu_completion_path() {
    let ctx = test_context();
    let t = SubmissionTracker::new();
    let s = t.next_serial();
    ctx.queue().submit([]); // empty submission is enough to complete
    t.signal_submitted(ctx.queue(), s);
    ctx.device().poll(wgpu::PollType::Wait).expect("poll");
    assert!(t.completed() >= s, "on_submitted_work_done raised the watermark");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p pulsar_scenedb --features gpu --test gpu_store tracker 2>&1 | tail -4`
Expected: compile FAIL — `SubmissionTracker` unresolved

- [ ] **Step 3: Implement `tracker.rs`**

```rust
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Monotonic submission serials + completion watermark (C6). Frame-counter
/// arithmetic is forbidden (spec §20.1): completion is only ever inferred
/// from `Queue::on_submitted_work_done`.
pub struct SubmissionTracker {
    next: AtomicU64,
    completed: Arc<AtomicU64>,
}

impl SubmissionTracker {
    pub fn new() -> Self {
        Self { next: AtomicU64::new(1), completed: Arc::new(AtomicU64::new(0)) }
    }

    /// Reserve the serial for the next submission batch.
    pub fn next_serial(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed)
    }

    /// Register completion for work submitted up to `serial`. The queue
    /// timeline is FIFO: when this callback fires, all work ≤ serial is done.
    pub fn signal_submitted(&self, queue: &wgpu::Queue, serial: u64) {
        let completed = Arc::clone(&self.completed);
        queue.on_submitted_work_done(move || {
            completed.fetch_max(serial, Ordering::AcqRel);
        });
    }

    /// Highest serial confirmed complete by the GPU.
    pub fn completed(&self) -> u64 {
        self.completed.load(Ordering::Acquire)
    }

    /// Test hook: the controllable completion signal (design §9) standing in
    /// for real GPU timing in retirement-invariant tests.
    #[doc(hidden)]
    pub fn force_complete(&self, serial: u64) {
        self.completed.fetch_max(serial, Ordering::AcqRel);
    }
}

impl Default for SubmissionTracker {
    fn default() -> Self {
        Self::new()
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p pulsar_scenedb --features gpu --test gpu_store 2>&1 | tail -3`
Expected: PASS (3 tests)

- [ ] **Step 5: Commit**

```bash
git add crates/core/pulsar_scenedb/src/gpu/tracker.rs crates/core/pulsar_scenedb/tests/gpu_store.rs
git commit -m "feat(scenedb): SubmissionTracker — serials + on_submitted_work_done watermark (C6)"
```

---

### Task 7: GPU — `SceneBuffer<T>` with dirty-bitmask delta-sync

**Files:**
- Create: `crates/core/pulsar_scenedb/src/gpu/buffer.rs` (replace stub)
- Test: add to `tests/gpu_store.rs`

**Interfaces:**
- Consumes: `crate::gpu::as_bytes`, `crate::page::Pod`.
- Produces:
  - `SceneBuffer<T: Pod>::new(device: &wgpu::Device, label: &str, capacity: u32) -> Self` — one persistent row-indexed SSBO (`STORAGE | COPY_DST | COPY_SRC`), size = `capacity * size_of::<T>()`, never reallocated.
  - `mark_row_dirty(&self, row: u32)` — atomic; callable through `&self`.
  - `sync(&self, queue: &wgpu::Queue, cpu: &[T]) -> SyncStats` — streaming coalescer: contiguous dirty rows become single `write_buffer` calls; clears all bits; panics if `cpu.len() > capacity` (SSBOs never grow).
  - `buffer(&self) -> &wgpu::Buffer`, `capacity(&self) -> u32`.
  - `struct SyncStats { pub ranges: u32, pub bytes: u64 }` — the delta-minimality instrument.
  - Design note (Rev 3 refinement of §4): ranges are streamed directly to `write_buffer` as they close, so no range list — and no scratchpad — is needed in the sync path; zero mid-frame heap allocation holds by construction. `Scratchpad::get_u64` (Task 2) remains for the M2b harvest path.

- [ ] **Step 1: Write failing tests** (append to `tests/gpu_store.rs`)

```rust
use pulsar_scenedb::gpu::SceneBuffer;

fn mat(seed: f32) -> [f32; 16] {
    core::array::from_fn(|i| seed + i as f32)
}

fn as_f32s(bytes: &[u8]) -> Vec<f32> {
    bytes.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect()
}

#[test]
fn delta_correctness_gpu_bytes_match_cpu_column() {
    let ctx = test_context();
    let buf = SceneBuffer::<[f32; 16]>::new(ctx.device(), "instances", 8);
    let cpu: Vec<[f32; 16]> = (0..4).map(|i| mat(i as f32 * 100.0)).collect();
    for row in 0..4 {
        buf.mark_row_dirty(row);
    }
    let stats = buf.sync(ctx.queue(), &cpu);
    assert_eq!(stats.ranges, 1, "4 contiguous dirty rows coalesce into one write");
    assert_eq!(stats.bytes, 4 * 64);
    let gpu = as_f32s(&readback(&ctx, buf.buffer(), 4 * 64));
    let expect: Vec<f32> = cpu.iter().flatten().copied().collect();
    assert_eq!(gpu, expect, "GPU bytes == CPU transform column, by row");
}

#[test]
fn delta_minimality_clean_frame_writes_nothing_and_scattered_rows_coalesce() {
    let ctx = test_context();
    let buf = SceneBuffer::<[f32; 16]>::new(ctx.device(), "instances", 64);
    let cpu: Vec<[f32; 16]> = (0..64).map(|i| mat(i as f32)).collect();
    // Warm upload.
    for row in 0..64 {
        buf.mark_row_dirty(row);
    }
    buf.sync(ctx.queue(), &cpu);
    // Zero-mutation frame writes nothing.
    let stats = buf.sync(ctx.queue(), &cpu);
    assert_eq!((stats.ranges, stats.bytes), (0, 0), "clean frame is free");
    // Scattered dirty rows: {3}, {10,11,12}, {60} → exactly 3 ranges.
    for row in [3u32, 10, 11, 12, 60] {
        buf.mark_row_dirty(row);
    }
    let stats = buf.sync(ctx.queue(), &cpu);
    assert_eq!(stats.ranges, 3, "contiguous runs coalesce; no clean-row uploads");
    assert_eq!(stats.bytes, 5 * 64);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p pulsar_scenedb --features gpu --test gpu_store delta 2>&1 | tail -4`
Expected: compile FAIL — `SceneBuffer` unresolved

- [ ] **Step 3: Implement `buffer.rs`**

```rust
use crate::page::Pod;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, Ordering};

/// Delta-sync instrumentation: how many `write_buffer` ranges and bytes the
/// last sync issued. The delta-minimality gates assert on this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncStats {
    pub ranges: u32,
    pub bytes: u64,
}

/// One persistent **row-indexed** scene SSBO plus its row dirty bitmask
/// (M2a §3/§4). Generic over the C5 element type. Allocated once at capacity;
/// never reallocates.
pub struct SceneBuffer<T: Pod> {
    buf: wgpu::Buffer,
    capacity: u32,
    dirty: Vec<AtomicU64>,
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
        let words = capacity.div_ceil(64) as usize;
        Self {
            buf,
            capacity,
            dirty: (0..words).map(|_| AtomicU64::new(0)).collect(),
            _elem: PhantomData,
        }
    }

    /// Mark a row for re-upload (writes and compaction moves). Atomic — the
    /// write window may be threaded.
    #[inline]
    pub fn mark_row_dirty(&self, row: u32) {
        debug_assert!(row < self.capacity, "row {row} beyond SSBO capacity {}", self.capacity);
        self.dirty[(row / 64) as usize].fetch_or(1u64 << (row % 64), Ordering::Relaxed);
    }

    #[inline]
    fn is_dirty(&self, row: u32) -> bool {
        self.dirty[(row / 64) as usize].load(Ordering::Relaxed) & (1u64 << (row % 64)) != 0
    }

    /// Coalesce contiguous dirty rows into minimal `write_buffer` ranges,
    /// upload from the CPU column (byte-identical layout, C5 — a straight
    /// memcpy), clear all bits. Ranges stream directly to the queue: no range
    /// list, no mid-frame heap allocation. A zero-mutation frame writes
    /// nothing. Rows ≥ `cpu.len()` (popped by compaction) are only cleared.
    pub fn sync(&self, queue: &wgpu::Queue, cpu: &[T]) -> SyncStats {
        assert!(
            cpu.len() as u32 <= self.capacity,
            "CPU column ({}) exceeds SSBO capacity ({}) — scene buffers never reallocate",
            cpu.len(),
            self.capacity
        );
        let stride = std::mem::size_of::<T>() as u64;
        let n = cpu.len() as u32;
        let mut stats = SyncStats { ranges: 0, bytes: 0 };
        let mut run_start: Option<u32> = None;
        for row in 0..n {
            match (self.is_dirty(row), run_start) {
                (true, None) => run_start = Some(row),
                (false, Some(start)) => {
                    self.flush(queue, cpu, start, row, stride, &mut stats);
                    run_start = None;
                }
                _ => {}
            }
        }
        if let Some(start) = run_start {
            self.flush(queue, cpu, start, n, stride, &mut stats);
        }
        for word in &self.dirty {
            word.store(0, Ordering::Relaxed);
        }
        stats
    }

    fn flush(
        &self,
        queue: &wgpu::Queue,
        cpu: &[T],
        start: u32,
        end: u32,
        stride: u64,
        stats: &mut SyncStats,
    ) {
        let bytes = super::as_bytes(&cpu[start as usize..end as usize]);
        queue.write_buffer(&self.buf, start as u64 * stride, bytes);
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
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p pulsar_scenedb --features gpu --test gpu_store 2>&1 | tail -3`
Expected: PASS (5 tests)

- [ ] **Step 5: Commit**

```bash
git add crates/core/pulsar_scenedb/src/gpu/buffer.rs crates/core/pulsar_scenedb/tests/gpu_store.rs
git commit -m "feat(scenedb): SceneBuffer<T> row-indexed SSBO with coalescing delta-sync (M2a §4)"
```

---

### Task 8: GPU — `GenerationBuffer` (slot-indexed)

**Files:**
- Create: `crates/core/pulsar_scenedb/src/gpu/generation.rs` (replace stub)
- Test: add to `tests/gpu_store.rs`

**Interfaces:**
- Produces:
  - `GenerationBuffer::new(device: &wgpu::Device, max_slots: u32) -> Self` — `u32` per slot, sized to **max slots ever** (§8), `STORAGE | COPY_DST | COPY_SRC`.
  - `write(&self, queue: &wgpu::Queue, slot: u32, gen: u32)` — single-slot retirement write; panics beyond `max_slots`.
  - `rebuild(&self, queue: &wgpu::Queue, generations: &[u32])` — bulk upload from `HandleRegistry::generations()` (init + Test 14); tombstones (`u32::MAX`) upload as-is.
  - `buffer(&self) -> &wgpu::Buffer`, `max_slots(&self) -> u32`.

- [ ] **Step 1: Write failing test** (append to `tests/gpu_store.rs`)

```rust
use pulsar_scenedb::gpu::GenerationBuffer;

fn as_u32s(bytes: &[u8]) -> Vec<u32> {
    bytes.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect()
}

#[test]
fn generation_buffer_write_and_rebuild() {
    let ctx = test_context();
    let gens = GenerationBuffer::new(ctx.device(), 4);
    gens.rebuild(ctx.queue(), &[1, 5, u32::MAX, 2]);
    assert_eq!(as_u32s(&readback(&ctx, gens.buffer(), 16)), vec![1, 5, u32::MAX, 2]);
    gens.write(ctx.queue(), 1, 6); // retirement bumps slot 1
    assert_eq!(as_u32s(&readback(&ctx, gens.buffer(), 16)), vec![1, 6, u32::MAX, 2]);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p pulsar_scenedb --features gpu --test gpu_store generation 2>&1 | tail -4`
Expected: compile FAIL

- [ ] **Step 3: Implement `generation.rs`**

```rust
/// The lone **slot-indexed** buffer (M2a §3): mirrors
/// `HandleRegistry::generations()` so the GPU validates handles against VRAM
/// exclusively (C6). Sized to max slots ever allocated — can exceed live
/// count after churn; `u32::MAX` tombstones upload as-is and are never
/// reissued.
pub struct GenerationBuffer {
    buf: wgpu::Buffer,
    max_slots: u32,
}

impl GenerationBuffer {
    pub fn new(device: &wgpu::Device, max_slots: u32) -> Self {
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scenedb-generations"),
            size: max_slots as u64 * 4,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        Self { buf, max_slots }
    }

    /// Retirement write: the new generation must land here BEFORE the slot
    /// returns to the free pool (C6) — `GpuStore::retire` owns that ordering.
    pub fn write(&self, queue: &wgpu::Queue, slot: u32, gen: u32) {
        assert!(slot < self.max_slots, "slot {slot} beyond generation-buffer capacity {}", self.max_slots);
        queue.write_buffer(&self.buf, slot as u64 * 4, &gen.to_le_bytes());
    }

    /// Bulk upload from the CPU-authoritative registry (init / Test 14).
    pub fn rebuild(&self, queue: &wgpu::Queue, generations: &[u32]) {
        assert!(generations.len() as u32 <= self.max_slots);
        queue.write_buffer(&self.buf, 0, super::as_bytes(generations));
    }

    pub fn buffer(&self) -> &wgpu::Buffer {
        &self.buf
    }

    pub fn max_slots(&self) -> u32 {
        self.max_slots
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p pulsar_scenedb --features gpu --test gpu_store 2>&1 | tail -3`
Expected: PASS (6 tests)

- [ ] **Step 5: Commit**

```bash
git add crates/core/pulsar_scenedb/src/gpu/generation.rs crates/core/pulsar_scenedb/tests/gpu_store.rs
git commit -m "feat(scenedb): slot-indexed GenerationBuffer (M2a §3, C6)"
```

---

### Task 9: GPU — `GpuStore` orchestration (writer, deferred free, retire → compact → sync)

**Files:**
- Create: `crates/core/pulsar_scenedb/src/gpu/store.rs` (replace stub)
- Test: add to `tests/gpu_store.rs`

**Interfaces:**
- Consumes: everything from Tasks 3–8 — `CellStorage::{mark_pending_retire, commit_retire, compact_report, column_for, column_for_mut, row_of}` (`pub(crate)`, same crate), `PendingRetire`, `SceneBuffer`, `GenerationBuffer`, `SubmissionTracker`, `EngineGpuContext`.
- Produces:
  - `struct GpuStoreConfig { pub max_rows: u32, pub max_slots: u32 }`
  - `GpuStore::new(ctx: &EngineGpuContext, cfg: GpuStoreConfig) -> Self` (clones the Arcs)
  - `tracker(&self) -> &SubmissionTracker`
  - `write_transform(&self, cell: &mut CellStorage, handle: Handle, m: &[f32; 16]) -> bool` — THE mutation path for the mirrored column: writes the core column and sets the dirty bit in one operation (§4). Returns false for stale handles. Cell must have a `[f32; 16]` column.
  - `free_deferred(&mut self, cell: &mut CellStorage, handle: Handle, serial: u64) -> bool` — `mark_pending_retire` + enqueue `(pending, serial)` (§5 flow step 1).
  - `retire(&mut self, cell: &mut CellStorage) -> u32` — drains entries with `serial <= tracker.completed()`: generation-buffer write **then** `commit_retire` (§5 flow step 3); returns drained count.
  - `compact(&mut self, cell: &mut CellStorage)` — `compact_report` wrapper marking each destination row dirty (§4).
  - `sync(&mut self, cell: &CellStorage) -> SyncStats` — transform-column upload.
  - `transform_buffer(&self) -> &wgpu::Buffer`, `generation_buffer(&self) -> &wgpu::Buffer` — the read-only bind surface for a future Helio.
  - Note: design §7's `RetirementEngine` is folded into `GpuStore` as the `pending: VecDeque<QueuedRetire>` + `retire()` drain — with one crate there is no consumer for it as a standalone type; its behavior is gated directly by Test 6 (Task 10).
  - Frame-phase debug enforcement (§6): internal `phase: Phase` enum {`Write`, `Retired`, `Compacted`}; `retire` asserts `Write→Retired`, `compact` asserts `Retired→Compacted`, `sync` asserts `Compacted`→ back to `Write`; `write_transform`/`free_deferred` assert `Write`.

- [ ] **Step 1: Write failing tests** (append to `tests/gpu_store.rs`)

```rust
use pulsar_scenedb::gpu::{GpuStore, GpuStoreConfig};
use pulsar_scenedb::{CellStorage, CellType, TypeToken};

fn transform_cell(capacity: u32) -> CellStorage {
    let ct = CellType::new("m2a-instance")
        .with(TypeToken::of::<[f32; 16]>())
        .build()
        .unwrap();
    CellStorage::from_cell_type(&ct, capacity).unwrap()
}

fn store_and_cell(ctx: &EngineGpuContext) -> (GpuStore, CellStorage) {
    (
        GpuStore::new(ctx, GpuStoreConfig { max_rows: 64, max_slots: 64 }),
        transform_cell(64),
    )
}

/// Run one frame boundary: retire → compact → sync.
fn frame_boundary(store: &mut GpuStore, cell: &mut CellStorage) -> pulsar_scenedb::gpu::SyncStats {
    store.retire(cell);
    store.compact(cell);
    store.sync(cell)
}

#[test]
fn write_transform_is_the_single_mutation_path() {
    let ctx = test_context();
    let (mut store, mut cell) = store_and_cell(&ctx);
    let h = cell.alloc().unwrap();
    assert!(store.write_transform(&mut cell, h, &mat(9.0)));
    frame_boundary(&mut store, &mut cell);
    let row = cell.row_of(h).unwrap() as usize;
    let gpu = as_f32s(&readback(&ctx, store.transform_buffer(), 64 * 64));
    assert_eq!(&gpu[row * 16..row * 16 + 16], &mat(9.0));
    // Stale handle rejected.
    let dead = cell.alloc().unwrap();
    cell.free(dead);
    assert!(!store.write_transform(&mut cell, dead, &mat(0.0)));
}

#[test]
fn compaction_move_is_resynced_and_generation_buffer_matches_registry() {
    let ctx = test_context();
    let (mut store, mut cell) = store_and_cell(&ctx);
    let ha = cell.alloc().unwrap();
    let hb = cell.alloc().unwrap();
    let hc = cell.alloc().unwrap();
    for (h, s) in [(ha, 1.0f32), (hb, 2.0), (hc, 3.0)] {
        store.write_transform(&mut cell, h, &mat(s));
    }
    frame_boundary(&mut store, &mut cell);
    // Free hb via the deferred path; complete its serial; boundary again:
    let serial = store.tracker().next_serial();
    assert!(store.free_deferred(&mut cell, hb, serial));
    store.tracker().force_complete(serial);
    let stats = frame_boundary(&mut store, &mut cell); // retire → compact (hc moves) → sync
    assert!(stats.ranges >= 1, "the compaction move was re-uploaded");
    // Moved row's GPU bytes are correct at its NEW index:
    let hc_row = cell.row_of(hc).unwrap() as usize;
    let gpu = as_f32s(&readback(&ctx, store.transform_buffer(), 64 * 64));
    assert_eq!(&gpu[hc_row * 16..hc_row * 16 + 16], &mat(3.0));
    // Generation buffer matches the registry for every allocated slot:
    let regs = cell.registry().generations().to_vec();
    let gpu_gens = as_u32s(&readback(&ctx, store.generation_buffer(), 64 * 4));
    assert_eq!(&gpu_gens[..regs.len()], &regs[..]);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p pulsar_scenedb --features gpu --test gpu_store write_transform compaction_move 2>&1 | tail -4`
Expected: compile FAIL — `GpuStore` unresolved

- [ ] **Step 3: Implement `store.rs`**

```rust
use super::{EngineGpuContext, GenerationBuffer, SceneBuffer, SubmissionTracker, SyncStats};
use crate::cell::{CellStorage, PendingRetire};
use crate::handle::Handle;
use std::collections::VecDeque;
use std::sync::Arc;

/// Fixed store capacities (SSBOs never reallocate — exceeding them is a hard
/// error at the call site, §8).
#[derive(Debug, Clone, Copy)]
pub struct GpuStoreConfig {
    pub max_rows: u32,
    pub max_slots: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Write,
    Retired,
    Compacted,
}

struct QueuedRetire {
    pending: PendingRetire,
    serial: u64,
}

/// The SceneDB-owned device-side store (M2a §7): persistent scene SSBOs,
/// the mirrored-column writer, delta-sync, and the retirement drain.
/// Constructed on the engine-level device context (C0).
pub struct GpuStore {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    transforms: SceneBuffer<[f32; 16]>,
    generations: GenerationBuffer,
    tracker: SubmissionTracker,
    pending: VecDeque<QueuedRetire>,
    phase: Phase,
}

impl GpuStore {
    pub fn new(ctx: &EngineGpuContext, cfg: GpuStoreConfig) -> Self {
        Self {
            device: Arc::clone(ctx.device()),
            queue: Arc::clone(ctx.queue()),
            transforms: SceneBuffer::new(ctx.device(), "scenedb-instances", cfg.max_rows),
            generations: GenerationBuffer::new(ctx.device(), cfg.max_slots),
            tracker: SubmissionTracker::new(),
            pending: VecDeque::new(),
            phase: Phase::Write,
        }
    }

    pub fn tracker(&self) -> &SubmissionTracker {
        &self.tracker
    }

    /// The single mutation path for the GPU-mirrored transform column (§4):
    /// writes the core column AND sets the row's dirty bit in one operation.
    /// False for stale/invalid handles.
    pub fn write_transform(&self, cell: &mut CellStorage, handle: Handle, m: &[f32; 16]) -> bool {
        debug_assert_eq!(self.phase, Phase::Write, "mutation outside the write window");
        let Some(row) = cell.row_of(handle) else {
            return false;
        };
        let col = cell
            .column_for_mut::<[f32; 16]>()
            .expect("cell has no [f32; 16] transform column");
        col[row as usize] = *m;
        self.transforms.mark_row_dirty(row);
        true
    }

    /// §5 flow step 1: liveness-dead + pinned + enqueued against `serial`.
    /// Registry and GPU buffers unchanged until the serial completes.
    pub fn free_deferred(&mut self, cell: &mut CellStorage, handle: Handle, serial: u64) -> bool {
        debug_assert_eq!(self.phase, Phase::Write, "free_deferred outside the write window");
        let Some(pending) = cell.mark_pending_retire(handle) else {
            return false;
        };
        self.pending.push_back(QueuedRetire { pending, serial });
        true
    }

    /// §5 flow step 3, frame boundary, runs FIRST: for every queued entry
    /// whose serial is complete, write the new generation to VRAM, then
    /// commit in the registry — the gen bump reaches the GPU before the slot
    /// can be re-allocated (C6). Returns the number of slots retired.
    pub fn retire(&mut self, cell: &mut CellStorage) -> u32 {
        debug_assert_eq!(self.phase, Phase::Write, "retire must open the frame boundary");
        self.phase = Phase::Retired;
        let done = self.tracker.completed();
        let mut drained = 0;
        while let Some(front) = self.pending.front() {
            if front.serial > done {
                break; // FIFO serials: everything behind is also incomplete
            }
            let QueuedRetire { pending, .. } = self.pending.pop_front().unwrap();
            self.generations.write(&self.queue, pending.slot, pending.next_gen);
            cell.commit_retire(pending);
            drained += 1;
        }
        drained
    }

    /// Frame-boundary compaction (§4): every moved row's destination is
    /// marked dirty so the next sync re-uploads it.
    pub fn compact(&mut self, cell: &mut CellStorage) {
        debug_assert_eq!(self.phase, Phase::Retired, "compact must follow retire");
        self.phase = Phase::Compacted;
        cell.compact_report(|_from, to| self.transforms.mark_row_dirty(to));
    }

    /// Frame-boundary upload (§4): coalesced dirty-row write of the transform
    /// column; closes the boundary (next phase is the write window).
    pub fn sync(&mut self, cell: &CellStorage) -> SyncStats {
        debug_assert_eq!(self.phase, Phase::Compacted, "sync must follow compact");
        self.phase = Phase::Write;
        let col = cell
            .column_for::<[f32; 16]>()
            .expect("cell has no [f32; 16] transform column");
        self.transforms.sync(&self.queue, &col[..cell.rows_in_use() as usize])
    }

    /// Test 14: build a fresh store on a fresh device purely from the
    /// CPU-authoritative columns (no GPU-only state exists to lose).
    pub fn rebuild_from(ctx: &EngineGpuContext, cfg: GpuStoreConfig, cell: &CellStorage) -> Self {
        let mut store = Self::new(ctx, cfg);
        let rows = cell.rows_in_use();
        for row in 0..rows {
            store.transforms.mark_row_dirty(row);
        }
        let col = cell
            .column_for::<[f32; 16]>()
            .expect("cell has no [f32; 16] transform column");
        store.transforms.sync(&store.queue, &col[..rows as usize]);
        store.generations.rebuild(&store.queue, cell.registry().generations());
        store
    }

    pub fn transform_buffer(&self) -> &wgpu::Buffer {
        self.transforms.buffer()
    }

    pub fn generation_buffer(&self) -> &wgpu::Buffer {
        self.generations.buffer()
    }
}
```

Note: `column_for::<[f32; 16]>()` returns the full-capacity column slice; `sync`/`rebuild_from` clamp to `rows_in_use()`. `cell.rs`'s `PendingRetire` and the `pub(crate)` methods are visible here because `gpu` is a module of the same crate — this is exactly the Rev 3 point.

- [ ] **Step 4: Run tests**

Run: `cargo test -p pulsar_scenedb --features gpu --test gpu_store 2>&1 | tail -3`
Expected: PASS (8 tests)

- [ ] **Step 5: Run core suite for regressions**

Run: `cargo test -p pulsar_scenedb --lib --tests 2>&1 | tail -3`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/core/pulsar_scenedb/src/gpu/store.rs crates/core/pulsar_scenedb/tests/gpu_store.rs
git commit -m "feat(scenedb): GpuStore — mirrored-column writer, deferred free, retire→compact→sync (M2a §5/§6)"
```

---

### Task 10: Gate — Test 6 host-side (retirement invariant)

**Files:**
- Test: add to `crates/core/pulsar_scenedb/tests/gpu_store.rs`

**Interfaces:**
- Consumes: Task 9's `GpuStore` + Task 6's `force_complete` (the controllable completion signal).

- [ ] **Step 1: Write the gate test**

```rust
/// Test 6 host-side (design §9): the retirement invariant. A slot is never
/// reissued, and its row never reclaimed, before its serial completes and the
/// new generation is in the VRAM buffer; the handle stays row-resolvable but
/// harvest-dead during the window; afterwards it is rejected. No UB.
#[test]
fn test6_retirement_invariant() {
    let ctx = test_context();
    let (mut store, mut cell) = store_and_cell(&ctx);
    let h = cell.alloc().unwrap();
    store.write_transform(&mut cell, h, &mat(42.0));
    frame_boundary(&mut store, &mut cell);

    let row = cell.row_of(h).unwrap();
    let serial = store.tracker().next_serial();
    assert!(store.free_deferred(&mut cell, h, serial));

    // Serial INCOMPLETE: boundary runs but nothing retires.
    assert_eq!(store.retire(&mut cell), 0, "incomplete serial must not retire");
    store.compact(&mut cell);
    assert_eq!(cell.row_of(h), Some(row), "row not compacted while pinned");
    store.sync(&cell);
    let h2 = cell.alloc().unwrap();
    assert_ne!(h2.index(), h.index(), "slot not reissued while in flight");
    assert_eq!(cell.live_count(), 1, "pending row absent from harvest (only h2 lives)");

    // Serial COMPLETES: the drain writes VRAM gen BEFORE pooling the slot.
    store.tracker().force_complete(serial);
    assert_eq!(store.retire(&mut cell), 1);
    let gpu_gens = as_u32s(&readback(&ctx, store.generation_buffer(), 64 * 4));
    assert_eq!(gpu_gens[h.index() as usize], h.generation() + 1, "VRAM generation bumped");
    store.compact(&mut cell);
    store.sync(&cell);
    assert_eq!(cell.row_of(h), None, "old handle rejected after retirement");
    let h3 = cell.alloc().unwrap();
    assert_eq!(h3.index(), h.index(), "slot recycled only now");
    assert_eq!(h3.generation(), h.generation() + 1);
}
```

- [ ] **Step 2: Run**

Run: `cargo test -p pulsar_scenedb --features gpu --test gpu_store test6 2>&1 | tail -3`
Expected: PASS. If it fails, the bug is in Task 3/9 ordering — fix there, never weaken this gate.

- [ ] **Step 3: Commit**

```bash
git add crates/core/pulsar_scenedb/tests/gpu_store.rs
git commit -m "test(scenedb): Test 6 host-side retirement-invariant gate (C6)"
```

---

### Task 11: Gate — Test 14 (device-loss re-materialization)

**Files:**
- Test: add to `crates/core/pulsar_scenedb/tests/gpu_store.rs`

**Interfaces:**
- Consumes: Task 9's `GpuStore::rebuild_from`.

- [ ] **Step 1: Write the gate test**

```rust
/// Test 14 (C0 companion gate): drop the device + every buffer; create a
/// fresh device; rebuild the GPU side purely from Layer-1's authoritative
/// columns. Byte-identical recovery proves no GPU-only/derived scene state
/// exists (design §3 "derived data is not stored").
#[test]
fn test14_device_loss_rematerialization() {
    let cfg = GpuStoreConfig { max_rows: 64, max_slots: 64 };
    let mut cell = transform_cell(64);

    // Populate with churn so slot/row spaces diverge: alloc 8, retire 2.
    let ctx1 = test_context();
    let mut store = GpuStore::new(&ctx1, cfg);
    let hs: Vec<_> = (0..8).map(|_| cell.alloc().unwrap()).collect();
    for (i, &h) in hs.iter().enumerate() {
        store.write_transform(&mut cell, h, &mat(i as f32 * 10.0));
    }
    frame_boundary(&mut store, &mut cell);
    for &h in &[hs[2], hs[5]] {
        let s = store.tracker().next_serial();
        store.free_deferred(&mut cell, h, s);
        store.tracker().force_complete(s);
    }
    frame_boundary(&mut store, &mut cell);
    let before_rows = readback(&ctx1, store.transform_buffer(), 64 * 64);
    let before_gens = readback(&ctx1, store.generation_buffer(), 64 * 4);

    // Device loss: drop the store, then the entire device.
    drop(store);
    drop(ctx1);

    // Fresh device; rebuild from CPU-authoritative columns only.
    let ctx2 = test_context();
    let rebuilt = GpuStore::rebuild_from(&ctx2, cfg, &cell);
    let after_rows = readback(&ctx2, rebuilt.transform_buffer(), 64 * 64);
    let after_gens = readback(&ctx2, rebuilt.generation_buffer(), 64 * 4);

    let n = cell.rows_in_use() as usize * 64;
    assert_eq!(after_rows[..n], before_rows[..n], "row data byte-identical");
    let s = cell.registry().generations().len() * 4;
    assert_eq!(after_gens[..s], before_gens[..s], "generations byte-identical (incl. bumps)");
}
```

- [ ] **Step 2: Run**

Run: `cargo test -p pulsar_scenedb --features gpu --test gpu_store test14 2>&1 | tail -3`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add crates/core/pulsar_scenedb/tests/gpu_store.rs
git commit -m "test(scenedb): Test 14 device-loss re-materialization gate (C0 companion)"
```

---

### Task 12: Gate — Test 3 (WGSL byte-layout, naga reflection)

**Files:**
- Create: `crates/core/pulsar_scenedb/tests/gpu_layout.rs`

**Interfaces:**
- Consumes: `naga` dev-dep (Task 1). No device needed.
- Produces: a reusable `wgsl_struct_layout(src, name) -> (u32, Vec<(String, u32)>)` harness M3 extends for material/mesh-metadata structs.

- [ ] **Step 1: Write the test**

```rust
//! Test 3 (C5): host struct offsets vs naga reflection of the WGSL structs,
//! byte-exact. M2a scope: instance (64 B mat4) + generation (u32/slot). The
//! material/mesh-metadata rows follow their M3/M2b definitions.

/// The WGSL the (future, M3) shaders will declare for M2a's two buffers.
const M2A_WGSL: &str = r#"
struct Instance {
    transform: mat4x4<f32>,
}
@group(0) @binding(0) var<storage, read> instances: array<Instance>;
@group(0) @binding(1) var<storage, read> generations: array<u32>;
"#;

/// Reflect (size, [(member_name, offset)]) for a named struct in WGSL source.
fn wgsl_struct_layout(src: &str, name: &str) -> (u32, Vec<(String, u32)>) {
    let module = naga::front::wgsl::parse_str(src).expect("valid WGSL");
    let mut layouter = naga::proc::Layouter::default();
    layouter.update(module.to_ctx()).expect("layout");
    let (handle, ty) = module
        .types
        .iter()
        .find(|(_, t)| t.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("struct {name} not found"));
    let naga::TypeInner::Struct { members, .. } = &ty.inner else {
        panic!("{name} is not a struct");
    };
    let size = layouter[handle].size;
    let offsets = members
        .iter()
        .map(|m| (m.name.clone().unwrap_or_default(), m.offset))
        .collect();
    (size, offsets)
}

#[test]
fn test3_instance_struct_is_byte_exact() {
    let (size, members) = wgsl_struct_layout(M2A_WGSL, "Instance");
    // Host element: [f32; 16], 64 bytes, transform at offset 0 (C5).
    assert_eq!(size, 64, "WGSL Instance size == size_of::<[f32; 16]>()");
    assert_eq!(size as usize, std::mem::size_of::<[f32; 16]>());
    assert_eq!(members, vec![("transform".to_string(), 0)]);
}

#[test]
fn test3_generation_element_is_u32() {
    // array<u32> element: 4 bytes, matching HandleRegistry::generations().
    let module = naga::front::wgsl::parse_str(M2A_WGSL).expect("valid WGSL");
    let mut layouter = naga::proc::Layouter::default();
    layouter.update(module.to_ctx()).expect("layout");
    let (handle, _) = module
        .types
        .iter()
        .find(|(_, t)| matches!(t.inner, naga::TypeInner::Scalar(s) if s == naga::Scalar::U32))
        .expect("u32 type present");
    assert_eq!(layouter[handle].size, 4);
    assert_eq!(layouter[handle].size as usize, std::mem::size_of::<u32>());
}
```

(If the fork's naga API differs — e.g. `module.to_ctx()` or `naga::Scalar` naming — consult `crates/graphics/wgpu/naga/src/proc/layouter.rs` in the pinned submodule; adjust the harness, not the assertions.)

- [ ] **Step 2: Run**

Run: `cargo test -p pulsar_scenedb --features gpu --test gpu_layout 2>&1 | tail -3`
Expected: PASS (2 tests)

- [ ] **Step 3: Commit**

```bash
git add crates/core/pulsar_scenedb/tests/gpu_layout.rs
git commit -m "test(scenedb): Test 3 WGSL byte-layout harness via naga reflection (C5)"
```

---

### Task 13: Docs wrap-up + full verification

**Files:**
- Modify: `crates/core/pulsar_scenedb/src/lib.rs` (crate docs)
- Modify: `crates/core/pulsar_scenedb/README.md`

**Interfaces:** none (documentation).

- [ ] **Step 1: Update the crate-level docs**

In `lib.rs`'s module doc, replace the "Layer 2 orchestration is M2" status line with M2a status and add a bullet for the gpu module:

```rust
//! - `gpu` (feature `gpu`) — M2a GPU-resident store: `EngineGpuContext`,
//!   `SceneBuffer<T>` row-indexed SSBOs with coalescing delta-sync,
//!   slot-indexed `GenerationBuffer`, `SubmissionTracker`, and `GpuStore`'s
//!   pin-by-serial retirement (`retire → compact → sync`). The core stays
//!   graphics-free (C0); CI guards `--no-default-features`.
//!
//! Milestone status: M1 (Layer 1) complete; M2a (GPU store, delta-sync,
//! retirement) complete — verified headless by Tests 3, 6 (host), and 14.
//! M2b orchestration/streaming and the M3 Helio inversion follow.
```

- [ ] **Step 2: Update README.md**

Add an "M2a — GPU layer" section stating: the `gpu` feature, the run commands (`cargo test -p pulsar_scenedb --features gpu --test gpu_store` needs a local GPU; CI runs core only), and the C0 graphics-free guard command.

- [ ] **Step 3: Full verification matrix**

Run: `cargo check -p pulsar_scenedb --no-default-features`
Expected: green

Run: `cargo test -p pulsar_scenedb --lib --tests 2>&1 | tail -4`
Expected: PASS (117 pre-existing + new core tests)

Run: `cargo test -p pulsar_scenedb --features gpu --test gpu_store --test gpu_layout 2>&1 | tail -4`
Expected: PASS (all GPU + layout tests)

Run: `cargo check --workspace --exclude pulsar_docs --all-targets 2>&1 | tail -3`
Expected: green (no workspace regressions)

- [ ] **Step 4: Commit**

```bash
git add crates/core/pulsar_scenedb/src/lib.rs crates/core/pulsar_scenedb/README.md
git commit -m "docs(scenedb): M2a crate docs + README — GPU layer complete"
```

---

## Deferred (per design §10 — do NOT implement here)

Material buffer (M3), mesh-metadata/geometry/cluster buffers + load-time upload (M2b), bindless textures (M3), streaming grid/harvest/DEI/HLOD (M2b), compile-time phase-guard types (M2b), wiring `EngineGpuContext` into `engine_backend` (M4), re-wiring `query_aabb`/`query_frustum` onto `Scratchpad::get_u64` (M2b harvest).

## Verification (end-to-end)

The four commands in Task 13 Step 3 are the acceptance gate. The three named contract gates — Test 3 (`gpu_layout`), Test 6 host-side (`test6_retirement_invariant`), Test 14 (`test14_device_loss_rematerialization`) — must all pass on a real device. Test 13 (renderer teardown) is explicitly M3: it needs a Helio instance to drop; M2a establishes the ownership that makes it passable.
