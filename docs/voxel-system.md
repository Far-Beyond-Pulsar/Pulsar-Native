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

## Voxel terrain backend

The registered backend `helio.voxel-terrain` (`helio-pass-voxel-planet` in
Helio) renders destructible voxel worlds: planets, square planes and
infinite planes, each filled by a registered terrain generator.

- **Components.** `VoxelTerrainComponent` is the base of every world: its
  shape (`Sphere`, `Plane`, `InfinitePlane`) and size, voxel size (0.1 m to
  1 m), generator id, version and seed, and the edit journal. A generator's
  settings live in its settings component on the same entity, which the scene
  projection serializes into the generator's parameters. The constructors
  `VoxelTerrainComponent::planet(radius)`, `::plane(size)` and
  `::infinite_plane()` are the presets; a new component is a 4 km plane of
  the landform generator. Game-specific worlds (a planet with water and
  foliage, say) are classes whose prefab combines these components with
  others. The world is centred on its entity, which must sit at the origin.
- **Generators.** A terrain generator is a field: a CPU function for the
  surface height and ground material of every column, and a WGSL program
  that computes the same values bit for bit, built from Helio's integer
  noise library. Generators register by id and version
  (`helio_pass_voxel_planet::terrain::register`) and name their settings
  component; `terrain::generators()` lists them. Built in:

  | Generator | Settings component | Terrain |
  |-----------|--------------------|---------|
  | `helio.landform` v1 | `VoxelLandformComponent` | continents, basins, mountain ranges, hills; meadows, dry lands, rock, strata, snow |
  | `helio.flat` v1 | `VoxelFlatTerrainComponent` | level ground: surface over soil over rock |

  A generator's tests should call `engine::verify_field` (CPU against GPU)
  and `terrain::check_field` (declared bounds). Changing only settings
  rebuilds the world without recompiling shaders.
- **Editing.** Select **Voxel Sculpt** in the tool menu, then click or drag to
  dig; hold Shift to build with cobblestone. The backend ray casts exact cells
  in `f64` (orbital edits work) and appends a brush to the component's
  `edits` journal, advancing the source revision. A stroke that only appends
  brushes updates the cached world incrementally. Save the level to keep the
  edits.
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
