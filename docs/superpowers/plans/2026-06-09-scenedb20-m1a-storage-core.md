# SceneDB 2.0 — Milestone 1a: Storage Core (`pulsar_scenedb`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Seed `crates/pulsar_scenedb` from `pulsar_ecs` and build the Layer 1 storage fundamentals: spec-conformant handles, the handle registry with slot→row indirection, 64-byte-aligned paged SoA storage, atomic liveness bitmasks, deferred swap-and-pop compaction, and a scalar-reference spatial query.

**Architecture:** Per CONTRACTS.md (Stage 0): handles are stable slot IDs + generations (gen 0 invalid); pages are single contiguous 64-byte-aligned allocations with a column-offset header; deletions only flip liveness bits mid-frame; compaction runs at the frame boundary and maintains a slot→row table so handle dereference survives row movement. The SIMD scan, TypeToken/reflection bridge, and leases/scratchpads are **Milestone 1b** (planned after this lands) — here the spatial query is the scalar reference implementation that 1b's SIMD paths must match bit-for-bit.

**Tech Stack:** Rust 2021, criterion (benches), existing workspace lints. New crate deps identical to pulsar_ecs.

**Prerequisites:** Stage 0 complete (`docs/superpowers/specs/CONTRACTS.md` exists). All work on branch `scenedb`.

**Working directory:** `C:\Users\Sepehr\Desktop\Dev\Pulsar-Native` (repo root) unless stated.

---

### Task 1: Seed the crate

**Files:**
- Create: `crates/pulsar_scenedb/` (copy of `crates/pulsar_ecs/`)
- Modify: `crates/pulsar_scenedb/Cargo.toml`
- Modify: `Cargo.toml` (workspace root, `[workspace.dependencies]` block around line 65)

- [ ] **Step 1: Copy the crate with a shell cp (per user decision — pulsar_ecs stays untouched as reference)**

```powershell
Copy-Item -Recurse C:\Users\Sepehr\Desktop\Dev\Pulsar-Native\crates\pulsar_ecs C:\Users\Sepehr\Desktop\Dev\Pulsar-Native\crates\pulsar_scenedb
Remove-Item -Recurse -Force C:\Users\Sepehr\Desktop\Dev\Pulsar-Native\crates\pulsar_scenedb\.claude
```

- [ ] **Step 2: Rename the package**

In `crates/pulsar_scenedb/Cargo.toml` replace:

```toml
[package]
name = "pulsar_ecs"
version = "0.1.0"
edition = "2021"
description = "Pulsar archetypal Entity-Component System"
```

with:

```toml
[package]
name = "pulsar_scenedb"
version = "0.1.0"
edition = "2021"
description = "SceneDB 2.0 — engine-wide spatial database (Layer 1 storage core)"
```

- [ ] **Step 3: Register the workspace dependency**

In the root `Cargo.toml` `[workspace.dependencies]` section, directly below the `pulsar_ecs = { path = "crates/pulsar_ecs" }` line (~line 65), add:

```toml
pulsar_scenedb                     = { path = "crates/pulsar_scenedb" }
```

(`members = ["crates/*"]` is a glob, so no members edit is needed.)

- [ ] **Step 4: Fix internal crate-name self-references**

The copied tests/benches import `pulsar_ecs::…`. Repoint them:

```powershell
Get-ChildItem C:\Users\Sepehr\Desktop\Dev\Pulsar-Native\crates\pulsar_scenedb -Recurse -Include *.rs | ForEach-Object { (Get-Content $_.FullName -Raw) -replace 'pulsar_ecs', 'pulsar_scenedb' | Set-Content $_.FullName -Encoding utf8 -NoNewline }
```

- [ ] **Step 5: Verify the copy builds and its inherited test suite passes**

Run: `cargo test -p pulsar_scenedb`
Expected: PASS (same suite as pulsar_ecs, now under the new name)

- [ ] **Step 6: Commit**

```powershell
git add crates/pulsar_scenedb Cargo.toml
git commit -m "feat(scenedb): seed pulsar_scenedb from pulsar_ecs"
```

---

### Task 2: Handle type (CONTRACTS.md C1)

The inherited `Entity` treats generation 0 as live and uses `u64::MAX` as DANGLING. The spec contract differs: **generation 0 = invalid**, first live generation is 1, and `Handle(0)` is the canonical invalid value. Add a new `Handle` alongside `Entity` (Entity stays until the World internals migrate in Task 3+).

**Files:**
- Create: `crates/pulsar_scenedb/src/handle.rs`
- Modify: `crates/pulsar_scenedb/src/lib.rs` (add `pub mod handle;` + re-export)

- [ ] **Step 1: Write the failing tests**

Create `crates/pulsar_scenedb/src/handle.rs` with the test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_and_unpacks() {
        let h = Handle::new(14, 2);
        assert_eq!(h.index(), 14);
        assert_eq!(h.generation(), 2);
    }

    #[test]
    fn generation_zero_is_invalid() {
        assert!(!Handle::new(14, 0).is_valid());
        assert!(Handle::new(14, 1).is_valid());
        assert!(!Handle::INVALID.is_valid());
        assert_eq!(Handle::INVALID.index(), 0);
        assert_eq!(Handle::INVALID.generation(), 0);
    }

    #[test]
    fn max_index_and_generation_roundtrip() {
        let h = Handle::new(u32::MAX, u32::MAX);
        assert_eq!(h.index(), u32::MAX);
        assert_eq!(h.generation(), u32::MAX);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p pulsar_scenedb handle`
Expected: FAIL — `Handle` not defined (compile error)

- [ ] **Step 3: Implement Handle**

Above the test module in `handle.rs`:

```rust
use std::fmt;

/// A packed 64-bit handle per SceneDB 2.0 spec §3 / CONTRACTS.md C1.
///
/// Bits 0–31: stable slot index. Bits 32–63: generation.
/// Generation 0 is permanently reserved as invalid; live generations start at 1.
/// Unlike row positions, the slot index is stable for the allocation lifetime —
/// the registry's slot→row table absorbs compaction movement.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Handle(u64);

impl Handle {
    /// The canonical invalid handle (all zero — generation 0).
    pub const INVALID: Handle = Handle(0);

    #[inline]
    pub const fn new(index: u32, generation: u32) -> Self {
        Self(((generation as u64) << 32) | (index as u64))
    }

    /// Stable slot index (bits 0–31). NOT a row offset — resolve through the
    /// registry's slot→row table.
    #[inline]
    pub const fn index(self) -> u32 {
        self.0 as u32
    }

    /// Generation (bits 32–63). 0 = invalid.
    #[inline]
    pub const fn generation(self) -> u32 {
        (self.0 >> 32) as u32
    }

    #[inline]
    pub const fn is_valid(self) -> bool {
        self.generation() != 0
    }

    /// The raw packed value (e.g. for GPU upload).
    #[inline]
    pub const fn bits(self) -> u64 {
        self.0
    }
}

impl fmt::Debug for Handle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Handle({}v{})", self.index(), self.generation())
    }
}
```

In `lib.rs`, after `pub mod entity;` add `pub mod handle;`, and after `pub use entity::Entity;` add `pub use handle::Handle;`.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p pulsar_scenedb handle`
Expected: 3 tests PASS

