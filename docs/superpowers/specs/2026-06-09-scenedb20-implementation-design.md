# SceneDB 2.0 Implementation Design

**Date:** 2026-06-09 (revised 2026-06-12)
**Status:** In execution — Stage 0, M1, M2a, and M2b-α complete; M2b-β in progress;
M3/M4 not started. (Status refreshed 2026-07-16 per the holistic audit —
`docs/superpowers/specs/2026-07-16-scenedb20-holistic-audit.md`.)
**Spec of record:** `Dev/Research/public/drafts/SceneDB2.0.md` (Rev 2.3)
**Repos in scope:** `Pulsar-Native` (branch `scenedb`), `Helio` (branch `scenedb20`), `Research` (spec revisions)

---

## 1. Problem & Goal

The SceneDB 2.0 / Helio specification (Rev 2.1) defines an engine-wide spatial database
(page-aligned SoA storage, generational handles, concentric cell streaming, read-leases,
SIMD spatial queries) and a GPU-driven renderer contract (persistent SSBOs, Hi-Z compute
culling, indirect draws, virtual geometry, HLOD proxies, token-driven slot retirement).

### 1.0 The Ownership Law (governs every milestone — see spec §0 / CONTRACTS.md C0)

**SceneDB owns all scene data, CPU and GPU.** It owns the persistent device
buffers holding scene object state, relates each object's CPU and GPU sides via
its stable slot id, owns the CPU→GPU delta-sync, and owns the queries/indices
serving the whole engine — physics, editor, and the renderer hot loop. **Helio
owns no scene state** — only renderer-internal derived data (Hi-Z, command
scratch, framebuffers, pipelines). **Helio depends on SceneDB; SceneDB never
depends on Helio.** The wgpu device/queue is an engine-level context that
outlives any renderer. This is the proof obligation behind the whole design and
is gated by spec Test 13 (Stateless Renderer Teardown). Every milestone below is
a consequence of this law, not a negotiation with it.

### 1.1 Current progress (2026-07-16, refreshed per the holistic audit)

- **Stage 0 — DONE.** Spec Rev 2.3, frozen `CONTRACTS.md` (C0–C7) mirrored to Helio.
- **Milestone 1 (Layer 1) — DONE.** `crates/core/pulsar_scenedb` (seeded from `pulsar_ecs`,
  which stays as reference; graphics-free): handles, paged 64B-aligned SoA, liveness +
  swap-and-pop compaction, `TypeToken`/`CellType` bridged to `pulsar_reflection`,
  runtime-dispatched SIMD AABB+frustum queries (AVX2 verified bit-for-bit vs scalar),
  leases/scratchpads, double-buffered liveness snapshot, Part VI Test 1 + Test 2-host
  gates. All tasks dual-reviewed + holistic APPROVE.
- **M2a — DONE.** `pulsar_scenedb::gpu` module: `SceneBuffer`/`GenerationBuffer`
  delta-sync, pin-by-serial retirement, generation-shadow gating; Tests 3, 6 (host),
  14 verified headless. See `specs/2026-06-13-scenedb20-m2a-gpu-store-design.md`.
- **M2b-α — DONE.** Region-partitioned `SceneGpuStore`, asset store (`GeometryArena`/
  `MeshRegistry`/`ClusterBuffer`), compile-time phase machine, global-slot mirror
  buffer; Test 3/14 extensions and compile_fail gates green. See
  `specs/2026-07-14-scenedb20-m2b-streaming-orchestration-design.md`.
- **M2b-β — IN PROGRESS.** Streaming grid/residency and harvest pipeline (Tests
  10–12); Task 1 landed as of the holistic-audit commit (`060175b5`), execution
  paused pending this audit's remediation.
- **M3/M4 — NOT STARTED.**

### 1.2 The world being replaced

- Scene state lives in `engine_backend/src/scene/`: legacy `SceneDb`
  (flat `DashMap<ObjectId, Arc<SceneEntry>>` with atomic transforms — no SoA, no paging,
  no spatial index) plus `SceneMetadataDb`/`ComponentDb`/`HierarchyManager`.
- Persistence is divergent JSON: runtime `SceneFile` (v1/v2.x) in `pulsar_scene`,
  editor `LevelFile` (v2.1) in `ui_level_editor`.
- `pulsar_reflection` provides the working type system: `Reflectable` derive,
  inventory-based registration (`RUNTIME_TYPE_REGISTRY`), property metadata with byte
  offsets, JSON codec.
