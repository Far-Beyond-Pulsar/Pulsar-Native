# Voxel system

Pulsar's voxel terrain: destructible planets and planes of real voxels
(0.1 m to 1 m), authored as SceneDB components, edited in the level editor or
from scripts, and rendered by Helio's chunk pass. This document maps
the Pulsar side end to end: data, flow, editor, scripting, rendering
integration, decisions and pitfalls. The renderer itself (grid, generators,
chunk trees: residency, drawing, lighting) is documented in Helio's
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

## Layers

| Layer | Where | Owns |
|---|---|---|
| Components | Helio `helio-component` (`components/voxel_component.rs`) | `VoxelTerrainComponent` (the world's description, air included), generator settings components, `VoxelEditJournal` field, inspector metadata. |
| World queries | Helio `helio-component` (`components/voxel_world.rs`) | The CPU world of a terrain instance (cached), recipe/brush mapping, block API, scripting world methods, editor framing. |
| Canonical world | Helio `helio-pass-voxel-planet` | Grid, terrain generators, edits, `Planet` queries and ray casts, the render pass. |
| Scene projection | `engine_backend/src/scene/voxel_frame.rs` | Enabled SceneDB rows -> `VoxelSceneEntry` (configuration; no payload copies). `VoxelWorldSync`: journals and skies follow their world. |
| Ready-made worlds | `engine_backend/src/scene/voxel_presets.rs`, `assets/examples/voxel_*` | `VoxelWorldPreset` (Earth, Moon, Desert, Blocks): an example level's world added to a level in one step (`add_voxel_world`), taking over its sky, sun, haze, wind, ground cover and look. |
| Backend registry | `engine_backend/.../helio_renderer/voxel_backend.rs` | `VoxelRenderBackend` trait, `VoxelBackendRegistry`, the built-in `PlanetVoxelBackend` (`helio.voxel-terrain`). |
| Viewport renderer | `engine_backend/.../helio_renderer/renderer.rs` | Camera (planet-aware frame, altitude speed, ground collision), camera-relative frames, publishing views, brush and pick rays. |
| Editor | `ui_level_editor` (`state/voxel.rs`, `tool_modes/voxel_sculpt/`, `tool_modes/terrain/`, `ui/viewport/build_handlers.rs`, panel handlers) | Create Voxel Terrain (`SceneCommand::AddVoxelWorld`), sculpt tool, strokes and undo, focus, generator picker, settings components. |

## Data: components and the edit journal

**`VoxelTerrainComponent`** is the whole description of a voxel world, in
six inspector categories:

- **World:** enabled, shape (`Sphere`, `Plane`, `InfinitePlane`), planet
  radius or plane size, voxel size.
- **Generation:** generator (a `VoxelGeneratorRef` of id and version,
  serialized flat as `generator_id` / `generator_version`), seed.
- **Features:** what grows and lies on the ground (below).
- **Environment:** air (`VoxelAir`): `Custom` keeps the object's
  `AtmosphereComponent` and `GlobalFogComponent` as authored; `None` (a
  moon), `Earthlike`, `Thin` (Mars) and `Dense` (Titan) derive them, scaled
  by air density, sky tint and haze (see Sky and haze).
- **Appearance:** the terrain appearance JSON (below).
- **Editing:** editable, and the edit journal.

Levels saved with the old chunk-domain fields (renderer id, chunk layout,
LOD, palette, bounds) still load; saving drops them. Presets: `VoxelTerrainComponent::planet(radius)`, `::plane(size)`,
`::infinite_plane()`; a default component is a 4 km plane of the layered
terrain generator (`helio.terrain`). The world is centred on its object,
which must sit at the origin without rotation and with a uniform scale
(scale multiplies all sizes).

`Terrain appearance (JSON)` changes shading without rebuilding the world:
`palette` has 16 `[sRGB red, green, blue, roughness]` entries, `grass` has
three `[sRGB red, green, blue, 0]` entries (dry, meadow, lush), and `detail`
is `[patch contrast, pigment variation, edge shading, 0]`. Omitted fields
inherit defaults; an empty value restores them. The viewport resets colour history on changes.

**Features.** `features` is a list of spawns: what grows (growths of the
engine's flora: trees of a species, bushes, shrubs, cacti, boulders,
craters, each with its materials, sizes, seed salt and variant count) and
where (spacing, density, a noise field, ground materials, heights and how
they wander, relief, sink, a far form drawn as canopy past the near
levels), sizes in metres. Its JSON is Helio's
`helio_pass_voxel_planet::feature_recipe::FeatureRecipe`, field for field
(`VoxelFeatureSpawn` and `VoxelFeatureGrowth` are its reflected form for
the inspector's lists; a growth's card shows the fields its form reads,
material lists are edited as names or ids, "Grass, Dirt, 22"). The world
grows it with its seed in cells of its voxel size into the planet's
`FeatureSet` (`voxel_world::world_recipe`), so a level saves a few
kilobytes for forests to the horizon. Features are part of the ground: a
change rebuilds the world (a set the planet rejects, such as spawns out of
spacing order, keeps the last good world with the error), and edits made
before it are dropped. `FeatureRecipe::earth()`, `moon()` and `desert()`
are Helio's setups (`use_feature_preset`). Materials past the built-in
ones (the presets' needles, birch bark, sage and dry wood, ids 22 to 25)
get their look from the appearance JSON (`FeatureRecipe::appearance`).

**Generator settings** live in a separate component instance on the same
object, named by the generator (`VoxelTerrainLayersComponent` for Helio's
`helio.terrain`), found with `resolve_instance`. The projection serializes it
into the generator's parameters. Choosing a
generator in the inspector attaches its settings component. Settings changes
rebuild the world without recompiling shaders.

**Layer stacks.** `helio.terrain` builds every world from an ordered list of
layers: Warp, Continents, Mountains, Hills, Roughness, Erosion, Craters,
Basins and Plateau, each with a mask (everywhere, land, above deep sea), plus
caves, overhangs and a material style (Earthlike, Lunar, Layered). Whether a
sphere is a planet or a moon is the game's choice: the component's presets
(`earth`, `moon`, `flat`) are just data, and a game can build or randomize
its own stacks. Blueprints call `use_preset`, `add_layer`, `remove_layer`,
`layer_count` and `set_layer_enabled` / `height` / `scale` / `coverage` on
the settings component, and every setting of the world on the terrain
component (see [Scripting](#scripting)); the world rebuilds from the new
settings.

In the inspector the whole stack is one property with its own editor: a
preset picker, the layers and material rules as ordered lists (move, remove,
add), the caves, overhangs and materials, and the generator's validation
inline ("Not generated: ..."), so a preset or a reorder is one undoable edit.
A layer added, or switched to another kind, starts from that kind's
defaults (the generator's `Layer::new`), keeping only whether it is on and
its mask. While the stack is invalid the viewport keeps the last world that
built (the error says what to fix); it is not rebuilt every frame.

**The examples.** The projects `assets/examples/voxel_planet` (Earth),
`voxel_moon`, `voxel_desert` and `voxel_blocks` (each a `Pulsar.toml` and its
`scene/default.level`; open one with `just example voxel_planet`, or
`--project-path`) are Helio's voxel_flight setups
(`examples/voxel_flight/worlds.rs`) with seed 75, generically: layer stack,
features and their look, air, clouds, haze, and on Earth the engine meadow
(six `FoliageComponent`s on a Meadow object, a `WindComponent` of 4.5 m/s).
The editor spawns over the north pole under a sun 32 degrees high. An
unbound `PostProcessVolumeComponent` gives voxel_flight's look: AgX punchy,
exposure 2 (3.5 on the Moon, sunlit regolith under a black sky), contrast
1.06. `voxel_blocks` is Earth's world on flat ground without edges (a
flat layer stack, `InfinitePlane`). The `voxel_example_levels` test loads
each and compares it with voxel_flight's. These levels are the editor's
ready-made worlds (Create Voxel Terrain). voxel_flight writes a preset's layers, features and look
as a level holds them (`HELIO_VOXEL_FLIGHT_EXPORT=<dir>`).

**Random worlds.** `assets/examples/random_world.blueprint.json` is a
Blueprint that builds a planet, a moon or a desert world from its `seed`
variable, No Man's Sky style: it rolls with the deterministic `Seeded
Random` nodes (`seeded_random`, `seeded_random_range`, `seeded_random_int`:
equal seed and index, equal number, on every run and machine), applies a
preset and the world's air (none on a moon, thin on a desert), then adds a
crater layer and hills of random size through the components' natives. `pulsar_game`'s `random_world_blueprint` test compiles
the saved graph and runs it against a real terrain instance.

**Composition.** Game-specific worlds are classes whose prefab combines these
components with others (a planet with water and foliage components, say);
the voxel components know nothing about them.

**`edits: VoxelEditJournal`** is the ordered list of brushes (sphere or cube,
remove / add / paint, material, centre in world metres). It is the single
source of truth for destruction and construction: the sculpt tool, scripts
and the legacy sample methods all append to it, and saving the level saves
it (`{"terrain": .., "edits": [..]}`). Internally it is chunked and
prefix-hashed, so copying it is nearly free and "equal" / "only grew" are
O(1) checks. That matters because the projection copies it every frame.

Edits belong to the ground they were made on: the journal records that
terrain's fingerprint (`voxel_world::ground_fingerprint`: form, voxel size,
generator, seed, settings and the features' recipe). When any of these
change, the projection shows none of the old edits and the scene step drops
them from the journal (`VoxelWorldSync`: change cursors over the terrain,
layer, atmosphere and fog components, so frames that change none of them
cost nothing), so they are not saved either. A journal with no terrain yet
adopts the first one.

**Sky and haze.** The same sync keeps an `AtmosphereComponent` and a
`GlobalFogComponent` on a world's object on its ground: the sky centred on
a sphere at its radius, flat ground for a plane; the fog's heights over the
sphere, along world Y on a plane. A world whose air is a preset derives the
rest of both from it (`AtmosphereComponent::for_air`,
`GlobalFogComponent::for_air`: a ground haze thinning with altitude, its
extinction times air density and haze, Earth's the one voxel_flight flies
through), so hand edits to them revert; `Custom` air leaves them as
authored, and `None` turns both off. A level has the haze only if the
object has a `GlobalFogComponent`. The clouds are the atmosphere's (its
Clouds category: layer heights, coverage, type, extinction, weather,
billows, wisps, wind, albedo): every air keeps them as authored, so a
world's clouds are edited on its sky.

**Meadows.** The engine's foliage grows on the voxel ground the chunk pass
draws (the default graph links `ChunkPass::foliage_ground` to the placement
pass). A `FoliageComponent` is one foliage type: on voxel ground its Kind
`Block` draws stacks of cubes (Width min the cube), Grows on names the
terrain materials it grows on, Tufts gathers blades (count, lean at the
centre and rim, curve, fall, flower tips, patches) and Ground colour tints
each tuft by the turf. The level's `WindComponent` moves them. How much
foliage the renderer holds is the project's (Rendering > Foliage: range,
max density in tufts a square metre, blades a tuft); the Earth meadow
wants 200 m, 16 and 10 (the defaults thin it to 8 tufts a square metre
over 160 m).

**Sculpting.** A brush sample asks the renderer for the terrain hit under
the pointer (`request_pick`); when the answer arrives (a few frames later)
the exact edit is found by walking the base grid only a few cells around it,
wherever the terrain is. The walk used to start at the eye: aimed at a
mountain 20 km away it took seconds per sample, near the horizon tens of
seconds (the editor froze while sculpting). Without a renderer hit the walk
reaches at most 500 m.

## Flow of a frame

1. **Projection.** `project_voxel_entries(world)` maps enabled terrain
   instances to `VoxelSceneEntry`: id, visibility, shape and size, voxel size,
   generator config (id, version, seed, settings JSON, features recipe) and
  the edit journal. Nothing large is copied. The renderer reads it once per
   world revision (`VoxelSceneRead`), with the sun and what keeps the scene
   out of camera-relative frames.
2. **Environment.** The registry reports whether any active backend renders
   camera-relative, the ground's local vertical (`local_up`), the camera's
   `altitude` and a certified clip range. The renderer sets the frame's world
   origin from these when the scene allows it (every authored component and
   GPU source is reviewed for a moved origin; anything unknown keeps world
   coordinates and logs `VOXEL_CAMERA_SPACE`). The sky and the light the air
   casts come from an `AtmosphereComponent` through the environment join.
3. **Publish.** `publish_frame(entries, VoxelView)` hands the backend the
   f64 eye and the sun; it draws with the frame's own camera. `PlanetVoxelBackend` resolves
   the entry to a cached `Arc<Planet>`. Only appended edits extend the cached
   world in place, while other changes rebuild it. It then posts a
   `PlanetFrame` to its pass's mailbox.
4. **Render.** Helio's deferred graph runs the backend's pass in the GBuffer
   stage (see the Helio README for what happens inside). The viewport keeps
   rendering while `needs_frame` reports streaming work, even if the camera
   is still.

## Backends

`VoxelRenderBackend` is the contract a renderer implements. A row is drawn
by the one backend that `supports` it: a world by the renderer of its
generator; ambiguity is an error. Methods and why they exist:

| Method | Purpose |
|---|---|
| `renderer_id`, `supports` | Registration, selection. |
| `pass_factory`, `publish_frame`, `needs_frame` | Graph pass, per-frame view, streaming keep-alive. |
| `camera_relative_frames` | The backend wants camera-local GPU coordinates (planetary precision). |
| `local_up` | The local vertical (the planet's radial): the camera frame's up. |
| `configure_appearance` | Terrain appearance (palette, grass, detail) without rebuilding the world. |
| `camera_clip_range` | Near/far from certified empty space (near up to 50 km in orbit, 5 cm on the ground). |
| `altitude` | Height above the ground below the eye (camera speed). |
| `lift_out_of_ground` | Where to put an eye that entered solid voxels (dug air is not solid). |
| `edit_ray` | Exact f64 ray cast for brushes and single blocks, returning the brush to commit. |
| `temporal_quality` | TSR quality per viewport size. |
| `diagnostics` | One-line streaming state for logs (`PULSAR_VOXEL_STATS`). |

The renderer registers the built-in backend.

## Editor

- **Create.** Tool menu -> **Terrain**, World tab, **Create Voxel Terrain**:
  an Earth-like planet, the Moon, a desert planet or a flat block world
  (`voxel_presets`). One click adds a working world, one undo step: the
  terrain with its layers, features and look, the sky with clouds, the sun,
  the haze, the meadow and its wind, an unbound post-process look, and the
  camera over the ground. A level keeps one sky, wind and directional light:
  the world's replace the level's (its sun and look update the existing
  light and unbound volume), and its terrain replaces an existing one on
  that terrain's object. The World tab lists the level's terrains (a click
  selects one for the inspector) and switches to the sculpt tool.
- **Sculpt.** Tool menu -> **Voxel Sculpt**. Its dock panel picks dig /
  build / paint / flatten / smooth, sphere or cube, radius, material and
  **One block**; Shift swaps dig and build. A press starts a stroke. While the pointer drags, stamps are
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
| `world_shape()`, `set_world_shape(shape)` | `"sphere"`, `"plane"` or `"infinite_plane"`. |
| `planet_radius()`, `set_planet_radius(m)`; `plane_size()`, `set_plane_size(m)` | The world's size. |
| `set_voxel_size(m)` | The base voxel edge (0.1 to 1 m). |
| `seed()`, `set_seed(seed)` | The generator's seed. |
| `use_feature_preset(name)`, `feature_spawn_count()` | Features: `"earth"`, `"moon"`, `"desert"` or `"none"`. |
| `air()`, `set_air(name)` | `"custom"`, `"none"`, `"earthlike"`, `"thin"` or `"dense"`. |
| `air_density()`, `air_haze()`, `air_tint_red/green/blue()` and their `set_*` | How much air, its haze (the sky's aerosols and the ground haze) and the sky's tint. |
| `is_editable()`, `set_editable(on)` | Whether the world can be dug and built. |

Setters refuse invalid values (a negative radius, an unknown air) and leave
the world unchanged.

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
  follows the local vertical (`local_up`), carried along by the smallest
  rotation as it moves. Yaw and pitch are relative to it, so the horizon stays
  level anywhere on a planet. W/S move level, Q/E move along the vertical.
  View, movement, pan, picking, gizmos and brush rays all use the same
  basis. Saved and focus poses keep world-space yaw/pitch.
- **Speed** is the viewport speed x `clamp(altitude / 20 m, 1, 1e6)`: walking
  speed near the ground, orbit in seconds.
- **Ground collision.** A move that would end inside solid voxels keeps
  only the axes of the move that stay in air, so the camera slides along
  cave walls, dug tunnels and the ground. Only an eye that is already buried
  (spawned or teleported into rock) is lifted 0.5 m above the surface:
  lifting on contact threw the camera out of caves.

## Decisions and pitfalls

- **Camera-relative frames and world-space things.** Anything placed in
  world coordinates must be rebased by the world origin. The editor grid used
  not to be: it drew world y = 0 through the eye and, pressed against the
  ground with a 5 cm near plane, painted the whole view in the X axis' red.
  Helio's grid and billboards (light icons) are rebased; **gizmos and
  authored meshes in a voxel world are not yet**. Meshes near a planet's
  surface also need f64 or tiled transforms (f32 at 6371 km is ~0.5 m).
- **Depth is reversed with no far plane.** Depth 0 is infinity, which
  unprojects to w = 0. Build rays toward it as `far.xyz - far.w * eye`,
  never `far.xyz / far.w`, and read depth through the prelude
  (`helio_view_depth`, `helio_is_sky`, `helio_depth_nearest`).
- **Altitude, not clearance, for speed.** `air_clearance` is a conservative
  bound (0 anywhere below the highest possible mountain), right for near
  planes but it made the camera crawl at 10 m/s kilometres above lowland.
- **Edits are a journal, not voxel data.** The terrain is procedural, so a
  world is its recipe plus ordered brushes; that keeps saves small and CPU
  queries exact, and lets the GPU regenerate any chunk at any level. An
  edit's chunks regenerate drawn first and nearest first within the frame's
  generation budget (milliseconds against the measured cost of a footprint's
  columns and of a chunk): a dig shows the frame it is made, a 500 m blast
  settles over a few seconds. The chunks form an octree: rock and air a
  brush leaves whole are Solid and Empty nodes that take no chunk slots, so
  deep halls and hollowed cores render exactly.
- **The editor's default Sun points straight down (world -Y),** so the sun
  is overhead at the pole and lower elsewhere. A planet's air is an authored
  `AtmosphereComponent` placed at its owner (the example level's planet,
  with Earthlike air): the atmosphere pass rebases its centre by the frame's
  world origin, so the horizon and aerial perspective follow the planet at
  any distance. Without one, or with `None` air, there is no sky (space, a
  moon).

## Diagnostics and tests

- `PULSAR_VOXEL_STATS=1` logs, twice a second to the engine log
  (`%APPDATA%/Pulsar/Pulsar_Engine/data/logs/<time>/engine.log`), the
  camera altitude, speed scale, eye/forward/up, viewport and each backend's
  `diagnostics` line. The chunk pass reports whether residency `settled`,
  drawn nodes and chunks, resident chunks, `fading` level switches,
  `waiting` nodes the plan wants, `footprints` and `chunks` generated this
  frame, `seen` (nodes rays reported) and `reached` (Buried nodes rays met,
  generated next), the frame's chunk `budget`, `coarse` (the share of
  drawn nodes coarser than wanted), `out_of_slots` / `out_of_leaves`, CPU
  milliseconds per update phase and the last GPU draw, shadow and
  generation times. `waiting` that stays above 0 while the camera is
  still, or `coarse` near 1 while moving, point at generation not keeping
  up.
- Slow frames log `[HELIO FRAME SPIKE]` with CPU/GPU splits.
- The Flamegraph profiler (status bar) records scopes including
  `voxel_brush`, `voxel_altitude`, `voxel_project_entries`; stop recordings
  within ~30 s (it stops saving at 1M events).
- Tests: `cargo test -p engine_backend --test voxel_block_api` (block API,
  set_block, fills and ray casts, legacy sample edits, world cache, origin,
  generator picker), `--test voxel_component_schema` (inspector properties),
  `cargo test -p engine_backend --lib voxel` (backend: altitude, lift out of
  ground, stroke fill, settings components, unknown generator, paint and
  single blocks), `--lib voxel_presets` (a ready-made world in an empty level
  is its example; a second one takes over the first) and
  `--lib camera_frame_tests` (level horizon, round trips,
  smooth transport over the planet); `ui_level_editor` sculpt stroke tests
  (a stroke is one undo step) and the Create Voxel Terrain command (one undo
  step).
- Renderer-side performance and correctness are measured in Helio
  (`voxel_flight`: `HELIO_VOXEL_FLIGHT_HOLD=<view>` holds a view and times
  it, `HELIO_VOXEL_CHUNKS_ENGINE=1` through the chunk pass as the editor
  draws it, `HELIO_VOXEL_CHUNKS_FLY=descent|cruise` streams along a path),
  and `tests/chunks.rs` checks drawn hits against CPU ray casts.
- The editor's own renderer, driven headless:
  `cargo run --release -p engine_backend --example voxel_editor_harness`
  (views, flights, drag strokes, stress flights with a per-frame CSV,
  per-pass GPU profiles, project settings with `set:`, a ready-made world
  with `world:<preset>`; its module doc lists the steps). Its `view` prints the eye and sun, and voxel_flight renders
  the same view (`HELIO_VOXEL_FLIGHT_HOLD=eye`, `_EYE`, `_SUN`, `_LAYERS`,
  `_SEED`, `HELIO_VOXEL_CHUNKS_ENGINE=1`) for parity.
- `cargo test -p engine_backend --test voxel_example_levels`: the example
  levels (the ready-made worlds) against voxel_flight's worlds
  (`examples/voxel_flight/worlds.rs`, compiled into the test): ground,
  features, air, haze, clouds, meadow, wind, exposure, and a camera over the
  ground.

## Extending

- **A terrain generator**: implement and register it in Helio (see its
  README), give it a settings component registered in
  `voxel_component_runtime.rs`, and name that component in its info so the
  picker attaches it.
- **A voxel renderer**: implement `VoxelRenderBackend` and add it to the
  registry the renderer builds (`VoxelBackendRegistry::register`); it draws
  the worlds it `supports`.
- **A ready-made world**: add an example project under `assets/examples`
  with the world in its level, a `VoxelWorldPreset` naming it, a check in
  `voxel_example_levels`, and its button in the Terrain panel.
- **A tool**: return `ToolPointerResult::VoxelBrush` requests like
  `tool_modes/voxel_sculpt/`; the viewport queues them, commits come back
  from `edit_ray` and are appended to the journal with the scene history, so
  undo works.
