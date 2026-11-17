# Cables plugin design

## Ownership and data flow

`pulsar.cables` owns the definition and connector component classes, the ID
pairing, the definition GPU rows and the render feature.

- **Definitions** (`CableComponent`) are SceneDB source rows
  (`cable_definition_sources`, `#[gpu]` fields mirrored on edit). A source row
  sits at its owning object's scene slot, so the plugin's scene join packs
  the enabled ones into the leading rows of `cable_definitions`, as every
  kind's `*_sources` column is joined. The scene texture slot of the tile
  texture is written into the row by the mirror dispatch, because the slot
  belongs to the scene's texture store.
- **Connectors** (`CableConnectorComponent`) have no GPU rows. A connector is
  an ID and the position of its object.
- **Pairing** is CPU work, each frame, the way portals pair by Link ID.
  `CableSync` is a space producer (one per renderer): it reads the world's
  enabled connectors and definitions, pairs every two connectors that share
  the ID of an enabled definition, and publishes the frame's `CableSpans`
  (`cables_api::CABLE_SPANS`). It replaces the payload only when it changed.
  A connector ID with no definition, a duplicated definition and a crowded ID
  are logged once as warnings and drawn as nothing or as the first.
- **Drawing** is `CableRenderPass`: it turns the spans into two instance
  lists (camera-facing ribbons; 3D tubes, one instance per bundle lane),
  uploads them when the payload changes, and draws each list with one
  instanced draw. Cables are opaque scene geometry: the pass tests against
  the scene depth (reverse-Z, nearer wins) so meshes occlude cables, and
  writes depth so later passes (billboards, particles, fog) see them; a
  see-through texel (alpha below 0.5) is cut out rather than blended. The vertex stage generates the geometry and looks up the
  cable's definition row by ID.

## AAA comparison and scale

Unreal's Cable Component uses a Verlet-style segment chain with distance
constraints; rope solvers such as XPBD iterate stretch and bend constraints
for stability. This version draws an analytic sag and wind sway in the vertex
stage; a persistent per-cable simulation is future work and would add its
own GPU state keyed by span.

A cable pair is inherently `n(n-1)/2` for an ID with `n` connectors. The
producer bounds that (64 connectors per ID, 4,096 spans per frame) and the
draw bounds instances (65,536 per list), warning when it drops any. CPU work
scales with connectors per frame; uploads scale with changes to the span
payload (an endpoint that moves changes it); GPU work scales with drawn
strands, segments and pixels. Instance lists grow by doubling, never per
frame.

## Implementation shape

- `api`: the definition row ABI, the `CABLE_DEFINITIONS` buffer contract and
  the `CableSpans` payload and its contract.
- `component`: the authored classes, the definition source row and mirror,
  the definition join (`row_derivation`) and `CableSync`.
- `feature`: `CableRenderPass`, its shader and the cable debug views
  (Cable Diagnostics, Cable Solid).
- root: all plugin-gated registrations (component classes, render feature,
  scene rows, the space producer, texture binding).
