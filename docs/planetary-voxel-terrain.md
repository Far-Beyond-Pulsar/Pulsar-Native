# Planetary voxel terrain goal and architecture

Status: research-backed implementation plan, 2026-07-13

## Goal

Build a new production terrain path for a real-scale spherical planet with a true volumetric source representation, approximately 10 cm maximum local resolution, arbitrary digging/building/material edits, persistence, and the semantic ability to remove every part of the planet.

This is a new subsystem. It does not replace Helio's existing `voxel_demo` or `voxel_demo_raymarch`; those remain examples and regression baselines.

The target is not a heightmap, a thin shell presented as a volume, a pre-cut destruction mesh, or a dense grid hidden behind a smaller render distance. Far geometry must be derived from the same volumetric field as near geometry. Rasterized meshes are allowed only as a transient render/collision cache generated from that field.

## Feasibility boundary

For an Earth-radius body (`R = 6,371 km`) and 10 cm cells:

- One surface-cell layer contains about `5.10e16` cells.
- One bit per surface cell would require about `6.38 PB`.
- Four bytes per surface cell would require about `204 PB`.
- A dense full sphere contains about `1.08e24` cells, or `4.33 YB` at four bytes per cell.

Therefore, "10 cm planet" can only mean an exact virtual address space with 10 cm finest resolution. Uniform air, uniform solid regions, untouched procedural regions, and coarse distant regions must remain hierarchical descriptions. Finest pages are materialized only where observation, editing, physics, or authored data requires them.

Destroying the whole planet is still exact: setting the root override to `Air` changes the canonical terrain state without iterating over `1.08e24` cells. It does not imply simulating `1.08e24` independent rigid bodies. Detached-body simulation is a separately budgeted system.

## Architectural decision

Use a Virtual Sparse Brick Tree (VSBT):

1. A VDB-like mutable hierarchy is the canonical spatial index.
2. Uniform nodes encode `Air` or `Solid(material)` without children.
3. Untouched nodes reference a deterministic procedural field instead of materialized voxels.
4. Edited/detail leaves reference fixed-size voxel pages.
5. A bounded CPU page cache owns decompressed working data.
6. A bounded GPU hash/page table maps active page keys into large shared brick buffers.
7. View-driven LOD requests pages; every LOD is sampled from the same volumetric field.
8. GPU compute incrementally extracts terrain meshlets for normal raster rendering.

The hierarchy, edit log, and persisted pages are authoritative. GPU voxel pages and meshes are disposable caches.

### Why this combination

| Candidate | Strength | Rejection or role |
|---|---|---|
| Dense grid | Simple indexing and edits | Physically impossible at planetary scale |
| One global sparse voxel octree | Compresses uniform space and supplies LOD | Pointer traversal and mutation are too expensive for the resident hot path; retained as the conceptual hierarchy only |
| Sparse voxel DAG | Exceptional static repetition compression | Shared subtrees amplify mutation and copy-on-write cost; optional immutable archive compression, never the mutable canonical store |
| OpenVDB/NanoVDB as-is | Proven sparse hierarchy and GPU traversal | OpenVDB is C++/CPU-oriented; NanoVDB has static topology and is read-only for structural edits; use the data-layout lessons, not a drop-in dependency |
| Flat hashed brick map | Fast random access and dynamic GPU updates | Cannot alone represent global LOD, deep uniform solid space, or root-scale edits; use it as the resident GPU cache |
| Cubed-sphere surface shell | Efficient for conventional planet surfaces | Introduces face seams and special central/deep topology, and makes arbitrary full-volume destruction harder |
| VSBT plus resident brick cache | Mutable hierarchy, exact large edits, bounded residency, GPU-friendly pages | Chosen |

## Canonical data model

### Addressing

```text
PlanetId
PlanetPosition { lod0_cell: [i64; 3], subcell_m: [f64; 3] }
PageKey { lod: u8, page_xyz: [i64; 3] }
NodeState = Air | Solid(material) | Procedural(source) | Branch | Page(PageId)
```

Do not use one 64-bit Morton key for the full address. About 27 binary levels are required to span an Earth diameter down to 10 cm, which exceeds 64 bits when three coordinate bits are interleaved at every level. Keep level and signed axes explicit on CPU and serialize them canonically.

