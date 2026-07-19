# SceneDB 2.0 — Milestone 2b Design: Assets, Streaming Grid, Harvest & Phase Machine

**Date:** 2026-07-14 (rev 2 — post adversarial review)
**Status:** M2b implemented (α+β complete) — α: region-partitioned SceneGpuStore, asset store, phase machine. β: streaming grid (domains/hysteresis/cross-fade/budget), region-recycle promotion/eviction (serial-pinned eviction, recycled-region tail scrub), no-alloc harvest seams + `HarvestPipeline`/DEI dense compaction, lease timeout/revocation. See Task 10's acceptance matrix and `src/lib.rs`'s milestone-status doc for the verifying test list.
**Governs:** spec §0/C0 (ownership), §5 (concentric streaming), §6/C5 (asset registry, 72 B mesh metadata, 48 B ClusterNode), §8.3–8.5/C4 (harvest, DEI), §9 (leases/scratchpads), C3 (frame phases), C6 (retirement)
**Spec of record:** `docs/superpowers/specs/SceneDB2.0.md` (Rev 2.3)
**Master design:** `specs/2026-06-09-scenedb20-implementation-design.md` §5b
**Predecessor:** `specs/2026-06-13-scenedb20-m2a-gpu-store-design.md` (Rev 3, complete)

> **Rev 2 note.** The first draft repeated M2a-Rev-1's class of error and the
> adversarial review caught it against the real code: (D1) row-granularity
> harvest pins would pin every live row of every observed cell at every
> boundary — compaction starves in steady state; they are **dropped** in favor
> of the queue-ordering argument (§6.1). (D2) recycled regions poison the
> generation shadow and strand pending retires; §4.1 now carries an explicit
> eviction/promotion **reset ledger**. (S1) global-row tokens are produced in
> the partition scan, not the SIMD kernels. (S2) a **global-slot mirror
> buffer** is added so GPU handle validation (C6/§3.3) has a data path. (S3)
> the Scratchpad/snapshot seams are new API, not wiring. (S4/S5) slot-region
> sizing corrected; region pools are size-classed. (S6) tracker caller
> contracts hardened. (S7) the milestone is split: **M2b-α** (2b.0 assets +
> 2b.3 phase machine) then **M2b-β** (2b.1 grid + 2b.2 harvest).

---

## 1. Goal & position in the roadmap

M2b turns the single-cell M2a mechanism into the **engine-wide orchestration
layer**: many cells in a streaming grid, global (not per-cell) device buffers,
load-time asset upload, the harvest pipeline that will feed Helio, and the
compile-time phase machine that hard-enforces what M2a enforced by
debug-assert. Still **no Helio, no rendering**: harvest output is consumed by
tests standing in for the M3 cull pass, and all gates run headless.

### 1.1 Milestone split (binding)

M2a (one cell, two buffers) took 13 reviewed tasks; each 2b workstream is
M2a-sized, and their dependencies cut cleanly:

| Milestone | Scope | Depends on |
|---|---|---|
| **M2b-α** | 2b.0 asset store (GeometryArena, MeshRegistry, ClusterBuffer, HLOD entries) + 2b.3 compile-time phase machine + global-slot mirror + SceneGpuStore reshape (regions, size-class pools) | M2a only |
| **M2b-β** | 2b.1 streaming grid/residency + 2b.2 harvest pipeline (partition, DEI, scratchpad/lease wiring) | M2b-α |

Gates: **Test 10** (lease stall/revocation, β), **Test 11** (grid boundary
oscillation, β), **Test 12** (sparse-cell DEI compaction, β), **Test 3
extension** (72 B mesh metadata + 48 B ClusterNode, α), **Test 14 extension**
(re-materialize geometry/metadata/slot-mirror buffers, α; multi-cell form, β).

## 2. The central reshape: global buffers, per-cell row spaces

Spec §10's scene SSBOs are **global**: one instance buffer, one mesh
configurator, one generation buffer for the whole scene, allocated once at
startup from configured maximums. M1/M2a give each cell an independent dense
row space `0..page.len()`. M2b reconciles them with **per-cell regions**:

