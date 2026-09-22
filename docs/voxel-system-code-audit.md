# Unified Voxel System — Phase 1 Code Audit

**Status:** read-only research complete; decision input for Phase 2, not an implementation specification. Findings are against the parent-pinned Helio checkout at `fe7aa140363d8870549ebad8198941a85d32a6f4` unless stated otherwise.

**Required architecture authority:** [`voxel-system-design.md`](voxel-system-design.md) and the shared rules in [`voxel-system-implementation-plan.md`](voxel-system-implementation-plan.md). In particular: persistent/canonical terrain state belongs to SceneDB; only transient/rebuildable state belongs in rendering; generic renderer crates must not understand voxel/terrain semantics; material IDs reuse the existing SceneDB material system; no arbitrary authored-entry cap; no render-thread generation or per-voxel write loop.

## Executive findings

1. **Do not merge the upstream branch wholesale as the architecture.** `origin/feat/tiny-voxel-stress-test` is one commit (`a0bf48758b149a40d8058eaa7ee8931f73233662`) directly atop `origin/main` (`022479388c68632d8720557a67f7784d53a9a8f1`). It adds a substantial experimental voxel pass but retains the old planetary pass, documents a one-world prototype, and modifies unrelated renderer passes. Treat it as algorithm/test inspiration and selectively reconcile its useful parts after the base decision.
2. **There are three current voxel-rendering implementations to disposition:** planetary, voxel mesh, and voxel raymarch. All three are plausible candidates for the user's unified voxel pass, but they represent distinct behaviors and APIs. Replacing planetary alone is not the same as consolidating all three; retain their user-visible capabilities or explicitly retire them with migration evidence.
3. **The SceneDB/reflection model supports the broad component shape, but not every claimed performance/atomicity property out of the box.** `#[property]`, `#[gpu]`, `Vec<T>` GPU pools, queued GPU mirroring, world-level revision, and opaque keyed buffer handles exist. The audited APIs do not themselves establish per-entry/chunk revisions, atomic large voxel batches, lockless general mutation, or durable serialization of GPU vectors.
4. **The existing material path is SceneDB-backed and already consumed as a keyed buffer by rendering.** A voxel palette can reference the existing material records, but the implementation must distinguish stable material IDs/row indices from compact per-voxel palette slots and validate missing/deleted IDs. Do not add a second material database.
5. **The 10 ms workload cannot be reproduced from this checkout.** `runtime/Cargo.toml` is absent and the `vr-blocks-client` source/flag parser was not located. Helio's profiler provides graph/pass CPU and GPU timing snapshots, not by itself total application frame latency or p50/p95/p99. The target remains an acceptance goal, not a result.
6. **Current data is sufficient to choose a careful import strategy, not to start implementation.** There are five shared modified paths between the local Helio research branch and upstream. The workload game is proprietary, does not belong to the user, and stays outside this repository; use it only through external test invocation. Never copy/vendor/commit its source here. If any game-owned test outputs must be created inside the worktree, narrowly ignore the exact output path first.

## Git and upstream audit

Observed refs and read-only commands:

| Ref / check | Result |
|---|---|
| Parent-pinned Helio gitlink | `fe7aa140363d8870549ebad8198941a85d32a6f4` |
| Local Helio branch | `codex/hlfs-rt-research-plan` at `fe7aa140`; clean during audit |
| `origin/main` | `022479388c68632d8720557a67f7784d53a9a8f1` |
| `origin/feat/tiny-voxel-stress-test` | `a0bf48758b149a40d8058eaa7ee8931f73233662` |
| Merge base (`origin/main` vs feature) | `022479388c68632d8720557a67f7784d53a9a8f1` |
| Left/right commit count | `0 1` — feature is one commit ahead of `origin/main` |
| Feature diff summary | 66 files; 10,208 insertions; 186 deletions |
| `git diff --check origin/main...origin/feat/tiny-voxel-stress-test` | No whitespace errors reported |

The five path overlaps between the current local Helio branch and upstream are:

- `crates/passes/3d/helio-pass-dof/src/lib.rs`
- `crates/passes/3d/helio-pass-portal-instances/src/lib.rs`
- `crates/passes/3d/helio-pass-sky/src/shaders/sky_lut.wgsl`
- `crates/passes/3d/helio-pass-tsr/shaders/tsr_main.wgsl`
- `crates/passes/3d/helio-pass-virtual-geometry/src/rendering.rs`

Path overlap is not the same as a textual conflict. Review these contents if any upstream commit is integrated. Other non-voxel upstream edits include deferred-light/material shader, shadow matrix, sky, TSR diagnostics/tests, DOF, portals, and virtual geometry. None was established as a prerequisite for the voxel architecture.

### Upstream voxel implementation: reuse candidates and constraints