- [ ] **Step 5: Commit**

```powershell
git add crates/pulsar_scenedb/src/handle.rs crates/pulsar_scenedb/src/lib.rs
git commit -m "feat(scenedb): spec-conformant Handle (gen 0 invalid, stable slot index)"
```

---

### Task 3: Handle registry with slot→row indirection (CONTRACTS.md C1)

The allocator behind handles: free pool, generation bump on recycle, **permanent retirement at gen u32::MAX**, and the slot→row table that compaction (Task 6) updates.

**Files:**
- Create: `crates/pulsar_scenedb/src/registry.rs`
- Modify: `crates/pulsar_scenedb/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/pulsar_scenedb/src/registry.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocate_starts_at_generation_one() {
        let mut reg = HandleRegistry::new();
        let h = reg.allocate(7);
        assert_eq!(h.generation(), 1);
        assert_eq!(reg.row_of(h), Some(7));
    }

    #[test]
    fn stale_handle_rejected_after_free() {
        let mut reg = HandleRegistry::new();
        let h1 = reg.allocate(0);
        assert!(reg.free(h1));
        assert_eq!(reg.row_of(h1), None, "stale handle must not resolve");
        let h2 = reg.allocate(0);
        assert_eq!(h2.index(), h1.index(), "slot is recycled");
        assert_eq!(h2.generation(), h1.generation() + 1);
        assert_eq!(reg.row_of(h2), Some(0));
    }

    #[test]
    fn double_free_rejected() {
        let mut reg = HandleRegistry::new();
        let h = reg.allocate(0);
        assert!(reg.free(h));
        assert!(!reg.free(h), "second free of the same handle must fail");
    }

    #[test]
    fn invalid_handle_never_resolves() {
        let reg = HandleRegistry::new();
        assert_eq!(reg.row_of(Handle::INVALID), None);
    }

    #[test]
    fn slot_retired_at_generation_max() {
        let mut reg = HandleRegistry::new();
        let h = reg.allocate(0);
        let slot = h.index();
        reg.force_generation(slot, u32::MAX - 1); // test hook
        let h = Handle::new(slot, u32::MAX - 1);
        assert!(reg.free(h));
        // Recycling this slot would need gen u32::MAX → permanently retired.
        let h2 = reg.allocate(0);
        assert_ne!(h2.index(), slot, "retired slot must never be reissued");
    }

    #[test]
    fn set_row_redirects_lookup() {
        let mut reg = HandleRegistry::new();
        let h = reg.allocate(5);
        reg.set_row(h.index(), 2);
        assert_eq!(reg.row_of(h), Some(2));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p pulsar_scenedb registry`
Expected: FAIL — `HandleRegistry` not defined

- [ ] **Step 3: Implement HandleRegistry**

Above the tests in `registry.rs`:

```rust
use crate::handle::Handle;

/// Sentinel row meaning "slot has no live row" (also the null token, C4).
pub const NULL_ROW: u32 = 0xFFFF_FFFF;

/// Slot allocator + generation validator + slot→row indirection (spec §3,
/// CONTRACTS.md C1). One instance per cell.
///
/// Invariants:
/// - `generations[slot]` is the live generation if the slot is allocated,
///   or the generation a recycled allocation *will get* if free.
/// - `slot_to_row[slot] == NULL_ROW` iff the slot is unallocated.
/// - A slot whose next generation would be `u32::MAX` is moved to `retired`
///   and never reissued (spec §3.2).
pub struct HandleRegistry {
    generations: Vec<u32>,
    slot_to_row: Vec<u32>,
    free: Vec<u32>,
    retired_count: u32,
}

impl HandleRegistry {
    pub fn new() -> Self {
        Self {
            generations: Vec::new(),
            slot_to_row: Vec::new(),
            free: Vec::new(),
            retired_count: 0,
        }
    }

    /// Allocate a slot pointing at `row`. Returns the new live handle.
    pub fn allocate(&mut self, row: u32) -> Handle {
        if let Some(slot) = self.free.pop() {
            let gen = self.generations[slot as usize];
            self.slot_to_row[slot as usize] = row;
            return Handle::new(slot, gen);
        }
        let slot = self.generations.len() as u32;
        self.generations.push(1);
        self.slot_to_row.push(row);
        Handle::new(slot, 1)
    }

    /// Free a live handle. Returns false for stale/invalid/double-free.
    /// The slot's generation is bumped immediately; if it would reach
    /// u32::MAX the slot is permanently retired instead of pooled.
    pub fn free(&mut self, handle: Handle) -> bool {
        if !self.is_live(handle) {
            return false;
        }
        let slot = handle.index() as usize;
        self.slot_to_row[slot] = NULL_ROW;
        let next = handle.generation().wrapping_add(1);
        self.generations[slot] = next;
        if next == u32::MAX {
            self.retired_count += 1;
        } else {
            self.free.push(handle.index());
        }
        true
    }

    /// Current row for a handle, validating the generation. None if stale,
    /// invalid, or freed.
    #[inline]
    pub fn row_of(&self, handle: Handle) -> Option<u32> {
        if !self.is_live(handle) {
            return None;
        }
        Some(self.slot_to_row[handle.index() as usize])
    }

    #[inline]
    pub fn is_live(&self, handle: Handle) -> bool {
        if !handle.is_valid() {
            return false;
        }
        let slot = handle.index() as usize;
        slot < self.generations.len()
            && self.generations[slot] == handle.generation()
            && self.slot_to_row[slot] != NULL_ROW
    }

    /// Redirect a slot to a new row. Called by frame-boundary compaction
    /// when swap-and-pop moves an element (spec §4.4).
    #[inline]
    pub fn set_row(&mut self, slot: u32, row: u32) {
        self.slot_to_row[slot as usize] = row;
    }

    /// Read-only view of the generation array (uploaded to the VRAM
    /// validation buffer in Layer 2/3).
    pub fn generations(&self) -> &[u32] {
        &self.generations
    }

    pub fn retired_count(&self) -> u32 {
        self.retired_count
    }

    /// Test hook: force a slot's stored generation (used to exercise the
    /// u32::MAX retirement path without 4.3 B iterations).
    #[cfg(test)]
    pub(crate) fn force_generation(&mut self, slot: u32, gen: u32) {
        self.generations[slot as usize] = gen;
    }
}

impl Default for HandleRegistry {
    fn default() -> Self {
        Self::new()
    }
}
```

