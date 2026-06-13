# SceneDB 2.0 — Milestone 1b: TypeToken Bridge, SIMD Queries & Leases Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Complete Layer 1 of SceneDB 2.0 — bridge column types to `pulsar_reflection` via a dense `TypeToken`, add runtime-dispatched SIMD query paths that match the M1a scalar reference bit-for-bit, add frustum queries, and add read-leases + thread-local scratchpad pools with decay and timeout revocation. Gated by Part VI Test 1 (multi-threaded contention) and Test 2 host-half (stale-handle rejection).

**Architecture:** Builds entirely on the M1a storage core (`crates/pulsar_scenedb`). Per CONTRACTS.md C7, the `TypeToken` reuses the crate's existing dense `ComponentId(u32)` allocator (inherited from the ECS seed) as the token id-space and binds it to a `ColumnDesc` (Pod layout) plus an optional `&'static RuntimeTypeInfo` from `pulsar_reflection`'s `RUNTIME_TYPE_REGISTRY`. Cell types declare their column set by token with the holistic 128-byte stride check (C2/§7.1). The SIMD layer keeps the scalar `query_aabb`/`query_frustum` as the bit-for-bit oracle and adds an AVX2 arm behind runtime feature dispatch (scalar fallback for AVX-512/NEON/unsupported, with those optimized arms a documented follow-on). Leases use a fixed 64-slot pool with a per-cell atomic bitmask; scratchpads are thread-local with the 8-frame/50% decay policy; revocation uses double-buffered liveness per §9.2.1.

**Tech Stack:** Rust 2021, `std::arch` x86_64 intrinsics with `is_x86_feature_detected!` runtime dispatch, `pulsar_reflection` (Reflectable/RuntimeTypeInfo/RUNTIME_TYPE_REGISTRY — already a workspace dep of the crate), criterion.

**Prerequisites:** Milestone 1a complete (branch `scenedb`, HEAD at the M1a wrap-up). Frozen contracts at `docs/superpowers/specs/CONTRACTS.md` (C1–C7). Spec Rev 2.2 at `docs/superpowers/specs/SceneDB2.0.md`. Design doc `docs/superpowers/specs/2026-06-09-scenedb20-implementation-design.md` (Milestone 1).

**Working directory:** `C:\Users\Sepehr\Desktop\Dev\Pulsar-Native` (repo root, branch `scenedb`) unless stated.

## M1a API this milestone builds on (current, verified)

- `Handle` — `new/index/generation/is_valid/bits/INVALID`, derives `Ord`.
- `HandleRegistry` — `new/allocate(row)/free/row_of/is_live/set_row/generations()/retired_count()`. `NULL_ROW: u32 = 0xFFFF_FFFF`.
- `Page`/`PageLayout`/`ColumnDesc`/`Pod`/`LayoutError` — `column_slice::<T: Pod>/column_slice_mut/column_ptr/column_ptr_mut (pub(crate))/layout()/column_descs()/capacity()/column_count()`. `MAX_STRIDE_BYTES=128`, `MAX_PAGE_CAPACITY=1024`, `DEFAULT_PAGE_CAPACITY=256`, `COLUMN_ALIGN=64`. `LayoutError::{StrideExceeded,BadCapacity,AlignmentExceeded}`.
- `LivenessMask` — `new/set_live/set_dead/is_live/live_count/dead_rows/words()`.
- `CellStorage` — `new(user_columns: &[ColumnDesc], capacity)/alloc/free/compact/row_of/user_column::<T: Pod>(usize)/user_column_mut/live_count/rows_in_use/liveness()/registry()`. Column 0 is the implicit u32 slot-ID column; user columns are offset +1.
- `SpatialCell` — `new(capacity)/alloc(Aabb)/query_aabb(&Aabb, &mut [u32]) -> u32/free/compact/row_of/rows_in_use/live_count/storage()/storage_mut()`. `Aabb { min: [f32;3], max: [f32;3] }`. `SPATIAL_COLUMNS = 6`. Bounds column order: MIN_X=0,MAX_X=1,MIN_Y=2,MAX_Y=3,MIN_Z=4,MAX_Z=5.
- Inherited dense id allocator (component.rs): `component_id::<T>() -> ComponentId(u32)` (sequential from 1, 0 reserved), `resolve_id(TypeId)`, `type_of(ComponentId)`, `component_count()`.
- `pulsar_reflection` re-exports available as a dep: `RUNTIME_TYPE_REGISTRY`, `RuntimeTypeInfo { type_id, type_name, size, align, structure, color }`, `FieldInfo { name, type_info, offset }`, `Reflectable`.

## Scoping decisions (surfaced for review)

1. **No proc-macro in M1b.** The spec's `#[register_render_type(...)]` compile-time sugar with a `const` stride assertion is deferred to the integration milestones (M3/M4) when real subsystems register types. M1b provides the runtime registration mechanism (`TypeToken`, `CellType` builder) with a registration-time `Result` stride check. A follow-on proc-macro can wrap this with a const assertion later without changing the storage mechanism.
2. **One SIMD arm (AVX2) in M1b.** The dispatch scaffold detects AVX-512/AVX2/NEON/scalar; AVX2 is fully implemented (broadly available on x86_64 dev/CI). AVX-512 and NEON route to the scalar reference now (correct, just not yet faster) and are a documented follow-on. This keeps M1b testable on the dev machine and CI without untestable intrinsics.
3. **Leases are infrastructure-only in M1b.** The lease/scratchpad types and their decay/revocation logic are built and unit-tested here, but the *phase machine* that mandates lease acquisition around harvest is M2 (Layer 2). M1b proves the primitives work; M2 wires them into the frame loop.

---

### Task 1: TypeToken — dense token bridging Pod layout + reflection

A `TypeToken` binds a column type to its dense `ComponentId`, its `ColumnDesc`, and (when the type is registered with `pulsar_reflection`) its `&'static RuntimeTypeInfo`. This is the C7 registration unit.

**Files:**
- Create: `crates/pulsar_scenedb/src/token.rs`
- Modify: `crates/pulsar_scenedb/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/pulsar_scenedb/src/token.rs` with the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_carries_dense_id_and_layout() {
        let t = TypeToken::of::<f32>();
        assert_eq!(t.desc(), crate::page::ColumnDesc::of::<f32>());
        // Same type → same dense id (stable across calls).
        assert_eq!(TypeToken::of::<f32>().id(), t.id());
        // Different types → different ids.
        assert_ne!(TypeToken::of::<u32>().id(), t.id());
    }

    #[test]
    fn token_id_matches_component_id() {
        // The token id-space IS the crate's ComponentId allocator (C7).
        assert_eq!(TypeToken::of::<u64>().id(), crate::component::component_id::<u64>());
    }

    #[test]
    fn unregistered_type_has_no_reflection_info() {
        // A bare Pod type not registered with pulsar_reflection resolves to None.
        struct LocalUnregistered(#[allow(dead_code)] u32);
        // SAFETY: trivially Pod for the test (Copy + zero-valid). Not exported.
        unsafe impl crate::page::Pod for LocalUnregistered {}
        impl Clone for LocalUnregistered { fn clone(&self) -> Self { Self(self.0) } }
        impl Copy for LocalUnregistered {}
        assert!(TypeToken::of::<LocalUnregistered>().type_info().is_none());
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p pulsar_scenedb token`
Expected: FAIL — `TypeToken` not defined.

- [ ] **Step 3: Implement TypeToken**

Above the tests in `token.rs`:

```rust
use crate::component::{component_id, ComponentId};
use crate::page::{ColumnDesc, Pod};
use pulsar_reflection::{RuntimeTypeInfo, RUNTIME_TYPE_REGISTRY};
use std::any::TypeId;

/// A dense, typed handle to a registered SceneDB column type (spec §7,
/// CONTRACTS.md C7).
///
/// Binds three things for a column element type `T: Pod`:
/// - a **dense `ComponentId`** (the crate's existing sequential u32 id-space,
///   reused so SceneDB columns and ECS components share one id allocator);
/// - the **`ColumnDesc`** (size/align) used to lay the column out in a page;
/// - the **`TypeId`**, used to look up the optional `pulsar_reflection`
///   `RuntimeTypeInfo` for serialization / editor metadata.
///
/// A `TypeToken` is `Copy` and cheap; construct it with [`TypeToken::of`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct TypeToken {
    id: ComponentId,
    type_id: TypeId,
    desc: ColumnDesc,
}

impl TypeToken {
    /// Token for column element type `T`. Allocates `T`'s dense id on first
    /// use (per-process), then returns the same id forever.
    #[must_use]
    pub fn of<T: Pod + 'static>() -> Self {
        Self {
            id: component_id::<T>(),
            type_id: TypeId::of::<T>(),
            desc: ColumnDesc::of::<T>(),
        }
    }

    /// The dense column-type id (== `component_id::<T>()`).
    #[inline]
    #[must_use]
    pub fn id(self) -> ComponentId {
        self.id
    }

    /// The column layout descriptor for one element.
    #[inline]
    #[must_use]
    pub fn desc(self) -> ColumnDesc {
        self.desc
    }

    /// The `pulsar_reflection` metadata for this type, if it was registered
    /// (via `#[derive(Reflectable)]` / `#[pulsar_type]`). `None` for bare Pod
    /// types with no reflection registration — those still work as columns;
    /// they just carry no serialization/editor metadata.
    #[must_use]
    pub fn type_info(self) -> Option<&'static RuntimeTypeInfo> {
        RUNTIME_TYPE_REGISTRY.get_by_id(self.type_id)
    }
}
```

In `lib.rs` add `pub mod token;` and `pub use token::TypeToken;`.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p pulsar_scenedb token`
Expected: 3 tests PASS.