`PlanetPosition.lod0_cell` is the signed authoritative 10 cm address.
`subcell_m` is normalized to the half-open interval `[0, 0.1)` on every axis;
negative positions use Euclidean cell decomposition. Invalid or non-finite
remainders are rejected during construction and deserialization. Large
absolute coordinates are never collapsed into one floating-point value before
the active frame origin is subtracted.

GPU page addresses are camera-relative 32-bit values plus a per-frame origin. WGSL has no concrete 64-bit integer type, and absolute planet coordinates must never reach shader arithmetic as one `f32`.

### Page layout

Initial measurement candidate:

- Logical page: `32^3` cells (3.2 m at LOD0).
- Dirty/dispatch unit: `8^3` microbrick.
- Cell word: signed 16-bit density/distance, 8-bit material palette index, 8-bit flags.
- One-voxel halo is generated or fetched for meshing but is not duplicated in persistence.
- Uniform channels use a constant representation and allocate no dense array.

The 32-bit cell word is deliberately aligned for Rust, storage buffers, atomics/copies, and WGSL. Smaller encodings may be added only after profiling demonstrates that bandwidth dominates and decode cost is acceptable.

### Procedural base plus edits

The base planet is a deterministic fixed-point function returning density and material. It may call authored data sources, but identical inputs must return identical values independent of view LOD.

Every terrain mutation is an ordered operation:

```text
EditOp {
    sequence,
    shape,
    mode: Union | Subtract | Replace | Paint,
    material,
    planet_space_bounds,
}
```

Small edits materialize affected LOD0 pages. Large edits attach high in the hierarchy and are evaluated lazily by descendants. Background compaction folds operation tails into versioned pages and collapses newly uniform subtrees. The edit kernel uses deterministic integer/fixed-point math so persistence, multiplayer, CPU generation, and GPU previews cannot silently disagree.

## Coordinates and large-world behavior

Pulsar owns canonical planet-space coordinates. Helio and Rapier consume a camera/simulation-local frame:

```text
canonical planet position (i64/f64)
        -> subtract active frame origin
        -> local f32 transform for rendering/physics
```

The active origin snaps to a stable LOD0 page boundary. Rebasing changes derived transforms and cache keys, not canonical object or terrain positions.

This avoids an engine-wide conversion of existing scene components, Helio camera buffers, mesh transforms, and Rapier APIs from `f32`. Existing objects remain valid. New large-world objects opt into a `LargeWorldTransform`/planet-frame component, while legacy transforms remain local-world components.

## Streaming and LOD

- Maintain a 2:1-balanced active hierarchy: adjacent rendered leaves differ by at most one LOD.
- LOD0 starts at 10 cm inside a measured interaction radius (initial target: 64 m), then cell size doubles by level.
- Requests are driven by projected error, camera motion prediction, edit bounds, physics bubbles, and shadow visibility—not distance alone.
- Page generation, disk/network reads, decompression, meshing, and collision cooking use separate queues with deadlines and cancellation.
- A request scheduler reserves capacity for edits and collision so fast camera movement cannot starve gameplay.
- Pages transition through explicit states: `Absent -> Requested -> CPUReady -> GPUResident -> Meshed`, each carrying a generation number to reject stale jobs.
- GPU data uses a few large suballocated buffers. Never allocate one wgpu buffer per terrain page.
- Coarse pages use conservative filtering so thin solid features do not disappear or flip topology unpredictably.

Successive complete demand plans use a bounded two-set handoff. The currently
committed visible/prefetch set remains resident while its replacement is
materialized. The handoff commits, and obsolete pages become evictable, only
after every replacement page is CPU-ready. The union of both sets has its own
explicit transition-page budget; a plan that cannot fit is rejected without
mutating the committed set. A newer camera plan cancels uncommitted obsolete
work, and page-generation retirement prevents a late result from republishing
an evicted page. Dense CPU pages may then be dropped while their compacted
content hash and hierarchy record remain authoritative; rehydration must
reproduce that hash from the deterministic generator and ordered edit prefix.

The planet seen from orbit is coarse voxel geometry from the same field, not a heightmap proxy. Atmosphere and oceans may be separate render systems, but they do not replace terrain geometry.

## Rendering decision

The production path is GPU-extracted meshlets rendered through Helio's normal depth, G-buffer, shadow, lighting, Hi-Z, and temporal passes. Per-pixel voxel ray marching remains useful for validation and picking experiments, but is not the primary planetary renderer because surface-dominant scenes pay traversal cost at every shaded pixel.

