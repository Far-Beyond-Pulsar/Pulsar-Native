# Authored mesh LOD

Authored object LOD and renderer-internal geometric LOD are separate layers.

- `LODComponent` chooses an authored mesh asset for an object according to
  distance thresholds and bias. Its choice is evaluated for each render view.
- The selected asset can then use the renderer's own geometric LOD. Helio's
  virtual-geometry path (`.agents/plans/virtual-geometry.md`) is an alternative to
  authored LOD, not a layer on top of it: while an instance is VG-active its
  `LODComponent` is inert; classic mesh renderers do not need to use it.
- The component expresses representation policy. It does not own memory
  budgets, pinning, or eviction.
- Transitions are discrete. A future renderer may add a per-view cross-fade;
  the component does not expose transition fields until a renderer consumes
  them.

## Data and execution path

LOD levels are ordered authored mesh references with increasing world-space
distance thresholds. Positive `lod_bias` scales those thresholds by
`2^-bias`, selecting coarser levels sooner. The component writes a compact,
growable GPU source table keyed by its component-instance entity. The engine
installs `AuthoredLodSelection` after the camera-independent scene join. It
uses the active view camera, owner generation, visibility, and authored
thresholds to choose an asset. The selector remains view-scoped and does not
scan the CPU world.

The view reads selection demand back asynchronously and requests the selected
asset through the shared `Residency<LoadedMesh>` cache. While loading, the
ordinary `StaticMeshComponent` remains the fallback. Once resident, the view
leases the asset's geometry in SceneDB's shared interned vertex and index
pools. The lease is released when the view no longer selects the asset; asset
metadata and unselected variants are not retained as strong component
references. SceneDB continues to own the CPU payload residency budget, while
view-owned GPU leases make active geometry lifetime explicit.

The render derivation patches the joined `static_objects` rows with the
selected asset's section ranges and bounds. It also publishes a geometry
generation marker. Selection and resident asset table changes invalidate
ObjectBatch's draw grouping, while transform-only updates keep the existing
optimized path. If an authored level is not resident, no invalid geometry is
published and the base static mesh draws.

## Boundaries

- Do not encode Helio meshlet indices or meshlet error thresholds in
  `LODComponent`.
- Do not synchronously load assets or scan the `World` from a render pass.
- Do not keep every authored variant permanently resident because the
  component references it. Authored references and active residency demand
  are separate relationships.
- Keep authored level selection independent of which renderer consumes it;
  classic mesh renderers can use it without adopting meshlets.
- Cross-fades require pass-owned per-view transition state and are outside
  the component contract until implemented.

## Verification

`crates/editor/ui_level_editor/tests/render_acceptance.rs` verifies that the
rendered mesh changes across a distance threshold, positive bias selects the
coarser asset sooner, and disabling `LODComponent` restores the base mesh.
The `LODComponent` is no longer registered as unfinished.

This work is tracked by
[Pulsar-Native#1053](https://github.com/Far-Beyond-Pulsar/Pulsar-Native/issues/1053).
