# SceneDB 2.0 — Milestone 2a Design: GPU-Resident Store, Delta-Sync & Retirement

**Date:** 2026-06-13 (rev 3, 2026-07-12 — single-crate GPU layer)
**Status:** Approved (design); implementation plan to follow
**Governs:** spec §0 / CONTRACTS.md **C0** (Ownership Law), C5 (layouts), C6 (retirement)
**Spec of record:** `docs/superpowers/specs/SceneDB2.0.md` (Rev 2.3)
**Master design:** `docs/superpowers/specs/2026-06-09-scenedb20-implementation-design.md`

> **Rev 2 note.** The first draft conflated M1's row/slot index spaces, assumed a
> "clean seam" over `HandleRegistry::free` that the atomic free API can't provide,
> and ignored `compact()`'s interaction with in-flight GPU rows. This revision
> fixes all three against the actual M1 code (`cell.rs`, `registry.rs`,
> `liveness.rs`). The corrected model: **row-indexed scene buffers + a
> slot-indexed generation buffer; a pin-by-submission-serial mechanism that
> retirement and compaction both respect; new Layer-1 retirement primitives; and
> GPU-mirrored columns written through a GPU-layer writer so Layer 1 stays
> graphics-free.**

> **Rev 3 note (key call).** There is **no separate `pulsar_scenedb_gpu` crate**.
> The GPU layer lands **inside `pulsar_scenedb`** as a feature-gated module
> (`pulsar_scenedb::gpu`, `gpu` cargo feature, optional wgpu dep). C0 is an
> *ownership* law, not a crate-shape law — and Rev 2's own findings motivated
> this: the separate crate had no clean seam over `HandleRegistry::free`, forcing
> the retirement primitives and the mirrored-column writer to become *public*
> Layer-1 API whose only legitimate caller was the GPU crate. In one crate they
> are `pub(crate)`: private machinery instead of misusable surface. The
> graphics-free core is now enforced by feature boundary
> (`cargo check -p pulsar_scenedb --no-default-features` in CI) rather than crate
> boundary. Everything else in Rev 2 — index spaces, delta-sync, pin-by-serial
> retirement, tests — is unchanged. CONTRACTS.md C0 and master design §5a
> amended to match; spec §0 was already crate-agnostic ("SceneDB's GPU layer").

---

## 1. Goal & position in the roadmap

M2a builds **only** the cross-device memory-management core that operationalizes
the Ownership Law (C0): a feature-gated **`pulsar_scenedb::gpu` module** that
**owns** the persistent scene GPU buffers, **delta-syncs** the graphics-free
core's columns into them, and runs the **retirement engine** that safely recycles
slots and rows against GPU completion. There is **no Helio, no rendering, no streaming grid** in
M2a — it is verified headless on a wgpu device via buffer readback, with a
controllable submission-completion signal standing in for the (future) GPU
consumer.

This is where "SceneDB owns all GPU data, Helio owns nothing" physically begins.
M2a proves the store, the delta-sync, and the retirement stand alone and that the
GPU side **re-materializes from the CPU-authoritative columns** (Test 14).

### 1.1 Milestone map (context)

| Milestone | Scope | Status |
|---|---|---|
| Stage 0, M1 (Layer 1) | Spec/contracts; CPU SoA store, handles, queries, leases | **Done** |
| **M2a (this doc)** | `pulsar_scenedb::gpu` (feature-gated module): generic SceneBuffer machinery, instance + generation buffers, delta-sync, pin-by-serial retirement | **Designing** |
| M2b | Asset integration (geometry/vertex-index + cluster/meshlet buffers, **owned by SceneDB**, load-time upload); concentric streaming grid; harvest + DEI; HLOD cross-fade; full compile-time phase machine | Planned (master design) |
| M3 | Helio inversion: bind SceneDB buffers, **C5 shader rework**, material-buffer definition, bindless texture array, cull/indirect/VG/HLOD passes; **Test 13** | Planned |
| M4 | Integration, feature-flag switchover, ECS replacement | Planned |