One adjustment to the test from Step 1: `force_generation` sets the stored generation, so `stale_handle_rejected_after_free`'s assumptions hold as written. In `slot_retired_at_generation_max`, after `force_generation(slot, u32::MAX - 1)` the stored gen is `u32::MAX - 1`, the handle `Handle::new(slot, u32::MAX - 1)` is live, and `free` bumps to `u32::MAX` → retirement. No edit needed.

In `lib.rs` add `pub mod registry;` and `pub use registry::{HandleRegistry, NULL_ROW};`.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p pulsar_scenedb registry`
Expected: 6 tests PASS

- [ ] **Step 5: Commit**

```powershell
git add crates/pulsar_scenedb/src/registry.rs crates/pulsar_scenedb/src/lib.rs
git commit -m "feat(scenedb): HandleRegistry with slot->row indirection and permanent retirement"
```

---

### Task 4: Page-aligned SoA storage (CONTRACTS.md C2)

A `Page` is one contiguous 64-byte-aligned allocation: header-described columns, each column starting on a 64-byte boundary. Columns are described by `ColumnDesc { size, align }` at construction (the TypeToken macro layer arrives in M1b; here layouts are built programmatically).

**Files:**
- Create: `crates/pulsar_scenedb/src/page.rs`
- Modify: `crates/pulsar_scenedb/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/pulsar_scenedb/src/page.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn two_column_layout() -> PageLayout {
        // column 0: u64 entity ids; column 1: f32 bounds-min-x
        PageLayout::new(&[ColumnDesc::of::<u64>(), ColumnDesc::of::<f32>()], 256)
            .expect("layout fits stride budget")
    }

    #[test]
    fn columns_are_64_byte_aligned() {
        let page = Page::new(&two_column_layout());
        for col in 0..2 {
            let ptr = page.column_ptr(col) as usize;
            assert_eq!(ptr % 64, 0, "column {col} must start on a cache line");
        }
    }

    #[test]
    fn capacity_default_and_ceiling() {
        assert!(PageLayout::new(&[ColumnDesc::of::<u64>()], 1024).is_ok());
        assert!(PageLayout::new(&[ColumnDesc::of::<u64>()], 1025).is_err());
        assert!(PageLayout::new(&[ColumnDesc::of::<u64>()], 0).is_err());
    }

    #[test]
    fn stride_guardrail_128_bytes() {
        // 16 u64 columns = 128 bytes/element → ok; 17 → reject (C2).
        let cols: Vec<ColumnDesc> = (0..16).map(|_| ColumnDesc::of::<u64>()).collect();
        assert!(PageLayout::new(&cols, 256).is_ok());
        let cols: Vec<ColumnDesc> = (0..17).map(|_| ColumnDesc::of::<u64>()).collect();
        assert!(matches!(
            PageLayout::new(&cols, 256),
            Err(LayoutError::StrideExceeded { stride: 136 })
        ));
    }

    #[test]
    fn column_write_read_roundtrip() {
        let layout = two_column_layout();
        let mut page = Page::new(&layout);
        {
            let ids = page.column_slice_mut::<u64>(0);
            ids[0] = 0xDEAD_BEEF;
            ids[255] = 42;
        }
        {
            let xs = page.column_slice_mut::<f32>(1);
            xs[0] = -1.5;
        }
        let ids = page.column_slice::<u64>(0);
        assert_eq!(ids[0], 0xDEAD_BEEF);
        assert_eq!(ids[255], 42);
        let xs = page.column_slice::<f32>(1);
        assert_eq!(xs[0], -1.5);
    }

    #[test]
    fn len_starts_zero_capacity_from_layout() {
        let page = Page::new(&two_column_layout());
        assert_eq!(page.len(), 0);
        assert_eq!(page.capacity(), 256);
    }

    #[test]
    #[should_panic(expected = "column type size mismatch")]
    fn wrong_element_size_panics() {
        let page = Page::new(&two_column_layout());
        let _ = page.column_slice::<u32>(0); // column 0 is u64
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p pulsar_scenedb page`
Expected: FAIL — `PageLayout`/`Page` not defined

- [ ] **Step 3: Implement PageLayout and Page**

Above the tests in `page.rs`:

```rust
use std::alloc::{alloc_zeroed, dealloc, Layout};

/// Hard ceiling on per-page element capacity (spec §4.3).
pub const MAX_PAGE_CAPACITY: u32 = 1024;
/// Recommended default capacity (spec §4.3).
pub const DEFAULT_PAGE_CAPACITY: u32 = 256;
/// Combined per-element stride limit across all columns (spec §7.1, C2).
pub const MAX_STRIDE_BYTES: u32 = 128;
/// Every column starts on a cache-line boundary (spec §4.2).
pub const COLUMN_ALIGN: usize = 64;

/// Size/alignment descriptor for one column's element type.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ColumnDesc {
    pub size: u32,
    pub align: u32,
}

