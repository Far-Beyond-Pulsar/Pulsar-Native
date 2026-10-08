# Phase 4: every render component reaches a pass, or is reported unsupported

Status: **in progress** (Pulsar-Native#1035). Builds on Phase 3 ([14-phase-3.md](14-phase-3.md)).

Phase 4 exit (from the plan): each supported render component has an actual GPU consumer and observable effect, and each unsupported capability is explicitly reported. No exposed component silently succeeds through an undrained queue or a no-op runtime behavior. `PendingWorldWrites` and the inert renderer behavior dispatch are deleted.

## Decisions (approved)

1. **Unsupported reporting.** A component class declares that it has no render consumer. The properties card shows a warning for it, and the renderer logs it once per class.
2. **Sky and sun.** The sun comes from the authored directional light, as now. The sky stays a project/renderer setting. `sky_components` (a sky component) is reported unsupported.
3. **LOD.** `LODComponent` is reported unsupported: nothing consumes it.
4. **Decals, corona, sprites.** Reported unsupported: no authored component exists for them.
5. **GPU uploads with no consumer are removed.** This covers the physics and rigidbody GPU companions and `LightComponentGpuMirror` (the lighting input is the derived `LightSourceRow`).

## Stages

1. Fog (global, local volume), post-process volume and camera post-process; the editor camera post-process CPU copy removed.
2. Water volumes, reflection captures (one schema owns the buffer), portals.
3. Foliage; spline debug lines from data; the HLFS pass off the CPU `World`; voxel audit.
4. `PendingWorldWrites` and the behavior dispatch deleted; unsupported reporting; the pass rows closed.

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

Deviation from decision 1: the one-time unsupported log fires when a component is attached, not in the renderer. Logging from the renderer would mean scanning the scene every frame.

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

**Reflection captures: unsupported.** Deferred lighting samples a capture only through a baked cubemap layer (`cubemap_index`), and the engine runs no probe baker (`helio-bake` is not a dependency). A placed capture would therefore never contribute.
- The component keeps its authored settings and writes no rows.
- The duplicate `ReflectionCaptureGpuComponent` schema and its binding are deleted, so `helio_pass_deferred_light::ReflectionCaptureComponent` is the only schema of `reflection_captures`.
- Supporting captures needs probe baking (or dynamic captures) in the engine.

**Portals: unsupported.** Helio's portal passes draw linked pairs: each authored `helio_pass_portal_cull::PortalComponent` names its peer, and `PortalProjectionBridge` turns the pairs into view and chain rows at dense, reserved entity slots.
- The engine's `PortalComponent` authors no peer; its `portal_id` was never a link.
- The editor world cannot reserve dense entity slots.
- Its old queued write (a one-portal chain with no peer) could not have drawn a portal even with a drain.
- Supporting portals needs an authored peer reference and a portal contract that does not depend on entity slots.

Both classes stop queueing writes now. Stage 4 adds the unsupported report: the card warning and a one-time log when the component is attached. `helio-component` no longer depends on the portal-cull or deferred-light passes.

Tests:
- `environment_join_rows.rs` has `water_volumes_are_packed_into_the_leading_rows`. Two volumes whose instance rows sit past row 8 land in rows 0 and 1, with their surface heights and the owner's scale applied. Disabling the first moves the second up. Hidden or detached volumes free their rows.
- `environment_components_reach_the_frame` has a water case. The pool changes the frame, and moving it away or disabling it restores the frame. Depth never changes.
- The sweep shows only the known failures (see Stage 1).

