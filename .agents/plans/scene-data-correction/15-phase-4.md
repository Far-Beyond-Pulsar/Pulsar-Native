# Phase 4: every render component reaches a pass, or is reported unfinished

Status: **complete** (Pulsar-Native#1035), pending review. Builds on Phase 3 ([14-phase-3.md](14-phase-3.md)).

Phase 4 exit (from the plan): each supported render component has an actual GPU consumer and observable effect, and each unsupported capability is explicitly reported. Phase 4 reports these as **unfinished**, each with a tracking issue (see [Unfinished features](#unfinished-features)). No exposed component silently succeeds through an undrained queue or a no-op runtime behavior. `PendingWorldWrites` and the inert renderer behavior dispatch are deleted.

## Decisions (approved)

1. **Unfinished reporting.** A component class declares that it has no render consumer yet, with its tracking issue. The properties card shows a warning with the issue, and the engine logs it once per class. (Approved as "unsupported"; renamed to "unfinished" with tracking issues at the owner's request.)
2. **Sky and sun.** The sun comes from the authored directional light, as now. The sky stays a project/renderer setting. `sky_components` (a sky component) is unfinished (#1057).
3. **LOD.** `LODComponent` is reported unfinished: nothing consumes it (#1053).
4. **Decals, corona, sprites.** Unfinished: no authored component exists for them (#1058, #1059, #1060).
5. **GPU uploads with no consumer are removed.** This covers the physics and rigidbody GPU companions and `LightComponentGpuMirror` (the lighting input is the derived `LightSourceRow`).

## Stages

1. Fog (global, local volume), post-process volume and camera post-process; the editor camera post-process CPU copy removed.
2. Water volumes, reflection captures (one schema owns the buffer), portals.
3. Foliage; spline debug lines from data; the HLFS pass off the CPU `World`; voxel audit.
4. `PendingWorldWrites` and the behavior dispatch deleted; unfinished reporting; the pass rows closed.

## Stage 1: fog, post-process volumes, camera post-process

Done. Each authored component writes a derived **source row** in its owner's local space through the same GPU registration path the lights use (Phase 2). A graph-owned derivation, Helio's `EnvironmentJoin` (`helio-default-graphs/src/environment_join.rs`), turns those rows into the rows the passes already read. Nothing is queued and nothing is built on the CPU.

| Authored component | Source row (buffer) | Join output | Read by |
|---|---|---|---|
| `GlobalFogComponent` | `GlobalFogSourceRow` (`global_fog_sources`) | `global_fog_media` | volumetric fog, fog composite |
| `LocalFogVolumeComponent` | `LocalFogSourceRow` (`local_fog_sources`) | `local_fog_media` | volumetric fog, fog composite |
| `PostProcessVolumeComponent` | `PostProcessVolumeSourceRow` (`post_process_volume_sources`) | `post_process_volumes` | post-process resolver, volumetric fog |
| `CameraPostProcessComponent` | `CameraPostProcessSourceRow` (`camera_postprocess_sources`) | `camera_postprocess` | post-process resolver |

- The join keeps a row only when its instance is enabled and its owner is live (generation match). Fog and post-process volumes also turn off when the owner is hidden. A camera baseline is not visual, so hiding its owner does not affect it.
- Local fog and post-process volumes are placed from the owner's transform. The world AABB of the owner-oriented box takes the owner's scale and rotation into account. Moving the owner moves the volume on the next frame.
- A disabled component writes an inert row (`enabled` 0, or zeroed volume params). Detaching clears the row.
- The four behaviors no longer push into `PendingWorldWrites`; their `sync_component` bodies are empty until Stage 4 deletes the dispatch.
- **The editor camera post-process copy is gone.** The toolbar's bloom toggle used to write a `CameraPostProcessComponent` row for view 0. It now sets the post-process resolver's defaults (`PostProcessVolumeBlendPass::set_defaults`). Those defaults are a renderer setting, not scene data, and an authored camera component still overrides them. The pass `CameraPostProcessComponent` schema is no longer registered as a column.
- **Pre-existing fog bug, fixed.** When post-process fog was disabled, the resolver reported a fog range of 1.0, so the froxel grid held world media only in the first unit in front of the camera. As a result, world fog volumes never showed. The range is now gated on `fog_enabled` and falls back to 1000 (`volumetric_fog.wgsl`, `cs_resolve`).

Tests:
- `engine_backend/tests/environment_join_rows.rs` reads the join's outputs back. It covers:
  - placement under a scaled and rotated owner;
  - hidden, disabled-instance and disabled-component gating;
  - moving an owner;
  - detaching.
- `ui_level_editor/tests/render_acceptance.rs` has `environment_components_reach_the_frame`. Each of the four components, added through `AddObjectWithComponents`, changes the rendered frame. Disabling it, or moving a local volume away, restores the frame. Depth never changes.
- `engine_backend/tests/editor_bloom_toggle.rs` checks that the toolbar toggle changes the resolver baseline and writes no scene row.

Deviation from decision 1: the one-time unfinished log fires when a component is attached, not in the renderer. Logging from the renderer would mean scanning the scene every frame.

Sweep (`helio_component`, `engine_backend`, `pulsar_game`, `ui_level_editor`, run with `--no-fail-fast`):
- Everything passes except the four failures Phase 3 already recorded: the gizmo hover test, the light mapping intensity test and the two `toggle_button` doctests.
- `voxel_block_api::the_cached_world_follows_the_journal_and_the_settings` fails intermittently when tests run in parallel. Helio's voxel world cache is process-global and keyed by entity bits alone. The test passes when run serially. This predates Phase 4 and is tracked separately.

## Stage 2: water volumes, reflection captures, portals

**Water volumes: supported.** `WaterVolumeComponent` derives `WaterVolumeSourceRow` (`water_volume_sources`): its local size, its surface height above the owner (in `w`), then the water row after its bounds. A disabled component derives a zero row. The environment join places the volume the same way as fog and post-process volumes, and sets the surface height to the owner's Y plus the offset, scaled with the owner.

The water passes read a fixed number of leading rows: `MAX_SIM_VOLUMES`, which is 8, and deferred lighting reads only row 0. So the join *packs* placed volumes into those rows, in instance order, instead of keeping each instance's row (`cs_compact_rows`: one workgroup with an ordered prefix sum).
- A volume keeps its row while the volumes ahead of it are unchanged.
- Placed volumes beyond the eighth are not drawn.
- The pass `WaterVolumeComponent` is no longer registered as a column.

Limitation: the simulation reads wave spring, damping, scale and wind from pass-wide settings (`WaterSimPass` setters). The component's per-volume fields for these are carried in the row but have no effect.

**Reflection captures: unfinished (#1054).** Deferred lighting samples a capture only through a baked cubemap layer (`cubemap_index`), and the engine runs no probe baker (`helio-bake` is not a dependency). A placed capture would therefore never contribute.
- The component keeps its authored settings and writes no rows.
- The duplicate `ReflectionCaptureGpuComponent` schema and its binding are deleted, so `helio_pass_deferred_light::ReflectionCaptureComponent` is the only schema of `reflection_captures`.
- Supporting captures needs probe baking (or dynamic captures) in the engine.

**Portals: unfinished (#1055).** Helio's portal passes draw linked pairs: each authored `helio_pass_portal_cull::PortalComponent` names its peer, and `PortalProjectionBridge` turns the pairs into view and chain rows at dense, reserved entity slots.
- The engine's `PortalComponent` authors no peer; its `portal_id` was never a link.
- The editor world cannot reserve dense entity slots.
- Its old queued write (a one-portal chain with no peer) could not have drawn a portal even with a drain.
- Supporting portals needs an authored peer reference and a portal contract that does not depend on entity slots.

Both classes stop queueing writes now. Stage 4 adds the unfinished report: the card warning and a one-time log when the component is attached. `helio-component` no longer depends on the portal-cull or deferred-light passes.

Tests:
- `environment_join_rows.rs` has `water_volumes_are_packed_into_the_leading_rows`. Two volumes whose instance rows sit past row 8 land in rows 0 and 1, with their surface heights and the owner's scale applied. Disabling the first moves the second up. Hidden or detached volumes free their rows.
- `environment_components_reach_the_frame` has a water case. The pool changes the frame, and moving it away or disabling it restores the frame. Depth never changes.
- The sweep shows only the known failures (see Stage 1).


## Stage 3: foliage, splines, HLFS, voxel audit

**Foliage: supported.** `FoliageComponent` derives `FoliageSourceRow` (`foliage_sources`): its foliage type row, a layer (half extent, infinite flag, altitude range) and its wind row. One source row feeds three join tables, each packed into leading rows:
- `foliage_types`: up to 64 rows.
- `foliage_layers`: up to 64 rows. Each layer is a world-aligned square centred on the owner, scaled by the owner's X and Z scale, spanning the authored altitudes.
- `foliage_wind`: one row, from the first placed component.

The join gained two abilities: a table can read a slice of a wider source row, and it can gate on one of the row's words (here, density). Tables that are not placed from transforms no longer re-derive when an unrelated object moves.

The foliage passes changed in three ways:
- **Type selection.** Placement draws a candidate's type from the leading rows that have a density (a per-workgroup scan), so a packed table's empty rows do not dilute density.
- **Liveness gate.** Both passes run only while a type row is live (`foliage_type_liveness`, an async readback), so an all-empty table costs nothing, as an absent column did.
- **Residency.** Residency now follows the type *and* layer contents; it used to follow only the type buffer's reallocation count, which a packed table never changes. The blade seed follows the types alone, so a tile re-placed because only the layers changed grows the same blades.

Limitations, recorded in the ledger:
- Grass grows on flat ground at Y 0, because the editor has no terrain capture.
- Every type grows in every layer.
- The first component's wind applies to all.
- Wind does not animate: no frame clock reaches the row.
- The interactor and material colour fields have no consumer.

**Splines: change-driven.** `SplineLines` reads SceneDB's change journal for:
- spline values and attachment state;
- spline owners' transforms, visibility and selection.

It rebuilds the editor's spline debug lines only when one of those changes. Previously the renderer walked every spline on each scene sync.

**HLFS: outside the engine.** `HlfsPass` reads its TLAS from the renderer's `RenderEnvironment`, not the World. The World reader is `SceneDbRayTracing`, a host-side adapter in the pass crate that only Helio's examples call; the engine never builds the HLFS graph. A TLAS needs CPU instance inputs, so a host adapter is the right shape. Moving it out of the pass crate is queued as a Helio follow-up.

**Voxel audit.** The voxel-planet pass reads no scene buffers. The host hands it a CPU `PlanetFrame` (`Arc<Planet>` and the sun), and the pass syncs journal edits to the GPU incrementally, which is real, change-driven work.

Changed in this stage:
- **Scene read cached by revision.** The renderer's voxel scene read (terrain entries, generator settings, sky and mesh flags, sun) used to run on every frame, including camera-only ones. It is now cached by world revision.
- **Sun visibility.** The voxel sun ignores directional lights whose owner is hidden, matching the scene join.

Recorded, not changed:
- Generator settings reach the generator as a JSON string. That is the generator plugin boundary, now serialised only when the world changes.
- The CPU planet rebuild runs synchronously on the render thread.
- Renderer brush commits bypass `append_edits`' validation and block events.
- Two CPU planet caches exist (the renderer's and `voxel_world::WORLDS`).
- A free-standing `VoxelComponent` has no renderer. Stage 4 reports it unfinished (#1056).

## Stage 4: the queue and dispatch deleted; unfinished reporting

**`PendingWorldWrites` is deleted.** Every producer stopped queueing in Stages 1 to 3. The queue had no production drain.

**The runtime-behavior dispatch is deleted.** Nothing in production called `apply_runtime_behavior_for_class`. Its only caller was `pulsar_scene::SceneLoader`, a legacy import adapter with no callers of its own, and every `sync_component` body was already empty. Removed:
- `SceneLoader`;
- every `#[register_runtime_behavior]`;
- the macro itself, its derive and the `engine_class(runtime_behavior)` flag;
- `pulsar_world_registry`'s typed `dispatch` path, which was also uncalled.

`ComponentRuntimeBehavior` stays only to carry `CLASS_NAME` for `#[register_world_component]`. Its trait and `RuntimeBehaviorRegistration` live in the `pulsar-reflection` submodule. That repository is outside this change, so they remain there, unused.

**Unfinished reporting** (decision 1). `pulsar_world_registry::declare_unfinished_component!(class, reason, issue)` registers a class the engine keeps but does not consume yet, with the issue that tracks the work. Its data still attaches, edits, saves and loads normally. On top of that:
- the properties card shows `Unfinished: <reason>. Tracked in <issue>.` under the class name;
- attaching the first instance logs the reason and issue once per class. This is the deviation recorded in Stage 1: the log fires at attach, not in the renderer.

Declared unfinished:
- `LODComponent`: nothing consumes it (#1053);
- `ReflectionCaptureComponent`: no probe baker (#1054);
- `PortalComponent`: no peer link, and no dense projection slots (#1055);
- free-standing `VoxelComponent`: no renderer (#1056).

Decals, corona and sprites (decision 4), and the sky (decision 2), have no authored component, so there is no card to warn on. Their pass rows are recorded `unfinished` with their issue. The ledger gains an `unfinished` status whose `test` must name the check or the tracking issue.

**Unconsumed GPU uploads removed** (decision 5). Every `#[engine_class]` with `#[gpu]` fields or `#[sub_props]` used to generate a GPU companion, register a SceneDB dispatch for it, and auto-register and upload its row on the first insert. That covered `LightComponentGpuMirror`, the physics and rigidbody companions, and the zero-byte companions of the environment components. None of those rows had a reader. The companion now uploads only for a class that opts in with `#[engine_class(gpu_rows)]`; no engine class does. The companion type and `GpuMirrored::to_gpu_mirror` mapping stay as CPU helpers: `LightSourceRow` is built from the light's mirror. Tests:
- `engine_class_derive`'s mirror tests opt in;
- `without_gpu_rows_the_companion_uploads_nothing` checks the default;
- helio-component's `light_component_gpu_mirror.rs`, which tested the now-removed row, is deleted. The light's real input, `LightSourceRow`, is covered by `scene_join_rows` and the frame tests.

**Ledger.**
- Every class row has `runtime_behavior = false`, and physics and rigidbody have no GPU columns.
- The authored environment schemas are verified by the join tests.
- The four unfinished classes are `unfinished`, tested by `helio-component/tests/unfinished_components.rs`, with their issues.
- Pass rows for unfinished features (sky, decals, corona, reflection captures, portals) are `unfinished` with their issue. Rows with no authored source and no planned one (legacy fog, volumetric fog settings, foliage interactors) are `out-of-scope`, each with its reason.

Stage 4 sweep: `helio_component`, `engine_backend`, `pulsar_game`, `ui_level_editor`, `pulsar_class`, `pulsar_physics`, `pulsar_scene`, `pulsar_world_registry`, `scene_inventory` and `engine_class_derive`, with `--no-fail-fast`. 40 test targets pass. The only failures are the ones already known: the gizmo hover test, the light mapping intensity test, the two `toggle_button` doctests and the parallel `voxel_block_api` cache flake.

## Phase 4 exit

- **Every supported render component has a consumer and an observable effect, checked by a frame test:**
  - global and local fog;
  - post-process volumes;
  - camera post-process;
  - water volumes;
  - foliage (blades drawn);
  - meshes and lights (Phase 2);
  - splines (editor lines, change-driven).
- **Every unsupported capability is reported as unfinished** on its card and in the log, or in the ledger where no authored component exists, each with a tracking issue.
- **No exposed component succeeds silently through an undrained queue or a no-op behavior:** `PendingWorldWrites` and the dispatch are gone.

## Unfinished features

Each is reported in the engine (the card and a one-time log, for authored classes) and in the ledger, and tracked by a sub-issue of #1035. Finishing one means a data contract like the other Phase 4 components, a frame test, and removing its `declare_unfinished_component!`.

| Feature | Where it stands | Issue |
|---|---|---|
| `LODComponent` | authored, no consumer | #1053 |
| `ReflectionCaptureComponent` | authored; needs probe baking or dynamic captures | #1054 |
| `PortalComponent` | authored; needs peer links and a portal contract without dense entity slots | #1055 |
| Free-standing `VoxelComponent` | authored (payload store and sessions work); no renderer | #1056 |
| Sky component | pass schema only; the sky is a project setting | #1057 |
| Decals | pass schema only | #1058 |
| Corona (particle emitters) | pass schema only | #1059 |
| Sprites / billboards | pass draws editor light icons only | #1060 |

## Left open (recorded, not done here)

- **Ledger rows not yet analysed.** `[[pass]]` rows remain `unverified`: each pass crate's observable output is not yet individually tested. The gbuffer `RenderGroupComponent` / `SectionedObjectComponent` / sublevel rows and `water_hitboxes` stay `unverified`.
- **Voxels.**
  - The CPU planet rebuild runs on the render thread.
  - Renderer brush commits bypass `append_edits`.
  - There are two CPU planet caches.
  - Generator settings cross as JSON, the plugin boundary.
- **Foliage.**
  - Wind does not animate.
  - Types share layers.
  - There are no terrain heights in the editor.
- **Water.** Per-volume simulation dynamics have no effect.
- **HLFS.** `SceneDbRayTracing` lives in the pass crate; moving it is a Helio follow-up.
- **`pulsar-reflection` submodule.** `RuntimeBehaviorRegistration` and `apply_runtime_behavior_for_class` are unused there.
- **Pre-existing test failures**, as in Phase 3:
  - the gizmo hover test;
  - the light mapping intensity test;
  - the `toggle_button` doctests;
  - `voxel_block_api` when run in parallel (process-wide cache keyed by entity bits).

