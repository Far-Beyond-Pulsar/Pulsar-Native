# Phase 0 closure: inventory, decisions and failure baseline

Status: **Phase 0 deliverables for review** (Pulsar-Native#1035). Baseline: Pulsar-Native `6593c311e`, Helio `05c2f7d7`, SceneDB `e74df984`, Pulsar-Reflection `2dab12bf`.

Phase 0 changes no runtime behavior. It adds an executable ledger, a read-only reproduction, a workspace fix that makes existing tests runnable, and written decisions. Nothing here installs a subscription, refresh, resync or other repair path. Every existing workaround is recorded in the ledger for removal by the structural phases.

Exit criteria from the plan, and where each is met:

| Exit criterion | Where |
|---|---|
| Every exposed component has a disposition | [`ledger.toml`](ledger.toml) `[[class]]` rows, enforced by `cargo test -p scene_inventory` |
| Reproduction demonstrates the broken stage | [Failure baseline](#failure-baseline), `crates/editor/ui_level_editor/tests/phase0_render_baseline.rs` |
| Instance identity and layout ownership are written down | [Decisions](#decisions) D1, D2 |
| Runnable validation targets are known | [Validation targets](#validation-targets) |

## The executable ledger

[`ledger.toml`](ledger.toml) has one row per:

- **class**: every reflection class the editor's "Add component" menu lists (`REGISTRY.get_class_names()`), with linked facts: World registration, own GPU columns, runtime behavior;
- **schema**: every `#[derive(SceneStore)]` type with `#[gpu]` fields linked into the editor's registries, with the buffers it declares, its removal behavior and who writes its rows;
- **buffer**: every SceneDB GPU buffer key, with the types that declare it and the packages that look it up;
- **pass**: every Helio pass crate, whether the default graph composes it, and which buffers it reads;
- **site**: every production call site of a lifecycle API the plan removes or replaces (shared-queue drains, render subscriptions, GPU refresh/mark/arm calls, CPU projection, mirror replay, live JSON hydrate, `RenderProps` sync, behavior dispatch, `PendingWorldWrites`, forced resync, and the add/edit producers).

`crates/core/scene_inventory` links every crate that registers components or schemas, recomputes the fact columns from the linked `inventory` registries and a scan of production source, and fails on a missing row, a stale row, a changed fact, or an empty human column. Each failure prints the TOML row to add. New registrations, buffers, passes and workaround call sites therefore cannot land without an entry that says where they belong in the plan.

Status vocabulary: `broken` (reproduced, named test), `at-risk` (source defect against the plan, not reproduced), `unverified`, `verified` (named test proves it), `out-of-scope` (reason in `disposition`).

Scanner limits: buffer consumers are literal `BufferKey::of("…")` lookups, so a consumer that builds the key from a constant or `T::buffer_key()` shows as none (those rows say "consumer not identified", not "unused"). Plugin components loaded from DLLs at runtime are not linked into the test and are covered by the plugin row in the open items below.

Current ledger: 21 classes, 46 GPU schemas, 31 buffers, 49 pass crates, 60 call sites. Status counts: 11 broken, 121 at-risk, 60 unverified, 15 out-of-scope, 0 verified.

Headline facts the ledger now enforces:

- Production code registers GPU columns for only 13 of the 46 linked schemas (all in `helio_bridge::ensure_gpu_mirror`). SceneDB drops writes to unregistered columns, so every generated `*GpuMirror` companion, plus post-process volumes, fog, portals, foliage, reflection captures and sky, never reaches the GPU.
- Eight render components write through `PendingWorldWrites`, which has no production drain.
- `LODComponent` has no consumer at all.
- `RenderMetrics::draw_calls`/`vertices_drawn` are never written.

## Decisions

These record the Phase 0 resolution of the decisions the plan requires before downstream API migration. D1 and D2 adopt the review packet's proposals, which the Phase 0 evidence supports; the `REVIEW:` questions they leave open are listed so they are answered before Phase 1 freezes an API.

### D1. Component instance model

**Decision (proposed for approval): one SceneDB entity per attached component instance**, as [01-authority-and-identity.md](01-authority-and-identity.md) proposes. The component entity holds the registered component value plus typed attachment metadata: stable `ComponentInstanceId`, owner object, order, enabled state, parent instance (presentation only), class-slot provenance. GPU rows are keyed by the component entity and join the owner's transform/visibility through an explicit owner key.

Phase 0 evidence that the current model cannot be kept:

- `World::insert<T>` overwrites: one value per Rust type per entity (SceneDB `world.rs`). The editor allows several instances of a class per object.
- `scene_edit::components::attach_component_instance` hydrates only the first enabled instance of a class; later instances and disabled ones live as JSON in the attachment record. A second same-class instance is therefore never a typed value.
- When hydration fails, the attachment record keeps the full JSON and the object still lists the component as attached and enabled (`hydrate_canonical_component` logs and returns `false`; the record is written regardless). See the light baseline below.
- Property edits address instances by list index (`update_live_component_property(…, component_index, …)`) and refuse an index that is not the live typed one; script routing (`pulsar_script_object_model::routing`) re-hydrates dormant instances through JSON.

SceneDB capability check at `e74df984`:

| Needed for D1 | Present? |
|---|---|
| Spawn/despawn entities, typed metadata components | yes (`World::spawn`, `insert<T>`) |
| Erased insert/remove of a registered value that runs the normal write hooks | **no**: no erased insert; only typed `insert<T>`/`get_mut<T>` |
| Independent change readers | yes: per-type journals with cursors (`open_change_cursor`, `read_changes`); entries carry `(entity, Inserted/Mutated/Removed)` only, so they are invalidations that prompt a re-read, matching [04](04-state-notifications.md) |
| Generic GPU-mirror replay when attached after population | **no**: `attach_gpu_mirror` does not replay; `ensure_gpu_mirror` re-inserts a fixed list |
| Owner-key GPU join | not provided; Phase 1/2 work |

Open `REVIEW:` questions to answer before Phase 1 exits (recommendations in brackets): cascade on object despawn [cascade in the same transaction]; nesting [presentation-only metadata]; ID format and scope [opaque 128-bit IDs unique per scene]; script reference shape [component-instance ID, with a validated object+type facade that errors on ambiguity]; world/global components [allowed on a world entity, same lifecycle].

### D2. Layout ownership

**Decision (proposed for approval):**

- The **component feature crate** owns the authored struct and its GPU-upload declaration (`#[gpu]` fields through `SceneStore`). It names no pass.
- **SceneDB** owns packing, buffers, generations, dirty ranges and mirror lifecycle, generically for every schema.
- The **render-graph crate** (`helio-default-graphs`) owns the mapping from schema to pass inputs and every pass dependency.
- A **pass crate** owns its algorithm and shader-side declaration of its inputs, checked against the schema's layout hash. It does not own the authored type.

What the ledger shows today, all migration scope:

- Most scene inputs are pass-owned types: `StaticObjectComponent`, `MaterialComponent`, `MeshComponent` (gbuffer), forward-lit's `LightComponent`, `CameraPostProcessComponent`/`PostProcessVolumeComponent`, fog, water, portal, foliage and reflection-capture types. Their rows are written by CPU projection (`helio_bridge`, `editor_rows`, `editor_postprocess`) or by the undrained `PendingWorldWrites` queue, never by the authored component's own write.
- Two buffers have two declaring types: `builtin_mesh_vertex`/`builtin_mesh_index` (authored `StaticMeshComponent` and gbuffer's `MeshComponent`) and `reflection_captures` (authored `ReflectionCaptureComponent` and deferred-light's `ReflectionCaptureGpuComponent`).
- `helio-pass-hlfs` reads the CPU `World` through change cursors.
- Name collisions between authored and pass-owned types (`LightComponent`, `PortalComponent`) are why schema rows are keyed by full path.

### D3. Supported render capabilities

Classification of every exposed class (detail and status per row in the ledger):

| Class | Capability today |
|---|---|
| StaticMeshComponent, MaterialOverrideComponent, LightComponent | Phase 2 vertical slice; reached only through CPU projection |
| CameraPostProcess, GlobalFog, LocalFogVolume, Foliage, Portal, PostProcessVolume, ReflectionCapture, WaterVolume | Phase 4; stranded behind `PendingWorldWrites` and/or unregistered buffers |
| LODComponent | unsupported: no consumer |
| SplineComponent | editor debug lines from a CPU scan |
| Voxel, VoxelTerrain, VoxelLandform, VoxelFlatTerrain | own voxel_frame/voxel_source path; audit in Phase 4 |
| Physics, Rigidbody, ClassInstance, NativeScript | non-render |
| decals, corona, sky, sprites | pass inputs with no authored component |

### D4. Workspace and test ownership

`helio_component` is now an explicit root-workspace member. Its manifest already declared the root workspace (so its inventory registrations share the editor's SceneDB/reflection copies), but the root `exclude` of `crates/renderer/helio` made it a non-member, and Cargo refused `cargo test -p helio_component`. Listing it in `members` overrides the exclusion without changing `Cargo.lock`.

Hosted CI runners have no GPU adapter, and four `helio_component` test binaries panic without one. The new `ci` nextest profile (`.config/nextest.toml`, used by `ci.yml` and `release.yml`) excludes exactly those binaries by name instead of letting them count as passed; they are GPU-machine targets below.

## Validation targets

| Command | Needs GPU | Run in this PR |
|---|---|---|
| `cargo test -p scene_inventory` | no | yes; all 7 ledger checks pass |
| `cargo test -p ui_level_editor --test phase0_render_baseline -- --nocapture` | yes (skips without one) | yes, RTX 3060, Vulkan, Windows; passed |
| `cargo test -p helio_component` | yes for four binaries | built; not run |
| `cargo nextest run --profile ci --all` | no | not run locally (nextest not installed) |
| SceneDB: `cargo test -p pulsar_scenedb --features gpu --lib` / `--test gpu_layout` | no | covered by existing CI steps; not run here |

## Failure baseline

From `phase0_render_baseline` (Windows, Vulkan, RTX 3060). The editor producers are `AddObject` + `add_component`, and the panel's subscribe/drain functions are called directly.

| Case | Typed | GPU pools | Draw + material rows | Depth/color change |
|---|---|---|---|---|
| Mesh present before the first frame | yes | 24 v / 36 i | yes | 0 / 0 |
| Mesh added after the first frame, panel closed (and after an edit) | yes | 24 / 36 | **no** | 0 / 0 |
| Same, panel open, panel drains first (and after an edit) | yes | 24 / 36 | **no** | 0 / 0 |
| Same, panel open, renderer drains | yes | 24 / 36 | no; **yes only after an edit** | 0 / 0 |

Lights, added after the first frame:

- The panel's own Add component payload and the legacy flat-intensity shape **fail to hydrate**, yet the attachment stays enabled with its JSON.
- A `to_json` payload hydrates, but no forward-lit light row is created.

Broken stage demonstrated: the draw row. It depends on render subscriptions and on who drains the shared queue first. The light hydration failure is separate from the mesh failure.

**Inconclusive:** final pixels. Even the mesh with every row present changes no depth or color in this headless harness, at 8 or 40 frames. Whether that is a harness gap (no positive control yet) or a real draw failure is not established. The object-batch draw count could not be read, because `HelioRenderer` exposes no batch statistics.

## Open items carried into Phase 1

- A positive pixel control for the headless harness, plus object-batch draw-count readback, so the final stage can be judged. Not attempted: capturing the real GUI editor.
- Plugin components loaded from DLLs are not linked into the inventory.
- Pass-dependency boundary manifest ledger (audit section 4) is not machine-checked.
- D1/D2 `REVIEW:` answers need maintainer approval.
