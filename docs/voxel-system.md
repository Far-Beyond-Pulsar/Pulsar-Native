# Voxel system

Pulsar's voxel terrain: destructible planets and planes of real voxels
(0.1 m to 1 m), authored as SceneDB components, edited in the level editor or
from scripts, and rendered by a registered Helio backend. This document maps
the Pulsar side end to end: data, flow, editor, scripting, rendering
integration, decisions and pitfalls. The renderer itself (grid, generators,
residency, tracing) is documented in Helio's
`crates/passes/3d/helio-pass-voxel-planet/README.md`
(`crates/renderer/helio/...` in this repository).

## Contents

1. [Layers](#layers)
2. [Data: components and the edit journal](#data-components-and-the-edit-journal)
3. [Flow of a frame](#flow-of-a-frame)
4. [Backends](#backends)
5. [Editor](#editor)
6. [Scripting](#scripting)
7. [Camera and coordinates](#camera-and-coordinates)
8. [Decisions and pitfalls](#decisions-and-pitfalls)
9. [Diagnostics and tests](#diagnostics-and-tests)
10. [Extending](#extending)
11. [Generic voxel data boundary](#generic-voxel-data-boundary)

## Layers

| Layer | Where | Owns |
|---|---|---|
| Components | Helio `helio-component` (`components/voxel_component.rs`) | `VoxelTerrainComponent`, generator settings components, `VoxelEditJournal` field, inspector metadata. |
| World queries | Helio `helio-component` (`components/voxel_world.rs`) | The CPU world of a terrain entity (cached), recipe/brush mapping, block API, scripting world methods, editor framing. |
| Canonical world | Helio `helio-pass-voxel-planet` | Grid, terrain generators, edits, `Planet` queries and ray casts, the render pass. |
| Scene projection | `engine_backend/src/scene/voxel_frame.rs` | Enabled SceneDB rows -> `VoxelSceneEntry` (configuration + shared capabilities, no payload copies). |
| Backend registry | `engine_backend/.../helio_renderer/voxel_backend.rs` | `VoxelRenderBackend` trait, `VoxelBackendRegistry`, the built-in `PlanetVoxelBackend` (`helio.voxel-terrain`). |
| Viewport renderer | `engine_backend/.../helio_renderer/renderer.rs` | Camera (planet-aware frame, altitude speed, ground collision), camera-relative frames, publishing views, brush and pick rays. |
| Editor | `ui_level_editor` (`state/voxel.rs`, `tool_modes/voxel_sculpt.rs`, dispatcher, panel handlers) | Sculpt tool, strokes and undo, focus, generator picker, settings components. |

## Data: components and the edit journal

**`VoxelTerrainComponent`** is the base of every voxel world. Inspector
properties: enabled, shape (`Sphere`, `Plane`, `InfinitePlane`), planet
radius or plane size, voxel size, generator (a `VoxelGeneratorRef` of id and
version, serialized flat as `generator_id` / `generator_version`), seed,
editable. The rest (renderer id, chunk layout, LOD, palette) stays serialized
but hidden. Presets: `VoxelTerrainComponent::planet(radius)`, `::plane(size)`,
`::infinite_plane()`; a default component is a 4 km plane of the landform
generator. The world is centred on its entity, which must sit at the origin
without rotation and with a uniform scale (scale multiplies all sizes).

`Terrain appearance (JSON)` changes shading without rebuilding the world:
`palette` has 16 `[sRGB red, green, blue, roughness]` entries, `grass` has
three `[sRGB red, green, blue, 0]` entries (dry, meadow, lush), and `detail`
is `[patch contrast, pigment variation, edge shading, 0]`. Omitted fields
inherit defaults; an empty value restores them. The viewport resets colour history on changes.

**Generator settings** live in a separate component on the same entity, named
by the generator (`VoxelLandformComponent` for `helio.landform`,
`VoxelFlatTerrainComponent` for `helio.flat`). The projection serializes it
into the generator's parameters. Choosing a generator in the inspector
attaches its settings component. Settings changes rebuild the world without
recompiling shaders.

**Composition.** Game-specific worlds are classes whose prefab combines these
components with others (a planet with water and foliage components, say);
the voxel components know nothing about them.

**`edits: VoxelEditJournal`** is the ordered list of brushes (sphere or cube,
remove / add / paint, material, centre in world metres). It is the single
source of truth for destruction and construction: the sculpt tool, scripts
and the legacy sample methods all append to it, and saving the level saves
it (as a plain list). Internally it is chunked and prefix-hashed, so copying
it is nearly free and "equal" / "only grew" are O(1) checks. That matters
because the projection copies it every frame.

## Flow of a frame

1. **Projection.** `project_voxel_entries(world)` maps enabled terrain rows
   to `VoxelSceneEntry`: id, visibility, shape and size, voxel size,
   generator config (id, version, seed, settings JSON), the edit journal and
   shared stores. Nothing large is copied.
2. **Environment.** The registry reports whether any active backend renders
   camera-relative and owns the outdoor sky, the ground's local vertical
   (`ambient_up`), the camera's `altitude` and a certified clip range. The
   renderer sets the frame's world origin, hemisphere ambient and fallback
   sky from these.
3. **Publish.** `publish_frame(entries, VoxelView)` hands the backend the
   f64 eye, camera basis, projection and sun. `PlanetVoxelBackend` resolves
   the entry to a cached `Arc<Planet>`. Only appended edits extend the cached
   world in place, while other changes rebuild it. It then posts a
   `PlanetFrame` to its pass's mailbox.
4. **Render.** Helio's deferred graph runs the backend's pass in the GBuffer
   stage (see the Helio README for what happens inside). The viewport keeps
   rendering while `needs_frame` reports streaming work, even if the camera
   is still.

## Backends

`VoxelRenderBackend` is the contract a renderer implements; a row names its
backend by `renderer_id` (or leaves it empty for unique capability matching
via `supports`; ambiguity is an error). Methods and why they exist:

| Method | Purpose |
|---|---|
| `renderer_id`, `supports` | Selection. |
| `pass_factory`, `publish_frame`, `needs_frame` | Graph pass, per-frame view, streaming keep-alive. |
| `camera_relative` | The backend wants camera-local GPU coordinates (planetary precision). |
| `outdoor_sky`, `ambient_up` | Sky ownership and the hemisphere-ambient axis (the planet's radial). |
| `camera_clip_range` | Near/far from certified empty space (near up to 50 km in orbit, 5 cm on the ground). |
| `altitude` | Height above the ground below the eye (camera speed). |
| `lift_out_of_ground` | Where to put an eye that entered solid voxels (dug air is not solid). |
| `edit_ray` | Exact f64 ray cast for brushes and single blocks, returning the brush to commit. |
| `temporal_quality` | TSR quality per viewport size. |
| `diagnostics` | One-line streaming state for logs (`PULSAR_VOXEL_STATS`). |

Register extra backends with `HelioRenderer::register_voxel_backend` before
the first frame.

## Editor

- **Sculpt.** Tool menu -> **Voxel Sculpt**. The toolbar picks dig / build /
  paint, sphere or cube, radius, material and **One block**; Shift swaps dig
  and build. A press starts a stroke. While the pointer drags, stamps are
  filled in between the previous and current hit (`stroke_fill`), so fast
  drags leave no gaps. Release ends the stroke. A whole stroke is one undo
  step (Ctrl+Z, Ctrl+Shift+Z or Ctrl+Y), settled 150 ms after the last stamp.
- **Focus.** **F** frames the selected terrain from 30 m above the ground
  under the camera (other objects: 8 m back along the view).
- **Inspector.** Readable labels; **Generator** is a searchable picker of the
  registered generators (a generator from an unloaded plugin stays selected
  and is reported).
- **Camera.** See below. The viewport's **Speed** setting multiplies the
  altitude speed.

## Scripting

World methods on `VoxelTerrainComponent`, for blueprints and scripts,
positions in world metres, material ids from `terrain::material` (0 is air):

| Method | Result |
|---|---|
| `get_block(x, y, z)` | Material of the block containing the point. |
| `set_block(x, y, z, material)` | Makes that block `material` (0 removes it). |
| `fill_sphere(x, y, z, radius, material)` | Fills or clears every block within `radius`. |
| `fill_cube(x, y, z, half_size, material)` | The same for a ground-aligned cube. |
| `raycast_distance(x, y, z, dx, dy, dz, max_distance)` | Distance to the first solid block, or -1. |
| `voxel_size()` | Block edge in metres. |

Script edits go into the same journal as the sculpt tool, so renderer, editor
and gameplay agree on every block. On generated terrain the older
`paint_sample` / `erase_sample` address the block at
`(sample + 0.5) * voxel_size`.

## Camera and coordinates

- **World space is f64.** The editor camera position, voxel queries and
  edits are f64 world metres. GPU frames over voxel worlds are
  camera-relative. The renderer's world origin is the eye (published through
  `Renderer::set_world_origin` and on to passes as
  `PrepareContext::world_origin`).
- **Planet-aware camera frame.** The camera keeps a reference frame whose up
  follows the local vertical (`ambient_up`), carried along by the smallest
  rotation as it moves. Yaw and pitch are relative to it, so the horizon stays
  level anywhere on a planet. W/S move level, Q/E move along the vertical.
  View, movement, pan, picking, gizmos and brush rays all use the same
  basis. Saved and focus poses keep world-space yaw/pitch.
- **Speed** is the viewport speed x `clamp(altitude / 20 m, 1, 1e6)`: walking
  speed near the ground, orbit in seconds.
- **Ground collision.** After moving, an eye inside solid voxels is lifted
  0.5 m above the surface (dug tunnels are air and can be entered).

## Decisions and pitfalls

- **Camera-relative frames and world-space things.** Anything placed in
  world coordinates must be rebased by the world origin. The editor grid used
  not to be: it drew world y = 0 through the eye and, pressed against the
  ground with a 5 cm near plane, painted the whole view in the X axis' red.
  Helio's grid and billboards (light icons) are rebased; **gizmos and
  authored meshes in a voxel world are not yet**. Meshes near a planet's
  surface also need f64 or tiled transforms (f32 at 6371 km is ~0.5 m).
- **Far plane at f32 infinity.** With near 5 cm and far 40 000 km the far
  plane unprojects to w = 0. Build rays toward it as `far.xyz - far.w * eye`,
  never `far.xyz / far.w`. Clouds, TSR, fog, lens flare and shadow cascades
  were fixed for this.
- **Altitude, not clearance, for speed.** `air_clearance` is a conservative
  bound (0 anywhere below the highest possible mountain), right for near
  planes but it made the camera crawl at 10 m/s kilometres above lowland.
- **Edits are a journal, not voxel data.** The terrain is procedural, so a
  world is its recipe plus ordered brushes; that keeps saves small and CPU
  queries exact, and lets the GPU regenerate any column at any level.
- **The editor's default Sun points straight down (world -Y),** so the sun
  is overhead at the pole and lower elsewhere. The planetary fallback sky
  receives the f64 world eye, scaled radius and scene Sun from the backend;
  its atmosphere follows the radial horizon, with dim diffuse lighting on
  the night side. Authored skies take precedence.

## Diagnostics and tests

- `PULSAR_VOXEL_STATS=1` logs, twice a second to the engine log
  (`%APPDATA%/Pulsar/Pulsar_Engine/data/logs/<time>/engine.log`), the
  camera altitude, speed scale, eye/forward/up, viewport and each backend's
  `diagnostics` line
  (resident / pending columns, jobs, levels, residency CPU times, pool
  `free_units` and `recycles`, and `lod_pressure`: above 1 the viewport
  wants more columns than the record or pool capacity holds, so Helio draws
  slightly coarser levels instead of stalling). `finest`
  is the finest active level: from high up it is above 0 by design (fine
  levels switch on only where local terrain can come near). Pending that
  stays high while the camera is still, or `jobs=63` frames while moving,
  point at residency starvation.
- Slow frames log `[HELIO FRAME SPIKE]` with CPU/GPU splits.
- The Flamegraph profiler (status bar) records scopes including
  `voxel_brush`, `voxel_altitude`, `voxel_project_entries`; stop recordings
  within ~30 s (it stops saving at 1M events).
- Tests: `cargo test -p engine_backend --test voxel_block_api` (block API,
  set_block, fills and ray casts, legacy sample edits, world cache, origin,
  generator picker), `--test voxel_component_schema` (inspector properties),
  `cargo test -p engine_backend --lib voxel` (backend: altitude, lift out of
  ground, stroke fill, settings components, unknown generator, paint and
  single blocks) and `--lib camera_frame_tests` (level horizon, round trips,
  smooth transport over the planet); `ui_level_editor` sculpt stroke tests
  (a stroke is one undo step).
- Renderer-side performance and correctness are measured in Helio
  (`voxel_flight`; `HELIO_VOXEL_FLIGHT_TRIP` replays an editor trip,
  `HELIO_VOXEL_FLIGHT_REPLAY=<engine.log>` replays the logged camera pose of
  a session logged with `PULSAR_VOXEL_STATS=1`, and
  `HELIO_VOXEL_FLIGHT_CRUISE=<m>` flies level at the editor's speed).

- `PULSAR_VOXEL_NATIVE_FLIGHT=1` runs a 27 s ascent/orbit/descent/cruise/arrival
  diagnostic after residency settles and cancels on camera input. Use a copied
  project with `PULSAR_VOXEL_STATS=1`. Helio's `HELIO_VOXEL_FLIGHT_LONG=<s>`
  is the sustained-travel equivalent; run it at the editor's resolution.

## Extending

- **A terrain generator**: implement and register it in Helio (see its
  README), give it a settings component registered in
  `voxel_component_runtime.rs`, and name that component in its info so the
  picker attaches it.
- **A voxel renderer**: implement `VoxelRenderBackend`, register it before
  the first frame, and select it by `renderer_id` on the component.
- **A tool**: dispatch `VoxelBrushRequest`s through the editor dispatcher
  like `tool_modes/voxel_sculpt.rs`; commits come back from `edit_ray` and
  are appended to the journal with the scene history, so undo works.

## Generic voxel data boundary

SceneDB also owns generic voxel sample data for bounded objects
(`VoxelComponent`) and external terrain sources. `helio-voxel-data` is the
CPU-only contract: signed chunk keys, 8^3 material samples, bounded
publication batches, revision checks, immutable snapshots, editing and
generator worker APIs. `VoxelSourceWriter` publishes to a component-owned
store; `VoxelSourceSession` (`scene/voxel_source.rs`) resolves a typed
SceneDB row and owns its publication and edit workers. Component
serialization contains authored configuration, not live payload bytes. The
`helio.voxel-terrain` backend does not consume live chunk payloads; it
reports an error rather than ignoring them. See
[the source API example](voxel-component-api-example.md).