- The global row-indexed buffers are partitioned into **cell regions**.
  Regions come from **size-class pools**: one pool per registered cell-type
  capacity (C2: default 256, hard max 1024), each pool a free list of
  fixed-size regions. Worst-case VRAM is exact per class:
  `Σ_class max_resident_cells(class) × capacity(class) × stride` — the §10
  "capacity set at initialization" contract, validated by the §5.3 budget
  check. `global_row = region_base + local_row`.
- M2a's `SceneBuffer<T>` gains a **region view**: `sync_region(queue, cpu,
  region_base)` — the same streaming dirty-word coalescer, offset by
  `region_base * stride`. Dirty state stays **per cell** (each resident cell
  keeps its own dirty words; the global buffer holds no global dirty mask).
  `GpuStore` (M2a, 1 store ↔ 1 cell) becomes the per-cell **CellGpuState**
  (dirty words + pending-retire queue + gen-shadow slice) owned by the new
  scene-wide **SceneGpuStore**, which owns the buffers, the region pools,
  one `SubmissionTracker`, and the generation buffer. Public API shape is
  preserved: `write_transform(cell_id, handle, m)` etc.
- **Slot spaces stay per-cell** (C1 handles are cell-scoped as in M1). The
  global generation buffer is partitioned by **slot regions** sized
  `capacity(class) + tombstone_headroom` (configurable, default 64). The
  registry invariant is `generations.len() ≤ capacity + retired_count`
  (slots do NOT grow with ordinary churn — only `u32::MAX` permanent
  retirement grows the space), so headroom only covers tombstones.
  **Slot-region exhaustion is a hard alloc error** (§8), and every
  generation write carries a region-bounds assert — an overflow must never
  land in a neighbor's region.
- **Global-slot mirror buffer (new, α):** a row-indexed `SceneBuffer<u32>`
  holding `global_slot(global_row)` — the data path for C6/§3.3 GPU handle
  validation (the M3 cull pass reads `slot = slot_mirror[row]`, then
  `generations[slot]`). **As shipped**, the sole maintenance trigger is a
  self-healing boundary scan in `sync_all`: for every occupied row it compares
  the row-scoped `slot_shadow` against the authoritative slot column
  (`0..rows_in_use`) and re-uploads exactly the mismatches, regardless of how
  the slot got there (write after alloc, compaction swap, or an alloc
  re-occupying a vacated row that is never written). This replaces the
  originally-proposed per-event triggers (mark at alloc, mark at each
  compaction move via `compact_report`'s `(from, to)`): Task 4 review found
  TWO fail-open paths in that scheme — (1) a retired slot recycled into a
  *different* row arrives with its generation already shadow-gated, so the
  slot-scoped gen-gate stays silent while the new row's mirror entry is stale;
  (2) an `alloc` that re-occupies a compaction-vacated row without ever
  calling `write_transform` has no per-event trigger at all. The single
  boundary-scan invariant closes both, dirty-tracked like transforms; Test 3
  row included. The shader consumer is M3; the buffer and its maintenance are
  M2b-α so M3 does not reshape the store.
- Harvest emits **global-row tokens** for the M3 consumer, but the SIMD
  kernels are untouched: `query_aabb`/`query_frustum` keep writing **local**
  tokens (bit-identity across scalar/AVX2/NEON arms is preserved). The
  **single-scan partition pass** (§5.2) — which already touches every token
  once — adds `region_base` to valid tokens as it routes them; the
  `0xFFFF_FFFF` sentinel is never offset. Zero extra passes.

**Why regions, not one giant slotmap:** compaction, liveness, leases, and
pins stay cell-local (all M1/M2a machinery unchanged); promotion/demotion is
a region alloc/free instead of a scene-wide reshuffle; and worst-case VRAM
is closed-form per size class.

## 3. 2b.0 — Asset store (M2b-α, load-time path)

A second store beside the scene store, same C0 ownership, different access
pattern (write-once at load, read-forever):

- **GeometryArena** — the global vertex + index buffers with a **range
  suballocator** (first-fit free list over byte ranges; allocations are
  whole-mesh, freed only on asset unload — no per-frame churn). Upload via
  `queue.write_buffer` at load time (cold path; staging-belt optimization is
  deferred until profiled).
- **MeshRegistry** — flat host-side `Vec<MeshMetadata>` in the exact C5 72 B
  layout (`#[repr(C)]`, scalar fields only), mirrored 1:1 into the mesh
  configurator SSBO; `mesh_index` is the registry index. Host struct is
  uploaded directly — no conversion (§6). The C5 XOR rule (`lod_count` vs
  `cluster_table_offset`) is validated at registration; violation is a hard
  registration error.
