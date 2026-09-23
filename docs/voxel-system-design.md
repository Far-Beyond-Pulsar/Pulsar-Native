# Unified Voxel System — Design Summary

**Status:** living design note; architecture and API proposal, not an implementation contract yet.

**Purpose:** define one SceneDB-owned voxel system that covers editable cube/voxel objects, ordinary sculpted terrain, unbounded procedural flat worlds, distant horizons, and destructible rounded planets. The system should scale by SceneDB entry count and by available memory/work budgets rather than an arbitrary fixed count of terrain objects.

## Non-negotiable ownership rules

1. **SceneDB owns canonical live component state, not persistence.** Authored component properties, generator descriptions, material-ID lists, user edits, and generated/external chunk data that currently defines the terrain live as SceneDB component state. Helio does not promise durable storage, save/load, undo, replication, or process-restart recovery. Component APIs may expose snapshots/batches for explicit exfiltration/import/export; user scripts/tools own persistence policy, settings, and storage.
2. **Rendering owns only transient or reproducible derived state.** GPU-resident bricks, extracted surfaces, acceleration trees, page tables, upload staging, work queues, visibility lists, and frame-local coordinates may live in the renderer. They are caches and can be discarded and rebuilt from SceneDB state plus deterministic source descriptions.
3. **The generic rendering engine does not know what terrain or voxels are.** `helio-core`, the generic renderer, generic SceneDB projection/synchronization, and generic graph scheduler must not contain voxel types, terrain branching, voxel buffers, or terrain-specific lifecycle rules.
4. **Voxel-specific rendering knowledge lives in the voxel pass crate.** That crate interprets voxel/terrain GPU projections, manages transient GPU voxel resources, performs voxel traversal/surface work, and emits the ordinary renderer outputs expected by the rest of Helio.
5. **SceneDB entries scale to N.** No application-level `max_planets = 4`, fixed global component count, or fixed per-scene page ceiling. The renderer may impose a device-derived transient-memory/work budget and evict/rebuild cache entries, but those limits do not cap authored SceneDB entries.
6. **Frame work is bounded and nonblocking.** Script generation, chunk ingestion, snapshot/export work, planning, and heavy voxel processing must not synchronously stall the render thread. Updates are revisioned, batched, prioritized, coalesced where safe, and published when complete. Any persistence I/O is user-owned and must be scheduled by the user’s script/tool outside the render-thread critical path.

Render work consumes a consistent SceneDB projection/snapshot and keeps no unique authoritative copy of the world. “Canonical” here means current in-memory scene state; it does not imply persistence or durability.

## Product model: two authorable components

The system exposes two concepts backed by the same voxel encoding, page/chunk addressing, material lookup, editing contract, and render pass.

The reflected `VoxelComponent` and `VoxelTerrainComponent` live alongside the other reflected SceneDB components in Helio's existing `helio-component` crate. Their schemas and authored SceneDB properties are the allowed component-facing declaration of the feature; implementation-only voxel types, algorithms, storage interpretation, and render behavior belong to the dedicated voxel pass crate. Generic core/renderer/graph crates must not acquire voxel semantics. The unified pass may absorb voxelized mesh rendering, but it does not replace ordinary static/conventional mesh rendering.

The existing voxelized mesh path (surface extraction and meshlet rasterization) is in scope for unification. Raymarch is initially a candidate optional rendering backend *inside* the unified pass, not a separate pass or state/API. Its current fullscreen DDA path is behaviorally distinct, so preserve it only until shared-data/material/depth integration and matched profiling show whether it provides a worthwhile mode; retire the backend if not. This is a measurement gate, not a claim that raymarch is faster.

### `VoxelComponent`

A deliberately small, explicit voxel object. It starts as a cube, can be deformed, and selects from the existing Helio material system.

Expected author-facing properties:

| Property | Proposed type | Meaning |
|---|---|---|
| `enabled` | `bool` | Whether this entry participates in rendering and voxel queries. |
| `voxel_size_m` | `f32` | Edge length of one LOD0 voxel. |
| `dimensions` | `VoxelDimensions` (three `u32`s) | Initial cube/grid dimensions; default is a cube. |
| `origin` | `VoxelOrigin` (three `i64` cell coordinates) | Integer-aligned placement/address anchor. Scene transform remains authoritative for object placement where appropriate. |
| `surface_mode` | `VoxelSurfaceMode` | `Blocky` or `Smooth`, if supported by the selected extraction path. |
| `material_ids` | `Vec<u32>` | Palette entries. Each ID indexes a material already present in Helio’s existing SceneDB-backed material data/buffer consumed by the material pass. |
| `initial_material` | `u32` | Palette index/material ID used to initialize the cube. |
| `editable` | `bool` | Whether external tools/scripts may deform this voxel object. |

