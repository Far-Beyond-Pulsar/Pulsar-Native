# Helio scene API migration audit

Status: partial Phase 13 migration. `SceneDbHandle` is now a cloneable
`GpuMirrorHandle` projection, not `Arc<Mutex<SceneDb>>`; Helio never locks or
flushes the authoritative CPU database. Pass-owned components and the legacy
scene surface are still present, so this document records the remaining work
below.

Phase 14 re-ran the inventory against the current checkout and confirmed that
the parent workspace has no direct legacy actor insertion for mesh or light
content outside the bridge. Those entities are projected from SceneDB rows by
`rebuild_static_mesh_frame` and `rebuild_light_frame`. No UI source was changed by this
migration slice; the separate current UI submodule modifications are recorded below.

## Contract used for this audit

The cached SceneDB checkout used by the workspace is
`C:\Users\redst\.cargo\git\checkouts\scenedb-cee79fad79f37adc\c761890`
(the workspace lock resolves the same SceneDB family). Its README and source
API establish the target ownership rule:

- `pulsar_scenedb::World` is the authoritative entity/component scene store.
- Creation and mutation are `World::spawn`/`spawn_bundle`, `insert`/
  `insert_bundle`, `get_mut`, `remove`, and `despawn`.
- GPU mirroring is attached to the World and flushed at the SceneDB phase
  boundary; it is not a renderer-owned object/actor registry.
- SceneDB does not depend on Helio, and the graphics-free core remains a
  separate layer.

The relevant cached APIs read during this audit are `World::spawn`,
`World::spawn_bundle`, `World::insert`, `World::insert_bundle`,
`World::get_mut`, `World::remove`, `World::despawn`, and `SceneDb::step` /
`SceneDb::step_gpu`. The cached `SceneDb` source also documents that the GPU
step is optional for World-only mirrored state.

## Legacy Helio API inventory

The legacy surface is still present in the Helio submodule. The definitions
are in `crates/renderer/helio/crates/helio/src/scene/`:

| API | Definition | Migration meaning |
|---|---|---|
| `SceneActor` and constructors (`mesh`, `object`, `light`, `decal`, `sky`, `water_*`, `post_process_volume`, etc.) | `scene/actor.rs:977-1054` | Actor wrappers combine identity, resource insertion, and renderer ownership. SceneDB entities/components must become canonical. |
| `Scene::insert_actor` | `scene/lifecycle.rs:48-56` | Calls `on_attach`, stores boxed actors in `custom_actors`, and returns `SceneActorId`; this is the main legacy insertion path. |
| `Scene::insert_*` / `remove_*` / `update_*` resource APIs | `scene/resources/`, `scene/objects/`, `scene/water.rs`, `scene/portals.rs`, `scene/foliage.rs`, `scene/postprocess.rs`, `scene/sublevels.rs`, `scene/virtual_geometry/` | These remain renderer-owned compatibility APIs until the SceneDB-to-Helio bridge is completed. |
| `Renderer::scene_mut` | `crates/helio/src/renderer/renderer_impl.rs:472` in the Helio submodule | High-level seam; intentionally not changed in this audit. |

### Parent-workspace callers

These are the exact non-UI callers in the parent workspace that still cross
the legacy scene seam (line numbers are from the Phase 14 audit):

- `crates/core/engine_backend/src/scene/helio_bridge.rs:158` — binds the
  SceneDB-owned static-mesh pools to Helio's draw path.
- `crates/core/engine_backend/src/scene/helio_bridge.rs:234,313` — replaces
  Helio static-mesh instances from pass-owned `StaticMeshRenderInput` records.
- `crates/core/engine_backend/src/scene/helio_bridge.rs:239-240` — mints the
  renderer-local default material; this remains necessary until SceneDB has a
  material component and stable material projection.
- `crates/core/engine_backend/src/scene/helio_bridge.rs:336,354` — rebinds
  the SceneDB transform buffer and replaces pass-owned light inputs.