- **ClusterBuffer** — global cluster-DAG buffer of C5 48 B `ClusterNode`
  entries (`self_error < parent_error` validated at registration, in the
  NaN-rejecting `!(a < b)` form); VG meshes reference it via
  `cluster_table_offset`. **Node 0 is a reserved sentinel** (α final-review
  finding): C5's XOR rule reads `cluster_table_offset == 0` as "no cluster
  table", so a real table at offset 0 would be unregistrable — real tables
  start at node 1, `len()` starts at 1, and `max_nodes` budgets include the
  sentinel.
- **HLOD proxies** are ordinary `MeshRegistry` entries indexed by a
  **cell-level handle**: a dedicated proxy cell type (one row per
  content-bearing grid cell; transform = cell placement, mesh = proxy).
  Proxy cells are a **permanently-resident size class** — spec §5.1 requires
  outer-cell proxies rendered every frame, so their regions never evict and
  the §5.3 budget counts them as an always-on term (M1 of the review). They
  ride the normal instance path; the "bypasses cluster culling" distinction
  is an M3 shader concern.
- **Material registry (32 B) stays M3** (C5: layout "defined in M3 plan").
  The buffer is allocated at configured max; its element layout, writer, and
  Test 3 row land in M3.

**Test 3 extension (α):** the naga harness (`tests/gpu_layout.rs`,
`wgsl_struct_layout`) gains `MeshMetadata` (size 72, all field offsets),
`ClusterNode` (size 48, incl. `bounding_sphere` at 32), and the slot-mirror
element — asserted explicitly against **storage** address-space layout
(Task 12 ledger note).

**Test 14 extension (α):** device loss re-materializes geometry, cluster,
metadata, and slot-mirror buffers from host-authoritative state in addition
to instance + generation. Precondition per cell (M3 of the review): every
cell's pending-retire queue is drained before rebuild — the gate drains all
cells first, mirroring `rebuild_from`'s documented guard.

## 4. 2b.1 — Streaming grid & residency (M2b-β)