The upstream commit adds `crates/passes/3d/helio-pass-tiny-voxel/` and `tools/voxel-planet/`. It includes a stored-terrain path, chunk/edit/world types, selection and journal workers, shaders, picking-related code, and tests. The branch still contains `helio-pass-planetary-voxel`; it does not replace or remove it.

Verified mismatches or questions against the target:

- `helio-pass-tiny-voxel/src/engine.rs` defines `SharedVoxelFrame = Arc<Mutex<Option<EngineVoxelFrame>>>`: useful prototype handoff code, but not a substitute for SceneDB as the durable authority or a lockless general state model.
- `src/world.rs` defines `MAX_EDITS = 65_536`. This is an authored-edit ceiling and cannot become a system-wide product limit.
- `src/engine/residency.rs` defines `BRICK_CAPACITY = 65_536`. This is transient residency, not necessarily an authored-data cap, but the final cache budget should be device-derived, observable, and eviction-capable.
- Chunk/shader coordinates use `i32` in examples such as `src/chunks.rs` and `src/engine/stored.wgsl`; address range and precision must be designed for huge/unbounded domains.
- The README describes a single editable world, approximate distant GPU occupancy, exact CPU picking/collision at 10 cm, and complete-cut publication. This is a stress-tested planetary path, not a documented N-entry SceneDB component contract.
- The README names `build_default_graph_external_with_tiny_voxels` and a `tiny_voxel_extension` test, but these symbols/files were not found in the branch's tracked contents. This is an integration/documentation mismatch, not proof of a compile failure.

No build or tests were run on the upstream branch; buildability and performance are unknown.

## SceneDB, reflection, and renderer boundary

### What exists

- `#[engine_class(..., scene_store)]` composes the engine reflection class with the SceneDB `SceneStore` derive. `#[property]` controls reflected/editor-visible property exposure; it is separate from GPU projection. Macro implementation and constraints are in `crates/core/engine_class_derive/src/lib.rs` (notably `derive_engine_class` and `engine_class`).
- The audited macro supports GPU-marked properties and variable-length `Vec<T>` GPU fields. A non-property fixed-layout GPU-only scalar may not be accepted by the current derive; validate exact constraints before freezing the combined component/GPU row design.
- Helio's `StaticMeshComponent` marks `vertices` and `indices` as GPU vectors and uses `#[serde(skip)]` (`crates/renderer/helio/crates/helio-component/src/components/static_mesh_component.rs`, around line 333). This is a GPU-pool precedent, not proof that those vectors are canonical serialized state.
- `GpuColumnSet` exposes GPU columns in declaration order; SceneDB recognizes `Vec<T>` as variable-length storage. The GBuffer `MeshComponent` shows named GPU vector buffers and generated handle accessors (`crates/renderer/helio/crates/passes/3d/helio-pass-gbuffer/src/components.rs`).
- `World::flush_gpu_mirror` flushes deferred mirror writes and can coalesce adjacent row writes. SceneDB GPU pools/registries use `RwLock`s and growable buffers expose epochs/handles. This supports batching and efficient handle-based consumption, but the observed implementation is **not evidence that arbitrary SceneDB mutation is lockless**.
- SceneDB offers a world-level mutation revision/subscriptions. That is not, on its own, a per-entry or per-chunk stale-work token.
- `helio-core/src/scene_input.rs` defines `SceneBufferProjection`, which carries keyed opaque `BufferHandle`s. GBuffer resolves the `"materials"` key (`helio-pass-gbuffer/src/lib.rs`) and its shader indexes the material array using an input material ID (`shaders/gbuffer.wgsl`). `MaterialComponent` is stored in the SceneStore material buffer (`helio-pass-gbuffer/src/components.rs`).

### Required API consequences

- Keep canonical generator configuration, edits, and non-regenerable chunk results in SceneDB-backed CPU state. GPU projections and renderer vectors remain derived/rebuildable.
- Define a terrain-owned stable component/source identity plus entry/chunk revision tokens. A world-wide revision alone is too coarse for efficient invalidation and stale-worker rejection.
- Define a batch transaction/publication API with validation, bounded queueing/back-pressure, coalescing rules, stale-result rejection, and atomic visible revision. The generic `World` APIs audited do not provide all these semantics automatically.
- Resolve GPU buffer handles/epochs safely after growth/rebinding. Do not hold generic renderer locks while running generation, meshing, or terrain traversal.
- Store per-voxel values as compact palette slots if useful; the component palette maps those slots to existing SceneDB material IDs. Spell out ID stability, removal/reuse, bounds checks, and fallback behavior.
- Preserve the one-type reflection goal only where the current macro supports it. Keep editor-visible authored properties marked as properties; do not expose internal projection fields just to satisfy GPU layout. If macro limits force a separate projection type, keep the persistent component and projection contract explicit and generic outside the voxel pass.