- Helio is a wgpu-based GPU-driven renderer consumed as a pinned git dep (`f7e0a54`);
  it already has a compute-based virtual-geometry pass. **Critically, today the engine
  reconciles it via per-frame `sync_scene()` — Helio currently owns GPU scene state and
  the engine pushes to it. That is the legacy push-model the Ownership Law (§1.0)
  overturns:** under 2.0, Helio owns no scene state and reads SceneDB's buffers.

The goal is the full spec, both repos, with SceneDB 2.0 ultimately **replacing the ECS**
as the engine's single authoritative cross-device memory management system.

## 2. Locked Decisions

| Decision | Choice | Rationale |
|---|---|---|
| **Data ownership (LAW)** | **SceneDB owns all scene data CPU+GPU; Helio owns no scene state and depends on SceneDB** | Spec §0 / CONTRACTS.md C0; single authoritative cross-device store, enforced by dependency direction; gated by Test 13 |
| **Crate structure** | One crate: `pulsar_scenedb` = graphics-free core (Layer 1, done) + feature-gated GPU layer (module `pulsar_scenedb::gpu` behind the `gpu` feature, optional wgpu dep; owns device scene buffers). **No separate GPU crate.** Helio depends on `pulsar_scenedb` with `features = ["gpu"]` | Single owner of both device sides of every slot (C0); retirement/pin/compaction reach column internals as `pub(crate)` peers instead of widened public API; graphics-free core enforced by `cargo check --no-default-features` in CI; forbids any `pulsar_scenedb`→Helio edge |
| **GPU device ownership** | wgpu `Device`/`Queue` is an engine-level context shared by reference; outlives any renderer | Required so scene buffers survive a Helio teardown (Test 13) |
| Scope | Full spec, both repos, one master plan | User decision |
| Core home | `cp crates/pulsar_ecs → crates/pulsar_scenedb`; original kept as reference; copy evolved into Layer 1 (done) | Preserves working ECS lineage (handles, dense ids, benches) while allowing a clean break |
| Type system | `pulsar_reflection` (`Reflectable`/`RUNTIME_TYPE_REGISTRY`) backs Part III's type contract | Working, already integrated with editor + serialization; field byte offsets available |
| GPU strategy | Adapt spec to wgpu/WGSL | Keeps the working renderer and the existing VG pass; spec gets a mapping appendix |
| Migration | Parallel build + feature-flag switchover; one-time JSON migration tool | Architectures too different for in-place evolution; editor must keep working |
| Ordering | Bottom-up by layer: Stage 0 → L1 → L2 → L3 → integration | User decision; each layer fully verified before the next |

### 2.1 Vulkan → wgpu adaptation (normative for all milestones)

| Spec mechanism | wgpu implementation |
|---|---|
| Timeline semaphores for slot retirement | `Queue::on_submitted_work_done` callbacks keyed by a monotonically increasing host-side submission serial; retirement queue drains only serials whose callback has fired |
| Task/mesh shaders for VG | Compute-shader cluster cull emitting per-meshlet `DrawIndexedIndirect` records, drawn via `multi_draw_indexed_indirect` (extends the existing `helio-pass-virtual-geometry` design) |
| `vkCmdDrawIndexedIndirectCount` | GPU writes an atomic draw counter; CPU reads it back (or conservatively submits max count with `instance_count = 0` for dropped slots); count clamp happens host-side after the compute pass, exactly as §14.2 already prescribes |
| GLSL + `GL_EXT_scalar_block_layout` | WGSL layout rules. All shared structs are authored with scalar `f32`/`u32` fields (no `vec3` members, which carry 16-byte alignment in WGSL) so byte offsets match the host contract. Verified by Test 3 via naga reflection |
| `GL_EXT_nonuniform_qualifier` bindless | wgpu binding_array / texture arrays as supported by the custom fork; capability-gated |
| AVX-512 SIMD scans | Portable SIMD (runtime feature detection: AVX2/AVX-512/NEON, scalar fallback). Throughput targets validated by criterion benches, not instruction-set assumptions |

## 3. Stage 0 — Spec Reconciliation & Frozen Contracts

**Deliverables:** Spec Rev 2.2 (Research repo) + `CONTRACTS.md` (shared, lives in
Pulsar-Native, mirrored into Helio).

