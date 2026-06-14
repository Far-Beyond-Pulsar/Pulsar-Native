# SceneDB 2.0 — Milestone 2a Design: GPU-Resident Store, Delta-Sync & Retirement

**Date:** 2026-06-13
**Status:** Approved (design); implementation plan to follow
**Governs:** spec §0 / CONTRACTS.md **C0** (Ownership Law), C5 (layouts), C6 (retirement)
**Spec of record:** `docs/superpowers/specs/SceneDB2.0.md` (Rev 2.3)
**Master design:** `docs/superpowers/specs/2026-06-09-scenedb20-implementation-design.md`

---

## 1. Goal & position in the roadmap

M2a builds **only** the cross-device memory-management core that operationalizes
the Ownership Law (C0): a new `pulsar_scenedb_gpu` crate that **owns** the
persistent scene GPU buffers, **delta-syncs** Layer-1 (`pulsar_scenedb`) columns
into them, and runs the **retirement engine** that safely recycles slots against
GPU completion. There is **no Helio, no rendering, no streaming grid** in M2a —
it is verified headless on a wgpu device via buffer readback.

This is the milestone where "SceneDB owns all GPU data, Helio owns nothing"
physically begins. Helio does not bind these buffers until M3; M2a proves the
store, the sync, and the retirement stand on their own and that the GPU side can
be **re-materialized from the CPU-authoritative columns** (Test 14) with no
renderer in the loop.

### 1.1 Milestone map (context)

| Milestone | Scope | Status |
|---|---|---|
| Stage 0, M1 (Layer 1) | Spec/contracts; CPU SoA store, handles, queries, leases | **Done** |
| **M2a (this doc)** | `pulsar_scenedb_gpu` store + delta-sync + retirement | **Designing** |
| M2b | Asset integration (geometry/vertex-index buffers + load-time upload), concentric streaming grid, harvest pipeline + DEI, HLOD cross-fade state | Planned (master design) |
| M3 | Helio inversion: bind SceneDB buffers, C5 shader rework, cull/indirect/VG/HLOD passes; **Test 13** | Planned |
| M4 | Integration, feature-flag switchover, ECS replacement | Planned |

Everything M2a defers has a named home above — nothing is hand-waved.

## 2. Crate structure & ownership (C0)

A new crate `crates/pulsar_scenedb_gpu`:

- **Depends on** `wgpu` (the Far-Beyond-Pulsar fork, rev-matched to Helio's
  `fce5b80…` so buffers are shareable across the crate boundary) **and**
  `pulsar_scenedb` (Layer 1). **It must never depend on Helio.** A future Helio
  depends on *it* for the scene buffers. CI guards the absence of any
  `pulsar_scenedb*` → Helio edge.
- **Device context:** the crate is constructed with `Arc<wgpu::Device>` +
  `Arc<wgpu::Queue>` supplied by the engine context — the same engine-owned
  device Helio already receives via `Renderer::new_with_external_device`. The
  store holds the `Arc`, so the device and every scene buffer **outlive any
  renderer instance** (the Test 13 precondition; M2a establishes the ownership,
  M3 proves the teardown).
- **Exposes** read-only buffer/bind-group references for a future Helio to bind.
  Nothing flows renderer → store.

Layer 1 (`pulsar_scenedb`) remains **graphics-free** (spec §0); all wgpu contact
lives in `pulsar_scenedb_gpu`.

## 3. The GPU-resident store

Four persistent SSBOs, allocated once at init to configured scene maxima, in the
**canonical compact C5 layout** (SceneDB owns the lean, authoritative layout;
Helio conforms in M3). Allocated with `STORAGE | COPY_DST` (and `COPY_SRC` for
readback in tests). Never reallocated mid-frame.

| Buffer | Element | Bytes | Source column / data | Dirty cadence |
|---|---|---|---|---|
| Instance | row-major `mat4` transform | 64 | transform column (authoritative) | hot (per moving object) |
| Material | PBR params | 32 | material column | rare |
| Mesh metadata | per-mesh config (§6.1) | 72 | mesh-meta column | rare |
| Generation | `u32` per slot | 4 | `HandleRegistry::generations()` | on retirement |

**Derived data is not stored.** The normal matrix and world-space AABB are
functions of the transform; they are computed in-shader (M3) or, for the world
AABB, already maintained CPU-side as the M1 spatial-bounds columns for the SIMD
cull. Storing them in the GPU instance buffer would be exactly the redundant
cross-device data the architecture exists to eliminate.