- **Grid:** uniform, world-space, configured cell width; `CellCoord` → cell.
  Cells materialize lazily; each materialized cell gets a **dense cell id**
  from the residency map (stable for the cell's lifetime) which indexes the
  per-cell metadata SSBO (M4 of the review).
- **Domain classification** (frame boundary only, §5): inner = cell AABB
  intersects the **union of observer AABBs**; margin = within margin radius;
  outer = everything else. Multi-observer per §5.4. Asymmetric hysteresis per
  §5.5: promotion at `CellBounds + Δpad`, demotion at `CellBounds + Δpad +
  δhyst`, default `Δpad = 10%` of cell width (Test 11's jitter gate).
- **Budget validation** (§5.3): `StreamingBudget` computes both §5.3
  inequalities from configured radii, per-class capacities, mean proxy and
  geometry sizes, **plus a bounded world extent / max-materialized-cells
  input** (lazy grids are otherwise unbounded) and the permanent proxy-class
  term. `StreamingGrid::new` fails hard on violation, returning
  `Err(BudgetError)` (as shipped). The designer-facing stress-position walker
  tool is deferred to M4/editor.

### 4.1 Residency actions and the reset ledger (binding)

All transitions happen at the Retire/Compact boundary, never mid-frame.

**Promotion outer→margin (region acquire):**
1. Allocate a row region + slot region from the cell's size-class pool
   (hard error → cell stays outer, telemetry, §8).
2. **Generation region rebuild:** bulk-upload `registry().generations()`
   into the slot region (the M2a `rebuild` pattern at region offset) and
   **reseed the cell's gen-shadow slice from the same values**. Never assume
   zero-init: the region may be recycled from another cell (D2).
3. Reset the cell's dirty words to all-dirty for live rows (full region sync
   at the next boundary — the "streaming warm-up"), including the slot
   mirror column.
4. Cross-fade α starts rising (world-distance-driven, §5.2), mirrored into
   the per-cell metadata SSBO (`f32` α + `u32` domain, indexed by dense cell
   id).

**Promotion margin→inner:** no residency change; domain flag flips —
simulation/harvest eligibility only.

**Demotion margin→outer (eviction):**
1. The region pair is **pinned by the last submission serial that could
   reference it** and enters the region free list only after that serial
   completes (the M2a pin-by-serial pattern at region granularity — the only
   serial pinning that survives Rev 2, see §6.1).
2. **Pending-retire disposition (D2) — eviction-timing refinement, binding
   for β Task 4 (audit-remediation, see
   `docs/superpowers/specs/2026-07-16-scenedb20-holistic-audit.md`):** every
   retire queued in the cell's pending-retire queue is committed **CPU-side
   immediately at eviction** (`commit_retire`: unpin row, bump registry
   generation, pool slot) — **zero VRAM writes**, not deferred to
   region-free-completion as originally drafted. The "wait for the region
   pin's serial" wording existed only to prevent a write landing in a freed
   (possibly re-allocated) region; since the commit never touches VRAM, there
   is nothing for a late write to corrupt, so the commit needs no such gate.
   The region pin (step 1) is unchanged and still governs when the row/slot
   **byte ranges** themselves re-enter the free list — only the retire
   *commit* moves earlier, to immediately at eviction.
3. The cell's gen-shadow slice and dirty words are **dropped** (re-created
   at next promotion via the rebuild above). CPU-side cell data persists
   (host memory is authoritative); handles remain valid CPU-side in every
   domain. Staged harvest tokens need no action: they are frame-scoped (C4)
   and transitions run post-cull at the boundary (C3).

## 5. 2b.2 — Harvest pipeline (M2b-β)

Input: a view set (frusta/AABBs). Output: per-view staging arrays feeding
the (future) M3 cull pass. Zero-alloc after warm-up:

1. **Query** per resident inner cell per view (M1b SIMD kernels, unchanged,
   local tokens) into per-view scratch, under a cell read-lease against a
   **liveness snapshot captured into scratch** (see §5.1 API additions).
2. **Single-scan partition:** one pass over each cell's token run, routing
   each valid token by its mesh's registry class (traditional LOD / VG /
   HLOD-proxy) into three per-view staging arrays, **adding `region_base`
   to each valid token as it routes** (S1) — sentinels dropped here, never
   offset. Homogeneous cells take one class branch for the whole run;
   heterogeneous cells fall back to per-token metadata lookup.
3. **DEI** (§8.5, C4): per cell-run, `DEI = valid/total`; if `< 25%`, a SIMD
   masked compress-store strips sentinels into a dense payload + remap table
   (Test 12). Scalar reference first, AVX2 arm bit-for-bit after (M1b
   discipline). The **remap table is an M3-frozen interface** (M2 of the
   review): `remap[dense_i] = original_run_index` as `u32`, byte-layout
   tested in α's Test 3 harness style; its consumer (GPU cull or CPU
   re-expansion) is chosen in the M3 plan.
4. **Lease timeout** (§9.2.1): leases still held 2.0 ms into the
   frame-boundary isolation phase are revoked via M1b's `RevocationFlag`;
   the holder's results are pushed to the stale-validation lane
   (re-validated against live generations on use); compaction proceeds.
   Persistent revocation from one client = that client's bug (Test 10).

### 5.1 New API this requires (not "wiring" — S3)

- `Scratchpad::get_u32_u64(len32, len64) -> (&mut [u32], &mut [u64])` —
  split-borrow of both buffers simultaneously (the current `get_u32`/
  `get_u64` are exclusive `&mut self` borrows and cannot be held together).