- `crates/core/engine_backend/src/subsystems/render/helio_renderer/renderer.rs:667`
  — advances Helio's foliage wind clock.
- `crates/core/engine_backend/src/subsystems/render/helio_renderer/renderer.rs:1104-1213,1250-1271,1337-1380`
  — editor-facing `SceneActorId` selection, picking, and transform write-back.
  This is deliberately outside the migration because it is editor state, not
  scene content; no UI source is touched.
- `crates/core/engine_backend/src/subsystems/render/helio_renderer/renderer.rs:1414,1422`
  — global sky and hidden-group setup.
- `crates/core/engine_backend/src/subsystems/render/helio_renderer/renderer.rs:1522,1541`
  — Helio foliage-handle cleanup and portal-pair actions.
- `crates/core/pulsar_game/src/windowed_app.rs:168` — advances the same
  foliage wind clock for the standalone runtime.
- `crates/renderer/helio/crates/examples/outdoor_rocks.rs` — migrated
  billboard records and renderer construction; its mesh/material/light/VG
  setup remains legacy Helio scene content until corresponding SceneDB
  components exist.

No UI crate was included in the migration scope. The current checkout nevertheless has
uncommitted changes inside the `crates/ui/wgpui-component` submodule
(`crates/ui/src/lib.rs` and `crates/ui/src/profiler/mod.rs`), so this document must not claim
that UI framework files are untouched without first resolving that independent worktree state.

## Verified Phase 13 slice

`helio-pass-billboard::BillboardComponent` and its generated GPU mirror exist,
but the migration is not complete. `Renderer::set_billboard_instances` is
still a compatibility vector setter; Helio no longer attempts to mutate a
SceneDB through the injected handle. The frontend must spawn/update/despawn
the pass component and flush its World mirror before rendering. The legacy
vector and fallback publication path remain and prevent Phase 13 acceptance.

Coverage is compile-checked by the pass component tests and by the Helio
nested-workspace checks for `helio`, `helio-pass-billboard`, and
`helio-default-graphs`. A GPU readback/integration test for this exact Helio
pass path is not present yet.

### Renderer-owned library/component callers

## Verified Phase 15 slice: outdoor-rocks lights

The three lights in `crates/renderer/helio/crates/examples/outdoor_rocks.rs`
are now SceneDB-owned `SceneLight` components. Startup spawns the sun and two
fill lights into the example's shared `SceneDb::world`; the Q/E sun control
updates that world row; and each frame projects the world rows into Helio's
transient `LightRenderInput` list. The example no longer calls
`SceneActor::light`, `Scene::update_light`, or any other legacy light insertion
API. Mesh, material, virtual-geometry, and billboard setup remain unchanged.

The projection has a focused unit test covering world ownership, entity
identity, and position propagation. `cargo check --manifest-path
crates/renderer/helio/crates/examples/Cargo.toml --bin outdoor_rocks` and the
filtered `cargo test` for that test pass. This is an example-only slice: no
UI source was changed and no shared legacy API was removed.

The migrated file still matches the mechanical legacy inventory because its
mesh/material/virtual-geometry setup remains intentionally deferred. Its light
records do not remain in that category: the sun and two fill lights are
SceneDB-owned, and the per-frame projection is an adapter read of those rows.

## Verified Phase 16 slice: parent-workspace material projection

`engine_backend::scene::rebuild_static_mesh_frame` now reads the typed
`helio_component::MaterialOverrideComponent` from each authoritative SceneDB
mesh entity. It projects those fields into renderer-local Helio material slots,
updating a slot in place when an override changes; Helio retains only the
transient `MaterialId`/GPU material projection. Meshes without an override keep
the existing shared default material, so the current UI and scene-file behavior
remain unchanged. No legacy Helio API was removed because compatibility callers
in Helio's component/editor paths still use those APIs.