The cube’s occupancy and subsequent edits are canonical live SceneDB component state. The render pass may cache the resulting pages/bricks, but the cache is not the authoritative source. A script may export/import that state and decide whether/how to save it.

### `VoxelTerrainComponent`

A general terrain source using the same voxel material and edit model, plus procedural generation, bounds/domain, LOD, streaming, and external source configuration. Planet shape is one option; it is not a requirement of the component.

Expected author-facing property groups:

| Group | Proposed properties and types | Purpose |
|---|---|---|
| Identity | `enabled: bool`, `terrain_id: String`, `source_revision: u64` | Stable SceneDB-entry identity and stale-result rejection. Empty IDs may derive from stable entity/component identity. |
| Domain | `domain: TerrainDomain`, `origin: VoxelOrigin`, `voxel_size_m: f64` | `Infinite`, bounded box, spherical domain, or a future custom bound; canonical integer voxel coordinates plus cell scale. |
| Shape | `shape: TerrainShape` | Plane, sphere/planet, box, or registered custom field. A sphere changes the source function, not the storage/render protocol. |
| Generator | `generator: TerrainGeneratorDescriptor`, `seed: u64` | Stable generator kind/version, parameters, and deterministic seed. Never serialize a closure or raw function pointer. |
| Modifiers | `modifiers: Vec<TerrainModifier>` | Ordered, typed layers such as noise, strata, caves, stamps, imported fields, and registered custom modifiers. Each descriptor carries stable type/version and serializable parameters. |
| Materials | `material_ids: Vec<u32>` | Palette/list of IDs into the existing Helio SceneDB-backed material records. Voxel samples store a compact palette index or material ID according to the finalized GPU encoding. |
| Surface | `surface_mode: VoxelSurfaceMode`, `normal_mode: VoxelNormalMode` | Select blocky or smooth surface presentation and supported normal/material interpolation. These are independent of the generator’s shape. |
| LOD | `lod_policy: VoxelLodPolicy`, `target_error_px: f32`, `detail_distance_m: f32` | Screen-space or authored detail policy. Detail is view-driven; it does not imply a finite terrain boundary. |
| Streaming | `prefetch_distance_m: f32`, `priority: i32` | Predictive work and scheduling priority. These affect transient work allocation, not authored-world existence. |
| Editing | `editable: bool`, `edit_policy: VoxelEditPolicy` | Whether sculpting is allowed and how edits combine with procedural source data. |

The exact property breakdown can be tuned for the property inspector. The important contract is that authorable values are reflected/serialized SceneDB properties, not hidden renderer settings.

## Reflection and SceneDB component layout

Use the existing `#[engine_class(..., scene_store)]` / SceneDB derive pattern to keep authoring values and GPU projection fields on the same component type when practical:

- Fields with `#[property]` are reflected and shown in the editor.
- CPU-only runtime fields without `#[property]` stay out of the inspector.
- Fixed-layout GPU projection fields use `#[gpu(...)]`, with the appropriate mirror/update policy. They are not editor properties unless separately marked `#[property]`.
- Variable-length GPU data uses the SceneDB variable-length buffer mechanism where it is a real rendering input. Canonical live edits/chunks remain SceneDB component data; the GPU projection is a derived mirror/cache.
- `String`, generator descriptors, enums, edit history, export metadata, and source-provider identifiers stay CPU-side unless a compact numeric projection is needed by shaders.
- Any GPU row is plain fixed-layout data (`Pod`-compatible fields and padding as required). It contains indices/ranges/flags, not Rust-owned handles or renderer lifetimes.

Conceptual single-struct shape (illustrative, not compile-ready):