- [ ] **Step 5: Commit**

```powershell
git add crates/pulsar_scenedb/src/token.rs crates/pulsar_scenedb/src/lib.rs
git commit -m "feat(scenedb): TypeToken bridging dense ComponentId, ColumnDesc, and reflection RuntimeTypeInfo"
```

---

### Task 2: CellType registration + token-keyed CellStorage construction

A `CellType` is an ordered set of `TypeToken`s declaring a cell's user columns, validated holistically against the 128-byte stride budget (§7.1) at registration time. `CellStorage` gains a token-keyed constructor and type-keyed column access (resolving token → physical column index), which is harder to misuse than the positional `user_column::<T>(usize)`.

**Files:**
- Create: `crates/pulsar_scenedb/src/cell_type.rs`
- Modify: `crates/pulsar_scenedb/src/cell.rs`
- Modify: `crates/pulsar_scenedb/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/pulsar_scenedb/src/cell_type.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::TypeToken;

    #[test]
    fn builds_layout_from_tokens() {
        let ct = CellType::new("test")
            .with(TypeToken::of::<f32>())
            .with(TypeToken::of::<u32>())
            .build()
            .unwrap();
        assert_eq!(ct.user_column_count(), 2);
        // Token resolves to its user-column index in declaration order.
        assert_eq!(ct.column_index(TypeToken::of::<f32>()), Some(0));
        assert_eq!(ct.column_index(TypeToken::of::<u32>()), Some(1));
        assert_eq!(ct.column_index(TypeToken::of::<u64>()), None);
    }

    // Four DISTINCT 32-byte Pod column types for the stride test (using the
    // same token twice would trip the duplicate check before the stride check).
    #[derive(Copy, Clone)] struct B32([u8; 32]);
    #[derive(Copy, Clone)] struct C32([u8; 32]);
    #[derive(Copy, Clone)] struct D32([u8; 32]);
    #[derive(Copy, Clone)] struct E32([u8; 32]);
    // SAFETY: all-zero is valid for a byte array; Copy, no Drop.
    unsafe impl crate::page::Pod for B32 {}
    unsafe impl crate::page::Pod for C32 {}
    unsafe impl crate::page::Pod for D32 {}
    unsafe impl crate::page::Pod for E32 {}

    #[test]
    fn holistic_stride_check_rejects_over_budget() {
        // 4 distinct × 32 bytes = 128 user bytes + the 4-byte slot column → 132.
        let r = CellType::new("fat")
            .with(TypeToken::of::<B32>())
            .with(TypeToken::of::<C32>())
            .with(TypeToken::of::<D32>())
            .with(TypeToken::of::<E32>())
            .build();
        // Holistic budget counts the slot-ID column too: 128 + 4 = 132 > 128.
        assert!(matches!(r, Err(CellTypeError::StrideExceeded { stride: 132 })));
    }

    #[test]
    fn duplicate_token_rejected() {
        let r = CellType::new("dup")
            .with(TypeToken::of::<f32>())
            .with(TypeToken::of::<f32>())
            .build();
        assert!(matches!(r, Err(CellTypeError::DuplicateColumn { .. })));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p pulsar_scenedb cell_type`
Expected: FAIL — `CellType` not defined.

- [ ] **Step 3: Implement CellType**

Above the tests in `cell_type.rs`:

```rust
use crate::page::{ColumnDesc, MAX_STRIDE_BYTES};
use crate::token::TypeToken;

#[derive(Debug, PartialEq, Eq)]
pub enum CellTypeError {
    /// Combined stride (slot-ID column + all user columns) exceeds 128 bytes.
    StrideExceeded { stride: u32 },
    /// The same token was declared twice.
    DuplicateColumn { id: crate::component::ComponentId },
    /// No columns declared.
    Empty,
}

/// A registered cell composition: the ordered set of column types a cell
/// stores, validated holistically against the per-element stride budget
/// (§7.1 / CONTRACTS.md C2). Build with the fluent API, then hand to
/// [`CellStorage::from_cell_type`](crate::cell::CellStorage::from_cell_type).
#[derive(Clone, Debug)]
pub struct CellType {
    name: &'static str,
    tokens: Vec<TypeToken>,
}

impl CellType {
    #[must_use]
    pub fn new(name: &'static str) -> Self {
        Self { name, tokens: Vec::new() }
    }

    /// Declare the next user column. Order is significant: it becomes the
    /// user-column index.
    #[must_use]
    pub fn with(mut self, token: TypeToken) -> Self {
        self.tokens.push(token);
        self
    }

    /// Validate and freeze the layout. Performs the **holistic** stride check
    /// across the implicit u32 slot-ID column plus every declared user column
    /// (§7.1): splitting a layout into many small columns cannot bypass the
    /// 128-byte budget.
    pub fn build(self) -> Result<RegisteredCellType, CellTypeError> {
        if self.tokens.is_empty() {
            return Err(CellTypeError::Empty);
        }
        // Reject duplicate column types (same dense id declared twice).
        for i in 0..self.tokens.len() {
            for j in (i + 1)..self.tokens.len() {
                if self.tokens[i].id() == self.tokens[j].id() {
                    return Err(CellTypeError::DuplicateColumn { id: self.tokens[i].id() });
                }
            }
        }
        // Holistic stride: slot-ID column (u32 = 4 bytes) + all user columns.
        let user_stride: u32 = self.tokens.iter().map(|t| t.desc().size).sum();
        let stride = user_stride + ColumnDesc::of::<u32>().size;
        if stride > MAX_STRIDE_BYTES {
            return Err(CellTypeError::StrideExceeded { stride });
        }
        Ok(RegisteredCellType {
            name: self.name,
            tokens: self.tokens,
        })
    }
}

/// A validated cell composition. Maps tokens → user-column indices and yields
/// the `ColumnDesc` list a `CellStorage` page needs.
#[derive(Clone, Debug)]
pub struct RegisteredCellType {
    name: &'static str,
    tokens: Vec<TypeToken>,
}

impl RegisteredCellType {
    #[must_use]
    pub fn name(&self) -> &'static str {
        self.name
    }

    #[must_use]
    pub fn user_column_count(&self) -> usize {
        self.tokens.len()
    }

    /// User-column index for a token, or None if the token isn't part of this
    /// cell type.
    #[must_use]
    pub fn column_index(&self, token: TypeToken) -> Option<usize> {
        self.tokens.iter().position(|t| t.id() == token.id())
    }

    /// The user-column `ColumnDesc` list (in declaration order) for building
    /// the page layout.
    #[must_use]
    pub fn user_descs(&self) -> Vec<ColumnDesc> {
        self.tokens.iter().map(|t| t.desc()).collect()
    }
}
```

Note: the test calls `CellType::...build().unwrap()` and then uses methods on the result; rename the test's binding type expectation — `build()` returns `RegisteredCellType`. The test as written binds `ct` to the `RegisteredCellType` and calls `user_column_count`/`column_index`, which exist. The `with` returns `CellType`; the `b.build()` in the stride test is called on a `CellType`. All consistent.

In `lib.rs` add `pub mod cell_type;` and `pub use cell_type::{CellType, CellTypeError, RegisteredCellType};`.

- [ ] **Step 4: Add the token-keyed constructor + column access to `CellStorage`**

In `cell.rs`, add these methods to `impl CellStorage` (alongside the existing `new`), plus store the registered cell type for token→index resolution. Add a field and methods:

First, add an optional cell-type map. Change the struct to carry the token→user-index map (a small `Vec<(ComponentId, usize)>` is fine — column counts are tiny):

```rust
// add `use crate::cell_type::RegisteredCellType;` and
// `use crate::token::TypeToken;` to the imports at the top of cell.rs
```

Add to `impl CellStorage`:

```rust
    /// Build a cell from a registered cell type (token-keyed). Preferred over
    /// `new` for typed call sites.
    pub fn from_cell_type(
        cell_type: &RegisteredCellType,
        capacity: u32,
    ) -> Result<Self, crate::page::LayoutError> {
        let descs = cell_type.user_descs();
        let mut storage = Self::new(&descs, capacity)?;
        storage.token_index = cell_type
            .user_descs()
            .iter()
            .enumerate()
            .map(|(i, _)| i)
            .zip(cell_type_tokens(cell_type))
            .map(|(idx, id)| (id, idx))
            .collect();
        Ok(storage)
    }

    /// Typed column access by token (resolves token → user-column index).
    /// Returns None if the token isn't a column of this cell.
    pub fn column_for<T: crate::page::Pod + 'static>(&self) -> Option<&[T]> {
        let id = TypeToken::of::<T>().id();
        let idx = self.token_index.iter().find(|(tid, _)| *tid == id).map(|(_, i)| *i)?;
        Some(self.user_column::<T>(idx))
    }

    pub fn column_for_mut<T: crate::page::Pod + 'static>(&mut self) -> Option<&mut [T]> {
        let id = TypeToken::of::<T>().id();
        let idx = self.token_index.iter().find(|(tid, _)| *tid == id).map(|(_, i)| *i)?;
        Some(self.user_column_mut::<T>(idx))
    }
```

Add the field to the struct definition:

```rust
    /// Token id → user-column index, populated only via `from_cell_type`.
    token_index: Vec<(crate::component::ComponentId, usize)>,
```

Initialize `token_index: Vec::new()` in the existing `new` constructor's struct literal.

Add a free helper at the bottom of `cell.rs` (module scope) that extracts token ids from a registered cell type — since `RegisteredCellType` doesn't expose tokens directly, add a `pub fn token_ids(&self) -> Vec<ComponentId>` accessor to `RegisteredCellType` in `cell_type.rs` instead and use it:

In `cell_type.rs`, add to `impl RegisteredCellType`:

```rust
    /// Token dense ids in declaration order (for building token→index maps).
    #[must_use]
    pub fn token_ids(&self) -> Vec<crate::component::ComponentId> {
        self.tokens.iter().map(|t| t.id()).collect()
    }
```

Then simplify `from_cell_type` to use it (replace the `.zip(cell_type_tokens(...))` line):

```rust
    pub fn from_cell_type(
        cell_type: &RegisteredCellType,
        capacity: u32,
    ) -> Result<Self, crate::page::LayoutError> {
        let descs = cell_type.user_descs();
        let mut storage = Self::new(&descs, capacity)?;
        storage.token_index = cell_type
            .token_ids()
            .into_iter()
            .enumerate()
            .map(|(idx, id)| (id, idx))
            .collect();
        Ok(storage)
    }
```

(Delete the earlier `cell_type_tokens` reference — it was a sketch; `token_ids()` replaces it.)

- [ ] **Step 5: Add a CellStorage token-access test**

Append to the `#[cfg(test)] mod tests` in `cell.rs`:

```rust
    #[test]
    fn token_keyed_column_access() {
        use crate::cell_type::CellType;
        use crate::token::TypeToken;
        let ct = CellType::new("xy")
            .with(TypeToken::of::<f32>())
            .build()
            .unwrap();
        let mut c = CellStorage::from_cell_type(&ct, 16).unwrap();
        let h = c.alloc().unwrap();
        let row = c.row_of(h).unwrap() as usize;
        c.column_for_mut::<f32>().unwrap()[row] = 9.0;
        assert_eq!(c.column_for::<f32>().unwrap()[row], 9.0);
        // u32 is not a user column of this cell type → None.
        assert!(c.column_for::<u64>().is_none());
    }
```

- [ ] **Step 6: Run to verify pass**

Run: `cargo test -p pulsar_scenedb cell_type`
Run: `cargo test -p pulsar_scenedb cell`
Expected: 3 cell_type tests PASS; cell tests (now 7) PASS.

- [ ] **Step 7: Commit**

```powershell
git add crates/pulsar_scenedb/src/cell_type.rs crates/pulsar_scenedb/src/cell.rs crates/pulsar_scenedb/src/lib.rs
git commit -m "feat(scenedb): CellType registration with holistic stride check and token-keyed column access"
```

---

### Task 3: SIMD dispatch scaffold + scalar arm extraction

Factor the AABB predicate into a dispatched function with a runtime-selected backend. This task adds the scaffold and re-routes `SpatialCell::query_aabb` through it with ONLY the scalar arm, proving the dispatch is transparent (the M1a property oracle must still pass). The AVX2 arm lands in Task 4.

**Files:**
- Create: `crates/pulsar_scenedb/src/simd.rs`
- Modify: `crates/pulsar_scenedb/src/spatial.rs`
- Modify: `crates/pulsar_scenedb/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/pulsar_scenedb/src/simd.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_arm_matches_manual_predicate() {
        // Six columns, 5 rows. Build by hand and compare against the kernel.
        let min_x = [0.0f32, 10.0, 0.5, -5.0, 100.0];
        let max_x = [1.0f32, 11.0, 2.0, -4.0, 101.0];
        let min_y = [0.0f32; 5];
        let max_y = [1.0f32; 5];
        let min_z = [0.0f32; 5];
        let max_z = [1.0f32; 5];
        let live = 0b11111u64; // all live
        let q = QueryBounds { min: [0.0, 0.0, 0.0], max: [3.0, 3.0, 3.0] };
        let cols = Columns { min_x: &min_x, max_x: &max_x, min_y: &min_y, max_y: &max_y, min_z: &min_z, max_z: &max_z };
        let mut out = [0u32; 5];
        let hits = aabb_scan_scalar(&q, &cols, &[live], 5, &mut out);
        // rows 0 (0..1), 2 (0.5..2) intersect [0,3]; rows 1,3,4 don't.
        assert_eq!(out, [0, crate::registry::NULL_ROW, 2, crate::registry::NULL_ROW, crate::registry::NULL_ROW]);
        assert_eq!(hits, 2);
    }

    #[test]
    fn dead_rows_excluded_by_liveness_word() {
        let min_x = [0.0f32, 0.0];
        let max_x = [1.0f32, 1.0];
        let min_y = [0.0f32; 2];
        let max_y = [1.0f32; 2];
        let min_z = [0.0f32; 2];
        let max_z = [1.0f32; 2];
        let live = 0b01u64; // row 0 live, row 1 dead
        let q = QueryBounds { min: [0.0; 3], max: [1.0; 3] };
        let cols = Columns { min_x: &min_x, max_x: &max_x, min_y: &min_y, max_y: &max_y, min_z: &min_z, max_z: &max_z };
        let mut out = [0u32; 2];
        let hits = aabb_scan_scalar(&q, &cols, &[live], 2, &mut out);
        assert_eq!(out, [0, crate::registry::NULL_ROW]);
        assert_eq!(hits, 1);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p pulsar_scenedb simd`
Expected: FAIL — types not defined.

- [ ] **Step 3: Implement the scaffold + scalar arm**

Above the tests in `simd.rs`:

```rust
use crate::registry::NULL_ROW;

/// Query AABB in the kernel's own scalar layout (min/max per axis).
#[derive(Copy, Clone)]
pub struct QueryBounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// Borrowed bounds columns for one cell, sliced to the row count.
pub struct Columns<'a> {
    pub min_x: &'a [f32],
    pub max_x: &'a [f32],
    pub min_y: &'a [f32],
    pub max_y: &'a [f32],
    pub min_z: &'a [f32],
    pub max_z: &'a [f32],
}

/// Runtime-dispatched AABB scan. Selects the best available backend; all
/// backends produce bit-identical `out` buffers (the scalar arm is the
/// reference). `liveness_words` is the raw `LivenessMask` word slice;
/// `len` is the physical row count.
///
/// Writes `out[r] = r` on hit, `NULL_ROW` on miss/dead, for `r in 0..len`.
/// Returns the hit count. `out.len()` must be >= `len`.
#[inline]
pub fn aabb_scan(q: &QueryBounds, cols: &Columns, liveness_words: &[u64], len: usize, out: &mut [u32]) -> u32 {
    // Scalar-only in this task. Task 4 adds the AVX2 branch here; AVX-512/NEON
    // remain routed to scalar (correct, not yet optimized — scoping note 2).
    aabb_scan_scalar(q, cols, liveness_words, len, out)
}

/// Scalar reference. The §8.2 predicate with ordered IEEE comparisons,
/// liveness ANDed last. M1b SIMD arms must match this bit-for-bit.
pub fn aabb_scan_scalar(
    q: &QueryBounds,
    cols: &Columns,
    liveness_words: &[u64],
    len: usize,
    out: &mut [u32],
) -> u32 {
    debug_assert!(out.len() >= len);
    let mut hits = 0u32;
    for row in 0..len {
        let live = liveness_words[row / 64] & (1u64 << (row % 64)) != 0;
        let visible = cols.min_x[row] <= q.max[0]
            && cols.max_x[row] >= q.min[0]
            && cols.min_y[row] <= q.max[1]
            && cols.max_y[row] >= q.min[1]
            && cols.min_z[row] <= q.max[2]
            && cols.max_z[row] >= q.min[2]
            && live;
        out[row] = if visible {
            hits += 1;
            row as u32
        } else {
            NULL_ROW
        };
    }
    hits
}
```

In `lib.rs` add `pub mod simd;` and `pub use simd::{aabb_scan, QueryBounds};` (keep `Columns`/`aabb_scan_scalar` crate-visible via the module; export only what consumers need).

- [ ] **Step 4: Route `SpatialCell::query_aabb` through the dispatcher**

In `spatial.rs`, replace the body of `query_aabb` (keep the signature, the doc comments incl. the M1a NaN/ordered + trailing-entry contracts) with a call into the kernel:

```rust
    pub fn query_aabb(&self, q: &Aabb, out: &mut [u32]) -> u32 {
        let len = self.storage.rows_in_use() as usize;
        assert!(out.len() >= len, "scratch buffer too small");
        let min_x = &self.storage.user_column::<f32>(COL_MIN_X)[..len];
        let max_x = &self.storage.user_column::<f32>(COL_MAX_X)[..len];
        let min_y = &self.storage.user_column::<f32>(COL_MIN_Y)[..len];
        let max_y = &self.storage.user_column::<f32>(COL_MAX_Y)[..len];
        let min_z = &self.storage.user_column::<f32>(COL_MIN_Z)[..len];
        let max_z = &self.storage.user_column::<f32>(COL_MAX_Z)[..len];
        // LivenessMask words are AtomicU64; load them into a plain slice view.
        // We read under &self in the harvest phase (no concurrent writers, per
        // the phase contract), so a relaxed snapshot is correct here.
        let words: Vec<u64> = self
            .storage
            .liveness()
            .words()
            .iter()
            .map(|w| w.load(std::sync::atomic::Ordering::Relaxed))
            .collect();
        let qb = crate::simd::QueryBounds { min: q.min, max: q.max };
        let cols = crate::simd::Columns { min_x, max_x, min_y, max_y, min_z, max_z };
        crate::simd::aabb_scan(&qb, &cols, &words, len, out)
    }
```