The bridge has focused coverage for base color/alpha, emissive intensity, and
roughness/metallic mapping. `cargo check -p engine_backend --features render`
and the filtered bridge tests pass. The nested Helio check remains blocked by
pre-existing `wgpu::Buffer`/scene-buffer API drift in the billboard and
virtual-geometry passes, while `tools/check_helio_scene_api_boundary.ps1`
passes.

The current inventory command reports **91 remaining matching Rust files**:

- 63 files under `crates/renderer/helio/crates/examples`
- 26 files under `crates/renderer/helio/crates/helio-web-demos`
- 1 file under `crates/renderer/helio/crates/helio-android-demos`
- 1 file under `crates/renderer/helio/crates_other`

These are distinct-file counts, not call-site counts: the parent-workspace
production seam is 3 files, the Helio component/asset adapter cohort is 8
files, and the Helio scene/editor implementation cohort is 13 files. The
outdoor-rocks light slice removes three legacy light insertions and one
per-frame light update, but does not reduce the file count until the remaining
mesh/material/virtual-geometry callers in that example are migrated.

For reproducibility, the same broad expression produces 2,846 matching
`rg -n` lines across those 91 files. That number is deliberately reported as
matching lines, not as call sites: the expression includes symbol references,
comments, definitions, and overlapping anchors from multiline calls. A
semantic call-site total cannot be inferred from this mechanical inventory
without parsing Rust. Do not add the 91 file count and the 2,846 matching-line
count together.

The remaining renderer-side callers are:

- `crates/renderer/helio/crates/helio/src/scene/actor.rs` — actor adapters
  internally call `insert_mesh`, `insert_light*`, `insert_object`, and the
  typed resource APIs.
- `crates/renderer/helio/crates/helio/src/scene/lifecycle.rs` —
  `insert_actor` and actor lifecycle tests.
- `crates/renderer/helio/crates/helio/src/editor/commands.rs` and
  `crates/renderer/helio/crates/helio/src/editor/state.rs` — editor operations
  retain `SceneActorId` and mutate `scene_mut`.
- `crates/renderer/helio/crates/helio-component/src/components/foliage_component/runtime.rs`
  — `add_foliage_*` and material insertion.
- `crates/renderer/helio/crates/helio-component/src/components/portal_component.rs`
  and `src/subsystems.rs` — `add_portal` and portal scene mutation.
- `crates/renderer/helio/crates/helio-component/src/components/water_volume_component.rs`
  — `insert_water_volume` / `remove_water_volume`.
- `crates/renderer/helio/crates/helio-component/src/components/reflection_capture_component.rs`
  — reflection-capture scene mutation.
- `crates/renderer/helio/crates/helio-component/src/components/post_process_volume_component.rs`
  — post-process-volume scene mutation.
- `crates/renderer/helio/crates/helio-component/src/components/static_mesh_component.rs`
  — documents the legacy `insert_actor(SceneActor::mesh(...))` path.
- `crates/renderer/helio/crates/helio-asset-compat/src/lib.rs` — asset import
  inserts actor/mesh/material/sectioned-mesh resources into `scene_mut`.

### Examples and snapshot callers

The legacy example surface remains intentionally untouched except for
`outdoor_rocks.rs`, which is now the SceneDB/billboard migration example. The
remaining exact set is mechanically reproducible with:

```powershell
rg -l 'SceneActor|\.scene_mut\(\)|insert_actor|insert_(mesh|material|light|object)|add_(portal|sublevel|foliage|water|reflection|post_process|voxel)' `
  crates/renderer/helio/crates/examples `
  crates/renderer/helio/crates/helio-web-demos `
  crates/renderer/helio/crates/helio-android-demos `
  crates/renderer/helio/crates_other --glob '*.rs'
```

This returns the remaining Helio examples (excluding the now-migrated
billboard records in `outdoor_rocks.rs`), WASM demos, Android demo,
`helio-web-demos/src/common.rs`, `helio-asset-compat` snapshot support, and
`crates_other/helio-snapshot/src/renderer.rs`; no production migration is
claimed for them.

