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

The first registered backend, `helio.tiny-voxel`, renders a procedural planet
with an authored base voxel size from 0.1 m to 1 m in 0.1 m increments. The
example uses 0.1 m. Changing `voxel_size` changes the base grid without rescaling
the planet or its edit coordinates; it is not a distance-based visual LOD.
The backend consumes `helio.tiny-voxel.default` generator revision 5 and an
optional versioned JSON recipe in `generator_parameters`. Its authored edits
are part of that recipe. It currently accepts one planet source at a time and
rejects live chunk payloads that it cannot interpret. The generic API still
stores those payloads for other registered backends. No raw-chunk renderer is
registered for `VoxelComponent` yet.
Its private GPU brick dimensions are independent of the component's generic
live-payload chunk metadata. Invalid replacement recipes clear the old frame
instead of leaving stale terrain visible.

Open [`assets/examples/voxel_planet.level`](../assets/examples/voxel_planet.level)
in Pulsar to see the backend through a normal `VoxelTerrainComponent`. The
editor camera stores its position as `f64` so the voxel pass can build an exact
nearby cell origin at planetary distance. The viewport submits a camera-local
matrix for this backend so depth and motion stay precise as the camera moves.
Select **Voxel Sculpt** in the tool menu, then click or drag to dig; hold Shift
to build. The backend raycasts exact cells in `f64`, writes its edit journal to
the terrain recipe, and increments the source revision. Save the level to keep
those edits. Ordinary mesh transforms and editor pick tools still use `f32`;
their precision at that scale is separate work.
Editing rays are clipped against the world's bounds, without a 1 km tool-reach
limit. The backend also supplies a planetary camera far plane and, in a
terrain-only scene, a conservative near plane derived from certified empty
space. Authored mesh rows retain the close near plane.
The hierarchy eye toggle hides terrain while keeping its authored sky, and
selecting a terrain does not draw a planet-sized transform gizmo at its origin.
The backend keeps an idle viewport rendering while its bounded brick jobs are
pending and releases its GPU residency when the source is removed.
Once a complete terrain cut exists, it remains visible during orbital travel
while a replacement refines. Refinement can still lag movement. Compatible
graph rebuilds preserve residency across resize and AA quality changes.

CPU queries and edits are exact on the selected base grid. Distant GPU
occupancy is currently reconstructed from sampled density; it is not an exact
filtered reduction of the edited leaf volume. Small edits can therefore be
visually unresolved until refinement. The current backend is not yet
qualified for AAA visual quality or seamless exact-detail arrival.