impl ColumnDesc {
    pub const fn of<T>() -> Self {
        Self {
            size: std::mem::size_of::<T>() as u32,
            align: std::mem::align_of::<T>() as u32,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum LayoutError {
    StrideExceeded { stride: u32 },
    BadCapacity { capacity: u32 },
}

/// Computed byte layout for a page: per-column offsets within one contiguous
/// allocation, every column 64-byte aligned (spec §4.2 page header contract).
#[derive(Clone, Debug)]
pub struct PageLayout {
    column_descs: Vec<ColumnDesc>,
    column_offsets: Vec<usize>,
    capacity: u32,
    total_bytes: usize,
}

impl PageLayout {
    pub fn new(columns: &[ColumnDesc], capacity: u32) -> Result<Self, LayoutError> {
        if capacity == 0 || capacity > MAX_PAGE_CAPACITY {
            return Err(LayoutError::BadCapacity { capacity });
        }
        let stride: u32 = columns.iter().map(|c| c.size).sum();
        if stride > MAX_STRIDE_BYTES {
            return Err(LayoutError::StrideExceeded { stride });
        }
        let mut offsets = Vec::with_capacity(columns.len());
        let mut cursor = 0usize;
        for col in columns {
            debug_assert!(col.align as usize <= COLUMN_ALIGN);
            cursor = next_multiple(cursor, COLUMN_ALIGN);
            offsets.push(cursor);
            cursor += col.size as usize * capacity as usize;
        }
        Ok(Self {
            column_descs: columns.to_vec(),
            column_offsets: offsets,
            capacity,
            total_bytes: next_multiple(cursor, COLUMN_ALIGN),
        })
    }

    pub fn capacity(&self) -> u32 {
        self.capacity
    }

    pub fn column_count(&self) -> usize {
        self.column_descs.len()
    }
}

#[inline]
fn next_multiple(n: usize, m: usize) -> usize {
    n.div_ceil(m) * m
}

/// One SoA page: a single 64-byte-aligned contiguous allocation holding all
/// columns. `len` counts live+dead rows up to the compaction frontier; the
/// liveness bitmask (liveness.rs) tracks which are alive.
pub struct Page {
    data: *mut u8,
    layout: PageLayout,
    alloc_layout: Layout,
    len: u32,
}

// SAFETY: Page owns its allocation exclusively; all access goes through
// &self/&mut self, so aliasing follows Rust's borrow rules.
unsafe impl Send for Page {}
unsafe impl Sync for Page {}

impl Page {
    pub fn new(layout: &PageLayout) -> Self {
        let alloc_layout =
            Layout::from_size_align(layout.total_bytes.max(COLUMN_ALIGN), COLUMN_ALIGN)
                .expect("page layout is valid");
        // SAFETY: size is non-zero (max'd with COLUMN_ALIGN), align is 64.
        let data = unsafe { alloc_zeroed(alloc_layout) };
        assert!(!data.is_null(), "page allocation failed");
        Self {
            data,
            layout: layout.clone(),
            alloc_layout,
            len: 0,
        }
    }

    #[inline]
    pub fn len(&self) -> u32 {
        self.len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    pub fn capacity(&self) -> u32 {
        self.layout.capacity
    }

    /// Reserve the next row, returning its index. None when full.
    pub fn push_row(&mut self) -> Option<u32> {
        if self.len >= self.layout.capacity {
            return None;
        }
        let row = self.len;
        self.len += 1;
        Some(row)
    }

    /// Drop the last row (used by swap-and-pop compaction).
    pub fn pop_row(&mut self) {
        debug_assert!(self.len > 0);
        self.len -= 1;
    }

    /// Raw pointer to a column's first element (for tests / future SIMD).
    pub fn column_ptr(&self, col: usize) -> *const u8 {
        // SAFETY: offset is within the allocation by PageLayout construction.
        unsafe { self.data.add(self.layout.column_offsets[col]) }
    }
}

/// Typed column access — a view of all `capacity` slots (including dead rows;
/// callers filter through liveness/len). Panics if `T`'s size doesn't match
/// the registered `ColumnDesc` — the M1b TypeToken layer makes this statically
/// safe; for now the size check guards against mis-typed access.
impl Page {
    pub fn column_slice<T>(&self, col: usize) -> &[T] {
        let desc = self.layout.column_descs[col];
        assert_eq!(
            desc.size as usize,
            std::mem::size_of::<T>(),
            "column type size mismatch"
        );
        // SAFETY: the column region holds `capacity` elements of `desc.size`
        // bytes, 64-byte aligned (≥ align_of::<T>()), zero-initialised, and
        // borrowed under &self.
        unsafe {
            std::slice::from_raw_parts(
                self.column_ptr(col) as *const T,
                self.layout.capacity as usize,
            )
        }
    }

    pub fn column_slice_mut<T>(&mut self, col: usize) -> &mut [T] {
        let desc = self.layout.column_descs[col];
        assert_eq!(
            desc.size as usize,
            std::mem::size_of::<T>(),
            "column type size mismatch"
        );
        // SAFETY: as column_slice, under &mut self.
        unsafe {
            std::slice::from_raw_parts_mut(
                self.column_ptr(col) as *mut T,
                self.layout.capacity as usize,
            )
        }
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        // SAFETY: data was allocated with alloc_layout in Page::new.
        unsafe { dealloc(self.data, self.alloc_layout) };
    }
}
```

Columns hold only `Copy`/Pod-style data in SceneDB (handles, indices, floats, bitmask words) — no `Drop` types; the M1b TypeToken layer enforces this with a `Pod` bound.

In `lib.rs` add `pub mod page;` and `pub use page::{ColumnDesc, LayoutError, Page, PageLayout, DEFAULT_PAGE_CAPACITY, MAX_PAGE_CAPACITY, MAX_STRIDE_BYTES};`.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p pulsar_scenedb page`
Expected: 6 tests PASS

- [ ] **Step 5: Run the full suite (no regressions in inherited ECS tests)**

Run: `cargo test -p pulsar_scenedb`
Expected: PASS

- [ ] **Step 6: Commit**

```powershell
git add crates/pulsar_scenedb/src/page.rs crates/pulsar_scenedb/src/lib.rs
git commit -m "feat(scenedb): 64-byte-aligned paged SoA storage with stride guardrail"
```

---

### Task 5: Atomic liveness bitmask (spec §4.4 first half)

Deletions mid-frame only flip a bit. The mask is `AtomicU64` words, 1 bit per element, supporting concurrent marking during simulation and sequential iteration during harvest.

**Files:**
- Create: `crates/pulsar_scenedb/src/liveness.rs`
- Modify: `crates/pulsar_scenedb/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_mask_is_all_dead() {
        let m = LivenessMask::new(256);
        assert_eq!(m.live_count(), 0);
        assert!(!m.is_live(0));
    }

    #[test]
    fn mark_live_and_dead() {
        let m = LivenessMask::new(256);
        m.set_live(3);
        m.set_live(64); // second word
        m.set_live(255);
        assert!(m.is_live(3) && m.is_live(64) && m.is_live(255));
        assert_eq!(m.live_count(), 3);
        m.set_dead(64);
        assert!(!m.is_live(64));
        assert_eq!(m.live_count(), 2);
    }

    #[test]
    fn dead_rows_iterates_marked_only() {
        let m = LivenessMask::new(128);
        for i in 0..10 {
            m.set_live(i);
        }
        m.set_dead(2);
        m.set_dead(7);
        let dead: Vec<u32> = m.dead_rows(10).collect();
        assert_eq!(dead, vec![2, 7]);
    }

    #[test]
    fn concurrent_marking_is_safe() {
        use std::sync::Arc;
        let m = Arc::new(LivenessMask::new(1024));
        for i in 0..1024 {
            m.set_live(i);
        }
        let handles: Vec<_> = (0..8)
            .map(|t| {
                let m = Arc::clone(&m);
                std::thread::spawn(move || {
                    for i in (t..1024).step_by(8) {
                        m.set_dead(i as u32);
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(m.live_count(), 0);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p pulsar_scenedb liveness`
Expected: FAIL — `LivenessMask` not defined

- [ ] **Step 3: Implement LivenessMask**

```rust
use std::sync::atomic::{AtomicU64, Ordering};

/// Atomic liveness bitmask — 1 bit per page element (spec §4.4, C2).
///
/// Mid-frame deletion flips a bit here; physical row removal is deferred to
/// frame-boundary compaction. Bits are set/cleared with relaxed RMW atomics:
/// cross-thread visibility of the *aggregate* mask is guaranteed by the
/// phase-boundary synchronization in Layer 2, not by per-bit ordering.
pub struct LivenessMask {
    words: Vec<AtomicU64>,
}

impl LivenessMask {
    pub fn new(capacity: u32) -> Self {
        let n_words = capacity.div_ceil(64) as usize;
        Self {
            words: (0..n_words).map(|_| AtomicU64::new(0)).collect(),
        }
    }

    #[inline]
    pub fn set_live(&self, row: u32) {
        self.words[(row / 64) as usize].fetch_or(1 << (row % 64), Ordering::Relaxed);
    }

    #[inline]
    pub fn set_dead(&self, row: u32) {
        self.words[(row / 64) as usize].fetch_and(!(1 << (row % 64)), Ordering::Relaxed);
    }

    #[inline]
    pub fn is_live(&self, row: u32) -> bool {
        self.words[(row / 64) as usize].load(Ordering::Relaxed) & (1 << (row % 64)) != 0
    }

    pub fn live_count(&self) -> u32 {
        self.words
            .iter()
            .map(|w| w.load(Ordering::Relaxed).count_ones())
            .sum()
    }

    /// Iterate dead row indices in `[0, len)` — the compaction work list.
    pub fn dead_rows(&self, len: u32) -> impl Iterator<Item = u32> + '_ {
        (0..len).filter(move |&row| !self.is_live(row))
    }

    /// Raw word access (uploaded alongside columns for GPU-side liveness).
    pub fn words(&self) -> &[AtomicU64] {
        &self.words
    }
}
```

In `lib.rs` add `pub mod liveness;` and `pub use liveness::LivenessMask;`.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p pulsar_scenedb liveness`
Expected: 4 tests PASS

- [ ] **Step 5: Commit**

```powershell
git add crates/pulsar_scenedb/src/liveness.rs crates/pulsar_scenedb/src/lib.rs
git commit -m "feat(scenedb): atomic liveness bitmask with deferred-compaction work list"
```

---

### Task 6: Cell storage — alloc/free/deref + frame-boundary compaction (spec §4.4, C3)

The integration piece: `CellStorage` owns one `Page` + `LivenessMask` + `HandleRegistry`, with a mandatory **slot-ID column** (column 0 stores each row's owning slot so compaction can fix the slot→row table after a swap). This is where the spec's central correctness property is proven: handles survive compaction; row indices don't.

**Files:**
- Create: `crates/pulsar_scenedb/src/cell.rs`
- Modify: `crates/pulsar_scenedb/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::ColumnDesc;

    /// One user column: f32 "x position" (column index 1; column 0 is the
    /// implicit slot-ID column).
    fn cell() -> CellStorage {
        CellStorage::new(&[ColumnDesc::of::<f32>()], 256).unwrap()
    }

    #[test]
    fn alloc_writes_and_reads_through_handle() {
        let mut c = cell();
        let h = c.alloc().unwrap();
        c.user_column_mut::<f32>(0)[c.row_of(h).unwrap() as usize] = 7.5;
        let row = c.row_of(h).unwrap();
        assert_eq!(c.user_column::<f32>(0)[row as usize], 7.5);
    }

    #[test]
    fn free_is_deferred_until_compact() {
        let mut c = cell();
        let h = c.alloc().unwrap();
        assert_eq!(c.live_count(), 1);
        assert!(c.free(h));
        // Row still physically present (deferred), but handle is dead and
        // the element no longer counts as live.
        assert_eq!(c.live_count(), 0);
        assert_eq!(c.row_of(h), None);
        assert_eq!(c.rows_in_use(), 1, "physical removal deferred");
        c.compact();
        assert_eq!(c.rows_in_use(), 0);
    }

    #[test]
    fn handles_survive_compaction_rows_do_not() {
        let mut c = cell();
        let ha = c.alloc().unwrap();
        let hb = c.alloc().unwrap();
        let hc = c.alloc().unwrap();
        // Write distinct values keyed by handle.
        for (h, v) in [(ha, 1.0f32), (hb, 2.0), (hc, 3.0)] {
            let row = c.row_of(h).unwrap() as usize;
            c.user_column_mut::<f32>(0)[row] = v;
        }
        let hb_row_before = c.row_of(hb).unwrap();
        c.free(hb);
        c.compact(); // swap-and-pop: hc moves into hb's old row
        // hc's handle still resolves to hc's data:
        let hc_row = c.row_of(hc).unwrap();
        assert_eq!(c.user_column::<f32>(0)[hc_row as usize], 3.0);
        // and it moved into the vacated row:
        assert_eq!(hc_row, hb_row_before, "swap-and-pop fills the hole");
        // ha untouched:
        let ha_row = c.row_of(ha).unwrap();
        assert_eq!(c.user_column::<f32>(0)[ha_row as usize], 1.0);
        assert_eq!(c.rows_in_use(), 2);
    }

    #[test]
    fn alloc_after_compact_reuses_rows_and_slots() {
        let mut c = cell();
        let h1 = c.alloc().unwrap();
        c.free(h1);
        c.compact();
        let h2 = c.alloc().unwrap();
        assert_eq!(h2.index(), h1.index(), "slot recycled");
        assert!(h2.generation() > h1.generation());
        assert_eq!(c.row_of(h2), Some(0), "row 0 reused");
        assert_eq!(c.row_of(h1), None, "old handle stays dead");
    }

    #[test]
    fn full_cell_returns_none() {
        let mut c = CellStorage::new(&[ColumnDesc::of::<f32>()], 1).unwrap();
        assert!(c.alloc().is_some());
        assert!(c.alloc().is_none());
    }

    #[test]
    fn compact_handles_multiple_holes_including_tail() {
        let mut c = cell();
        let hs: Vec<_> = (0..6).map(|_| c.alloc().unwrap()).collect();
        for (i, &h) in hs.iter().enumerate() {
            let row = c.row_of(h).unwrap() as usize;
            c.user_column_mut::<f32>(0)[row] = i as f32;
        }
        // Kill rows 1, 3, and the tail row 5.
        c.free(hs[1]);
        c.free(hs[3]);
        c.free(hs[5]);
        c.compact();
        assert_eq!(c.rows_in_use(), 3);
        for &(i, h) in &[(0usize, hs[0]), (2, hs[2]), (4, hs[4])] {
            let row = c.row_of(h).unwrap() as usize;
            assert_eq!(c.user_column::<f32>(0)[row], i as f32, "survivor {i} intact");
        }
        for &h in &[hs[1], hs[3], hs[5]] {
            assert_eq!(c.row_of(h), None);
        }
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p pulsar_scenedb cell`
Expected: FAIL — `CellStorage` not defined

- [ ] **Step 3: Implement CellStorage**

```rust
use crate::handle::Handle;
use crate::liveness::LivenessMask;
use crate::page::{ColumnDesc, LayoutError, Page, PageLayout};
use crate::registry::HandleRegistry;

/// Layer 1 storage for one cell: page + liveness + handle registry, wired
/// per spec §4.4 / CONTRACTS.md C1–C3.
///
/// Column 0 is always the implicit **slot-ID column** (`u32` owning slot per
/// row) — compaction reads it to fix the slot→row table after a swap. User
/// columns are addressed by their own index space (user column i = physical
/// column i + 1).
///
/// Mutation contract (enforced by Layer 2's phase machine in M2; by
/// discipline here): `alloc`/`free` during Simulate, `compact` only at the
/// frame boundary, reads during Harvest.
pub struct CellStorage {
    page: Page,
    liveness: LivenessMask,
    registry: HandleRegistry,
    user_column_count: usize,
}

impl CellStorage {
    pub fn new(user_columns: &[ColumnDesc], capacity: u32) -> Result<Self, LayoutError> {
        let mut columns = Vec::with_capacity(user_columns.len() + 1);
        columns.push(ColumnDesc::of::<u32>()); // slot-ID column
        columns.extend_from_slice(user_columns);
        let layout = PageLayout::new(&columns, capacity)?;
        Ok(Self {
            page: Page::new(&layout),
            liveness: LivenessMask::new(capacity),
            registry: HandleRegistry::new(),
            user_column_count: user_columns.len(),
        })
    }

    /// Allocate an element: claims a row, marks it live, issues a handle.
    pub fn alloc(&mut self) -> Option<Handle> {
        let row = self.page.push_row()?;
        let handle = self.registry.allocate(row);
        self.page.column_slice_mut::<u32>(0)[row as usize] = handle.index();
        self.liveness.set_live(row);
        Some(handle)
    }

    /// Mark an element dead. Physical removal is deferred to `compact()`.
    /// Returns false for stale/invalid handles.
    pub fn free(&mut self, handle: Handle) -> bool {
        let Some(row) = self.registry.row_of(handle) else {
            return false;
        };
        self.liveness.set_dead(row);
        self.registry.free(handle)
    }

    /// Frame-boundary swap-and-pop compaction (spec §4.4). Moves the last
    /// live row into each dead row, updates the moved element's slot→row
    /// entry via the slot-ID column, and shrinks the row frontier.
    pub fn compact(&mut self) {
        let mut len = self.page.len();
        let mut row = 0u32;
        while row < len {
            if self.liveness.is_live(row) {
                row += 1;
                continue;
            }
            // Shrink trailing dead rows first.
            while len > row + 1 && !self.liveness.is_live(len - 1) {
                len -= 1;
                self.page.pop_row();
            }
            if len == row + 1 {
                // The dead row is the tail — just drop it.
                len -= 1;
                self.page.pop_row();
                break;
            }
            // Swap last (live) row into the hole, column by column.
            let last = len - 1;
            self.swap_rows(row, last);
            // Fix the moved element's slot→row mapping.
            let moved_slot = self.page.column_slice::<u32>(0)[row as usize];
            self.registry.set_row(moved_slot, row);
            self.liveness.set_live(row);
            self.liveness.set_dead(last);
            len -= 1;
            self.page.pop_row();
            row += 1;
        }
    }

    /// Byte-wise swap of two rows across every physical column.
    fn swap_rows(&mut self, a: u32, b: u32) {
        for col in 0..self.user_column_count + 1 {
            let desc_size = {
                // Column element size from the page layout via a u8 view.
                // SAFETY-free path: copy through raw pointers per column.
                self.column_size(col)
            };
            let base = self.page.column_ptr(col) as *mut u8;
            // SAFETY: rows a, b < capacity; regions are disjoint (a != b)
            // and within the column span.
            unsafe {
                std::ptr::swap_nonoverlapping(
                    base.add(a as usize * desc_size),
                    base.add(b as usize * desc_size),
                    desc_size,
                );
            }
        }
    }

    fn column_size(&self, col: usize) -> usize {
        self.page.layout().column_descs()[col].size as usize
    }

    // ── accessors ──────────────────────────────────────────────────────────

    #[inline]
    pub fn row_of(&self, handle: Handle) -> Option<u32> {
        self.registry.row_of(handle)
    }

    pub fn user_column<T>(&self, user_col: usize) -> &[T] {
        self.page.column_slice::<T>(user_col + 1)
    }

    pub fn user_column_mut<T>(&mut self, user_col: usize) -> &mut [T] {
        self.page.column_slice_mut::<T>(user_col + 1)
    }

    pub fn live_count(&self) -> u32 {
        self.liveness.live_count()
    }

    /// Physical rows currently occupied (live + not-yet-compacted dead).
    pub fn rows_in_use(&self) -> u32 {
        self.page.len()
    }

    pub fn liveness(&self) -> &LivenessMask {
        &self.liveness
    }

    pub fn registry(&self) -> &HandleRegistry {
        &self.registry
    }
}
```

This requires two small additions to `page.rs`: a `pub fn layout(&self) -> &PageLayout` on `Page`, and `pub fn column_descs(&self) -> &[ColumnDesc]` on `PageLayout` (one-line accessors returning the stored fields).

In `lib.rs` add `pub mod cell;` and `pub use cell::CellStorage;`.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p pulsar_scenedb cell`
Expected: 6 tests PASS

- [ ] **Step 5: Run the full suite**

Run: `cargo test -p pulsar_scenedb`
Expected: PASS

- [ ] **Step 6: Commit**

```powershell
git add crates/pulsar_scenedb/src/cell.rs crates/pulsar_scenedb/src/page.rs crates/pulsar_scenedb/src/lib.rs
git commit -m "feat(scenedb): CellStorage with deferred swap-and-pop compaction and stable handles"
```

---

### Task 7: Spatial bounds + scalar AABB query (spec §8, scalar reference for M1b SIMD)

A `SpatialCell` wraps `CellStorage` with the six bounds columns (MinX, MaxX, MinY, MaxY, MinZ, MaxZ as separate f32 columns per spec §4.2) and the query that writes **null-sentinel-aligned row tokens** into a caller-provided scratch buffer (spec §8.3). This scalar implementation is the reference the M1b SIMD paths must match bit-for-bit.

**Files:**
- Create: `crates/pulsar_scenedb/src/spatial.rs`
- Modify: `crates/pulsar_scenedb/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn aabb(min: [f32; 3], max: [f32; 3]) -> Aabb {
        Aabb { min, max }
    }

    #[test]
    fn query_writes_token_per_row_position() {
        let mut c = SpatialCell::new(256).unwrap();
        let ha = c.alloc(aabb([0.0, 0.0, 0.0], [1.0, 1.0, 1.0])).unwrap();
        let _hb = c.alloc(aabb([10.0, 10.0, 10.0], [11.0, 11.0, 11.0])).unwrap();
        let hc = c.alloc(aabb([0.5, 0.5, 0.5], [2.0, 2.0, 2.0])).unwrap();

        let mut out = vec![0u32; c.rows_in_use() as usize];
        let n = c.query_aabb(&aabb([0.0, 0.0, 0.0], [3.0, 3.0, 3.0]), &mut out);

        assert_eq!(n, 2, "two hits");
        // Positional alignment (spec §8.3): out[row] = row for hits,
        // NULL_ROW sentinel for misses.
        assert_eq!(out[c.row_of(ha).unwrap() as usize], c.row_of(ha).unwrap());
        assert_eq!(out[1], crate::registry::NULL_ROW, "miss row holds sentinel");
        assert_eq!(out[c.row_of(hc).unwrap() as usize], c.row_of(hc).unwrap());
    }

    #[test]
    fn dead_elements_excluded() {
        let mut c = SpatialCell::new(256).unwrap();
        let h = c.alloc(aabb([0.0; 3], [1.0; 3])).unwrap();
        c.free(h);
        let mut out = vec![0u32; c.rows_in_use() as usize];
        let n = c.query_aabb(&aabb([-1.0; 3], [2.0; 3]), &mut out);
        assert_eq!(n, 0);
        assert_eq!(out[0], crate::registry::NULL_ROW);
    }

    #[test]
    fn touching_boxes_intersect() {
        // Spec §8.2 predicate uses ≤/≥ — shared faces count as overlap.
        let mut c = SpatialCell::new(256).unwrap();
        c.alloc(aabb([1.0, 0.0, 0.0], [2.0, 1.0, 1.0])).unwrap();
        let mut out = vec![0u32; 1];
        let n = c.query_aabb(&aabb([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]), &mut out);
        assert_eq!(n, 1, "face contact at x=1 is a hit");
    }

    #[test]
    fn property_matches_naive_reference() {
        use rand::{Rng, SeedableRng};
        let mut rng = rand::rngs::StdRng::seed_from_u64(0x5CE_DB);
        let mut c = SpatialCell::new(1024).unwrap();
        let mut boxes = Vec::new();
        for _ in 0..1000 {
            let min: [f32; 3] = std::array::from_fn(|_| rng.gen_range(-100.0..100.0));
            let ext: [f32; 3] = std::array::from_fn(|_| rng.gen_range(0.0..10.0));
            let max = [min[0] + ext[0], min[1] + ext[1], min[2] + ext[2]];
            let b = aabb(min, max);
            c.alloc(b).unwrap();
            boxes.push(b);
        }
        for _ in 0..50 {
            let qmin: [f32; 3] = std::array::from_fn(|_| rng.gen_range(-100.0..100.0));
            let qext: [f32; 3] = std::array::from_fn(|_| rng.gen_range(0.0..50.0));
            let q = aabb(qmin, [qmin[0] + qext[0], qmin[1] + qext[1], qmin[2] + qext[2]]);
            let mut out = vec![0u32; c.rows_in_use() as usize];
            let n = c.query_aabb(&q, &mut out) as usize;
            let expected: Vec<u32> = boxes
                .iter()
                .enumerate()
                .filter(|(_, b)| {
                    b.min[0] <= q.max[0] && b.max[0] >= q.min[0]
                        && b.min[1] <= q.max[1] && b.max[1] >= q.min[1]
                        && b.min[2] <= q.max[2] && b.max[2] >= q.min[2]
                })
                .map(|(i, _)| i as u32)
                .collect();
            let hits: Vec<u32> = out
                .iter()
                .copied()
                .filter(|&t| t != crate::registry::NULL_ROW)
                .collect();
            assert_eq!(hits, expected);
            assert_eq!(n, expected.len());
        }
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p pulsar_scenedb spatial`
Expected: FAIL — `SpatialCell`/`Aabb` not defined

- [ ] **Step 3: Implement SpatialCell**

```rust
use crate::cell::CellStorage;
use crate::handle::Handle;
use crate::page::{ColumnDesc, LayoutError};
use crate::registry::NULL_ROW;

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// User-column indices for the six bounds columns (spec §4.2: six separate
/// f32 arrays, not an array of structs).
const COL_MIN_X: usize = 0;
const COL_MAX_X: usize = 1;
const COL_MIN_Y: usize = 2;
const COL_MAX_Y: usize = 3;
const COL_MIN_Z: usize = 4;
const COL_MAX_Z: usize = 5;
/// Number of bounds columns; cell-type-specific columns start after these.
pub const SPATIAL_COLUMNS: usize = 6;

/// CellStorage + spatial bounds columns + the §8 query.
///
/// `query_aabb` is the **scalar reference implementation**: M1b's SIMD paths
/// (AVX2/AVX-512/NEON) must produce bit-identical output buffers.
pub struct SpatialCell {
    storage: CellStorage,
}

impl SpatialCell {
    pub fn new(capacity: u32) -> Result<Self, LayoutError> {
        let columns = [ColumnDesc::of::<f32>(); SPATIAL_COLUMNS];
        Ok(Self {
            storage: CellStorage::new(&columns, capacity)?,
        })
    }

    pub fn alloc(&mut self, bounds: Aabb) -> Option<Handle> {
        let h = self.storage.alloc()?;
        let row = self.storage.row_of(h).unwrap() as usize;
        self.storage.user_column_mut::<f32>(COL_MIN_X)[row] = bounds.min[0];
        self.storage.user_column_mut::<f32>(COL_MAX_X)[row] = bounds.max[0];
        self.storage.user_column_mut::<f32>(COL_MIN_Y)[row] = bounds.min[1];
        self.storage.user_column_mut::<f32>(COL_MAX_Y)[row] = bounds.max[1];
        self.storage.user_column_mut::<f32>(COL_MIN_Z)[row] = bounds.min[2];
        self.storage.user_column_mut::<f32>(COL_MAX_Z)[row] = bounds.max[2];
        Some(h)
    }

    /// Spec §8.2 predicate over all physical rows, writing positionally
    /// aligned row tokens into `out` (spec §8.3): `out[row] = row` on hit,
    /// `NULL_ROW` on miss/dead. `out.len()` must be ≥ `rows_in_use()`.
    /// Returns the hit count. Allocates nothing (spec §8.1).
    pub fn query_aabb(&self, q: &Aabb, out: &mut [u32]) -> u32 {
        let len = self.storage.rows_in_use() as usize;
        assert!(out.len() >= len, "scratch buffer too small");
        let min_x = &self.storage.user_column::<f32>(COL_MIN_X)[..len];
        let max_x = &self.storage.user_column::<f32>(COL_MAX_X)[..len];
        let min_y = &self.storage.user_column::<f32>(COL_MIN_Y)[..len];
        let max_y = &self.storage.user_column::<f32>(COL_MAX_Y)[..len];
        let min_z = &self.storage.user_column::<f32>(COL_MIN_Z)[..len];
        let max_z = &self.storage.user_column::<f32>(COL_MAX_Z)[..len];
        let liveness = self.storage.liveness();

        let mut hits = 0u32;
        for row in 0..len {
            let visible = min_x[row] <= q.max[0]
                && max_x[row] >= q.min[0]
                && min_y[row] <= q.max[1]
                && max_y[row] >= q.min[1]
                && min_z[row] <= q.max[2]
                && max_z[row] >= q.min[2]
                && liveness.is_live(row as u32);
            out[row] = if visible {
                hits += 1;
                row as u32
            } else {
                NULL_ROW
            };
        }
        hits
    }

    // ── delegation ─────────────────────────────────────────────────────────

    pub fn free(&mut self, handle: Handle) -> bool {
        self.storage.free(handle)
    }

    pub fn compact(&mut self) {
        self.storage.compact()
    }

    pub fn row_of(&self, handle: Handle) -> Option<u32> {
        self.storage.row_of(handle)
    }

    pub fn rows_in_use(&self) -> u32 {
        self.storage.rows_in_use()
    }

    pub fn live_count(&self) -> u32 {
        self.storage.live_count()
    }

    pub fn storage(&self) -> &CellStorage {
        &self.storage
    }

    pub fn storage_mut(&mut self) -> &mut CellStorage {
        &mut self.storage
    }
}
```

In `lib.rs` add `pub mod spatial;` and `pub use spatial::{Aabb, SpatialCell, SPATIAL_COLUMNS};`.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p pulsar_scenedb spatial`
Expected: 4 tests PASS

- [ ] **Step 5: Commit**

```powershell
git add crates/pulsar_scenedb/src/spatial.rs crates/pulsar_scenedb/src/lib.rs
git commit -m "feat(scenedb): spatial bounds columns + scalar-reference AABB query with sentinel tokens"
```

---

### Task 8: Storage benchmark

Quantify the new storage against the inherited archetype path so M1b's SIMD work has a baseline.

**Files:**
- Create: `crates/pulsar_scenedb/benches/scenedb_bench.rs`
- Modify: `crates/pulsar_scenedb/Cargo.toml` (add `[[bench]]` entry)

- [ ] **Step 1: Write the benchmark**

```rust
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use pulsar_scenedb::{Aabb, SpatialCell};

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

criterion_group!(benches, bench_query, bench_churn);
criterion_main!(benches);
```

Add to `crates/pulsar_scenedb/Cargo.toml`:

```toml
[[bench]]
name = "scenedb_bench"
harness = false
```

- [ ] **Step 2: Run the benchmark**

Run: `cargo bench -p pulsar_scenedb --bench scenedb_bench`
Expected: completes with reported times (record the `scalar_aabb_scan_1024` number — it's the M1b SIMD baseline)

- [ ] **Step 3: Full suite + commit**

Run: `cargo test -p pulsar_scenedb`
Expected: PASS

```powershell
git add crates/pulsar_scenedb/benches/scenedb_bench.rs crates/pulsar_scenedb/Cargo.toml
git commit -m "bench(scenedb): scalar query + churn baselines for M1b SIMD work"
```

---

### Task 9: Crate docs + milestone wrap-up

**Files:**
- Modify: `crates/pulsar_scenedb/src/lib.rs` (crate-level doc comment)
- Modify: `crates/pulsar_scenedb/README.md`

- [ ] **Step 1: Replace the inherited crate doc**

Replace the `//!` block at the top of `lib.rs` (currently describing the ECS) with:

```rust
//! SceneDB 2.0 — Layer 1 storage core (spec Rev 2.2, CONTRACTS.md C1–C4).
//!
//! Seeded from `pulsar_ecs` (which remains in-tree as the reference
//! implementation). This crate adds the spec-conformant storage layer:
//!
//! - [`Handle`] — packed u64, stable slot index + generation, gen 0 invalid
//! - [`HandleRegistry`] — slot allocator, generation validation, slot→row
//!   indirection, permanent retirement at gen `u32::MAX`
//! - [`Page`]/[`PageLayout`] — single-allocation 64-byte-aligned SoA pages,
//!   128-byte stride guardrail, 1024-element ceiling
//! - [`LivenessMask`] — atomic per-element liveness, deferred deletion
//! - [`CellStorage`] — alloc/free/deref + frame-boundary swap-and-pop
//!   compaction that preserves handle validity
//! - [`SpatialCell`] — six SoA bounds columns + the §8 AABB query writing
//!   sentinel-aligned row tokens into caller scratch (scalar reference;
//!   SIMD paths land in M1b and must match bit-for-bit)
//!
//! The inherited archetype ECS modules (`world`, `archetype`, `query`, …)
//! are retained and will be migrated onto paged storage in later milestones
//! (the SceneDB-replaces-ECS path, design doc §7).
```

- [ ] **Step 2: Update README.md**

Replace the copied pulsar_ecs README content with a short pointer:

```markdown
# pulsar_scenedb

SceneDB 2.0 Layer 1 storage core. Spec: `docs/superpowers/specs/SceneDB2.0.md`
(Rev 2.2). Contracts: `docs/superpowers/specs/CONTRACTS.md`. Design:
`docs/superpowers/specs/2026-06-09-scenedb20-implementation-design.md`.

Seeded from `pulsar_ecs` (kept as reference). See the crate docs for the
module map. Milestone status: M1a (storage core) — handles, paged SoA,
liveness, compaction, scalar spatial query.
```

- [ ] **Step 3: Final verification**

Run: `cargo test -p pulsar_scenedb`
Expected: PASS
Run: `cargo clippy -p pulsar_scenedb -- -D warnings`
Expected: clean (fix any lints surfaced)

- [ ] **Step 4: Commit**

```powershell
git add crates/pulsar_scenedb/src/lib.rs crates/pulsar_scenedb/README.md
git commit -m "docs(scenedb): crate-level docs and README for M1a storage core"
```

---

## Milestone exit criteria

- `cargo test -p pulsar_scenedb` green (inherited ECS suite + ~25 new storage tests).
- `cargo bench -p pulsar_scenedb --bench scenedb_bench` produces baselines.
- `pulsar_ecs` is byte-for-byte untouched (verify: `git diff --stat main -- crates/pulsar_ecs` shows only pre-existing local bench changes, nothing from this milestone).
- Handles provably survive compaction (Task 6 tests) — the spec's central correctness property.

## Deferred to M1b (next plan)

TypeToken registration macros + `pulsar_reflection` bridge (C7), SIMD query paths with runtime dispatch matching the Task 7 scalar reference bit-for-bit, read-leases + scratchpad pools with decay (C4), multi-cell grid container, frustum queries, Test 1 (multi-threaded contention) and Test 2 host-half (formal stale-handle suite) as named Part VI gates.