The current inventory produces 91 matching Rust files in the example/demo
cohort: 63 example files, 26 web-demo files, 1 Android-demo file, and 1
snapshot renderer. Again, these are files, not calls. The parent-workspace
production seam count is 3 files, the Helio component/asset-adapter count is
8 files, and the Helio scene/editor implementation count is 13 files. The
production adapter files are exactly:

- `crates/renderer/helio/crates/helio-component/src/components/foliage_component/runtime.rs`
- `crates/renderer/helio/crates/helio-component/src/components/portal_component.rs`
- `crates/renderer/helio/crates/helio-component/src/components/post_process_volume_component.rs`
- `crates/renderer/helio/crates/helio-component/src/components/reflection_capture_component.rs`
- `crates/renderer/helio/crates/helio-component/src/components/static_mesh_component.rs`
- `crates/renderer/helio/crates/helio-component/src/components/water_volume_component.rs`
- `crates/renderer/helio/crates/helio-component/src/subsystems.rs`
- `crates/renderer/helio/crates/helio-asset-compat/src/lib.rs`

The 13 implementation files are `helio/src/editor/{commands,state}.rs`,
`helio/src/{lib,mesh,picking,renderer/config}.rs`, and
`helio/src/scene/{actor,core,foliage,lifecycle,mod,portals,sublevels}.rs`;
the count includes the seven `scene/*` files and the six non-scene files
shown by the command.
Examples remain an explicitly deferred compatibility cohort because they are
independent Helio demonstrations rather than parent production callers.

## Guard policy

`tools/check_helio_scene_api_boundary.ps1` is the mechanical guard. It fails
if `helio-core` gains a dependency on a `helio-pass-*` crate or a new
pass-specific public method in `src/scene`. The existing
`reflection_captures_buffer` compatibility accessor is explicitly allowlisted
so the guard reports new violations rather than pretending the current tree is
already clean. Generic GPU buffer operations and pass read-only resource fields
are not scene-content ownership APIs and are outside this guard.

Run it from the repository root:

```powershell
pwsh -File tools/check_helio_scene_api_boundary.ps1
```

No compatibility API was removed in Phase 14. Removing `Scene::insert_actor`,
`Renderer::scene_mut`, or the typed foliage/portal/water helpers is not
compile-safe while the callers above and the renderer-owned Helio component
adapters remain. The safe migration boundary is now the SceneDB bridge:
mesh/light content has no direct parent-workspace actor insertion, and its
GPU data is passed as transient projections. The next removals require, in
order, SceneDB material data, a foliage/wind component projection, portal and
post-process projections, and an editor selection projection. This audit
intentionally does not modify the high-level renderer seam, `helio-core`
scene implementation, UI, or the existing examples/snapshot callers.

## Mandatory object-to-component inventory

This is the acceptance inventory for every former `SceneActor` variant and
scene/resource creator. A projection is either SceneDB's generated GPU mirror
through `GpuMirrorHandle`/`SceneGpuStore`, or a documented frame-derived
transient buffer. `Missing` entries are blockers, not compatibility exemptions.