Each buffer maps **1:1 by slot index** to a Layer-1 column (or the registry's
generation array). The byte layout is byte-identical host↔device (C5), so a sync
is a `memcpy` (`queue.write_buffer`) with **no conversion step** — Test 3
(host↔naga byte-exact) lands in this crate.

**Geometry / vertex-index buffers** are SceneDB-owned per C0 as well, but their
upload is **load-time** (driven by mesh asset loading), a fundamentally different
access pattern from the per-frame delta path. They are therefore **M2b** (asset
integration), not M2a. M2a's store is the per-instance scene state + generation.

Capacity overflow (more live slots than the configured max) is a hard,
surfaced error at allocation request time — not a silent mid-frame realloc
(persistent SSBOs never reallocate, per §10). Growth strategy (resize-at-frame-
boundary vs. fixed ceiling) is a tuning decision captured in the plan; the
default is a configured fixed ceiling with a telemetry-surfaced error on
exhaustion.

## 4. Delta-sync

The mechanism that ends per-frame full re-upload.

- **Dirty tracking:** each mirrored column carries a **dirty bitmask** — atomic
  `u64` words, 1 bit per slot, the exact pattern of the M1 `LivenessMask`. A
  Layer-1 write to a slot's transform/material/mesh-meta sets that slot's dirty
  bit (a cheap atomic OR). The transform mask is the hot one; material and
  mesh-meta masks are near-empty most frames.
- **Sync sub-phase:** after the simulation writes complete, at the frame
  boundary, `pulsar_scenedb_gpu` for each buffer scans its dirty words,
  **coalesces contiguous dirty slots into byte ranges**, and issues the minimal
  set of `queue.write_buffer(buffer, range_offset, &cpu_bytes[range])` calls
  (memcpy into the byte-identical layout), then clears the bits. A frame with
  zero mutations issues zero writes.
- **No scan-and-diff, no shadow copy.** The dirty bitmask *is* the change record;
  we never re-upload clean rows or diff against a previous-frame snapshot.
- **Zero mid-frame heap allocation:** the dirty-word scan and the coalesced-range
  list use a persistent scratch buffer (the M1 `Scratchpad`, extended with the
  `get_u64`/range-list capacity it needs — the §8.1 carry-forward).

The dirty bitmask integrates with the phase ordering: bits accumulate during the
simulation write window and are drained exactly once at the sync sub-phase, so a
slot mutated several times in a frame uploads once.

## 5. Retirement engine (C6) — interposing on M1's free

This is the subtle correctness core. Today M1's `CellStorage::free` bumps the
generation and returns the slot to the free pool **immediately** — safe CPU-side,
but unsafe while the GPU may still reference that slot's instance/material data.
C0/C6 require that **the same system owns both the slot allocator (Layer 1) and
the GPU buffer (M2a)** so retirement is one coherent operation. M2a provides it:

