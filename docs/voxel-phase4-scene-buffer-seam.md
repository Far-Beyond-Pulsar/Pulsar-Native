# Phase 4 — generic SceneDB buffer seam audit

**Disposition:** existing generic Helio contracts satisfy the central renderer
boundary; Phase 4 adds a specialized-pass proof, not voxel-specific logic to
generic crates.

## Existing path

1. Pulsar SceneDB owns its `SceneGpuStore` and attaches a `GpuMirrorHandle` to
   the `World` (`engine_backend/src/scene/helio_bridge.rs`). Registered
   component columns and their GPU buffers are owned by SceneDB.
2. Helio's frontend adapter (`helio/src/renderer/input.rs`) creates a
   per-frame `SceneBufferProjection` from the SceneDB mirror.
3. `helio-core::SceneInput`, `PrepareContext`, and `PassContext` carry a
   type-erased, read-only projection of `BufferKey` to `BufferHandle`
   (`helio-core/src/scene_input.rs`, `context.rs`). The core does not inspect
   bytes or know component layouts.
4. A specialized pass implements generic `RenderPass`, declares graph
   resources, reads the opaque handle in `execute`, and publishes ordinary
   graph outputs. `RenderGraph::add_pass` registers it without a semantic
   branch in the generic scheduler.

The executable contract test is
`helio-pass-voxel-mesh/tests/opaque_scene_buffer_contract.rs`. It implements a
consumer using an opaque application key and the generic render-pass interface.
The voxel pass test suite also validates the component payload writer and its
separate CPU-side publication contract.

## Important boundary / follow-on integration

Current voxel component payload bytes are canonical live SceneDB component
state. They are intentionally not `#[gpu]` mirrors and do not appear in the
current SceneDB GPU buffer projection. Phase 5 must add a specialized,
incremental bridge from those payload revisions into transient GPU residency;
it must not bulk-copy every chunk each frame. The projected GPU buffers and
voxel pass residency remain rebuildable cache state, never the only copy of
authored data. If that bridge needs another handle, it must remain opaque and
generic in Helio core.

`SceneBufferProjection::get` currently resolves by scanning its small frame
projection. This is appropriate for the current limited set of buffer keys;
do not replace it with per-entity terrain lookup or embed voxel policy. If
profiling later shows lookup cost, optimize the key registry generically and
measure construction plus lookup cost together.

No renderer-core change was needed for this phase. The user-requested 10 ms
radius-128 workload remains a later end-to-end acceptance test; this seam audit
is not a performance qualification.