## 2. Module structure, device context & ownership (C0)

**One crate.** The GPU layer is `pulsar_scenedb::gpu` — a module of
`crates/core/pulsar_scenedb`, gated end-to-end:

- **Feature:** `gpu = ["dep:wgpu"]`, **off by default** (`default = []`).
  `wgpu = { workspace = true, optional = true }` — the workspace already pins the
  Far-Beyond-Pulsar fork at `fce5b80…`, rev-matched to Helio so buffers are
  shareable. All GPU code lives under `src/gpu/` behind `#[cfg(feature = "gpu")]`;
  headless-wgpu integration tests declare `required-features = ["gpu"]`.
- **Graphics-free core, enforced:** `cargo check -p pulsar_scenedb
  --no-default-features` must stay green in CI — the core (storage, queries,
  SIMD, leases) compiles with zero graphics dependency. The no-`pulsar_scenedb`
  → Helio edge guard remains. These two checks replace the old crate boundary.
- **Privileged access is the point:** the GPU layer's retirement, pinning, and
  mirrored-column writer reach core internals (columns, liveness words, registry
  free list, `compact()`) as **`pub(crate)`** peers. Nothing GPU-only becomes
  public API for the rest of the engine to misuse (§4, §5).
- **Device context:** constructed with `Arc<wgpu::Device>` + `Arc<wgpu::Queue>`
  supplied by a concrete **engine-level owner** — an `EngineGpuContext` that holds
  the `Arc`s and is owned by `engine_backend` *above* both SceneDB's GPU layer
  and any renderer. (Today the device reaches Helio through
  `Renderer::new_with_external_device`; M2a/M4 formalize `EngineGpuContext` as the
  single owner so the device + scene buffers provably outlive a renderer — the
  Test 13 precondition. Building `EngineGpuContext` is an M2a task.)
- **Exposes** read-only buffer/bind-group references for a future Helio to bind
  (Helio depends on `pulsar_scenedb` with `features = ["gpu"]`). Nothing flows
  renderer → store.

The graphics-free core keeps all authority over CPU state; all wgpu contact and
all dirty/GPU state live in the `gpu` module.

## 3. Index spaces (the corrected foundation)

M1 has **two distinct index spaces**, and the GPU buffers must respect both:

- **Row index** — dense `0..page.len()`, how `Page` columns are addressed
  (`column_slice::<T>(col)[row]`), **reshuffled by `compact()`** swap-and-pop. The
  harvest output is also row-scoped (C4: row arrays valid for the issuing frame).
- **Slot index** — the stable handle id; `HandleRegistry.generations()` is
  slot-indexed, sparse, never compacted, with `u32::MAX` tombstones for retired
  slots. `slot_to_row[slot]` maps the two.

Therefore:

| Buffer | Index space | Element | Bytes | Mirrors | Sync trigger |
|---|---|---|---|---|---|
| **Instance** | **row** | row-major `mat4` transform | 64 | transform column (dense, by row) | dirty rows (writes + compaction moves) |
| **Generation** | **slot** | `u32` | 4 | `HandleRegistry.generations()` | first mirrored write after alloc + retirement + bulk rebuild (Test 14) |

> **Implementation finding (Task 9).** "Written by retirement" alone is a hole:
> a slot that is allocated and never retired would keep VRAM's zero-init
> generation forever, so the GPU (which validates handles against the VRAM
> generation buffer *exclusively*, C6) would reject every live handle. The
> store therefore keeps a CPU-side **uploaded-generation shadow** (`AtomicU32`
> per slot): `write_transform` stamps a slot's generation to VRAM only when it
> differs from the shadow — once after allocation, once per retirement — so
> the hot loop stays delta-minimal (verified by a generation-write-count gate).