NOTE on the `let words: Vec<u64>` line — keep exactly this explanatory comment above it (the allocation is a known interim that M2 removes by threading the Task 7 `Scratchpad` through the query path; do NOT write a bare `TODO`):

```rust
        // Liveness snapshot. This allocates; the §8.1 no-allocation contract is
        // honored end-to-end in M2, which threads the Task 7 Scratchpad through
        // the harvest path. Acceptable here because M1b proves the kernels.
```

`crate::simd::Columns` is `pub` within the crate (referenced by full path above), so no extra `use` is needed in spatial.rs.

- [ ] **Step 5: Run to verify pass**

Run: `cargo test -p pulsar_scenedb simd`
Run: `cargo test -p pulsar_scenedb spatial`
Expected: 2 simd tests PASS; all 4 spatial tests (incl. `property_matches_naive_reference`) STILL PASS — proving the dispatch is transparent.

- [ ] **Step 6: Commit**

```powershell
git add crates/pulsar_scenedb/src/simd.rs crates/pulsar_scenedb/src/spatial.rs crates/pulsar_scenedb/src/lib.rs
git commit -m "feat(scenedb): SIMD dispatch scaffold; route query_aabb through scalar kernel"
```

---

### Task 4: AVX2 AABB scan arm + cross-arm property test

Implement the AVX2 backend (8 f32 lanes/iteration) with **ordered** comparisons (matching the M1a NaN contract), and a property test asserting it is bit-identical to the scalar arm across random inputs — the bit-for-bit oracle.

**Files:**
- Modify: `crates/pulsar_scenedb/src/simd.rs`

- [ ] **Step 1: Write the failing cross-arm property test**

Append to the `#[cfg(test)] mod tests` in `simd.rs`:

```rust
    #[test]
    #[cfg(target_arch = "x86_64")]
    fn avx2_matches_scalar_bit_for_bit() {
        if !is_x86_feature_detected!("avx2") {
            eprintln!("AVX2 not available on this host; skipping");
            return;
        }
        use rand::{Rng, SeedableRng};
        let mut rng = rand::rngs::StdRng::seed_from_u64(0xA7F2 ^ 0x5CEDB);
        for _ in 0..200 {
            let len = rng.gen_range(0..=300usize);
            let gen_col = |rng: &mut rand::rngs::StdRng| (0..len).map(|_| rng.gen_range(-100.0f32..100.0)).collect::<Vec<_>>();
            let min_x = gen_col(&mut rng); let max_x: Vec<f32> = min_x.iter().map(|&m| m + rng.gen_range(0.0..10.0)).collect();
            let min_y = gen_col(&mut rng); let max_y: Vec<f32> = min_y.iter().map(|&m| m + rng.gen_range(0.0..10.0)).collect();
            let min_z = gen_col(&mut rng); let max_z: Vec<f32> = min_z.iter().map(|&m| m + rng.gen_range(0.0..10.0)).collect();
            let n_words = (len + 63) / 64;
            let words: Vec<u64> = (0..n_words).map(|_| rng.gen::<u64>()).collect();
            let q = QueryBounds {
                min: [rng.gen_range(-100.0..100.0), rng.gen_range(-100.0..100.0), rng.gen_range(-100.0..100.0)],
                max: [rng.gen_range(-100.0..100.0), rng.gen_range(-100.0..100.0), rng.gen_range(-100.0..100.0)],
            };
            let cols = Columns { min_x: &min_x, max_x: &max_x, min_y: &min_y, max_y: &max_y, min_z: &min_z, max_z: &max_z };
            let mut out_s = vec![0u32; len];
            let mut out_v = vec![0u32; len];
            let hs = aabb_scan_scalar(&q, &cols, &words, len, &mut out_s);
            // SAFETY: guarded by the runtime feature check above.
            let hv = unsafe { aabb_scan_avx2(&q, &cols, &words, len, &mut out_v) };
            assert_eq!(out_s, out_v, "AVX2 diverged from scalar at len={len}");
            assert_eq!(hs, hv);
        }
    }
```

(If `0xA7X2` is not valid hex, use `0xA7F2`.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p pulsar_scenedb simd::tests::avx2`
Expected: FAIL — `aabb_scan_avx2` not defined.

- [ ] **Step 3: Implement the AVX2 arm**

Add to `simd.rs` (module scope):

```rust
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub(crate) unsafe fn aabb_scan_avx2(
    q: &QueryBounds,
    cols: &Columns,
    liveness_words: &[u64],
    len: usize,
    out: &mut [u32],
) -> u32 {
    use std::arch::x86_64::*;
    debug_assert!(out.len() >= len);

    // Broadcast query bounds. Ordered comparisons (_CMP_*_OQ) so a NaN bound
    // yields false — bit-identical to the scalar `<=`/`>=` reference.
    let qmaxx = _mm256_set1_ps(q.max[0]);
    let qminx = _mm256_set1_ps(q.min[0]);
    let qmaxy = _mm256_set1_ps(q.max[1]);
    let qminy = _mm256_set1_ps(q.min[1]);
    let qmaxz = _mm256_set1_ps(q.max[2]);
    let qminz = _mm256_set1_ps(q.min[2]);

    let mut hits = 0u32;
    let mut row = 0usize;
    // Process 8 rows per iteration.
    while row + 8 <= len {
        let minx = _mm256_loadu_ps(cols.min_x.as_ptr().add(row));
        let maxx = _mm256_loadu_ps(cols.max_x.as_ptr().add(row));
        let miny = _mm256_loadu_ps(cols.min_y.as_ptr().add(row));
        let maxy = _mm256_loadu_ps(cols.max_y.as_ptr().add(row));
        let minz = _mm256_loadu_ps(cols.min_z.as_ptr().add(row));
        let maxz = _mm256_loadu_ps(cols.max_z.as_ptr().add(row));

        // box.min <= q.max  AND  box.max >= q.min, per axis (ordered).
        let mx = _mm256_and_ps(_mm256_cmp_ps(minx, qmaxx, _CMP_LE_OQ), _mm256_cmp_ps(maxx, qminx, _CMP_GE_OQ));
        let my = _mm256_and_ps(_mm256_cmp_ps(miny, qmaxy, _CMP_LE_OQ), _mm256_cmp_ps(maxy, qminy, _CMP_GE_OQ));
        let mz = _mm256_and_ps(_mm256_cmp_ps(minz, qmaxz, _CMP_LE_OQ), _mm256_cmp_ps(maxz, qminz, _CMP_GE_OQ));
        let geo = _mm256_and_ps(_mm256_and_ps(mx, my), mz);
        // 8-bit mask, one bit per lane (1 = geometric hit).
        let mut mask = _mm256_movemask_ps(geo) as u32;
        // AND in liveness for these 8 rows.
        let lw = liveness_words[row / 64];
        let live8 = ((lw >> (row % 64)) & 0xFF) as u32;
        mask &= live8;

        // Scatter results for the 8 lanes.
        for lane in 0..8usize {
            let r = row + lane;
            if (mask >> lane) & 1 != 0 {
                out[r] = r as u32;
                hits += 1;
            } else {
                out[r] = NULL_ROW;
            }
        }
        row += 8;
    }
    // Scalar tail (and any case where the 8-row liveness straddles a word
    // boundary is avoided because pages are 64-aligned and we step by 8,
    // so row%64 ∈ {0,8,...,56} and the 8-bit window never crosses a word).
    while row < len {
        let live = liveness_words[row / 64] & (1u64 << (row % 64)) != 0;
        let visible = cols.min_x[row] <= q.max[0]
            && cols.max_x[row] >= q.min[0]
            && cols.min_y[row] <= q.max[1]
            && cols.max_y[row] >= q.min[1]
            && cols.min_z[row] <= q.max[2]
            && cols.max_z[row] >= q.min[2]
            && live;
        out[row] = if visible { hits += 1; row as u32 } else { NULL_ROW };
        row += 1;
    }
    hits
}
```

Then wire the AVX2 branch into the `aabb_scan` dispatcher (it was scalar-only after Task 3). Replace the body of `aabb_scan` with:

```rust
#[inline]
pub fn aabb_scan(q: &QueryBounds, cols: &Columns, liveness_words: &[u64], len: usize, out: &mut [u32]) -> u32 {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            // SAFETY: guarded by the runtime feature check.
            return unsafe { aabb_scan_avx2(q, cols, liveness_words, len, out) };
        }
    }
    aabb_scan_scalar(q, cols, liveness_words, len, out)
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p pulsar_scenedb simd`
Expected: all simd tests PASS, including `avx2_matches_scalar_bit_for_bit` (200 random cases).
Run: `cargo test -p pulsar_scenedb spatial`
Expected: still green (query_aabb now uses AVX2 where available, still matches the naive oracle).

- [ ] **Step 5: Commit**

```powershell
git add crates/pulsar_scenedb/src/simd.rs
git commit -m "feat(scenedb): AVX2 AABB scan arm, bit-for-bit verified against scalar reference"
```

---

### Task 5: Frustum query (scalar reference) on SpatialCell

Add a 6-plane frustum query alongside the AABB query (spec §8.1: queries accept an AABB OR a frustum). Scalar reference; the SIMD frustum arm is Task 6.

**Files:**
- Modify: `crates/pulsar_scenedb/src/spatial.rs`
- Modify: `crates/pulsar_scenedb/src/simd.rs`
- Modify: `crates/pulsar_scenedb/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Append to the `#[cfg(test)] mod tests` in `spatial.rs`:

```rust
    fn unit_box(at: [f32; 3]) -> Aabb {
        Aabb { min: at, max: [at[0] + 1.0, at[1] + 1.0, at[2] + 1.0] }
    }

    #[test]
    fn frustum_keeps_inside_culls_outside() {
        let mut c = SpatialCell::new(64).unwrap();
        let _inside = c.alloc(unit_box([0.0, 0.0, 0.0])).unwrap();
        let _outside = c.alloc(unit_box([100.0, 0.0, 0.0])).unwrap();
        // Six planes of an axis-aligned box [-10,10]^3, inward normals.
        // Plane: (nx,ny,nz,d) with point inside iff n·p + d >= 0.
        let planes = [
            [1.0, 0.0, 0.0, 10.0],   // x >= -10
            [-1.0, 0.0, 0.0, 10.0],  // x <= 10
            [0.0, 1.0, 0.0, 10.0],
            [0.0, -1.0, 0.0, 10.0],
            [0.0, 0.0, 1.0, 10.0],
            [0.0, 0.0, -1.0, 10.0],
        ];
        let f = Frustum { planes };
        let mut out = vec![0u32; c.rows_in_use() as usize];
        let n = c.query_frustum(&f, &mut out);
        assert_eq!(n, 1, "only the box at origin is inside");
        assert_eq!(out[0], 0);
        assert_eq!(out[1], crate::registry::NULL_ROW);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p pulsar_scenedb frustum`
Expected: FAIL — `Frustum`/`query_frustum` not defined.

- [ ] **Step 3: Implement the scalar frustum kernel**

In `simd.rs`, add the frustum scalar scan. The conservative AABB-vs-plane test: a box is outside a plane iff its farthest corner along the inward normal is still behind the plane. For inward normal `n` and offset `d` (inside iff `n·p + d >= 0`), the box is **outside** iff the "positive vertex" (corner maximizing `n·p`) has `n·p_far + d < 0`. Cull only when outside ALL... no — cull when outside ANY plane.

```rust
/// Six frustum planes, each `[nx, ny, nz, d]` with inward normal; a point `p`
/// is inside the plane iff `nx*px + ny*py + nz*pz + d >= 0`.
#[derive(Copy, Clone)]
pub struct FrustumPlanes {
    pub planes: [[f32; 4]; 6],
}

/// Scalar frustum scan. A box passes iff, for every plane, its positive
/// vertex (the corner farthest along the inward normal) is inside. Writes
/// `out[r] = r` on pass, `NULL_ROW` on cull/dead. Returns the pass count.
pub fn frustum_scan_scalar(
    f: &FrustumPlanes,
    cols: &Columns,
    liveness_words: &[u64],
    len: usize,
    out: &mut [u32],
) -> u32 {
    debug_assert!(out.len() >= len);
    let mut hits = 0u32;
    for row in 0..len {
        let live = liveness_words[row / 64] & (1u64 << (row % 64)) != 0;
        let bmin = [cols.min_x[row], cols.min_y[row], cols.min_z[row]];
        let bmax = [cols.max_x[row], cols.max_y[row], cols.max_z[row]];
        let mut inside = live;
        let mut p = 0;
        while inside && p < 6 {
            let pl = f.planes[p];
            // Positive vertex: pick max-projection corner per axis.
            let px = if pl[0] >= 0.0 { bmax[0] } else { bmin[0] };
            let py = if pl[1] >= 0.0 { bmax[1] } else { bmin[1] };
            let pz = if pl[2] >= 0.0 { bmax[2] } else { bmin[2] };
            if pl[0] * px + pl[1] * py + pl[2] * pz + pl[3] < 0.0 {
                inside = false; // positive vertex behind plane → fully outside
            }
            p += 1;
        }
        out[row] = if inside { hits += 1; row as u32 } else { NULL_ROW };
    }
    hits
}

/// Runtime-dispatched frustum scan (scalar for now; AVX2 arm in Task 6).
#[inline]
pub fn frustum_scan(f: &FrustumPlanes, cols: &Columns, liveness_words: &[u64], len: usize, out: &mut [u32]) -> u32 {
    frustum_scan_scalar(f, cols, liveness_words, len, out)
}
```

- [ ] **Step 4: Add `Frustum` + `query_frustum` to `SpatialCell`**