Helio's existing meshlet/virtual-geometry implementation is a prerequisite to audit, not a trusted dependency. The current implementation uses compute culling plus classic indexed indirect draws, duplicates all LOD meshlets per instance, labels rather than measures LOD error, selects LOD from cluster-local projected radius, and performs position-only welding that can destroy vertex-attribute seams. The planetary path may share it only after Helio's dedicated meshlet correctness, bounded-rebuild, caller-compatibility, and performance gates pass. Otherwise the planet pass will use a terrain-specialized publisher behind the same descriptor/draw contract while the generic path is repaired independently.

Two extraction algorithms must be measured on the same page format:

1. GPU Transvoxel: proven crack stitching and predictable lookup-table work.
2. Feature-preserving manifold dual contouring: fewer vertices and better sharp-feature retention, with a more expensive Hermite/QEF pipeline.

The production choice is a benchmark gate, not a preference. Dual contouring is promoted if it remains within 25% of Transvoxel's dirty-page generation time while materially reducing mesh size or geometric error and passing manifold/crack tests. Otherwise Transvoxel is promoted. Both use the same authoritative voxels; there will not be two terrain systems.

Mesh generation stages are classify, compact active cells, solve/emit vertices, emit topology/LOD transitions, build meshlets and bounds, and publish an indirect-draw range. Old meshlets remain visible until the replacement generation is complete.

## Destruction and detached matter

- Subtract/add/paint/replace edits operate at 10 cm inside the high-resolution interaction region.
- Dirty propagation touches affected microbricks, neighbor halos, coarse ancestors, render meshlets, collision pages, navigation, and persistence versions.
- A root or high-level uniform override supports exact continental or whole-planet removal without leaf expansion.
- Structural separation is evaluated around edited regions with a sparse connectivity job. Supported terrain remains in the planet tree; bounded detached islands become `VoxelBody` entities.
- Detached bodies have explicit voxel, volume, and active-body budgets. Beyond the budget, the terrain state is still exact, but debris simulation is aggregated or deferred. The engine must report that degradation rather than silently dropping terrain edits.

## Physics

Rapier remains local `f32` physics. Switching the whole engine to `rapier3d-f64` would touch every collider, rigid body, query, and component caller and still would not solve GPU precision.

The terrain subsystem provides:

- Exact voxel-field ray/sweep queries for tools, digging, and surface sampling.
- Incremental triangle colliders cooked from active near-field pages for general rigid bodies.
- A command queue into `PhysicsEngine` for add/replace/remove terrain colliders; the physics thread remains the sole owner of Rapier mutation.
- Planet-frame gravity direction and magnitude supplied to simulation bubbles.
- Generation IDs so an old collider result cannot replace newer terrain.

## Persistence and multiplayer

- Base data: generator/version/content hashes.
- Mutable data: hierarchical overrides, content-addressed compressed pages, and an ordered edit tail.
- Storage: only through `engine_fs::virtual_fs`, preserving Pulsar's local/remote/P2P providers.
- Save snapshots publish a new root atomically; interrupted writes cannot corrupt the previous root.
- Multiplayer is server-authoritative. Edits carry stable IDs and sequence numbers.
- Interest management operates on `PageKey` ranges and simulation bubbles.
- Join-in-progress transfers root metadata, requested page hashes/pages, then the edit tail—not an entire planet save through one RPC.
- Page hashes and deterministic replay are verified in tests across thread counts and supported platforms.

## Ownership in the repositories

### Pulsar

Create `crates/subsystems/pulsar_terrain` as the authoritative runtime crate. It owns planet definitions, coordinates, hierarchy, generation, edit scheduling, persistence, streaming, collision requests, and replication-facing data.

Create `PlanetTerrainComponent` in that runtime crate. Do not repurpose the current editor-only `TerrainComponent` or `ProceduralTerrainComponent` during the prototype. Their runtime methods are empty today, and retaining them avoids a scene-format migration while the new contract is validated.

`pulsar_rendering` consumes immutable render deltas/page uploads from `pulsar_terrain`; it does not own terrain state. `engine_backend` registers the terrain subsystem through the existing subsystem/plugin injection path. The level editor provides inspectors and debug views for the runtime component.

### Helio

Create new crates:

