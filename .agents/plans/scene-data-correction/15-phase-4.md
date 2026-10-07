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