- `LivenessSnapshot::capture_into(mask, len, words: &mut [u64])` — snapshot
  into caller scratch (the existing `capture` allocates its own `Vec`).
- Words-parameterized query entry points on `SpatialCell` (the kernels
  already take `&[u64]`; the public `query_aabb`/`query_frustum` today
  allocate a `Vec<u64>` internally — **both** functions, spatial.rs — this
  is the §8.1 no-alloc carry-forward, closed here).

## 6. 2b.3 — Phase machine (M2b-α, compile-time)

C3 order, as zero-size witness types consumed and produced by phase
transitions — misuse is a compile error, not a debug assert:

```text
Frame::begin() -> SimulateA -> SimulateB -> Harvest -> (Cull/Draw: M3 no-ops)
    -> Boundary { retire -> transitions -> compact -> sync } -> Frame
```

- Mutation APIs (`write_transform`, `free_deferred`, `alloc`) require
  `&SimulatePhase` witnesses; harvest APIs require `&HarvestPhase`; boundary
  ops are methods on the `BoundaryPhase` witness that consume it in order.
  M2a's runtime `Phase` enum is retained inside `SceneGpuStore` as a
  debug-assert backstop (FFI/plugin code can't be type-checked).
  **Amendment (M2b-α final review):** `alloc`'s witness-gating lands with the
  M4 `World` integration, not here — `CellStorage::alloc` is core (graphics-
  free, C0) CPU API and cannot itself take a `gpu`-module witness type without
  breaking the no-Helio/no-rendering-dependency boundary the core is built
  on. `write_transform`/`free_deferred` are already gated because they live on
  `SceneGpuStore` (gpu-module) itself; `alloc` gating instead has to happen at
  the `World` call site that holds both a `CellStorage` and a phase witness,
  which is M4 scope.
- **Hard-enforces the M2a carry-forwards:** raw mirrored-column writes and
  direct `cell.compact()` lose their footguns because the phase-gated store
  API is the only path exposed at the `World` level (`column_for_mut` stays
  `pub` for non-mirrored columns, with the M2a doc warnings). `free()`'s
  immediate path is demoted to `pub(crate)` at the World level: the
  phase-gated deferred path is the only public deletion on GPU-backed worlds
  (closes the release-mode double-pool hazard, Task 3 ledger).
- **Tracker caller contracts hardened (S6):** `free_deferred` debug-asserts
  nondecreasing serials per pending queue (the retire drain's FIFO
  early-break assumes it — one out-of-order enqueue would silently stall
  every retirement behind it); `signal_submitted` documents (and the phase
  driver enforces by construction) that it is called only after the work for
  that serial is submitted.

### 6.1 No harvest pins (D1 — binding rationale)

Rev 1 proposed pinning live rows for the duration of an in-flight harvest.
Traced against `cell.rs`/`store.rs` this starves compaction in steady state:
an inner cell is harvested every frame, its cull serial is incomplete at
that same frame's boundary, so every live row of every observed cell would
be pinned at every boundary and dead rows would never be reclaimed
(`rows_in_use` ratchets to capacity while `live_count` stays low). It also
made "retire wins" unreachable (`mark_pending_retire` returns `None` on any
pinned row → deferred frees would silently leak).

Row-granularity harvest pins are **unnecessary**: harvest tokens are
frame-scoped (C4); the cull consuming them is submitted during Harvest/Cull;
and the boundary's `write_buffer` syncs are queue-ordered **after** that
submission — the GPU cull of frame N always reads pre-boundary buffer
contents, so compaction moving a harvested row cannot corrupt an in-flight
cull. This queue-FIFO argument is the invariant, stated here normatively.
The **only** serial pinning in M2b is at **region granularity** (eviction,
§4.1). The pin bitmask remains single-purpose (retirement), exactly as M2a
shipped it; `mark_pending_retire`, `write_transform`, and `compact_report`
keep their M2a semantics unchanged (M2a Test 6 remains the gate).

If a future consumer genuinely reads scene buffers across a boundary
(cross-frame async compute), the mechanism is region- or buffer-level serial
pinning at that consumer's granularity — never per-row-per-frame.

## 7. Components

- `SceneGpuStore` — global buffers (instance, slot mirror, mesh
  configurator, cluster, material placeholder, per-cell metadata,
  generation), size-class region pools (rows + slots), one
  `SubmissionTracker`, per-cell `CellGpuState` map. *(α)*
- `RegionPool` — per-size-class fixed-region free list; O(1) alloc/free;
  serial-pinned free. *(α)*
- `GeometryArena` / `MeshRegistry` / `ClusterBuffer` — §3. *(α)*
- `FramePhases` — witness types + `World`-level phase driver. *(α)*
- `StreamingGrid` — coords, dense cell ids, domain classification,
  hysteresis, α cross-fade, transition queue drained at the boundary. *(β)*
- `HarvestPipeline` — per-view partition + DEI compaction over scratchpads,
  lease/snapshot handling. *(β)*
- Extended M1b types per §5.1: `Scratchpad` split-borrow,
  `LivenessSnapshot::capture_into`, words-parameterized queries. *(β)*

## 8. Error handling

- Row- or slot-region exhaustion at promotion: hard error surfaced to
  streaming telemetry; the cell stays in its current domain (degraded draw
  distance, never UB or realloc). Slot-region overflow inside a resident
  cell (tombstone headroom exhausted): hard alloc error on that cell.
- Region-bounds assert on every generation and slot-mirror write (a write
  must never land in a neighbor's region).
- Geometry arena exhaustion at load: hard error at asset registration.
- Budget violation at startup: constructor failure (§4).
- Mesh metadata XOR-rule / cluster-error-monotonicity violations: hard
  registration errors.
- Lease-pool exhaustion blocks the query (spec §9.2); revocation per §9.2.1.
- All M2a invariants (C6 ordering, pin/compact interactions) unchanged and
  re-asserted at region granularity.

## 9. Testing (headless, no Helio)

**α gates:** Test 3 extension (§3); Test 14 extension (§3, with the
all-cells-drained precondition); phase-machine compile-fail tests (trybuild
or `compile_fail` doc-tests) proving mutation outside Simulate / harvest
outside Harvest do not compile; region-pool tests (size classes, serial-
pinned free, bounds asserts); slot-mirror maintenance under alloc +
compaction moves (mutation-tested per the Task 10 precedent).

**β gates:**
- **Test 10 — lease stall:** hold a lease past the 2.0 ms isolation window
  (test-controlled clock); assert revocation fires, reads continue against
  the snapshot, results re-validate against live generations, compaction
  proceeded, and the revocation was attributed/logged.
- **Test 11 — grid oscillation:** sub-pad observer jitter across a boundary
  for N frames ⇒ zero transitions; decisive crossing ⇒ exactly one
  promotion; demotion boundary verified `δhyst` beyond promotion.
- **Test 12 — DEI compaction:** sparse cell (< 25%) ⇒ dense payload + remap
  round-trip, byte counts prove no sentinel bandwidth; ≥ 25% bypasses.
- **Residency gates:** promotion warm-up syncs the whole region including
  slot mirror and rebuilt generation region (recycled-region test: promote
  cell A, evict, promote cell B into the same region, assert B's generations
  and shadow are correct — the D2 regression test); eviction returns the
  region only after its serial completes; queued retires commit CPU-side
  with zero VRAM writes into freed regions (gen-write-count instrument, per
  the M2a shadow-gate test pattern); global-row tokens from the partition
  match region math.
- Bench additions (`scenedb_bench`): region sync throughput, harvest
  partition + DEI compaction per 1024-row cell, promotion/demotion cost —
  the M2a-deferred sync benches land here.

## 10. Deferred

- Global meshlet buffer (spec §19) → **M3** asset-store addition
  (SceneDB-owned per C0): built nowhere and deferred nowhere until this line
  (audit-remediation finding; the M3 plan must add it to the asset store
  beside `ClusterBuffer`).
- Material 32 B layout + writer + Test 3 row → **M3** (C5).
- Bindless texture array; cull/indirect/HLOD-stipple shaders; the remap
  table's consumer choice → **M3**.
- Disk/compressed outer-cell frames (§5 "compressed cell frames in host
  memory"): M2b keeps outer cells' full host data resident; the
  compress/serialize path is an M4/asset-pipeline concern.
- Streaming budget stress-walker tool (designer-facing) → **M4/editor**.
- Physics/editor client integration of the phase machine → **M4** (cutover).

## 11. M2b-α implementation findings & carry-forwards (binding on β/M3/M4)

Recorded from the α review loops and the whole-milestone final review
(implementation plan: `plans/2026-07-14-scenedb20-m2b-alpha-assets-phases.md`):

**Findings fixed in α (context for later readers):**
1. **Slot-mirror triggers were fail-open twice** — a slot-scoped gen-gate
   missed retire→compact→realloc slot recycling, and per-event triggers
   missed alloc-without-write into a compaction-vacated row (ghost duplicate
   that *validates*). Shipped mechanism: the **self-healing boundary scan**
   in `sync_all` is the sole mirror trigger (§2); both shapes are regression
   tests in `tests/gpu_store.rs`.
2. **NaN validation bypass** — `a >= b` is not `!(a < b)` under IEEE-754;
   cluster error-monotonicity now uses the NaN-rejecting form, mutation-tested.
3. **Cluster offset-0 representability** — node 0 reserved (§3).

**Carry-forwards (must be addressed by the named milestone):**
- **β — recycled slot-region TAIL scrub:** `register_cell` rebuilds only
  `gens.len()` generation entries and zero-seeds the shadow tail
  `[gens.len()..slot_capacity)`. On a *recycled* region (eviction→promotion)
  that tail keeps the prior cell's VRAM generations while the shadow says 0 —
  fail-open for an allocated-but-never-written slot. Unreachable in α (no
  region ever recycles); β's promotion path (§4.1) must scrub or fully
  rebuild the tail. This extends the §4.1 reset ledger.
- **β — slot-region exhaustion surfaces as a write-time panic**, not the §8
  "hard alloc error" (the store cannot intercept `CellStorage::alloc`); β's
  residency layer owns turning it into a graceful degradation.
- **β/M4 — phase-machine hardening:** the witness types do not stop
  hoarded/duplicated Simulate witnesses (debug-assert-only coverage, silent
  in release) and nothing enforces that a boundary ever runs (liveness). A
  lifetime-carrying witness (`SimulateA<'frame>`) is the candidate fix; the
  honest coverage map lives in `src/gpu/phase.rs` module docs.
- **M3 — cull/draw witness stages** are absent (`HarvestPhase::end()` goes
  straight to `BoundaryPhase`); inserting them is an API break M3 must plan.
- **M4 — `alloc` witness-gating and `free()` demotion** at the World level
  (§6 amendments).
- **CI/tooling:** GPU test suites must never run concurrently on shared
  hardware (device contention hangs); run with `--test-threads=1`,
  sequentially per target.
- **M3/M4 — harvest `region_base` re-resolution contract:** `region_base`
  must be re-resolved in the ISSUING frame via `grid.gpu_id(coord)` →
  `store.row_region_base(id)`, never cached across a frame boundary — a base
  cached across a boundary that evicted and re-promoted the cell into a
  different region emits wrong global-row tokens SILENTLY (a stale `CellId`
  fails loud; a stale `u32` does not). The World driver (M4) owns this chain;
  documented on `HarvestPipeline::harvest_cell`/`harvest_views` (§5).
- **M3 — slot-mirror tail is stale-but-inert:** slot-mirror entries beyond
  `rows_in_use` are never written by the boundary scan and must never be
  read; the code-comment contract in `sync_all` promotes this from an
  implicit assumption to a binding carry-forward for M3's cull-pass consumer.
- **M4 — `register_cell`/`unregister_cell` are un-phase-gated `pub`:**
  residency transitions are supposed to happen only at the frame boundary
  (§4.1), but nothing today stops an in-crate or `pub(crate)` caller from
  invoking either function mid-frame — that discipline is convention only
  until the World driver (M4) gates them behind a boundary witness.