The [26 September full-graph validation report](https://github.com/Far-Beyond-Pulsar/Helio/blob/156f6a7b28e3ef224facbd950d9b6f9753c327cc/crates/passes/3d/helio-pass-tiny-voxel/VALIDATION_2026_09_26.md)
contains reproducible flights, raw frame timings, capture audits and the
remaining quality failures. A larger generation-batch experiment was reverted
because it reduced arrival latency but worsened descent frame times. These
offscreen measurements are not a populated level or whole-editor performance
qualification.

The [preceding profiling checkpoint](https://github.com/Far-Beyond-Pulsar/Helio/blob/24e522966c164bf0f82e8fbbe35d6b9e21f7d0b0/crates/passes/3d/helio-pass-tiny-voxel/VALIDATION_2026_09_26_GOAL.md)
pins Helio `24e52296` (terrain runtime `c5f80ee0`, evidence `61364975`,
plus profiler identity). It retains source-frame-labelled
terrain/graph timestamps, allocation reporting, exact primary-ray replay,
sunlight readback checks and a leaf-exit rounding correction. Empty-brick
skipping remains disabled and the parent-link experiment was reverted: neither
met the repeated performance gates. The retained source passed 25 terrain tests
(one CPU benchmark ignored), the full deferred-graph GPU test, and a 1,180-frame
1080p Quality flight with 21 capture audits. The latter was heavily instrumented
and does not establish acceptance timings. This Pulsar pointer passed all nine
native voxel integration tests. Arrival, far fidelity, stable frame times and
the visible contour/noise defects remain unresolved.

The preceding Helio checkpoint is `ab07c84e`. The
[canonical audit](https://github.com/Far-Beyond-Pulsar/Helio/blob/ab07c84e9d8d46304631845ed8220a29eff43cbf/crates/passes/3d/helio-pass-tiny-voxel/CANONICAL_FIDELITY_2026_09_26.md)
fixes a half-voxel displacement at clamped brick entry. The
[bounds and traversal checkpoint](https://github.com/Far-Beyond-Pulsar/Helio/blob/ab07c84e9d8d46304631845ed8220a29eff43cbf/crates/passes/3d/helio-pass-tiny-voxel/BOUNDS_AND_TRAVERSAL_2026_09_26.md)
adds tighter conservative certificates for the same canonical field and fixes
a near-coincident corner crossing that could cycle sunlight traversal.
The CPU selection benchmark requests 5.8% fewer ground bricks, 23.1% fewer at
200 m and 16.0% fewer at 1 km; some cold selections are slower. The adaptive
Quality ground budget improves from 2.289 to 1.831 pixels. Neither those counts
nor the unchanged 512 MiB material pool establish a GPU performance saving.
Validation includes 27 terrain tests, two voxel-field tests, 904,932 exact
noise samples and 546,940 edited/procedural cell checks. The 1,124-frame Quality
recording passed 368 capture audits; the 1,165-frame native diagnostic passed
21. These recorded/instrumented flights are not performance acceptance.
Far GPU cells still disagree with exact CPU occupancy; the appearance, regional
publication, latency, populated-editor and simulation gates remain open.
All nine native voxel integration tests passed with this paired pointer.

The preceding pointer `66100ff8` introduced the
[canonical reference experiment](https://github.com/Far-Beyond-Pulsar/Helio/blob/66100ff8990c94ab5374e857355fe2b7d047b774/crates/passes/3d/helio-pass-tiny-voxel/CANONICAL_REFERENCE_2026_09_26.md)
adds an opt-in source-faithful far tracer with published-snapshot edit isolation.
The feature is disabled in the engine's normal configuration. It agrees with
864 sampled CPU rays in the retained diagnostic, but direct recipe evaluation
is much too slow to qualify as the runtime far renderer. The report includes
the rejected arithmetic variants, source-specific flights and remaining
visual/performance gates. It does not resolve the default renderer's far-field
or regional-publication limitations.
All nine native voxel integration tests passed with that pointer.

The current pointer is `daf5d51d`. The optional
[regional publication experiment](https://github.com/Far-Beyond-Pulsar/Helio/blob/daf5d51dfed73cf5d631603a925001d504a59f2b/crates/passes/3d/helio-pass-tiny-voxel/REGIONAL_PUBLICATION_2026_09_26.md)
publishes ready regions with clipped ancestor payloads. A recorded 1,168-frame
720p flight made 488 partial publications and passed 367 primary/sunlight
validity captures. However, visual inspection exposed patch boundaries and
complete arrival refinement still took 3.38 seconds. It is rejected for default
adoption. Both this experiment and the canonical reference remain disabled in
the engine. The report preserves source snapshots, raw diagnostics, control
runs and open gates; these results do not qualify the terrain goal.
All nine native voxel integration tests passed with `daf5d51d`; all 9,387
regional evidence entries and the retained source snapshot verified from the
fresh submodule checkout.

Related CI repairs and their validation are recorded in
[task-pool startup validation](task-pool-startup-validation.md).

Pulsar's Flamegraph recorder supplies CPU/thread context and saves the Helio GPU
durations. The GPU events now retain the profiler instance, producing GPU frame, observing CPU
frame, readback lag and dropped/overflowed query counts in saved event metadata.
The profiler identity distinguishes restarted frame counters after graph rebuilds.
The frame bar is labelled **estimated GPU placement** because this bridge has
durations, not calibrated CPU/GPU clocks or original GPU scope offsets. It no
longer assigns a delayed GPU result to the CPU scope active during readback, or
adds the graph envelopes a second time as child passes. Pass durations are
packed for display; their visual order and CPU overlap are not measured facts.
Use the source-frame-labelled flight CSV for terrain comparisons and a native
GPU capture when exact execution scheduling or shader hardware counters are
needed. This change does not add terrain sub-stage scopes to the editor trace.
The final paired revision passed 21 renderer-focused tests, including delayed
GPU identity, reused frame numbers and metadata round-tripping through SQLite.
Helio's four profiling tests also passed, including real GPU timestamp queries.

The graph capture test runs with `HELIO_VOXEL_CAPTURE` set to an output PNG path:
`cargo test -p helio-default-graphs --test voxel_pass_graph` from the Helio
submodule. The capture validates pass ordering, attachment formats, and visible
terrain. It is not a whole-editor performance qualification.

See [the source API example](voxel-component-api-example.md) for batch
publication and snapshot export.