```rust
#[engine_class(category = "Voxel", clone, debug, serialize, deserialize, scene_store)]
pub struct VoxelTerrainComponent {
    // Authored SceneDB properties
    #[property]
    pub enabled: bool,
    #[property]
    pub terrain_id: String,
    #[property(category = "Domain")]
    pub domain: TerrainDomain,
    #[property(category = "Domain")]
    pub shape: TerrainShape,
    #[property(category = "Generation")]
    pub generator: TerrainGeneratorDescriptor,
    #[property(category = "Generation")]
    pub modifiers: Vec<TerrainModifier>,
    #[property(category = "Materials")]
    pub material_ids: Vec<u32>,
    #[property(category = "Surface")]
    pub surface_mode: VoxelSurfaceMode,

    // Internal SceneDB/GPU projection; not editor properties
    #[gpu(buffer = "voxel_terrain_rows", mirror = DirtyTracked)]
    #[serde(skip)]
    gpu_source_flags: [u32; 4],
    #[gpu(buffer = "voxel_terrain_rows", mirror = DirtyTracked)]
    #[serde(skip)]
    gpu_lod_and_material_range: [u32; 4],
}
```

The actual GPU projection layout must be settled with the voxel shader bindings and SceneDB `SceneStore` requirements. Do not mirror every property just because it can be packed; mirror only fields read by GPU code. Large `Vec` data is not automatically copied into a per-entry GPU row.

## Materials

- Do not create an independent voxel material database or duplicate material definitions in the voxel pass.
- A component’s `material_ids` are IDs into the existing Helio material records in the SceneDB-backed buffer read by the materials pass.
- Voxel occupancy/material sampling yields one of those IDs (or a palette slot that resolves to one of those IDs). The voxel pass writes the material reference in the GBuffer/material channel contract that the existing material pass expects.
- Per-voxel encoding may be compressed, but it must not silently cap the system at the upstream prototype’s two-bit/four-material format. The representation can be palette-indexed per terrain entry, with palette entries resolving to SceneDB material IDs.
- SceneDB material-record lifetime and material updates remain owned by the existing material system. Voxel pages must tolerate a material ID becoming stale or unavailable with a defined fallback/validation path.

## External generator and chunk APIs

Scripts and other subsystems interact through SceneDB entries and a terrain/voxel service, not by mutating renderer internals.

### Create and configure

1. Create/insert a `VoxelComponent` or `VoxelTerrainComponent` on a SceneDB entity in an otherwise empty level.
2. Set its domain, shape, source descriptor, seed/modifiers, material-ID palette, surface mode, and edit options through ordinary SceneDB component writes.
3. For a registered procedural source, resolve `generator.type_id + version` through a source registry. The component stores stable source identity and serializable parameters as live scene state; it does not imply that Helio persists them.

### Publish external data in batches

The desired scripting shape is conceptually:

```rust
let entry = scene_db.spawn_with(VoxelTerrainComponent { /* configuration */ });
let writer = voxel_service.source_writer(entry)?;
writer.publish_batch(revision, chunk_updates)?;
```

Proposed public concepts (names provisional; voxel semantics live in the dedicated voxel pass/service boundary):

- `VoxelSourceId` / `TerrainEntryId`: stable identity derived from the SceneDB entry plus component identity.
- `VoxelSourceDescriptor`: registered source kind, version, parameters, seed, and declared domain.
- `VoxelChunkKey`: signed integer chunk/page coordinate plus LOD and owning entry identity.
- `VoxelChunkUpdate`: key, source revision, encoding, and a CPU-side immutable payload or deterministic-generation request.
- `VoxelChunkBatch`: one atomic/revisioned set of updates and invalidations.
- `VoxelSourceWriter::publish_batch(...)`: validates and publishes canonical changes to SceneDB component state in one bounded batch, then signals derived work.
- `VoxelSourceReader::read_batch(...)` / snapshot API: obtains a consistent view of canonical component/chunk state for scripts, tools, and derived processing.
- `VoxelSourceSnapshot::export_data(...)` / corresponding import API: exposes a versioned representation users can serialize, transmit, or otherwise store themselves; no engine-owned file format, location, or persistence setting is implied.
- `VoxelTerrainQuery`: bounded sample/read request for gameplay, collision, editing, or diagnostics. Large queries should support asynchronous results; render passes must not read back the entire field.
- `VoxelUpdateReceipt`: accepted revision, coalesced/superseded update counts, and asynchronous completion/diagnostic state.

Chunk size and encoding should be canonical and shared by generation, editing, user-script export/import, and rendering. Arbitrary producer chunk sizes can be accepted only if a bounded adapter splits/repackages them without blocking the frame thread.

### Update and publication semantics

