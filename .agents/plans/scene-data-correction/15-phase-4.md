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