- `helio-planet-voxel-core`: GPU POD layouts, page/upload protocol, limits, and validation helpers.
- `helio-pass-planetary-voxel`: resident page pool, compute extraction, meshlet culling/draw, G-buffer/shadow integration, and profiling.
- A new `planet_voxel_demo` example for isolated validation.

The pass owns independent buffers. It must not reuse or resize `GpuScene::voxel_brick_pool`, `voxel_data_pool`, `VoxelTerrain`, `VoxelMeshPass`, or `VoxelRayMarchPass`.

Helio's locked default graph needs an additive graph-extension/build hook at the geometry stage. With no extension supplied, the pass list and behavior must be byte-for-byte/API-equivalent to today. Pulsar opts into the planetary pass explicitly.

## Compatibility audit before engine changes

| Proposed change | Known users | Compatibility requirement |
|---|---|---|
| New Helio planet crates/pass | No existing callers | Additive; current voxel crates and demos unchanged |
| Helio meshlet repair/replacement | `libhelio`, virtual-mesh upload/rebuild, `helio-pass-virtual-geometry`, default graphs, VG examples/debug views, voxel mesh pass terminology | Preserve public callers and default graph behavior; add real geometry/GPU tests; no mandatory mesh-shader feature |
| Default-graph geometry extension hook | All `Renderer` constructors and examples through `helio-default-graphs` | Disabled by default; existing pass order snapshot and examples must remain unchanged |
| Pulsar Helio dependency update | `engine_backend`, `pulsar_game`, `pulsar_rendering`, reflection primitives, asset thumbnails | First isolate current-Helio API adaptation; run targeted checks before terrain work |
| Planet-local render frame | Pulsar scene sync, editor camera, picking/gizmos | Opt-in path; legacy f32 scene objects keep current semantics |
| Terrain subsystem registration | `EngineBackend` subsystem/plugin injection and generated games | Explicit dependency ID; clean init/shutdown; no editor dependency in shipped runtime |
| Terrain collider command queue | `PhysicsEngine`, `PhysicsQueryService`, physics components | Additive API; physics thread remains sole mutator; existing queries unchanged |
| Persistent page store | `engine_fs` local/remote/P2P providers | No direct `std::fs`; atomic root and versioned format tests |

Pulsar currently pins Helio commit `b88e366d`, while the retained voxel baselines are at Helio `3210590`. The pin cannot simply be changed: the current Pulsar call to `Renderer::new_with_external_device` uses an older constructor surface. Compatibility milestone 0 must adapt and verify the two direct constructor paths (`engine_backend` and `pulsar_game`) before the planet pass is introduced.

## Provisional performance and quality gates

Baseline target: desktop, 1440p, 60 Hz, 6-core CPU, 16 GB system RAM, 8 GB VRAM, commodity NVMe, and a WebGPU-class discrete GPU. These are goals to measure, not current claims.

| Metric | Promotion gate |
|---|---|
| Steady terrain GPU time | <= 4.0 ms p95 at the reference traversal scenes |
| Main/render-thread terrain CPU | <= 0.5 ms p95; no synchronous generation or save I/O |
| Terrain-caused frame spike | No p99 frame above 25 ms in the fly/teleport/edit traces |
| GPU terrain memory | <= 2 GB configurable budget, including pages, meshes, tables, and staging |
| CPU terrain memory | <= 3 GB configurable budget |
| Small edit (<= 1 m radius) | Replacement mesh visible within 2 frames p95 |
| Medium edit (<= 10 m radius) | First visible response <= 100 ms, completed near field <= 500 ms p95 |
| Root-scale delete | Canonical state update <= 10 ms and visible invalidation within 2 frames; refinement remains asynchronous |
| Ground-to-orbit precision | <= 1 mm local jitter/error inside the interaction bubble |
| LOD quality | No holes/T-junction cracks in adversarial 2:1 boundaries; manifold gate for dual contouring |
| Persistence | Save/load/replay produces identical root/page hashes |
| Regression | Both existing Helio voxel demos still compile and run their automated smoke path |

Reference traces must include ground walking, supersonic surface flight, ground-to-orbit travel, teleport, cave traversal, repeated drilling, 10 m explosion, kilometer-scale subtraction, whole-root deletion, and save/reload during background work.

## Execution plan

### Milestone 0: compatibility baseline