- SceneDB component publication is the canonical live-state change. It updates component/source revision and canonical changed chunk/edit data in one batch; it is not a disk commit or durability guarantee.
- A batch may supersede older pending work for the same entry/chunk. Work/results carry `(entry identity, source revision, chunk generation)` so stale results are rejected.
- Render work builds replacement pages/bricks off-thread or on GPU. Keep the last complete resident cut visible until the replacement is ready; do not interpret “not generated yet” as known air.
- Publish completed derived pages atomically at a generation boundary. After publication, reclaim obsolete transient pages when no current tree references them.
- If source code is external, it must provide deterministic results for a stable version/seed or explicitly publish the produced chunk data into SceneDB when the data itself is the canonical live state.
- User-authored persistence, undo/redo, collaboration, and replication tools may consume/export SceneDB revisions and batches. The render pass does not maintain a second edit journal, and Helio does not implement these durability/workflow features as part of this design.

## Infinite, bounded, and planetary domains

One address and page protocol should support all domains:

- **Infinite flat/procedural:** unbounded signed chunk coordinates; only demanded regions are generated/resident. “Infinite” means no authored edge, not infinite allocation.
- **Bounded flat/sculpted:** explicit bounds permit outside-region emptiness and more pruning.
- **Planet/sphere:** a spherical base field and planet-relative integer coordinates; same page protocol and edit path as other shapes.
- **Custom field:** registered deterministic source or externally supplied chunks, subject to coordinate/domain and revision contracts.

The current Pulsar terrain hierarchy’s finite `root_lod` and the upstream tiny-voxel prototype’s `i32` coordinate limits are not sufficient as-is for the full unbounded/high-precision goal. The target uses signed wide integer canonical coordinates, page-local/camera-relative GPU coordinates, and checked arithmetic. GPU floats must never represent huge absolute world coordinates.

An unbounded world and N SceneDB entries remain logically unlimited by authored count. Physical caches stay finite and device-derived: priority-aware eviction, regeneration, coalescing, work budgets, and per-entry fairness are required.

## Rendering integration boundary

```text
SceneDB entries and buffers (authoritative state)
        │ generic component/GPU-mirror projection
        ▼
Generic Helio renderer and graph scheduler (terrain-agnostic)
        │ generic render-pass scheduling / resources
        ▼
Voxel terrain render pass (only renderer crate with voxel semantics)
        │ transient GPU bricks, page tables, trees, upload staging
        ▼
Existing GBuffer + existing SceneDB-backed material-pass contract
```

The generic renderer must not contain a `VoxelTerrainComponent` type, `VoxelWorld`, terrain registry, terrain config, voxel page table, or special terrain synchronization branch. Integration should use generic SceneDB GPU buffer handles and the generic pass/plugin/graph-extension mechanism. If today’s default graph builder must instantiate a terrain-specific Rust type directly, that is an architectural leak to remove or contain in a pass-specific extension crate rather than spreading into Helio core.

Only transient/rebuildable rendering data belongs in the render-pass crate: selected trees, resident brick slots, GPU density/material brick data, in-flight generation jobs, visibility, current camera-local origin, and frame counters. No unique canonical edits, generator configuration, chunk ownership, or user persistence journal lives there.

## Upstream implementation to mine

The upstream Helio branch inspected for this design is `feat/tiny-voxel-stress-test`, commit `a0bf4875` (based on Helio `main` commit `02247938`). It is a prototype source, not a clean merge target.

Potentially reusable ideas:

- View-selected hierarchical brick tree and complete-cut publication.
- Background selection worker and bounded GPU brick-generation batches.
- Compact stored bricks with conservative density bounds for skipping empty regions.
- Reusing unchanged resident bricks while staging replacement bricks.
- Exact near-field voxel behavior with coarser distant representation.
- GPU timestamps/stage profiling and CPU oracle/GPU validation patterns.
- Coalescing asynchronous generation/publication concepts, implemented around SceneDB live component state; persistence is left to user scripts/tools.

Do not inherit its product constraints as requirements:

- One global `World`/one frame source.
- Fixed Earth/default landform recipe.
- `i32` cell-space bounds and fixed edit cap.
- Four two-bit material values.
- Renderer-owned `World`/edit history or a renderer-local authoritative journal.
- A claim of frame-time qualification based only on unit/GPU tests.

The unified `helio-pass-voxel-mesh` handles voxel rendering. Smooth and blocky output are modes of that pass. Its transient residency and generation-tagged publication must remain distinct from canonical SceneDB component state.