| Former object/API family | Required SceneDB owner | GPU projection or transient classification | Status |
|---|---|---|---|
| `SceneActor::Sky` | `helio-pass-sky::SkyComponent` | Generated packed atmosphere/cloud row through `GpuMirrorHandle`; LUT is frame-local | In progress: pass-owned component and lockless projection are wired; legacy callers remain to be migrated |
| `Mesh`, `insert_mesh`, `insert_object` | `helio-component::StaticMeshComponent` + `engine_backend::scene::{SectionedMeshComponent,MeshObjectComponent}` + `Transform` + material component | Generated mesh pools/transforms; transient draw instances | Partial: parent bridge exists; section/object state now has typed SceneDB equivalents |
| `Light`, `insert_light*` | `helio-component::LightComponent` | Generated fields; transient compact light list | Partial: parent projection exists; Helio APIs remain |
| `VirtualMesh`, `insert_virtual_mesh` | virtual-geometry pass mesh component | SceneDB meshlet pools; cull work is transient | Missing: legacy VG mesh registry remains |
| `VirtualObject`, `insert_virtual_object` | virtual-geometry pass object component | SceneDB transform/material rows; meshlet work transient | Missing: legacy VG object registry remains |
| `Decal`, `insert_decal*` | decal pass/component crate | Generated decal rows; visible list transient | Missing: no accepted SceneDB decal component |
| `WaterVolume`, `insert_water_volume` | `helio-component::WaterVolumeComponent` | Generated rows; simulation surface/clipmap transient | Partial: component exists; Helio registry remains |
| `WaterHitbox`, `insert_water_hitbox` | water pass hitbox component | Generated rows; interaction results transient | Missing: no accepted component |
| `PostProcessVolume`, `insert_post_process_volume` | `helio-component::PostProcessVolumeComponent` | Generated rows; sorted/blended list transient | Partial: component exists; registry remains |
| `ReflectionCapture`, `insert_reflection_capture*` | `helio-component::ReflectionCaptureComponent` | Generated rows; probe scheduling transient | Partial: component exists; registry remains |
| foliage type/layer/interactor `add_*` | foliage component/pass components | Generated authored rows; blades/tiles are dynamic frame output | Partial: component path exists; old stores remain |
| portal `add/update/remove_*` | `helio-component::PortalComponent` | Generated rows; chains/masks are frame-derived | Partial: component path exists; registry remains |
| sublevel `add/update/remove_*` | frontend sublevel component | SceneDB placement rows; loaded draw batch transient | Missing: no accepted component |
| `insert_material*` | `engine_backend::scene::MaterialComponent` plus existing `MaterialOverrideComponent` | SceneDB identity/fields; renderer slot is borrowed projection | Partial: both component forms project through the bridge; Helio material table remains transient |
| `insert_texture*` | `engine_backend::scene::TextureComponent` | SceneDB asset identity/pixels; residency/page tables transient | Component equivalent added; legacy Helio insertion remains for compatibility callers |
| sectioned mesh/object APIs | `SectionedMeshComponent` + `MeshObjectComponent` | SceneDB asset/transform rows; section list and object draw records transient | Component equivalents added; legacy Helio multi-mesh pools remain compatibility-only |

## Verified Phase 17 slice: persistent mesh/material/texture resource components

The parent workspace now exposes typed `SceneDB` components for texture
assets, material assets and texture bindings, sectioned mesh geometry, and
mesh-object render state. `insert_render_resources` inserts any complete
resource bundle directly on an existing `World` entity; no Helio handle,
`wgpu` object, dense arena index, or renderer lock is part of the persistent
state. The static-mesh bridge prefers `MaterialComponent` data and falls back
to the existing per-object override component, projecting authored values to a
renderer-local Helio slot while retaining SceneDB as the authority.

This slice intentionally does not delete Helio's compatibility insertion APIs:
the nested Helio examples and pass/component adapters still call them, and
the Helio submodule has independent uncommitted work. Removing those APIs
without migrating that cohort would be a destructive cross-repository change.

Validation note: `git diff --check` passes. Workspace Cargo checks remain
blocked by the pre-existing local Helio path-dependency conversion: the Helio
workspace inherits `helio-core` from a root dependency table entry that is
not present yet. Once that dependency table is completed, rerun
`cargo check -p engine_backend --features render`, `cargo test -p
engine_backend`, and the root `just check`/`just test` gates.
| group visibility/membership APIs | generic visibility/group component | Group-mask projection; cull lists transient | Missing: groups remain in Helio state |

The table includes non-actor creators because they create persistent scene
content just as surely as constructors. Phase 13 cannot be accepted while any
`Missing` row, old creator, or old persistent container remains.
