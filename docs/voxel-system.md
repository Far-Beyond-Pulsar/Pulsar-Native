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

The default Helio render graphs do not currently draw voxel data. A renderer
integration should read SceneDB component snapshots through this contract and
own its own GPU residency, extraction, and rendering resources. The renderer
must not become the canonical source of voxel data. The exact adapter to a
new renderer can be defined when that implementation is available.

See [the source API example](voxel-component-api-example.md) for batch
publication and snapshot export.