1. **Resolve the logged analysis issues** (the "Claude Analysis" block embedded in the
   spec). Headline resolutions:
   - **Handle index vs swap-and-pop (blocker):** handles carry a *stable slot ID*;
     pages store rows densely; a slot→row indirection table (one u32 per slot, updated
     during compaction) bridges them. Harvested index arrays remain frame-scoped as
     specified, but handle dereference is now well-defined across frames.
   - **Second writer:** physics solver writeback is modeled explicitly — a dedicated
     sub-phase of the simulation phase with write-leases, so "single routine writer"
     becomes "single writer *per phase*".
   - **Hi-Z same-frame ordering:** an explicit depth-pyramid rebuild pass is specified
     between the traditional pass and the VG object-level cull.
   - **Mesh metadata alignment:** drop the false "16-byte alignment preserved" claim;
     re-derive the 72-byte layout under WGSL rules with scalar fields.
   - **Hi-Z floor-mip wording** corrected per the analysis; **near-plane bypass**
     narrowed with a view-space pre-test; **VG error metric** gains the
     bounding-sphere-radius correction; **read-lease bitmask** respecified for dynamic
     thread pools (lease slots, not thread IDs).
2. **Merge `SceneDataCorrections.md`** into Rev 2.2: lease timeout/revocation with
   double-buffered liveness, holistic cross-component stride check, spatial hysteresis
   (10% cell-width padding), DEI ≥ 25% dense-compaction rule, adversarial tests 10–12.
   Stride-limit conflict resolved in favor of Rev 2.1's **128 bytes**.
3. **Add the wgpu adaptation appendix** (§2.1 above) and the SIMD strategy.
4. **Freeze contracts** in `CONTRACTS.md`: handle bit layout, page header format,
   slot→row indirection semantics, all SSBO byte layouts (host Rust + WGSL, with the
   naga-reflection test as the enforcement mechanism), `TypeToken` API surface,
   frame-phase ordering state machine, lease acquire/release/revoke API.
   After Stage 0, contract changes require touching `CONTRACTS.md` first.

## 4. Milestone 1 — Layer 1: Storage Core (`crates/pulsar_scenedb`)

Seed: `cp -r crates/pulsar_ecs crates/pulsar_scenedb` (then rename package; pulsar_ecs
stays untouched as reference). Work order:

1. **1.1 Handle registry.** 64-bit packed handles (32-bit slot index, 32-bit
   generation), generation 0 = `INVALID_HANDLE`, slots permanently retired at
   `u32::MAX`, free pool. Evolves the existing generational `Entity`.
2. **1.2 Paged SoA storage.** `SceneDBCell` pages: one contiguous allocation per page,
   header with length/capacity/column byte offsets, every column 64-byte aligned,
   capacity chosen per cell type (256 default, 1024 ceiling). Replaces archetype
   `Vec`-per-component storage.
3. **1.3 Liveness + deferred compaction.** Atomic liveness bitmask (1 bit/element),
   deletions mark only; swap-and-pop compaction runs at the frame-boundary phase and
   maintains the slot→row indirection table.
4. **1.4 Compile-time type registration.** `TypeToken`s generated per registered type,
   bridged to `pulsar_reflection` (`EngineClass`/inventory) so editor metadata,
   serialization, and SceneDB columns share one registration. Per-cell-type column
   layouts; 128-byte stride guardrail as a `const` assertion; holistic cross-component
   stride check at cell-composition level.
5. **1.5 Spatial queries.** Six bounds columns (MinX..MaxZ), SIMD AABB and frustum
   scans writing into caller-provided scratch buffers, null-sentinel
   (`0xFFFF_FFFF`) unified index token output, multi-view concurrent queries.
6. **1.6 Leases & scratchpads.** Per-cell atomic lease mask, thread-local scratchpad
   pools with the 8-frame 50% decay policy, lease timeout → revocation via the
   double-buffered liveness mask.

**Verification:** Test 1 (multi-threaded contention, thread-sanitizer where available),
Test 2 host half (stale-handle rejection), property tests comparing SIMD scans against
a scalar reference, criterion benches extending `pulsar_ecs/benches/ecs_bench.rs`
(SoA page scan vs archetype iteration vs legacy `SceneDb` DashMap).

## 5. Milestone 2 — Layer 2 (split into M2a + M2b)

Layer 2 is decomposed so the cross-device memory-management core (the heart of
the Ownership Law) is built and verified standalone, before the spatial-streaming
orchestration. Detailed design for M2a:
`docs/superpowers/specs/2026-06-13-scenedb20-m2a-gpu-store-design.md`.

### 5a. Milestone 2a — GPU-resident store, delta-sync & retirement

**This is where the Ownership Law (§1.0) becomes real.** M2a builds the
device-side store as a **feature-gated GPU layer inside `pulsar_scenedb`**
(module `pulsar_scenedb::gpu`, `gpu` cargo feature, optional wgpu dep — no
separate crate) and proves it standalone (no Helio, no rendering, headless wgpu).

