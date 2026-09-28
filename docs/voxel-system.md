# Voxel data boundary

SceneDB owns authored voxel configuration and live canonical chunk payloads.
`VoxelComponent` describes a bounded object; `VoxelTerrainComponent` describes a
terrain source. Both expose the same opaque payload store, material-ID palette,
and revisioned data contract. The components do not hold GPU buffers or choose
a drawing method.

`helio-voxel-data` is the CPU-only shared contract. It defines signed chunk
keys, 8³ material samples, bounded publication batches, revision checks,
immutable snapshots, editing, and generator worker APIs. `VoxelSourceWriter`
publishes to a component-owned store. `VoxelSourceSession` in Pulsar resolves a
typed SceneDB row and owns its publication and edit workers. The caller owns
any durable export or import policy; component serialization contains authored
configuration, not live payload bytes.

Pulsar maps enabled SceneDB rows to `VoxelSceneEntry` values. A voxel
backend registers a stable renderer ID, a pass factory, and a frame publisher.
The default Helio deferred graph inserts those registered passes after geometry
and before decals and lighting. A row may name its renderer explicitly or leave
the ID empty for unique capability matching. An unknown or ambiguous backend is
reported as an error. Each backend validates its own source format, chunk size,
LOD, domain, and generator requirements; the generic component does not impose
one terrain representation. GPU residency remains transient and rebuildable.

## Voxel planet backend

The registered backend `helio.voxel-planet` renders a destructible, Earth-sized
voxel planet (`helio-pass-voxel-planet` in Helio). It consumes terrain rows
whose generator is `helio.voxel-planet.default`, version 1.

- **Grid.** Exact voxels on an equal-angle cube sphere aligned with gravity.
  The component's `voxel_size` sets the base voxel (0.1 m to 1 m); the planet
  keeps the same shape at every voxel size.
- **Recipe.** `generator_parameters` holds a JSON `PlanetSourceRecipe`: the
  planet (radius and landform) and the ordered brush edits. Empty means the
  default Earth-sized planet with no edits. A nonzero component `seed`
  replaces the landform seed. The row must be unbounded, with its origin at
  the planet centre.
- **Editing.** Select **Voxel Sculpt** in the tool menu, then click or drag to
  dig; hold Shift to build with cobblestone. The backend ray casts exact
  cells in `f64` (clipped to the planet shell, so orbital edits work),
  appends a sphere brush to the recipe and advances the source revision.
  A stroke that only appends brushes updates the cached planet
  incrementally. Save the level to keep the edits.
- **Rendering.** Terrain is traced per pixel through a GPU-driven clipmap of
  exact voxel columns. Nearby voxels stay crisp; distant cells use filtered
  appearance, with no smooth terrain and no visible LOD pop. The viewport
  renders camera-relative, and the backend supplies near and far planes from
  certified empty space. Sunlight is traced towards the scene's directional
  light, and ambient is a hemisphere around the local vertical.
- **Performance.** On an RTX 3060, terrain GPU time is 3.7 ms (p95) at 720p
  and 4.7 ms at 1080p Quality over flights from walking to orbit and back.
  See Helio's `voxel_flight` example and its gates.

Live chunk payloads (the generic sample edits) are not consumed by this
backend yet. It reports an error rather than ignoring them.

See [the source API example](voxel-component-api-example.md) for batch
publication and snapshot export.