**Scene buffers are row-indexed** — dense, matching the columns and the harvest,
so a sync is a contiguous `write_buffer` memcpy with no conversion (C5). **The
generation buffer is the lone slot-indexed buffer** — it must match handle slots
so the GPU validates handles, and it is sized to **max slots ever allocated**
(can exceed live count after churn; tombstones are uploaded as-is and never
reissued). Because row-indexed buffers move under compaction, **`compact()` marks
every moved row dirty** so the next sync re-uploads it (§4, §5).

**Derived data is not stored.** Normal matrix and world AABB are functions of the
transform — computed in-shader (M3) or reused from the M1 spatial-bounds columns.
Storing them would be the redundant cross-device data the architecture removes,
and it is what makes Test 14 a pure re-upload (§9).

**Buffer scope.** M2a builds the **generic `SceneBuffer<T: Pod>`** machinery and
proves it on the **instance/transform buffer** (64 B mat4, fully C5-defined, the
hot per-frame delta path) plus the **generation buffer** (slot-indexed,
retirement-written). These two fully exercise delta-sync + retirement + Test 14 +
Test 3. **Deferred, each plugging into the same `SceneBuffer<T>` later:** material
(its 32 B PBR layout is "defined in M3 plan" per C5 → M3); mesh metadata and
geometry/vertex-index/cluster/meshlet (per-asset, load-time upload → M2b, **owned
by SceneDB**); bindless texture array (→ M3). M2a does not byte-freeze any
undefined struct.

## 4. Delta-sync (row-indexed)

The mechanism that ends per-frame full re-upload.

- **Dirty tracking (row-indexed).** Each GPU-mirrored column has a **row dirty
  bitmask** — atomic `u64` words, 1 bit per row, the exact shape of M1's
  row-indexed `LivenessMask`, owned by the `gpu` module.
- **Write hook (one mutation path).** GPU-mirrored columns are **written through
  the `gpu` module's column-writer** (`store.write_transform(handle, mat4)`),
  which writes the byte into the core column **and** sets the row's dirty bit in
  one operation. The writer reaches the column via `pub(crate)` access — no
  public column-exposure API is needed (the Rev 2 contortion this replaces); the
  graphics-free core holds no dirty/GPU state (all of it is `#[cfg(feature =
  "gpu")]`). Raw `user_column_mut` remains for non-mirrored columns. (One
  mutation path for mirrored data; the only place a dirty bit is set.)
- **Compaction re-sync.** `compact()` (M1) moves a live row's bytes to a new index
  and updates `slot_to_row`. The GPU layer's compaction wrapper **marks both the
  destination row dirty** (its bytes changed) so the move is re-uploaded. (Source
  rows shrink out of `len` and are not synced.)
- **Sync sub-phase.** After the write window and after `compact()`, for each
  mirrored buffer: scan the dirty words, **coalesce contiguous dirty rows into byte
  ranges**, issue the minimal `queue.write_buffer(buf, row_offset, &col_bytes[range])`
  calls, clear the bits. A zero-mutation, zero-compaction frame writes nothing.
- **Zero mid-frame heap alloc.** The dirty-word scan and the coalesced-range list
  use the M1 `Scratchpad`, extended with `get_u64` + a range-list capacity (an
  explicit M2a task — the §8.1 carry-forward; `Scratchpad` has only `get_u32`
  today).

## 5. Retirement engine (C6) — pin-by-serial, with new Layer-1 primitives

The subtle correctness core. M1's `CellStorage::free` is **atomic and immediate**:
`set_dead(row)` then `HandleRegistry::free` which nulls `slot_to_row[slot]`, bumps
the generation, and pools the slot — all at once — and `compact()` then reclaims
the dead row, overwriting its bytes. None of that is safe while the GPU may still
reference the row. M2a replaces it with a pin-by-serial model.