1. **2a.0 GPU-resident store + device context (`pulsar_scenedb::gpu`).** New
   feature-gated module pulling `wgpu` (fork, rev-matched to Helio) as an
   optional dep, **never Helio**. Holds the engine-supplied
   `Arc<Device>`/`Arc<Queue>` (outlives any
   renderer). Owns four persistent SSBOs in canonical compact C5 layout: instance
   64 B (mat4 only — derived normal/AABB not stored), material 32 B, mesh metadata
   72 B, generation buffer u32/slot. Allocated once. Exposes read-only buffer/bind
   references for a future Helio. **Test 3** (host↔naga byte-exact) lands here.
2. **2a.1 Delta-sync.** Each mirrored column carries a dirty bitmask (atomic u64
   words, the M1 `LivenessMask` pattern); a Layer-1 write sets the slot bit. At the
   sync sub-phase, scan dirty words, coalesce contiguous dirty slots into byte
   ranges, issue minimal `queue.write_buffer` calls (memcpy, byte-identical layout,
   no conversion), clear bits. No scan-and-diff, no shadow copy, zero clean-row
   uploads. Threads the M1 `Scratchpad` (extended for the range list) for
   zero mid-frame heap allocation.
3. **2a.2 Retirement engine (interposes on M1 free).** A delete enqueues
   `(slot, gen, submission_serial)` and **withholds the slot**; `Queue::on_submitted_work_done`
   marks serials complete; at the frame-boundary drain, the new generation is
   written to the VRAM generation buffer **before** the slot returns to the free
   pool. Owns both the slot allocator (Layer 1) and the GPU buffer (2a.0) — one
   coherent operation (the reason C0 requires single ownership). Replaces M1's
   immediate-free in the GPU-backed configuration.
4. **2a.3 Minimal phase coordination.** Two ordered points only — a `sync()` point
   (drains dirty masks after the write window) and a `retire()` drain (frame
   boundary). The full compile-time phase-guard state machine is M2b/M3.

**Verification (headless wgpu, no Helio):** Test 3 (byte-exact), delta correctness
+ minimality (readback; no-mutation frame writes nothing), Test 6 host-side
(retirement invariant: slot not reissued until serial completes and generation
buffer written first), **Test 14** (device-loss re-materialization — rebuild the
GPU side from Layer 1's authoritative columns, byte-identical). Test 13 (renderer
teardown) needs Helio → M3.

### 5b. Milestone 2b — Asset integration & streaming orchestration

1. **2b.0 Asset integration.** SceneDB ownership of geometry/vertex-index buffers
   (and cluster/meshlet buffers) with **load-time** upload driven by mesh asset
   loading — a different access pattern from the per-frame delta path. The HLOD
   proxy registry (cell-handle-indexed, 72-byte mesh metadata entries).
2. **2b.1 Concentric cell grid.** Uniform grid; inner-core / active-margin /
   outer-buffer domains from the union of all observer AABBs; promotion/demotion
   at frame boundaries; hysteresis padding (§5.5); per-cell HLOD cross-fade state.
3. **2b.2 Harvest pipeline.** Single-scan partition of query output into
   traditional-LOD / VG / HLOD staging arrays; DEI computation and dense SIMD
   compaction when DEI < 25% (§8.5); zero-allocation via the `Scratchpad`.
4. **2b.3 Full frame-phase state machine.** Simulate → harvest → cull → draw →
   retire/compact with compile-time phase-scoped guard types.

**Verification:** Test 10 (editor lease stall), Test 11 (grid boundary oscillation),
Test 12 (sparse-cell DEI compaction).

## 6. Milestone 3 — Layer 3: Helio (stateless consumer)

Runs in the Helio repo against staged/mock harvest data; can overlap Milestone 2 once
Stage 0 contracts are frozen. **Per the Ownership Law (§1.0): Helio allocates and owns
NO scene buffers — those are created and owned by `pulsar_scenedb::gpu` (M2a.0). Helio
depends on `pulsar_scenedb` with `features = ["gpu"]`, receives the device context +
buffer/bind-group references, and owns only the derived per-frame data it produces.** Helio's work is
passes, not ownership.

1. **3.1 Bind SceneDB's buffers + own only derived data.** Helio takes the engine
   device context and SceneDB's persistent SSBO/bind-group handles (instance, material,
   mesh configurator, generation buffer, geometry, cluster/meshlet — all owned by
   M2a.0/M2b.0, the pre-split label this section originally used)
   and binds them. Helio creates and owns ONLY: pipelines, shaders, the Hi-Z pyramid,
   per-view draw-command/counter buffers, task payload scratch, framebuffers. **No scene
   buffer is allocated in Helio.** This is the precondition for **Test 13** (drop Helio,
   scene survives) — Helio must hold no handle that is the sole owner of scene state.