## Pass inventory and disposition

At the audited baseline, `crates/passes/3d` contains 47 pass directories; `crates/passes/2d` has no pass subdirectories.

| Disposition | Current code | Rationale / evidence |
|---|---|---|
| **Unified-pass candidates** | `helio-pass-planetary-voxel`, `helio-pass-voxel-mesh`, `helio-pass-voxel-raymarch` | Three distinct voxel paths today. Planetary is added in `helio-default-graphs/src/lib.rs` around line 790; mesh is wired around lines 801/1713 and used by a VR scene; raymarch has a separate demo. The product goal suggests one pass, but parity and API migration must be explicit. |
| **Required replacement** | `helio-pass-planetary-voxel` | User explicitly intends the old planetary pass to be removed and replaced, not maintained as a second planetary authority. It defines `PlanetaryVoxelRenderPass` in `.../helio-pass-planetary-voxel/src/render.rs` around line 576. Default graph APIs/tests expose/configure it (`helio-default-graphs/src/lib.rs`, `tests/planetary_extension.rs`). |
| **Must remain independent/shared** | `billboard`, `corona`, `debug`, `debug-overlay`, `decal`, `deferred-light`, `depth-prepass`, `dof`, `flare`, `foliage-gbuffer`, `foliage-place`, `forward-lit`, `fxaa`, `gbuffer`, `hiz`, `hlfs`, `indirect-dispatch`, `light-cull`, `object-batch`, `occlusion-cull`, `perf-overlay`, `planar-reflection`, `portal-cull`, `portal-instances`, `postprocess`, `radiance-cascades`, `shadow`, `shadow-cull`, `shadow-dirty`, `shadow-matrix`, `simple-cube`, `sky`, `smaa`, `ssao`, `ssr`, `transparent`, `tsr`, `underwater`, `virtual-geometry`, `volumetric-fog`, `water-caustics`, `water-sim`, `water-surface` | These render or support unrelated geometry, lighting, shadows, foliage, portals, water, or diagnostics. Voxel output can feed ordinary downstream render contracts; it does not subsume these responsibilities. |
| **Preserve pending scope decision** | `helio-pass-sdf`; foliage integrations | SDF may overlap density-field concepts, but evidence does not establish that its general SDF use is terrain and should be replaced. Foliage passes have specialized responsibilities and are not automatically part of voxel rendering. |

“Must remain” means out of voxel replacement scope, not that every pass participates in every default graph.

### Existing planetary dependency surface

Replacing the pass requires more than deleting its crate:

- `crates/passes/3d/helio-pass-planetary-voxel/`: 44 tracked Rust/WGSL/TOML/Python files, including render/residency, GPU publication/upload, Transvoxel transitions, and generated lookup data.
- `crates/helio-component/src/components/planet_terrain_component/`: component, runtime/cache, live runtime, and render adapter. `render_adapter.rs` imports and looks up `PlanetaryVoxelRenderPass` (around lines 11, 99–110); `live_runtime.rs` also finds the pass (around line 172).
- `crates/helio-default-graphs/`: typed planetary graph constructors, registration, dependency, and `planetary_extension` tests.
- `crates/examples/`: `planet_voxel_demo` directly uses the pass; separate mesh and raymarch demos need migration or explicit retirement.
- Workspace/manifests and downstream VR scenes include dependencies/usages. Transvoxel-derived code requires license/provenance review (`LICENSES/Transvoxel-MIT.txt`) before moving or removing it.

**Boundary issue to resolve:** `helio-component` has a typed adapter that knows the planetary pass. The generic `helio-core` scene projection can carry opaque buffers, but the correct home for voxel components/adapters is not yet decided. Generic graph crates also currently expose typed planetary constructors; classify whether a new domain-specific graph extension crate is needed so central renderer code remains terrain-agnostic.

## Removal/reuse estimate

Method reported by the pass inventory: count physical lines in tracked `.rs`, `.wgsl`, `.toml`, and `.py` files under each candidate pass crate; exclude docs, assets, demos, component adapters, and graph wiring.

| Current candidate pass | Files | Tracked source lines | Interpretation |
|---|---:|---:|---|
| Planetary voxel | 44 | 19,111 | Upper bound of old pass-crate code potentially removable after full replacement/parity; not a forecast |
| Voxel mesh | 9 | 2,261 | Additional only if its behavior is consolidated or retired |
| Voxel raymarch | 9 | 1,418 | Additional only if its behavior is consolidated or retired |
| **Combined** | **62** | **22,790** | Inventory size; algorithms, tests, shaders, or generated tables may be reused/moved instead of deleted |

These are not net code savings. Adapter/component code, graph wiring, manifests, demos, licenses, and downstream use are additional migration work. A final deletion claim must list each path/symbol as remove, move/reuse, or retain and cite replacement tests.