1. **Delete enqueues; it neither bumps the generation nor frees the slot.** A
   delete marks the element's **liveness bit dead immediately** (so it is excluded
   from all future harvests) and records `(slot, generation, submission_serial)`
   in a deferred-eviction list. Crucially, the handle's **generation stays valid**
   during the in-flight window — the GPU is still legitimately drawing this slot's
   data for the frame in which the element was harvested alive (§20.1: "clients
   safely use slot 14 gen 2" while frames pass). The slot index is withheld from
   reuse and the generation buffer is untouched.
2. **GPU completion signal.** Each frame's command submission carries a
   monotonically increasing **submission serial**; a
   `Queue::on_submitted_work_done` callback marks that serial complete (the wgpu
   adaptation of the spec's timeline semaphore, Appendix C). Frame-counter
   arithmetic is forbidden (§20.1).
3. **Retirement drain (frame boundary).** For every enqueued entry whose serial
   is now complete: **bump the generation** in the Layer-1 registry, write the new
   generation into the **VRAM generation buffer** (so both CPU and GPU now reject
   the stale handle), *then* return the slot index to the free pool. Order matters:
   the generation bump + buffer update happen before the slot can be re-allocated
   and re-uploaded — so a stale handle is never momentarily valid against a
   reused slot.

M2a therefore **defers** the M1 `HandleRegistry`/`CellStorage` free path: today
M1's `free` marks liveness dead *and* immediately bumps the generation + returns
the slot to the pool. In the GPU-backed configuration, the immediate gen-bump +
slot-recycle are split out and deferred to the retirement drain (the liveness-dead
marking stays immediate). M1's behavior is unchanged for non-GPU (test/headless-CPU)
use; the GPU store installs the deferral. This boundary is a clean seam — the
retirement engine is a `pulsar_scenedb_gpu` component that consumes Layer-1's
free/allocate primitives and gates the recycle on GPU serials.

## 6. Minimal phase coordination

M2a needs only two ordered points, not the full harvest-integrated phase machine
(that arrives with M2b's streaming/harvest):

- a **sync point** — after the simulation write window, drains the dirty masks;
- a **retirement drain** — at the frame boundary, processes completed serials.

These are expressed as explicit API calls (`store.sync()`, `store.retire()`)
with debug-assert guards that they run in order and outside the write window. The
compile-time phase-guard types and the full Simulate→Harvest→Cull→Draw→Retire
machine are M2b/M3 scope.

## 7. Components (units, each independently testable)

- `GpuContext` — holds `Arc<Device>`/`Arc<Queue>`; the engine-level handle.
- `SceneBuffer<T: Pod>` — one persistent SSBO + its dirty bitmask + the
  coalesce-and-upload logic. Generic over the C5 element type.
- `GpuStore` — owns the four `SceneBuffer`s, exposes `sync()` and the bind
  references; constructed from `GpuContext` + the Layer-1 cell(s).
- `RetirementEngine` — the deferred-eviction list + submission-serial tracking +
  the drain; wraps Layer-1's free/allocate.
- `SubmissionTracker` — monotonic serials + `on_submitted_work_done` bookkeeping.

Each has one responsibility, a small interface, and a headless unit test.

## 8. Error handling

- Capacity exhaustion: hard error at allocate, surfaced to telemetry; never a
  silent mid-frame realloc.
- Deleted element: excluded from harvests immediately (liveness-dead), but its
  handle's generation stays valid through the in-flight window so in-flight GPU
  references remain correct; the gen bump (CPU + VRAM generation buffer) happens
  together at the retirement drain. A handle is rejected (CPU and GPU) only after
  retirement; never UB.
- A slot is never reissued before its submission serial completes and its new
  generation is in both the registry and the VRAM buffer — the retirement
  invariant, asserted in tests.
- Sync/retire called out of order: debug-assert failure.

## 9. Testing (headless wgpu, no Helio)

A test harness creates a real wgpu device (the fork, headless — no surface).

- **Delta correctness:** mutate Layer-1 columns, `sync()`, map the buffers back
  (`COPY_SRC` → staging → read), assert the GPU bytes equal the CPU columns.
- **Delta minimality:** instrument `write_buffer` calls; assert a no-mutation
  frame writes nothing, and that N scattered dirty slots coalesce into the
  expected minimal range count (no clean-row uploads).
- **Byte-exact layout (Test 3):** host struct offsets vs naga reflection of the
  WGSL struct decls for instance/material/mesh-meta/generation, byte-exact.
- **Retirement invariant (Test 6 host-side):** delete a slot, advance frames with
  a mock-then-real completion signal; assert the slot is not reissued until its
  serial completes and the generation buffer is written first; no use-after-free
  detectable via the generation buffer.
- **Test 14 — device-loss re-materialization:** drop the device + all buffers;
  create a fresh device; rebuild the store from Layer-1's authoritative columns;
  assert byte-identical recovery. This proves the CPU side is the true authority
  and the GPU side is a derived mirror.

Test 13 (renderer teardown) is **M3** — it needs a Helio instance to drop. M2a
establishes the ownership that makes Test 13 passable.

## 10. Deferred (each to a named, planned milestone)

- Geometry/vertex-index buffer ownership + load-time upload → **M2b** (asset
  integration).
- Concentric streaming grid, harvest pipeline + DEI dense compaction, HLOD
  cross-fade state → **M2b**.
- Helio binding SceneDB's buffers, C5 shader rework, the render passes,
  **Test 13** → **M3**.
- Full compile-time phase-guard state machine → **M2b/M3**.
- Multi-view (shadow cascades, split-screen) GPU resources → **M3**.