2. **3.2 Cull compute pass.** Frustum test, near-plane W≤0 bypass with near-clip flag,
   Hi-Z occlusion with same-frame pyramid rebuild, floor-mip selection with 5%
   boundary dual-sampling and dynamically expanded 3×3/4×4 gather kernels,
   shader-side generation validation against the VRAM buffer.
3. **3.3 Indirect command generation.** Bounded `atomicAdd` slot allocation (no
   clamp-in-shader), per-view command buffers and counters, CPU-side count clamp.
   **Test 5** (overflow protection with sentinel-pattern validation).
4. **3.4 Virtual geometry hardening.** Evolve `helio-pass-virtual-geometry` to the
   full cluster DAG: 48-byte `ClusterNode`, error-driven two-condition node selection
   (with bounding-sphere-radius distance correction), backface cone culling,
   per-meshlet frustum culling — all in compute. **Test 8** (DAG error invariant).
5. **3.5 HLOD proxy path.** Cell-handle-indexed proxy meshes rendered for all outer
   buffer cells, Bayer-matrix stippled cross-fade on domain transitions, distance-based
   transition duration. **Test 7** (horizon continuity).

Plus GPU-side **Test 2** (stale handle injected into the cull shader), **Test 4**
(transformation persistence sweeps with the absolute-matrix AABB method), and — the
gate that proves the Ownership Law holds — **Test 13** (drop the Helio instance, rebuild
it against the same SceneDB, scene renders identically with zero scene-data reload; the
device and every scene SSBO survive). **Test 13 is a hard merge gate for M3/M4: C0 is
unsatisfied until it passes.**

## 7. Milestone 4 — Integration, Switchover, ECS Replacement

1. **4.1 Renderer subsystem.** New harvest-driven render path in `engine_backend`
   replacing per-frame `sync_scene()`, behind a `scenedb2` feature flag.
2. **4.2 Editor migration.** `SceneDatabase` (ui_level_editor) re-backed by
   SceneDB 2.0; `pulsar_reflection` component editing and property UI unchanged.
3. **4.3 Persistence unification.** One scene file format; one-time migration tool
   covering runtime `SceneFile` v1/v2.x and editor `LevelFile` 2.1; `SceneLoader`
   ported to the new path.
4. **4.4 Switchover.** Tests 1–14 green in CI on both repos — including **Test 13
   (Stateless Renderer Teardown)** and **Test 14 (device-loss re-materialization)**, the
   Ownership Law gates; benchmark report vs legacy; default flag flipped; legacy
   `SceneDb`/`SceneMetadataDb` and the per-frame `sync_scene()` push-path deleted.
5. **4.5 ECS replacement.** Gameplay components registered as SceneDB cell types;
   `World`/query API surface (inherited from the pulsar_ecs copy) maintained for
   gameplay code; `pulsar_ecs` deprecated and removed from the workspace once no
   consumers remain.

## 8. Cross-Repo Logistics

- During development, Pulsar-Native carries a `[patch]` section pointing at the local
  Helio checkout; each milestone ends with a pinned-rev bump of the
  `helio`/`helio-asset-compat` git deps.
- Branches: `scenedb` (Pulsar-Native, exists), `scenedb20` (Helio, to create).
- Spec changes after Stage 0 flow Research → `CONTRACTS.md` → code, never code-first.

## 9. Error Handling Posture

- Stale handle access: rejected and logged; never a panic, never UB.
- Lease violations: hard error (debug builds) / timed revocation (release).
- SSBO overflow: draws dropped, counter clamped host-side, drop count surfaced to
  telemetry.
- Anything that can be a compile-time error is one: stride limits, phase-ordering
  violations, layout mismatches (via generated const assertions and Test 3 in CI).

## 10. Testing Strategy

- Part VI tests 1–8, Corrections tests 10–12, and **Ownership Law gates 13–14** are the
  acceptance gates, assigned to milestones as listed above. **Test 13 (teardown) is the
  binding acceptance criterion for CONTRACTS.md C0.**
- Every SIMD path has a scalar reference implementation and property tests.
- Byte-layout contracts enforced mechanically (naga reflection diff), not by review.
- Benchmarks (criterion) track: SoA scan throughput, query latency vs entity count,
  delta-sync cost per frame (dirty rows only), harvest cost per frame, cull dispatch +
  readback latency.

## 11. Out of Scope (deferred per spec Appendix B)

Streaming budget profiler tool, split-screen/portal budgeting analysis, dynamic light
visibility integration, VG offline asset build pipeline (cluster DAG baking), HLOD
proxy generation tooling, skinned/deformable mesh support.