- Record pass-order, compile, and smoke baselines for Helio's two existing voxel demos.
- Adapt Pulsar to current Helio behind a focused compatibility change.
- Audit both Pulsar renderer constructor paths and component/runtime registrations.
- Gate: both repositories cleanly build targeted crates; no planet code yet.

### Milestone 1: sparse terrain core

- Add `pulsar_terrain` with page keys, node states, deterministic generator interface, edit log, compaction, page codec, and `engine_fs` store.
- Add property/fuzz tests for hierarchy collapse, edit ordering, negative coordinates, LOD reduction, interrupted saves, and deterministic hashes.
- Add dense-vs-sparse and edit-amplification benchmarks.
- Gate: billion-cell logical test regions remain bounded by touched surface/pages; no O(world volume) operation.

### Meshlet prerequisite gate

- Audit all Helio meshlet construction, LOD, buffer rebuild, compute culling, indirect draw, shader, graph, debug, and example callers.
- Validate against meshoptimizer, Microsoft DirectX meshlet samples, NVIDIA task-culling guidance, and AMD cross-vendor sizing/profiling guidance.
- Fix attribute-safe indexing, real simplification error/ranges, object/region-consistent LOD coverage, exact cone tests, immutable descriptor instancing, bounded generation-tagged dynamic ranges, and overflow handling.
- Benchmark static assets and generated voxel surfaces against conventional indexed rendering.
- Gate: zero correctness false-rejects, no per-instance duplication of immutable descriptors, bounded dynamic rebuilds, measured win or justified parity, and no caller/default-graph regressions before shared use by planetary terrain.

### Milestone 2: Helio resident page cache

- Add the new core/pass crates and a standalone planet demo.
- Implement bounded shared buffers, page generations, uploads/evictions, neighbor halos, GPU profiling, and validation readbacks.
- Gate: no per-page GPU allocations, stale-job rejection passes, memory never exceeds configured budget.

### Milestone 3: extraction bake-off

- Implement Transvoxel and manifold dual-contouring prototypes over the same pages.
- Test cracks, topology, feature error, triangles, generation latency, and total frame time.
- Select one production extractor by the stated gate; remove the losing production path after retaining benchmark evidence.

### Milestone 4: real-radius coordinates and streaming

- Add planet-frame positions and camera-relative rendering.
- Implement 2:1 LOD requests from 10 cm near field through orbital scale.
- Add predictive scheduling, cancellation, and ground/orbit/teleport traces.
- Gate: precision, no cracks, bounded residency, and frame-time targets pass.

### Milestone 5: destruction and persistence

- Apply deterministic edits, dirty propagation, coarse rebuilds, background compaction, and atomic saves.
- Validate complete root removal and restore from a prior snapshot.
- Gate: edit latency, exact hashes, and crash-recovery tests pass.

### Milestone 6: Pulsar component and editor integration

- Register `PlanetTerrainComponent` and terrain subsystem.
- Bridge render deltas into the new Helio pass.
- Add editor diagnostics for page state, LOD, queue latency, memory, and edit bounds.
- Gate: standalone generated game does not link editor crates; legacy terrain components/scenes remain loadable.

### Milestone 7: physics and detached bodies

- Add voxel queries, local collider cooking, physics command queue, radial gravity, connectivity analysis, and bounded `VoxelBody` extraction.
- Gate: no stale colliders, no physics-thread ownership violation, and edit/collision convergence tests pass.

### Milestone 8: replication and production hardening

- Add authoritative edit sequencing, interest-based pages, snapshots, join-in-progress, corruption detection, budgets, and telemetry.
- Run all reference traces on minimum and target hardware.
- Promote only if every performance, quality, persistence, compatibility, and regression gate passes.

## Stop and redesign conditions

Do not continue adding features if any of these persist after focused optimization:

- 10 cm interaction pages cannot meet the edit-latency budget without unbounded mesh backlog.
- Coarse LOD cannot conserve topology/material well enough to avoid visible holes or destructive edit disagreement.
- Resident working-set growth is proportional to planet size rather than active views/edits.
- Persistence or multiplayer requires replaying the entire historical edit log for an active page.
- The new pass requires changing default Helio behavior or broadening mandatory GPU features for all users.
- Large-world support requires silently changing existing Pulsar transform semantics.

## Research basis