## Performance feasibility and benchmark contract

### What is verified now

- `Test-Path runtime/Cargo.toml` in the parent repo returns false. The `vr-blocks-client` crate and flags `--radius`, `--detail-radius`, `--shrubbery-trees`, and `--perf` were not available to inspect in this checkout.
- Helio's profiler (`crates/helio-core/src/profiling/mod.rs`, around lines 325–398) records per-pass CPU/GPU values and `total_cpu_ms` / `total_gpu_ms`; when present, graph-frame GPU timestamp is preferred over summed pass GPU time. Graph execution records timestamp data (`crates/helio-core/src/graph/execution.rs`, around line 1666).
- These snapshots do not prove total game/application frame latency, simulation/generation costs, or present interval, and they do not provide a ready p50/p95/p99 acceptance report. Do not substitute pass sums or a voxel microbenchmark for the user’s frame-time budget.
- The current planetary code has residency, extraction, Transvoxel transitions, and meshlet build/cull paths. Their existence indicates possible workload/cost centers, not that the external game exercises them or meets 10 ms.

### Risks to measure (not measured findings)

- Unbounded flat worlds: page demand/selection cost may scale with visible horizon; record queue depth, page request/generation/publication, upload volume, and evictions.
- Distant horizons: LOD cut completeness, transitions, residency pressure, popping/cracks during flight and teleport; report per-LOD selection and missing pages.
- Sculpting/destruction: batch commit cost, neighbor invalidation, re-extraction, upload, and delay to atomic publication under both isolated and sustained edits.
- Rounded planets: address precision, large-coordinate behavior, face/LOD boundaries, seams, and edit invalidation at target scale.
- N SceneDB entries: entity scan/scheduling/culling/binding cost, per-entry fairness/starvation, transient resident bytes, and cache churn independent of total visible geometry.
- External bulk publication: validation/serialization/copy costs, SceneDB contention, work queue/back-pressure, cancellation/stale results, staging memory, and producer-vs-frame-thread time.
- Upstream worker path: measure lock wait/hold, backlog, bytes copied, and selected-vs-published pages before reuse; do not assume its single-world/edit cap fits the N-entry SceneDB model.

### Inputs required to qualify 10 ms

1. For workload verification, provide the external checkout location/revision containing `runtime/Cargo.toml` and `vr-blocks-client`; invoke it from there without importing its proprietary source into this repository.
2. Resolve flag meanings, scene/terrain setup, seed, camera path, GPU selection, output resolution, and whether `--perf` changes execution or only reports metrics.
3. Define the accepted timing quantity and percentile (for example application frame interval p95 versus GPU graph time), warm-up/sample count, cold-start treatment, hardware/driver, and repetition count.
4. Report application frame/present interval separately from render-graph CPU/GPU and terrain-specific counters. Preserve timestamp availability/lag/drop information.
5. Run matched baseline/candidate scenarios for the specified radius/detail/tree case, then isolated unbounded-plane, far-horizon, dense-edit, planet-transition/destruction, many-entry, and bulk-update cases. Record quality/seam correctness alongside performance.

Until these inputs are supplied, the 10 ms statement is a test target only and cannot be declared achievable or achieved.

## Decisions needed before Phase 2

1. **Mesh/raymarch scope:** the overall goal reads as unifying every voxel/terrain path. Confirm that the replacement voxel pass should absorb both `helio-pass-voxel-mesh` and `helio-pass-voxel-raymarch`, preserving useful behaviors/demos, or specify any path that must remain separate.
2. **Component boundary:** can voxel component types/adapters live in `helio-component` as authoring/domain code, while generic renderer crates remain unaware, or must they move to a new domain-specific component crate? Generic renderer crates and render-pass crate boundaries should be named precisely.
3. **Upstream import:** should Phase 2 bring in only selected voxel algorithms/tests and reject unrelated graphics changes, or is there any explicit desire to include the five overlapping/non-voxel areas? Default recommendation is selective voxel reuse, not wholesale import.
4. **Benchmark source/metric:** where is the game checkout, and what measurement defines “frame time <= 10 ms”? The runtime command cannot be validated without that code and metric.
5. **External test integration:** how should this Helio checkout invoke the proprietary game at its external path (environment variable, documented local path, or existing harness)? No game-source ignore is needed while it stays external; only add narrow ignores for exact generated outputs if a test writes them into this worktree.

## Research method and limits

Four parallel workers performed bounded read-only slices: SceneDB/reflection/generic buffer seam; pass inventory and replacement estimate; upstream Git/code audit; performance/workload feasibility. Findings were reviewed and reconciled against the checked-out tree and Git refs. Workers were closed after their results were validated. No implementation files were changed; no merge, rebase, build, test, or benchmark was run. The parent worktree's pre-existing editor-test and UI-submodule changes were not touched.