The upstream commit also changes DOF, portal instances, sky, shadow matrices, TSR, deferred lighting/material shaders, and virtual geometry. Those are separate graphics changes; review and validate them independently from the voxel backend migration. Five paths overlap changes in the local Helio line and upstream commit: DOF, portal instances, sky LUT, TSR, and virtual geometry.

## Performance contract

The target stress workload is:

```text
cargo run --release --locked --manifest-path runtime/Cargo.toml \
  -p vr-blocks-client -- play --fly --radius 128 --detail-radius 4 \
  --shrubbery-trees --perf @args demo
```

Acceptance target: keep total frame time at or below **10 ms** for the agreed radius-128 workload, with the workload definition and hardware recorded. This has not yet been demonstrated by either implementation.

Required design controls:

- Bounded per-frame upload, page publication, and GPU generation work; carry excess work forward.
- Worker-side generation/planning and batched SceneDB writes; no per-voxel scene lock/write loop.
- Coalesce stale revisions and repeated updates to the same chunk.
- Preserve old valid resident terrain until replacements are ready.
- Fair priority scheduling across N entries, with visibility/distance and gameplay importance inputs.
- Device-derived transient-memory budgets; evict/rebuild generated pages instead of refusing new SceneDB entries.
- Avoid render-thread allocation/copying of dense chunk payloads and avoid GPU readback in normal operation.
- Profile CPU and GPU separately, with warmed p50/p95/p99/max; include steady flight, initial arrival, teleport, dense edits, multiple entries, material changes, and cache pressure.

## Migration outline

1. **Lock the data contract:** component fields, IDs, domain/addressing, edit/chunk encoding, source registry, revision semantics, material-ID resolution, and the division between SceneDB live-state ownership and user-owned persistence.
2. **Establish SceneDB canonical live storage:** reflected `VoxelComponent` and `VoxelTerrainComponent`; variable-length/chunk component data and bounded batch publication/read API; snapshot/import/export API for user scripts; ensure no duplicate authoritative renderer state. Do not add a durable sidecar/blob store or engine persistence settings.
3. **Extract a terrain-agnostic rendering seam:** generic graph/pass extension and generic SceneDB GPU-buffer access, with no voxel types in Helio core or generic renderer crates.
4. **Build the new voxel pass:** mine upstream stored-brick selection/residency/traversal; support multi-entry inputs; integrate the existing SceneDB material IDs; add selectable blocky/smooth behavior.
5. **Connect component projection and source writers:** script-created entries, registered custom generators, batched chunk publication, edit receipts, and stale-work rejection.
6. **Migrate gameplay and tools:** picking, collision/sample APIs, editor sculpting, and scripting use SceneDB-backed services and revisioned batches. Any save/undo workflow is implemented by user-facing scripts/tools consuming the component snapshot/export API, not by renderer-owned state or an engine-managed voxel persistence layer.
7. **Integrate tools and gameplay:** connect editor, picking, and gameplay consumers to unified components; verify required behavior.
8. **Qualify:** build the integrated workspace, run CPU/GPU correctness and graph tests, then run the exact release stress command and iterate against the 10 ms budget.

## Open decisions to resolve

- Exact canonical chunk/page dimension and CPU/GPU encodings (especially smooth density versus block material occupancy).
- Whether `VoxelComponent` uses a fixed cube-local grid, chunked sparse grid, or both; define cube default dimensions and deformation granularity.
- SceneDB in-memory representation/API for large chunk payloads and edits, including bounded transaction size and zero-copy/immutable snapshots. Durable encoding, storage location/settings, and restart recovery are owned by user scripts/tools and are not Helio requirements.
- Custom generator registration ABI/versioning and failure behavior when a provider is unavailable.
- How multiple overlapping voxel/terrain entries compose and how their GBuffer depth/material output is ordered.
- How smooth-mode extraction and LOD transitions will satisfy the required visual and performance targets.
- Exact SceneDB material handle/index type, buffer key, material channel encoding, and behavior for missing material IDs.
- Component transform versus canonical world origin ownership for planetary coordinates and floating-origin updates.
- Whether generated chunks are reproducible from a source descriptor or must be retained as canonical live supplied data; in either case, cross-run persistence is a user decision implemented outside Helio.
- How infinite-domain bounds, page coordinates, edit bounds, and physics coordinates avoid finite-root/i32 limits.
- Which parts of the upstream non-voxel shader changes are intended for the canonical Helio copy.