- [GigaVoxels: Ray-Guided Streaming for Efficient and Detailed Voxel Rendering](https://doi.org/10.1145/1507149.1507152): view/visibility-driven production and streaming of datasets beyond GPU memory, demonstrated at several billion voxels.
- [Efficient Sparse Voxel Octrees](https://research.nvidia.com/publication/2010-02_efficient-sparse-voxel-octrees) and the [extended technical report](https://research.nvidia.com/sites/default/files/pubs/2010-02_Efficient-Sparse-Voxel/laine2010tr1_paper.pdf): compact hierarchical traversal, contour data, filtering, and explicit storage-resolution tradeoffs.
- [High Resolution Sparse Voxel DAGs](https://icg.gwu.edu/sites/g/files/zaxdzs6126/files/downloads/highResolutionSparseVoxelDAGs.pdf): 19 billion static voxels in 945 MB, showing the value and mutation cost risk of shared-subtree compression.
- [VDB: High-Resolution Sparse Volumes with Dynamic Topology](https://museth.org/Ken/Publications_files/Museth_TOG13.pdf) and [OpenVDB overview](https://www.openvdb.org/documentation/doxygen/overview.html): effectively unbounded integer index space, dynamic sparse topology, fast access, and CSG-oriented hierarchy.
- [NanoVDB documentation](https://www.openvdb.org/documentation/doxygen/NanoVDB_8h.html): compact GPU-friendly VDB traversal, but explicitly read-only/static-topology for structural changes.
- [Real-time 3D Reconstruction at Scale using Voxel Hashing](https://www.graphics.stanford.edu/~niessner/niessner2013hashing.html): dynamic GPU-friendly hashed blocks, streaming, and storage only where surface data is observed.
- [Dual Contouring of Hermite Data](https://people.engr.tamu.edu/schaefer/research/dualcontour.pdf) and [Manifold Dual Contouring](https://doi.org/10.1109/TVCG.2007.1012): adaptive, crack-free, feature-preserving surface extraction and topology-preserving simplification.
- [The Transvoxel Algorithm](https://transvoxel.org/) and [Voxel-Based Terrain for Real-Time Virtual Simulations](https://transvoxel.org/Lengyel-VoxelTerrain.pdf): practical transition cells for seamless multiresolution volumetric terrain.
- [Voxel Tools for Godot](https://github.com/Zylann/godot_voxel), its [performance notes](https://voxel-tools.readthedocs.io/en/latest/performance/), and [streaming/persistence design](https://voxel-tools.readthedocs.io/en/latest/streams/): open-source evidence for chunk paging, asynchronous generation, Transvoxel LOD, modified-block persistence, collision updates, and the cost of many small GPU resources.
- [Voxel Plugin runtime edits](https://docs.voxelplugin.com/knowledgebase/blueprints/runtime-edits-and-sculpting): production-oriented evidence that large edits, sculpt accumulation, asynchronous work, persistence, physics, and replication must be designed as separate costs.
- [Unreal Engine large-world rendering](https://dev.epicgames.com/documentation/unreal-engine/large-world-coordinates-rendering-in-unreal-engine-5) and the [WGSL specification](https://www.w3.org/TR/WGSL/): use canonical high precision plus translated camera-local GPU space instead of absolute planet coordinates in shaders.
- [Atomontage Virtual Matter](https://www.atomontage.com/) and [Voxel Farm](https://voxelfarm.com/gaming.html): commercial evidence that progressively streamed microvoxels and mutable massive worlds are viable product directions, but their implementation claims are proprietary and are not treated as reproducible design evidence.
- [meshoptimizer clusterization](https://github.com/zeux/meshoptimizer#clusterization): production meshlet construction, bounds, cone-culling formulas, spatial splitting, compression, and cross-vendor cluster-size caveats.
- [Microsoft Direct3D 12 mesh shader samples](https://learn.microsoft.com/en-us/samples/microsoft/directx-graphics-samples/d3d12-mesh-shader-samples-win32/): reference conversion, culling, instancing, and object-level dynamic LOD organization.
- [NVIDIA mesh-shader introduction](https://developer.nvidia.com/blog/introduction-turing-mesh-shaders/) and [AMD optimization guidance](https://gpuopen.com/learn/mesh_shaders/mesh_shaders-optimization_and_best_practices/): primary vendor evidence for cluster-level early rejection, occupancy, and hardware-sensitive meshlet sizing.