**Pin-by-serial primitive.** A **row** can be *pinned* by a submission serial: it
is excluded from harvest (liveness-dead) yet its data is preserved and `compact()`
**skips it** until that serial completes. This is the general cross-device
lifetime tool. In M2a the concrete user is retirement; the broader user (live
rows pinned from harvest until their frame's draw completes) lands with the
harvest in M2b/M3 — same mechanism.

**New core primitives** (added to `CellStorage` in the graphics-free core — they
take a `u64` pin token, not a GPU type, and are **`pub(crate)`**: their only
caller is the `gpu` module's retirement engine, so they never become public
surface the rest of the engine could misuse):

- `mark_pending_retire(handle) -> row`: sets the liveness bit dead (excluded from
  next harvest) and records the row as pin-pending. **Does NOT** null
  `slot_to_row`, bump the generation, or pool the slot. The handle is *intentionally
  still resolvable by row* during the in-flight window (the GPU's last harvest of
  it is valid), but it will not appear in new harvests.
- `commit_retire(slot)`: the deferred remainder of the old `free` — null
  `slot_to_row[slot]`, bump the generation, return the slot to the pool, and clear
  the row's pin (now compactable).
- `compact()` gains a "skip pinned rows" rule: pinned rows are neither swapped away
  nor filled into.

**Flow.**

1. **Delete** → `mark_pending_retire(handle)` + enqueue
   `(slot, row, generation, submission_serial)` in the deferred-eviction list.
   Nothing in the registry or GPU buffers changes yet; the row is pinned.
2. **GPU completion** → each submission carries a monotonic **submission serial**;
   a `Queue::on_submitted_work_done` callback marks "all work ≤ serial S done." A
   `SubmissionTracker` infers the highest complete serial. (Headless: the test
   harness owns the `device.poll(Maintain::…)` pump that fires callbacks.)
   Frame-counter arithmetic is forbidden (§20.1).
3. **Retirement drain (frame boundary, runs first)** → for every enqueued entry
   whose serial is complete: write the new generation into the **VRAM generation
   buffer** (slot-indexed) and `commit_retire(slot)` in the registry. Order: the
   generation bump + buffer write happen **before** the slot can be re-allocated,
   so a stale handle is never momentarily valid against a reused slot.

**Frame-boundary order:** `retire()` → `compact()` → `sync()`. Retire unpins and
frees rows the GPU is done with; compact densifies the unpinned rows (marking
moved rows dirty); sync uploads the final row state. The next frame's harvest sees
the post-compaction layout.

This keeps the slot allocator (core) and the GPU buffer (`gpu` module) under one
owner — literally one crate, the reason C0 requires single ownership. M1's
immediate-free path is retained for non-GPU (CPU-only/headless-logic) use; the
GPU store installs the deferral.

## 6. Phase coordination (minimal)

M2a needs ordered frame-boundary points, not the full compile-time phase machine
(M2b). Three explicit calls, in order, outside the simulation write window:

- `store.retire()` — drain completed serials (commit_retire + gen-buffer writes).
- `cell.compact()` — swap-and-pop over unpinned rows; the GPU wrapper marks moved
  rows dirty.
- `store.sync()` — coalesce + upload dirty rows; clear bits.

Debug-asserts guard the ordering and that they run outside the write window. The
compile-time phase-guard types are M2b/M3.

## 7. Components (units, each independently testable)

- `EngineGpuContext` — owns `Arc<Device>`/`Arc<Queue>`; lives above SceneDB's GPU
  layer and any renderer (in `engine_backend`).
- `SceneBuffer<T: Pod>` — one persistent **row-indexed** SSBO + its row dirty
  bitmask + coalesce-and-upload. Generic over the C5 element type.
- `GenerationBuffer` — the **slot-indexed** SSBO mirroring `generations()`; written
  by retirement.
- `GpuStore` — owns the `SceneBuffer`(s) + `GenerationBuffer`; the column-writer
  (`write_transform`); `sync()`; exposes read-only bind references.
- `RetirementEngine` — deferred-eviction list + `mark_pending_retire`/`commit_retire`
  orchestration; the pin set; `retire()` drain.
- `SubmissionTracker` — monotonic serials + `on_submitted_work_done` → highest
  complete serial.

New core surface (graphics-free, **`pub(crate)`**): `CellStorage::mark_pending_retire`,
`commit_retire`, pinned-row tracking, and `compact()`'s skip-pinned rule.

All components above except `EngineGpuContext`'s owner live in
`pulsar_scenedb::gpu` under `#[cfg(feature = "gpu")]`.

## 8. Error handling

- Capacity exhaustion (live slots > configured max): hard error at allocate,
  surfaced to telemetry; no silent mid-frame realloc (SSBOs never reallocate).
- Generation buffer is sized to **max slots ever**, not live count.
- A deleted element is excluded from harvests immediately (liveness-dead) but its
  row data and generation persist (row pinned) through the in-flight window; the
  gen bump (CPU registry + VRAM buffer) happens together at `commit_retire`. A
  handle is rejected (CPU + GPU) only after retirement; never UB.
- A slot is never reissued, and its row never reclaimed by compaction, before its
  submission serial completes and the new generation is in both the registry and
  the VRAM buffer — the retirement invariant, asserted in tests.
- `retire`/`compact`/`sync` out of order, or mutation outside the write window:
  debug-assert failure.

## 9. Testing (headless wgpu, no Helio)

A test harness creates a real surfaceless wgpu device (the fork; STORAGE buffers +
`map_async` readback are supported) and owns the `device.poll` pump that drives
`on_submitted_work_done`.

- **Delta correctness:** write transforms via the column-writer, `sync()`, map the
  instance buffer back, assert GPU bytes == CPU transform column (by row).
- **Delta minimality:** instrument `write_buffer`; assert a no-mutation/no-compaction
  frame writes nothing, and that N scattered dirty rows coalesce into the expected
  minimal range count (no clean-row uploads).
- **Compaction re-sync:** free an element, advance to retirement, `compact()` moves
  a live row, `sync()`; assert the moved row's GPU bytes are correct at its new
  index and the generation buffer matches the registry.
- **Byte-exact layout (Test 3):** host struct offsets vs naga reflection of the
  WGSL instance (64 B mat4) + generation structs, byte-exact. (Material/mesh-meta
  Test 3 follow their M3/M2b definitions.)
- **Retirement invariant (Test 6 host-side):** `mark_pending_retire` a slot; with a
  controllable completion signal, assert: the row is not compacted and the slot not
  reissued while the serial is incomplete; the handle still resolves by row during
  the window but is absent from harvest; after the serial completes, `retire()`
  writes the generation buffer before the slot returns to the pool; the old handle
  is then rejected; no use-after-free.
- **Test 14 — device-loss re-materialization:** drop the device + all buffers;
  create a fresh device; rebuild instance + generation buffers from Layer-1's
  authoritative columns (`transform` column by row; `generations()` by slot);
  assert byte-identical recovery. Sound because M2a stores no GPU-only/derived data.
  (Re-materialization of geometry/cluster buffers is added when M2b builds them;
  M2b extends Test 14 — see §10.)

Test 13 (renderer teardown) is **M3** — it needs a Helio instance to drop. M2a
establishes the ownership that makes Test 13 passable.

## 10. Deferred (each to a named, planned milestone)

- Material buffer + its 32 B PBR layout definition + material Test 3 → **M3**
  (C5: "defined in M3 plan"), or earlier if its consumer firms up.
- Mesh-metadata, geometry/vertex-index, **cluster/meshlet** buffers (SceneDB-owned
  per C0) + load-time upload → **M2b**. **M2b also extends Test 14** to re-materialize
  these. M3 only *consumes/culls* them; their allocation/ownership stays in SceneDB.
- Bindless texture array (spec §10 G4) → **M3**.
- Concentric streaming grid, harvest + DEI, HLOD cross-fade → **M2b**.
- Full compile-time phase-guard state machine → **M2b**.
- Multi-view GPU resources; per-view uniforms/command buffers (Helio-owned derived
  data) → **M3**. (Boundary rule: scene-level observer/camera entities and their
  AABBs are SceneDB-owned; per-view GPU uniforms and command buffers are
  Helio-owned derived data.)