In `spatial.rs`, add the public `Frustum` type and the method (mirroring `query_aabb`'s column-binding + liveness snapshot):

```rust
/// Six inward-normal frustum planes (spec §8.1 frustum query input).
#[derive(Copy, Clone, Debug)]
pub struct Frustum {
    pub planes: [[f32; 4]; 6],
}

impl SpatialCell {
    /// Frustum query (§8.1). Same positional-token output contract as
    /// `query_aabb` (`out[r] = r` on pass, `NULL_ROW` on cull/dead;
    /// `out[rows_in_use()..]` untouched).
    pub fn query_frustum(&self, f: &Frustum, out: &mut [u32]) -> u32 {
        let len = self.storage.rows_in_use() as usize;
        assert!(out.len() >= len, "scratch buffer too small");
        let min_x = &self.storage.user_column::<f32>(COL_MIN_X)[..len];
        let max_x = &self.storage.user_column::<f32>(COL_MAX_X)[..len];
        let min_y = &self.storage.user_column::<f32>(COL_MIN_Y)[..len];
        let max_y = &self.storage.user_column::<f32>(COL_MAX_Y)[..len];
        let min_z = &self.storage.user_column::<f32>(COL_MIN_Z)[..len];
        let max_z = &self.storage.user_column::<f32>(COL_MAX_Z)[..len];
        // Liveness snapshot, sliced to the words covering rows 0..len (the
        // `liveness_words.len() == ceil(len/64)` kernel contract; M2 threads the
        // Task 7 Scratchpad through to honor §8.1 no-alloc).
        let n_words = (len as u64).div_ceil(64) as usize;
        let words: Vec<u64> = self.storage.liveness().words().iter().take(n_words)
            .map(|w| w.load(std::sync::atomic::Ordering::Relaxed)).collect();
        let fp = crate::simd::FrustumPlanes { planes: f.planes };
        let cols = crate::simd::Columns { min_x, max_x, min_y, max_y, min_z, max_z };
        crate::simd::frustum_scan(&fp, &cols, &words, len, out)
    }
}
```

In `lib.rs` extend the spatial re-export: `pub use spatial::{Aabb, Frustum, SpatialCell, SPATIAL_COLUMNS};` and the simd re-export to include `FrustumPlanes` if consumers need it (keep it crate-internal otherwise).

- [ ] **Step 5: Run to verify pass**

Run: `cargo test -p pulsar_scenedb frustum`
Run: `cargo test -p pulsar_scenedb --lib --tests`
Expected: frustum test PASS; full suite green.

- [ ] **Step 6: Commit**

```powershell
git add crates/pulsar_scenedb/src/spatial.rs crates/pulsar_scenedb/src/simd.rs crates/pulsar_scenedb/src/lib.rs
git commit -m "feat(scenedb): scalar frustum query (positive-vertex plane test) on SpatialCell"
```

---

### Task 6: AVX2 frustum arm + cross-arm property test

**Files:**
- Modify: `crates/pulsar_scenedb/src/simd.rs`

- [ ] **Step 1: Write the failing property test**

Append to `simd.rs` tests:

```rust
    #[test]
    #[cfg(target_arch = "x86_64")]
    fn avx2_frustum_matches_scalar() {
        if !is_x86_feature_detected!("avx2") { return; }
        use rand::{Rng, SeedableRng};
        let mut rng = rand::rngs::StdRng::seed_from_u64(0xF2057);
        for _ in 0..200 {
            let len = rng.gen_range(0..=300usize);
            let col = |rng: &mut rand::rngs::StdRng| (0..len).map(|_| rng.gen_range(-50.0f32..50.0)).collect::<Vec<_>>();
            let min_x = col(&mut rng); let max_x: Vec<f32> = min_x.iter().map(|&m| m + rng.gen_range(0.0..5.0)).collect();
            let min_y = col(&mut rng); let max_y: Vec<f32> = min_y.iter().map(|&m| m + rng.gen_range(0.0..5.0)).collect();
            let min_z = col(&mut rng); let max_z: Vec<f32> = min_z.iter().map(|&m| m + rng.gen_range(0.0..5.0)).collect();
            let words: Vec<u64> = (0..(len + 63) / 64).map(|_| rng.gen::<u64>()).collect();
            let mut planes = [[0.0f32; 4]; 6];
            for pl in &mut planes { for v in pl.iter_mut() { *v = rng.gen_range(-20.0..20.0); } }
            let f = FrustumPlanes { planes };
            let cols = Columns { min_x: &min_x, max_x: &max_x, min_y: &min_y, max_y: &max_y, min_z: &min_z, max_z: &max_z };
            let mut a = vec![0u32; len]; let mut b = vec![0u32; len];
            let ha = frustum_scan_scalar(&f, &cols, &words, len, &mut a);
            let hb = unsafe { frustum_scan_avx2(&f, &cols, &words, len, &mut b) };
            assert_eq!(a, b, "AVX2 frustum diverged at len={len}");
            assert_eq!(ha, hb);
        }
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p pulsar_scenedb simd::tests::avx2_frustum`
Expected: FAIL — `frustum_scan_avx2` not defined.

- [ ] **Step 3: Implement the AVX2 frustum arm and wire dispatch**

Add to `simd.rs`:

```rust
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub(crate) unsafe fn frustum_scan_avx2(
    f: &FrustumPlanes,
    cols: &Columns,
    liveness_words: &[u64],
    len: usize,
    out: &mut [u32],
) -> u32 {
    use std::arch::x86_64::*;
    debug_assert!(out.len() >= len);
    let mut hits = 0u32;
    let mut row = 0usize;
    while row + 8 <= len {
        let minx = _mm256_loadu_ps(cols.min_x.as_ptr().add(row));
        let maxx = _mm256_loadu_ps(cols.max_x.as_ptr().add(row));
        let miny = _mm256_loadu_ps(cols.min_y.as_ptr().add(row));
        let maxy = _mm256_loadu_ps(cols.max_y.as_ptr().add(row));
        let minz = _mm256_loadu_ps(cols.min_z.as_ptr().add(row));
        let maxz = _mm256_loadu_ps(cols.max_z.as_ptr().add(row));
        // inside accumulator: all-ones, ANDed by each plane's "inside" mask.
        let mut inside = _mm256_castsi256_ps(_mm256_set1_epi32(-1));
        for p in 0..6usize {
            let pl = f.planes[p];
            let nx = _mm256_set1_ps(pl[0]);
            let ny = _mm256_set1_ps(pl[1]);
            let nz = _mm256_set1_ps(pl[2]);
            let d = _mm256_set1_ps(pl[3]);
            // positive vertex per axis: nx>=0 ? maxx : minx (branchless blend
            // on the sign mask of the plane normal component).
            let selx = _mm256_cmp_ps(nx, _mm256_setzero_ps(), _CMP_GE_OQ);
            let sely = _mm256_cmp_ps(ny, _mm256_setzero_ps(), _CMP_GE_OQ);
            let selz = _mm256_cmp_ps(nz, _mm256_setzero_ps(), _CMP_GE_OQ);
            let px = _mm256_blendv_ps(minx, maxx, selx);
            let py = _mm256_blendv_ps(miny, maxy, sely);
            let pz = _mm256_blendv_ps(minz, maxz, selz);
            // dot = ((nx*px + ny*py) + nz*pz) + d
            // The association MUST match the scalar reference exactly —
            // `((a+b)+c)+d` — because f32 addition is not associative and the
            // property test asserts bit-identical results. Separate mul+add
            // (no FMA contraction) matches the scalar path's rounding.
            let dot = _mm256_add_ps(
                _mm256_add_ps(
                    _mm256_add_ps(_mm256_mul_ps(nx, px), _mm256_mul_ps(ny, py)),
                    _mm256_mul_ps(nz, pz),
                ),
                d,
            );
            // inside-this-plane iff dot >= 0 (ordered).
            let inplane = _mm256_cmp_ps(dot, _mm256_setzero_ps(), _CMP_GE_OQ);
            inside = _mm256_and_ps(inside, inplane);
        }
        let mut mask = _mm256_movemask_ps(inside) as u32;
        let lw = liveness_words[row / 64];
        mask &= ((lw >> (row % 64)) & 0xFF) as u32;
        for lane in 0..8usize {
            let r = row + lane;
            if (mask >> lane) & 1 != 0 { out[r] = r as u32; hits += 1; } else { out[r] = NULL_ROW; }
        }
        row += 8;
    }
    // Scalar tail.
    while row < len {
        let live = liveness_words[row / 64] & (1u64 << (row % 64)) != 0;
        let bmin = [cols.min_x[row], cols.min_y[row], cols.min_z[row]];
        let bmax = [cols.max_x[row], cols.max_y[row], cols.max_z[row]];
        let mut inside = live;
        let mut p = 0;
        while inside && p < 6 {
            let pl = f.planes[p];
            let px = if pl[0] >= 0.0 { bmax[0] } else { bmin[0] };
            let py = if pl[1] >= 0.0 { bmax[1] } else { bmin[1] };
            let pz = if pl[2] >= 0.0 { bmax[2] } else { bmin[2] };
            if pl[0]*px + pl[1]*py + pl[2]*pz + pl[3] < 0.0 { inside = false; }
            p += 1;
        }
        out[row] = if inside { hits += 1; row as u32 } else { NULL_ROW };
        row += 1;
    }
    hits
}
```

Then update `frustum_scan` dispatch to use AVX2 when available (mirror the `aabb_scan` dispatch):

```rust
#[inline]
pub fn frustum_scan(f: &FrustumPlanes, cols: &Columns, liveness_words: &[u64], len: usize, out: &mut [u32]) -> u32 {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            // SAFETY: guarded by the runtime feature check.
            return unsafe { frustum_scan_avx2(f, cols, liveness_words, len, out) };
        }
    }
    frustum_scan_scalar(f, cols, liveness_words, len, out)
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p pulsar_scenedb simd`
Expected: all simd tests PASS incl. `avx2_frustum_matches_scalar`.
Run: `cargo test -p pulsar_scenedb frustum` — still green.

- [ ] **Step 5: Commit**

```powershell
git add crates/pulsar_scenedb/src/simd.rs
git commit -m "feat(scenedb): AVX2 frustum arm, bit-for-bit verified against scalar reference"
```

---

### Task 7: Lease-slot pool + per-cell lease bitmask + thread-local scratchpads

The read-lease primitive (C4, §9.2): a fixed 64-slot pool, a per-cell atomic `u64` lease bitmask, and thread-local scratchpad pools with the 8-frame/50% decay policy. This also provides the reusable liveness-snapshot buffer that Tasks 3/5 flagged.

**Files:**
- Create: `crates/pulsar_scenedb/src/lease.rs`
- Modify: `crates/pulsar_scenedb/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/pulsar_scenedb/src/lease.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_release_lease_slots() {
        let mask = LeaseMask::new();
        let a = mask.acquire().unwrap();
        let b = mask.acquire().unwrap();
        assert_ne!(a.slot(), b.slot());
        assert!(mask.any_held());
        drop(a);
        drop(b);
        assert!(!mask.any_held(), "all leases released");
    }

    #[test]
    fn pool_exhaustion_returns_none() {
        let mask = LeaseMask::new();
        let mut held = Vec::new();
        for _ in 0..LEASE_SLOTS {
            held.push(mask.acquire().unwrap());
        }
        assert!(mask.acquire().is_none(), "65th acquire fails on a full pool");
        drop(held);
        assert!(mask.acquire().is_some(), "slot frees after release");
    }

    #[test]
    fn scratchpad_grows_then_decays() {
        let mut pad = Scratchpad::new();
        // Burst: request a big buffer.
        {
            let buf = pad.get_u32(1000);
            assert!(buf.len() >= 1000);
        }
        let cap_before = pad.capacity_u32();
        assert!(cap_before >= 1000);
        // First decay window: the burst's peak (1000) lands in THIS window, so
        // peak*2 >= cap → no decay (the window's peak must drop below 50% first).
        for _ in 0..DECAY_FRAMES {
            let _ = pad.get_u32(10);
            pad.end_frame();
        }
        // Second decay window: sustained low use (peak 10 << 50% of cap) → halve.
        for _ in 0..DECAY_FRAMES {
            let _ = pad.get_u32(10);
            pad.end_frame();
        }
        assert!(pad.capacity_u32() < cap_before, "capacity decayed after a low-usage window");
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p pulsar_scenedb lease`
Expected: FAIL — types not defined.

- [ ] **Step 3: Implement leases + scratchpad**

Above the tests in `lease.rs`:

```rust
use std::sync::atomic::{AtomicU64, Ordering};

/// Number of concurrent read-lease slots per cell (spec §9.2, matches the
/// bitmask width). Not bound to thread identity — acquired from a pool, so
/// dynamic pools / work-stealing / nesting all work.
pub const LEASE_SLOTS: usize = 64;

/// Per-cell atomic lease bitmask. A reader acquires a slot for the duration of
/// a query; the frame-boundary compaction checks `any_held()` is false before
/// swap-and-pop (enforced by Layer 2's phase machine in M2).
pub struct LeaseMask {
    bits: AtomicU64,
}

/// RAII lease guard — releases its slot on drop.
pub struct Lease<'a> {
    mask: &'a LeaseMask,
    slot: u32,
}

impl LeaseMask {
    #[must_use]
    pub fn new() -> Self {
        Self { bits: AtomicU64::new(0) }
    }

    /// Acquire a free lease slot, or None if the pool is exhausted.
    pub fn acquire(&self) -> Option<Lease<'_>> {
        loop {
            let cur = self.bits.load(Ordering::Acquire);
            if cur == u64::MAX {
                return None; // all 64 slots held
            }
            let slot = cur.trailing_ones(); // first 0 bit
            let bit = 1u64 << slot;
            if self
                .bits
                .compare_exchange_weak(cur, cur | bit, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Some(Lease { mask: self, slot });
            }
        }
    }

    #[must_use]
    pub fn any_held(&self) -> bool {
        self.bits.load(Ordering::Acquire) != 0
    }

    fn release(&self, slot: u32) {
        self.bits.fetch_and(!(1u64 << slot), Ordering::AcqRel);
    }
}

impl Default for LeaseMask {
    fn default() -> Self {
        Self::new()
    }
}

impl Lease<'_> {
    #[inline]
    #[must_use]
    pub fn slot(&self) -> u32 {
        self.slot
    }
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        self.mask.release(self.slot);
    }
}

/// Thread-local scratchpad with the 8-frame / 50% decay policy (spec §9.1).
/// Holds reusable query buffers so the harvest path never touches the heap
/// mid-frame after warm-up.
pub struct Scratchpad {
    u32_buf: Vec<u32>,
    peak_this_window: usize,
    frames_in_window: u32,
}

/// Frames of sustained low usage before halving (spec §9.1 default).
pub const DECAY_FRAMES: u32 = 8;

impl Scratchpad {
    #[must_use]
    pub fn new() -> Self {
        Self { u32_buf: Vec::new(), peak_this_window: 0, frames_in_window: 0 }
    }

    /// Borrow a u32 buffer of at least `len`, growing if needed. The buffer is
    /// not zeroed (callers overwrite `[0..used]`).
    pub fn get_u32(&mut self, len: usize) -> &mut [u32] {
        if self.u32_buf.len() < len {
            self.u32_buf.resize(len, 0);
        }
        self.peak_this_window = self.peak_this_window.max(len);
        &mut self.u32_buf[..len]
    }

    #[must_use]
    pub fn capacity_u32(&self) -> usize {
        self.u32_buf.len()
    }

    /// Advance the decay window. After `DECAY_FRAMES` frames whose peak usage
    /// stayed below 50% of capacity, halve the buffer.
    pub fn end_frame(&mut self) {
        self.frames_in_window += 1;
        if self.frames_in_window >= DECAY_FRAMES {
            let cap = self.u32_buf.len();
            if cap > 0 && self.peak_this_window * 2 < cap {
                let new_cap = cap / 2;
                self.u32_buf.truncate(new_cap);
                self.u32_buf.shrink_to_fit();
            }
            self.frames_in_window = 0;
            self.peak_this_window = 0;
        }
    }
}

impl Default for Scratchpad {
    fn default() -> Self {
        Self::new()
    }
}
```

In `lib.rs` add `pub mod lease;` and `pub use lease::{Lease, LeaseMask, Scratchpad, LEASE_SLOTS, DECAY_FRAMES};`.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p pulsar_scenedb lease`
Expected: 3 tests PASS.

- [ ] **Step 5: Commit**

```powershell
git add crates/pulsar_scenedb/src/lease.rs crates/pulsar_scenedb/src/lib.rs
git commit -m "feat(scenedb): lease-slot pool, per-cell lease bitmask, decaying thread-local scratchpads"
```

---

### Task 8: Lease revocation + double-buffered liveness snapshot (§9.2.1)

The revocation path: a long-held lease is forcibly revoked at the frame boundary by snapshotting the liveness into a double buffer, so compaction proceeds against the live layout while the stale holder reads the pinned snapshot. M1b implements the snapshot mechanism + revocation flag; the 2.0 ms timer wiring is M2.

**Files:**
- Create: `crates/pulsar_scenedb/src/snapshot.rs`
- Modify: `crates/pulsar_scenedb/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/pulsar_scenedb/src/snapshot.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::liveness::LivenessMask;

    #[test]
    fn snapshot_pins_liveness_at_capture_time() {
        let mask = LivenessMask::new(128);
        for i in 0..10 { mask.set_live(i); }
        let snap = LivenessSnapshot::capture(&mask, 10);
        // Mutate the live mask after the snapshot.
        mask.set_dead(3);
        // Snapshot still reflects capture-time state.
        assert!(snap.is_live(3), "snapshot is pinned");
        assert!(!mask.is_live(3), "live mask moved on");
        assert_eq!(snap.live_count(), 10);
    }

    #[test]
    fn revocation_flag_round_trips() {
        let rev = RevocationFlag::new();
        assert!(!rev.is_revoked());
        rev.revoke();
        assert!(rev.is_revoked());
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p pulsar_scenedb snapshot`
Expected: FAIL — types not defined.

- [ ] **Step 3: Implement the snapshot + revocation flag**

Above the tests in `snapshot.rs`:

```rust
use crate::liveness::LivenessMask;
use std::sync::atomic::{AtomicBool, Ordering};

/// A pinned, immutable copy of a cell's liveness words at capture time
/// (spec §9.2.1 double-buffered state mask). A revoked lease holder reads its
/// pinned snapshot while compaction proceeds against the live mask.
pub struct LivenessSnapshot {
    words: Vec<u64>,
    len: u32,
}

impl LivenessSnapshot {
    /// Capture `len` rows of `mask` into an owned snapshot (relaxed loads;
    /// the caller holds the phase barrier).
    #[must_use]
    pub fn capture(mask: &LivenessMask, len: u32) -> Self {
        let words = mask.words().iter().map(|w| w.load(Ordering::Relaxed)).collect();
        Self { words, len }
    }

    #[inline]
    #[must_use]
    pub fn is_live(&self, row: u32) -> bool {
        row < self.len && self.words[(row / 64) as usize] & (1u64 << (row % 64)) != 0
    }

    #[must_use]
    pub fn live_count(&self) -> u32 {
        self.words.iter().map(|w| w.count_ones()).sum::<u32>().min(self.len)
    }

    /// Raw snapshot words (for SIMD scans against the pinned topology).
    #[must_use]
    pub fn words(&self) -> &[u64] {
        &self.words
    }
}

/// A one-shot revocation flag for a lease (spec §9.2.1). Set by Layer 2 when a
/// lease exceeds its timeout; the holder re-validates against live generations
/// on use after seeing it set.
pub struct RevocationFlag {
    revoked: AtomicBool,
}

impl RevocationFlag {
    #[must_use]
    pub fn new() -> Self {
        Self { revoked: AtomicBool::new(false) }
    }

    pub fn revoke(&self) {
        self.revoked.store(true, Ordering::Release);
    }

    #[must_use]
    pub fn is_revoked(&self) -> bool {
        self.revoked.load(Ordering::Acquire)
    }
}

impl Default for RevocationFlag {
    fn default() -> Self {
        Self::new()
    }
}
```

In `lib.rs` add `pub mod snapshot;` and `pub use snapshot::{LivenessSnapshot, RevocationFlag};`.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p pulsar_scenedb snapshot`
Expected: 2 tests PASS.

- [ ] **Step 5: Commit**

```powershell
git add crates/pulsar_scenedb/src/snapshot.rs crates/pulsar_scenedb/src/lib.rs
git commit -m "feat(scenedb): double-buffered liveness snapshot and lease revocation flag (§9.2.1)"
```

---

### Task 9: Part VI Test 2 (host-half) — stale-handle rejection suite

A focused integration test proving the C1 stale-handle contract end-to-end at the host level (the GPU-side half is M3). This is a named verification gate.

**Files:**
- Create: `crates/pulsar_scenedb/tests/stale_handle.rs`

- [ ] **Step 1: Write the test**

Create `crates/pulsar_scenedb/tests/stale_handle.rs`:

```rust
//! Part VI — Test 2 (host half): stale-handle rejection.
//! Spec §21 Test 2 / CONTRACTS.md C1. The GPU-side validation is M3.

use pulsar_scenedb::{Aabb, SpatialCell};

#[test]
fn freed_handle_never_resolves() {
    let mut c = SpatialCell::new(64).unwrap();
    let h = c.alloc(Aabb { min: [0.0; 3], max: [1.0; 3] }).unwrap();
    assert!(c.row_of(h).is_some());
    c.free(h);
    assert_eq!(c.row_of(h), None, "freed handle must not resolve");
    c.compact();
    assert_eq!(c.row_of(h), None, "still dead after compaction");
}

#[test]
fn recycled_slot_rejects_old_generation() {
    let mut c = SpatialCell::new(64).unwrap();
    let h1 = c.alloc(Aabb { min: [0.0; 3], max: [1.0; 3] }).unwrap();
    c.free(h1);
    c.compact();
    let h2 = c.alloc(Aabb { min: [5.0; 3], max: [6.0; 3] }).unwrap();
    // Same physical slot index, bumped generation.
    assert_eq!(h2.index(), h1.index());
    assert!(h2.generation() > h1.generation());
    // The OLD handle must be rejected even though its slot is live again.
    assert_eq!(c.row_of(h1), None, "stale generation rejected");
    assert!(c.row_of(h2).is_some(), "fresh handle valid");
}

#[test]
fn stale_handle_absent_from_query_output() {
    // After free+compact, a query must never emit the freed element's row.
    let mut c = SpatialCell::new(64).unwrap();
    let ha = c.alloc(Aabb { min: [0.0; 3], max: [1.0; 3] }).unwrap();
    let hb = c.alloc(Aabb { min: [0.0; 3], max: [1.0; 3] }).unwrap();
    c.free(ha);
    c.compact();
    let mut out = vec![0u32; c.rows_in_use() as usize];
    let n = c.query_aabb(&Aabb { min: [-1.0; 3], max: [2.0; 3] }, &mut out);
    assert_eq!(n, 1, "only the surviving element is a hit");
    // The survivor (hb) resolves; ha does not.
    assert!(c.row_of(hb).is_some());
    assert_eq!(c.row_of(ha), None);
}
```

- [ ] **Step 2: Run to verify pass**

Run: `cargo test -p pulsar_scenedb --test stale_handle`
Expected: 3 tests PASS (no implementation needed — this validates existing behavior; if any fail, it's a real M1a regression to investigate, not a test to weaken).

- [ ] **Step 3: Commit**

```powershell
git add crates/pulsar_scenedb/tests/stale_handle.rs
git commit -m "test(scenedb): Part VI Test 2 host-half — stale-handle rejection gate"
```

---

### Task 10: Part VI Test 1 — multi-threaded contention gate

Proves the phase-separated concurrency model: one writer mutates a cell while reader threads run queries against a snapshot, with zero data races (run under the thread sanitizer when available) and no deadlock. Per the phase contract, readers operate on a captured `LivenessSnapshot` (no concurrent structural writes during the read window) — this test models that discipline directly.

**Files:**
- Create: `crates/pulsar_scenedb/tests/contention.rs`

- [ ] **Step 1: Write the test**

Create `crates/pulsar_scenedb/tests/contention.rs`:

```rust
//! Part VI — Test 1: multi-threaded contention.
//! Spec §21 Test 1. Models the phase contract: a write window (exclusive)
//! followed by a read window where N reader threads query a shared immutable
//! view concurrently. Verifies no data races (run with
//! `RUSTFLAGS="-Zsanitizer=thread"` on nightly where available) and no
//! deadlock over many rounds.

use pulsar_scenedb::{Aabb, SpatialCell};
use std::sync::Arc;
use std::thread;

#[test]
fn concurrent_readers_no_races_no_deadlock() {
    // Build a populated cell (write window, exclusive).
    let mut cell = SpatialCell::new(1024).unwrap();
    for i in 0..1024u32 {
        let f = i as f32;
        cell.alloc(Aabb { min: [f, 0.0, 0.0], max: [f + 1.0, 1.0, 1.0] }).unwrap();
    }
    // Freeze the cell as a shared immutable view for the read window.
    let cell = Arc::new(cell);

    // 8 reader threads, each running many AABB queries into its OWN scratch
    // buffer (spec §8.4: one scratch per view, concurrent reads are safe with
    // no in-progress writes).
    let mut handles = Vec::new();
    for t in 0..8u32 {
        let cell = Arc::clone(&cell);
        handles.push(thread::spawn(move || {
            let mut out = vec![0u32; cell.rows_in_use() as usize];
            let mut total = 0u64;
            for round in 0..2000u32 {
                let lo = ((t * 53 + round) % 1024) as f32;
                let q = Aabb { min: [lo, 0.0, 0.0], max: [lo + 64.0, 1.0, 1.0] };
                total += cell.query_aabb(&q, &mut out) as u64;
            }
            total
        }));
    }
    // All readers complete (no deadlock) and return plausible counts.
    let mut grand = 0u64;
    for h in handles {
        grand += h.join().expect("reader thread panicked");
    }
    assert!(grand > 0, "readers found hits");
}

#[test]
fn concurrent_queries_match_single_threaded() {
    // Determinism under concurrency: each thread's result for a fixed query
    // equals the single-threaded result.
    let mut cell = SpatialCell::new(512).unwrap();
    for i in 0..512u32 {
        let f = i as f32;
        cell.alloc(Aabb { min: [f, 0.0, 0.0], max: [f + 1.0, 1.0, 1.0] }).unwrap();
    }
    let q = Aabb { min: [0.0, 0.0, 0.0], max: [255.0, 1.0, 1.0] };
    let mut ref_out = vec![0u32; cell.rows_in_use() as usize];
    let expected = cell.query_aabb(&q, &mut ref_out);

    let cell = Arc::new(cell);
    let mut handles = Vec::new();
    for _ in 0..8 {
        let cell = Arc::clone(&cell);
        let q = q;
        handles.push(thread::spawn(move || {
            let mut out = vec![0u32; cell.rows_in_use() as usize];
            cell.query_aabb(&q, &mut out)
        }));
    }
    for h in handles {
        assert_eq!(h.join().unwrap(), expected, "concurrent query result is deterministic");
    }
}
```

- [ ] **Step 2: Run to verify pass**

Run: `cargo test -p pulsar_scenedb --test contention`
Expected: 2 tests PASS. (If a thread sanitizer is available on the toolchain: `RUSTFLAGS="-Zsanitizer=thread" cargo +nightly test -p pulsar_scenedb --test contention --target x86_64-unknown-linux-gnu` — on Windows/stable, the logical-race-freedom is still exercised by the concurrent run. Note in the commit which mode was used.)

- [ ] **Step 3: Commit**

```powershell
git add crates/pulsar_scenedb/tests/contention.rs
git commit -m "test(scenedb): Part VI Test 1 — multi-threaded contention gate"
```

---

### Task 11: SIMD benchmark + milestone docs wrap-up

Extend the bench with the SIMD-vs-scalar comparison (quantifying the AVX2 win over the M1a 762 ns baseline) and a frustum bench, and update the crate docs to reflect the completed Layer 1.

**Files:**
- Modify: `crates/pulsar_scenedb/benches/scenedb_bench.rs`
- Modify: `crates/pulsar_scenedb/src/lib.rs` (crate doc)

- [ ] **Step 1: Add SIMD + frustum benches**

Append two bench functions to `scenedb_bench.rs` and register them in the `criterion_group!`. Add the imports `use pulsar_scenedb::Frustum;` and use the existing `SpatialCell`/`Aabb`:

```rust
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
```

Update the `criterion_group!` line to include them:

```rust
criterion_group!(benches, bench_query, bench_churn, bench_aabb_dispatch, bench_frustum);
```

- [ ] **Step 2: Run the benches**

Run: `cargo bench -p pulsar_scenedb --bench scenedb_bench -- --warm-up-time 1 --measurement-time 2`
Expected: completes; record `dispatched_aabb_scan_1024` (compare to the M1a `scalar_aabb_scan_1024` ≈ 762 ns baseline to quantify the AVX2 speedup) and `frustum_scan_1024`.

- [ ] **Step 3: Update the crate doc**

In `lib.rs`, extend the module-doc bullet list (added in M1a Task 9) to include the M1b additions. After the `[`SpatialCell`]` bullet, add:

```rust
//! - [`TypeToken`]/[`CellType`] — dense column-type tokens bridged to
//!   `pulsar_reflection`; holistic-stride-checked cell composition
//! - SIMD query dispatch ([`aabb_scan`](simd::aabb_scan)) — AVX2 arms verified
//!   bit-for-bit against the scalar reference; frustum + AABB
//! - [`LeaseMask`]/[`Scratchpad`]/[`LivenessSnapshot`] — read-lease pool,
//!   decaying scratchpads, double-buffered revocation (§9; phase machine is M2)
//!
//! Milestone status: M1a (storage core) + M1b (type bridge, SIMD, leases) —
//! Layer 1 complete. Verified by Part VI Test 1 (contention) and Test 2 host
//! half (stale-handle). Layer 2 orchestration is M2.
```

- [ ] **Step 4: Final verification**

Run: `cargo test -p pulsar_scenedb --lib --tests`
Expected: PASS (all unit + the new integration tests).
Run: `cargo clippy -p pulsar_scenedb -- -D warnings` — ensure no warning references the new files (`token.rs`, `cell_type.rs`, `simd.rs`, `lease.rs`, `snapshot.rs`).

- [ ] **Step 5: Commit**

```powershell
git add crates/pulsar_scenedb/benches/scenedb_bench.rs crates/pulsar_scenedb/src/lib.rs
git commit -m "bench+docs(scenedb): SIMD/frustum benches and M1b crate-doc wrap-up"
```

---

## Milestone exit criteria

- `cargo test -p pulsar_scenedb --lib --tests` green, including: token (3), cell_type (3), simd (4+ incl. both AVX2 property oracles), lease (3), snapshot (2), the token-keyed cell test, frustum, and the two integration gates (`stale_handle` ×3, `contention` ×2).
- AVX2 AABB and frustum arms verified **bit-for-bit** against the scalar reference across ≥200 random cases each (the SIMD oracle).
- `TypeToken` reuses the dense `ComponentId` id-space and resolves `RuntimeTypeInfo` for reflection-registered types; `CellType` enforces the holistic 128-byte stride at registration.
- Lease pool, decaying scratchpads, and double-buffered liveness snapshot all unit-tested.
- Part VI Test 1 (contention) and Test 2 host-half (stale-handle) pass as named gates.
- Bench reports the AVX2 dispatch number against the M1a 762 ns scalar baseline.
- `pulsar_ecs` still untouched; working tree clean apart from the two pre-existing unrelated edits.

## Deferred beyond M1b

- **AVX-512 / NEON SIMD arms** (dispatch routes them to scalar now — correct, not yet optimized).
- **Proc-macro registration sugar** (`#[register_*_type]` with const stride assertion) — integration milestones.
- **Phase machine** that mandates lease acquisition around harvest, the 2.0 ms revocation timer, concentric cell grid, harvest/DEI partitioning, retirement engine, asset registries — **all Milestone 2 (Layer 2)**.
- **The §8.1 no-allocation guarantee end-to-end** — Tasks 3/5 snapshot liveness into a `Vec`; M2 wires the `Scratchpad` (Task 7) through the query path so harvest is heap-free after warm-up. (M1b ships the scratchpad primitive; M2 threads it through.)
