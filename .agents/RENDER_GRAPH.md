# Render pipelines and render graphs

A level chooses its **render pipeline** (World Settings, first section) by picking
an **asset**: the editor's shared asset picker lists the project's `.rendergraph`
files and, beside them, the ones plugins provide in their folders (the shipped
pipelines are the `pulsar.default_pipelines` plugin's files,
`plugins/pulsar.default_pipelines/Deferred.rendergraph` and the rest, with the
plugin named as their origin). A pipeline is anything that implements Helio's
`IntoGraph`. A render graph is a graph of **pass nodes**: one node builds one pass,
and the node's pins come from what that pass declares.

**Pipelines are not implicit.** There is no engine default. A new level has none
(`None` is a valid choice), a plugin either hardcodes the pipeline it needs or
leaves the choice to the level, and a renderer with no pipeline (none assigned, the
id not registered, the graph not compiling) draws Helio's status screen with the
reason instead of substituting another pipeline.

## Layers

| Layer | Where | What |
|---|---|---|
| `IntoGraph`, `RenderPipelineRegistry` | `helio` (`renderer/pipeline.rs`) | The extension seam. `RendererBuilder::with_graph(impl IntoGraph)` / `with_boxed_graph`. |
| Graph asset + compiler | `helio-graph-asset` | `GraphAsset` (`.rendergraph` JSON), `NodeRegistry`, `validate`, `compile`, `GraphAssetPipeline` (an `IntoGraph`), `probe`. Knows no pass. |
| Pass nodes | each pass crate's feature, `helio-spine`, `helio-presets` (`features.rs`) | One node per pass, registered by the feature that owns the pass (`registrar.node`); pins generated. |
| Presets, default graphs | `helio-presets` (`presets.rs`, `graphs.rs`) | `Preset::nodes`: the frame's structure as node lists over its slots and phases, the same for every renderer config (the anti-aliasing node follows the project's TSR setting when the graph builds). `graph_asset` / `deferred_graph_asset`: the same list as a laid-out, sectioned `GraphAsset`. Helio registers no pipeline and has no default. |
| Shipped pipelines | `plugins/vendor/default_pipelines` (`pulsar.default_pipelines`) | The pipelines the engine ships, as `.rendergraph` files in the plugin's folder, made from the presets for the enabled plugins; the migration of Helio's old built-in ids to them. |
| Engine registry | `engine_backend::…::helio_renderer::pipelines` | Process-wide pipeline and node registries; the pin probe (on the engine's device); the open project's `.rendergraph` registration (plugin folders included) and its watcher; the hidden `preview:N` pipelines; `PipelineIdMigration`. |
| Selection | level `world_settings.render_pipeline` (+ `pipeline_version`) | An asset picked in World Settings: a project-relative `.rendergraph` path (a plugin's folder included); `None` is no pipeline. Editor: poller pushes it to every viewport mailbox, which rebuilds the renderer at a frame boundary (also when the asset is saved again: `pipelines::PipelineState::Assigned.revision`). Game: `load_level` records it, `game_renderer` builds from it. |
| Editor | `plugins/vendor/render_bps` | The render graph editor on the Graphy canvas (`.agents/GRAPH_CANVAS.md`), plus a live preview. |
| Trait | `pulsar_traits::builtin` | `pulsar.IntoGraph`, provided into every project at `plugins/pulsar/traits/into_graph.trait.json` (the engine's mount). |

## Pass nodes

A pass node registers a constructor and nothing else (`graph_nodes::pass_node`):
`Enabled` parameter, generated pins, and optional parameters where the pass has
a choice (TSR quality, a colour input). The constructors are the old builders
taken apart. Cross-pass handles come from the graph (`find_pass`) or typed shared
state, and a node errors by name when what it needs has not been built
(`Occlusion Cull needs the Hi-Z Build node…`).

- **Every node builds.** A graph is ordered by its wires (ties by authoring order);
  nothing has to lead to the Output, which only ends the pipeline. To remove a
  pass, delete its node or set **Enabled** off; a node that reads from a disabled
  one is warned.
- **Optional passes are just nodes** (SSR, planar reflections, portals, foliage,
  water, TSR vs FXAA). In the graph means built: project graphics settings no longer
  switch them for graph pipelines. Resolution is still the renderer's: the internal
  resolution follows the project's TSR setting, so use the TSR node with TSR on.
- **Performance markers are not nodes.** A pass registered `with_marker_after()`
  gets the profiler's marker from the compiler, using the registry's marker.
- **Host inputs** (`host.<name>`) declare what the renderer supplies each frame.
  **SceneDB buffers** (`scenedb.<key>`, titled `SceneDB::GPU::<key>`) are one node
  per buffer. A pass that consumes one has an input for it.

## Plugin nodes

A plugin's pass is a node of its feature, not a registry call of its own. The
plugin's root submits `RenderFeatureRegistration` (`origin: pulsar_plugins::origin!()`,
`features`, `configure`); the feature's `Feature::register` calls `registrar.node(
NodeDescriptor)` with kind `helio.pass.<name>` (`pass_node` shape: `enabled`
parameter, `with_generated_pins`, `.in_slot(..)` or `.drawer(Phase, order)`).
`engine_backend::..::pipelines::graph_node_registry()` is the core nodes, Helio's
default features and `plugin_features()` (`pulsar_plugins::registered`, so only
enabled plugins). It is made again when `pulsar_plugins::generation()` changes
(the project's plugin set), as are the default pipelines plugin's files and the pin
probe.

- The probe builds nodes with `render_features::configure(config)`, the plugins'
  settings, so a node that builds nothing without its feature's settings has the
  pins of the pass it adds.
- A disabled plugin's nodes are unregistered and recorded absent
  (`NodeRegistry::mark_absent`, `render_features::disabled_feature_nodes`): a graph
  naming one fails validation with `missing node `<kind>` (plugin <id> disabled)`,
  and `GraphAssetPipeline` falls back as for any graph that does not compile.
- Shared state between a plugin's nodes and others goes through `BuildEnv::shared`
  and the registry's finishers (voxel offers its ground to foliage placement that way),
  not `find_pass`.
- Test: `plugins/vendor/voxel/tests/graph_node.rs`. Every plugin below has the same
  `tests/graph_node.rs`: each node registered with its feature, generated pins and
  `enabled`, the plugin's node kinds in the default pipeline's order (`Preset::nodes`),
  the compiled graph's passes in order (and none with every node `enabled` off), the
  probed pins, and the `missing node ... (plugin <id> disabled)` message. The GPU
  parts skip with no adapter.

Lights, sky, fog, water, foliage and particles (`<kind>` is `helio.pass.<name>`; slots
are the frame's, `Phase` drawers and `before_phase` compute as placed by the feature):

| Plugin | Node (kind) | Title | Default placement |
|---|---|---|---|
| lights | `shadow_matrix` | Shadows: Light Matrices | before `ShadowCasters`, -100 |
| lights | `light_cull` | Lighting: Tile Light Binning | `Visibility` 100 (nothing under light-field) |
| lights | `deferred_light` | Lighting: Deferred | `lighting.model.deferred` point |
| lights | `hlfs` | Lighting: HLFS | `lighting.model.light_field` point |
| lights | `billboard` | Overlay: Billboards | `Overlay` drawer 50 |
| sky | `atmosphere` | Sky: Atmosphere Tables | `Prepare` 0 |
| sky | `cloud_trace` | Sky: Cloud Trace | `LightingInputs` 10 |
| sky | `atmosphere_composite` | Sky: Composite | `Lighting` 0 |
| fog | `volumetric_fog` | Fog: Volumetric | `Media` 0 |
| fog | `fog_composite` | Fog: Composite | `Media` 10 |
| water | `water_transmission` | Water: Caustics | `LightingInputs` 20 (nothing when lighting runs first) |
| water | `water_sim` | Water: Simulate + Draw | `Overlay` drawer 30 |
| water | `water_reactivity` | Water: TSR Reactivity | `Transparent` drawer 50 (nothing without a reactive mask) |
| foliage | `foliage_place` | Foliage: Place Blades | before `GBuffer`, 0 |
| foliage | `foliage_boxes` | Foliage: Expand Boxes | before `GBuffer`, 5 (takes what placement made) |
| foliage | `foliage_gbuffer` | Foliage: Draw to GBuffer | `GBuffer` drawer 20 (takes what placement made) |
| particles | `corona` | Overlay: Light Coronas | `Overlay` drawer 10 |

Not nodes: the water plugin's planar-reflection capture builds a private second graph
(`reflection_capture.rs`) for its own offscreen view, not a pass of the pipeline.

### Nodes of meshes, materials, decals and sprites

Every pass the default deferred pipeline can hold from these plugins is a node of its
feature (`pass_node`, kind `helio.pass.<name>`), each with an `enabled` parameter and
generated pins. Tests: `plugins/vendor/{meshes,decals,sprites,materials}/tests/graph_node.rs`
(registered with the feature as owner, default placement order, a graph asset builds them
and `enabled` off bypasses them, probed flow pins, disabled plugin => absent).

| Plugin | Node kind | Title | Placement |
|---|---|---|---|
| meshes | `helio.pass.object_batch` | Scene: Object Batches | slot Prepare, -100 |
| meshes | `helio.pass.indirect_dispatch` | Culling: Prepare Dispatch | slot Visibility, -10 |
| meshes | `helio.pass.occlusion_cull` | Culling: Frustum + Occlusion | slot Visibility, 10 (needs the Hi-Z node, the spine's) |
| meshes | `helio.pass.shadow_dirty` | Shadows: Find Stale Faces | before phase ShadowCasters (needs the lights' dirty flags) |
| meshes | `helio.pass.shadow` | Shadows: Render Atlas | opens phase ShadowCasters |
| meshes | `helio.pass.gbuffer` | Geometry: GBuffer | opens phase GBuffer |
| meshes | `helio.pass.forward_lit` | Geometry: Forward Lit | opens phase Forward (builds only where no G-buffer phase runs) |
| meshes | `helio.pass.virtual_geometry` | Geometry: Virtual Geometry | phase GBuffer drawer, 30 |
| meshes | `helio.pass.transparent` | Transparency: Blend | drawer of phase Transparent |
| decals | `helio.pass.decal` | Geometry: Decals | phase GBuffer drawer, 100 |
| sprites | `helio.pass.sprite_overlay` | Output: Sprites | slot Display, 10 |
| materials | `host.material_textures` | Host::material_textures | host inputs (data only: it supplies the texture table, no pass) |

The virtual geometry node builds the whole chain (stream begin, BVH, inputs, early and
late cull and raster, stream end, dense tier, shade) as one unit, because the passes hand
each other runtime state and buffers; it builds only for a `VgSettings` runtime and where the
device supports it. The probe's config has no host part, so the meshes' `configure_renderer`
gives a config without `VgSettings` an idle placeholder runtime (a renderer's config has the
host part's before it runs), and the node gets the pins of the chain. The sprite overlay is
likewise one node for its cull and batch passes. `SpriteSimulatePass` and the 2D radiance
cascade passes (`helio-pass-sprite-simulate`, `helio-pass-radiance-cascades-2d`) have no
node: they are built by examples around buffers the host owns (slot velocities, occupancy
and emitter grids) with no frame contract to take them from, so a node would have no way to
build them. The Hi-Z build (`HiZBuildPass`) is the spine's node, not meshes'.

### Nodes of post, lens, GI, portals, voxel and the kernel

Splines draw through the debug lines (no pass of their own). Tests:
`plugins/vendor/{post,lens,gi,portals,voxel}/tests/graph_node*.rs` (same set as above).

| Owner | Node kind | Title | Placement |
|---|---|---|---|
| post | `helio.pass.post_process_volume_blend` | Post: Blend Volumes | slot Media, -10 |
| lens | `helio.pass.lens` | Lens: Elements | slot Resolved, 0 |
| lens | `helio.pass.lens_flare` | Lens: Flares | slot Resolved, 10 |
| gi | `helio.pass.gi` | GI: Probe Update | before `GBuffer`, 10 |
| gi | `helio.pass.gi_mesh_trace` | GI: Mesh Rays | slot Geometry, 20 |
| gi | `helio.pass.gi_resolve` | GI: Resolve | slot LightingInputs, 0 |
| portals | `helio.pass.portal_cull` | Portals: Cull | before `GBuffer`, -10 |
| portals | `helio.pass.portal_mask` | Portals: Footprint Mask | `GBuffer` drawer 10 |
| portals | `helio.pass.portal_instance` | Portals: Draw Through | `GBuffer` drawer 11 (needs Portal Cull) |
| portals | `helio.pass.portal_editor_overlay` | Overlay: Portal Openings (Editor) | `Overlay` drawer 20 |
| voxel | `helio.pass.voxel_chunks` | Voxel: Chunk Terrain | `GBuffer` drawer 40 |
| kernel (spine) | `gbuffer_clear`, `background`, `hiz`, `unlit`, `tsr`, `post_process` | Scene: Clear G-Buffer, Scene: Clear Background, Culling: Depth Pyramid (Hi-Z), Lighting: Unlit, Anti-Aliasing: TSR, Post: Tone Map + Grade | as the preset places them |
| kernel | `ssr`, `ssr_composite`, `planar_reflection`, `fxaa`, `buffer_view`, `perf_overlay`, `debug_draw_scene`, `debug_draw` | feature crates / `helio-presets` `RendererNodes` | see their features |
| kernel | `helio.pass.debug_overlay` | Output: Debug Overlay | none: no shipped pipeline draws it; place it after the display slot's nodes |

Voxel is one `ChunkPass` (residency, generation, G-buffer, GI rays, sun pages are inside
it), so it is one node. The debug overlay node takes the host's `DebugOverlayState` as the
`DebugOverlayHost` setting (`RendererConfig::with_feature`); with none it owns a hidden state,
reached through the built pass (`find_pass_mut::<DebugOverlayPass>()`); its `visible`
parameter shows it. Not nodes by design: the performance analyzers (markers the compiler
adds), `BakeInjectPass` (a persistent pass the host installs after a bake) and the status
screen's overlay (the renderer's own graph when no pipeline builds).

## Pins are generated

No node lists its resources by hand. `RenderPass::scene_buffers` (SceneDB keys a
pass consumes) joins `reads()` / `writes()` / `declare_resources` as what a pass
declares, and `helio_graph_asset::probe` builds the registered passes on a device
and derives each node's pins:

- outputs: everything its passes write, typed by how the pass declares it
  (`write_buffer` is a buffer, colour and group writes are textures, depth is depth);
- inputs: everything its passes read, by who supplies it:
  - written by an earlier node: an ordinary input;
  - written only by a *later* node: a **history** input (last frame's value; Hi-Z
    reads `depth`). Its wire does not order the graph, so feedback is not a cycle;
  - supplied by the host: a Scene input;
  - written by no node: an **optional** input (the pass binds a fallback);
- SceneDB: a Buffer input `scenedb.<key>` per consumed key.

The result goes into the registry's pin table, shared by every descriptor, so
registering order does not matter and nothing re-registers when a probe finishes. A graph
asset keeps the pins its generated nodes had when it was written (`GraphAsset::pins`, see
"Pin records and rebuilds"), so it validates where nothing probes.
`engine_backend` runs the probe once per plugin set, on demand (the render graph editor
opening, or a project graph that did not validate without pins), on the engine's one device
(`render_runtimes::global().device()`: the window's, a headless one only where no window
offered one). `graph_node_registry` waits for it; `current_node_registry` never does
(registering a pipeline must not wait). With no GPU the nodes have flow pins only (graphs
still build). `Compile` reports why when it failed.

The pin type of a SceneDB buffer is the buffer kind, not its row type.

## Pins

The editor compares pin type strings (`tex:pre_aa`, `ptex:history`, `depth:…`,
`buf:…`, `pbuf:…`, `scene:…`; flow is `execution`), so two pins connect only when
kind, persistence and resource name match, because Helio binds resources by name.

| Kind | Shape | Persistent |
|---|---|---|
| Flow | arrow | – |
| Texture | circle | second outline |
| Depth | diamond | – |
| Buffer | square | second outline |
| Scene (host input) | hexagon | – |

## Execution and data

Two kinds of wire. **Flow** (arrow) pins are the execution chain: **On Frame Start**
is the first node, every pass that executes hangs off it in order, and the chain
ends at the Output. **Data** pins carry resources. The compiler orders the graph by
both (ties by authoring order); a node that executes but is not on the chain from On
Frame Start is warned, and a graph needs exactly one On Frame Start and one Output.
Nodes that only supply data (host inputs, SceneDB buffers, resources) have no flow
pins and sit outside the chain.

## The default deferred graph

`deferred_graph_asset(registry)` is On Frame Start, every pass in execution order,
then the Output, chained by flow wires, and laid out. With probed pins it is also
wired producer to consumer: an input to the last earlier pass that writes the
resource, a history input to the last pass that writes it in the frame. Host and
SceneDB inputs get a source node **per pass that reads them**, placed beside that
pass (like a property node in a shader graph), so no wire crosses the graph to reach
its source. With no pins it is just the chain, which builds the same pipeline; the
shipped `Deferred.rendergraph` uses that form, so it needs no probe.

The graph **is** the preset: both are `Preset::nodes` (`graph_asset(preset, registry)`
takes the instances with their parameters), so the nodes features placed at the
frame's slots and phases join it in their place with no preset naming them, and
`sectionize` puts them in the lane of their category. Neither depends on the renderer
config: where the deferred preset follows the project's anti-aliasing it names the
**anti-aliasing node** (`helio.pass.antialiasing`, "Anti-Aliasing: Project Setting",
`helio-presets` `features.rs`), which runs the TSR node's constructor at the config's
quality when the project enables TSR and its `fallback` (the FXAA node, or none)
otherwise, and the frame setup's `reactive_mask` applies only when the frame resolves
temporally. So one asset is the pipeline for every project setting. The first node of the chain is **Frame: Lighting Model**
(`helio.frame.setup`, `features::FRAME_SETUP`, params `model`, `reactive_mask`): it
publishes the lighting model's id, the draw phases (`PhaseList`), specular layers and
lighting-input order, and the reactive mask, as the settings the nodes after it read. It
used to be code in the pipeline closure; as a node a `.rendergraph` states it as data.
A model nobody registered resolves to the unlit stand-in with one warning.

`graph_parity` (Helio) builds `deferred_graph_asset` as a fixed asset and compares its pass
list and swap policy with the preset's, with FXAA, TSR at two qualities and SSR. The `.rendergraph` files
the engine ships are the default pipelines plugin's (below).

A new `.rendergraph` from the file menu is **only an On Frame Start node flowing into an
Output node** (`render_bps::document::new_graph_asset`, also the file type's creation
template). Nothing else is placed: every pass is added from the palette, which lists every
registered node (`model::catalog` is `NodeRegistry::list`), including the ones the default
graph uses. A file with no nodes (the old `{"template": "deferred"}` marker) opens as
that new graph. The default graph above is what a provider ships as an asset, not what the
editor creates.

## The shipped pipelines (`pulsar.default_pipelines`)

The plugin `plugins/vendor/default_pipelines` owns the pipelines the engine ships.
It serves them through an `engine_fs::FileProvider` at
`plugins/pulsar.default_pipelines/` in every project:

| File | Preset | Replaces (old id) |
|---|---|---|
| `Deferred.rendergraph` | deferred model; TSR when the project enables it, FXAA otherwise (the anti-aliasing node) | `helio.deferred` (the old engine default), `helio.deferred_graph` |
| `DeferredFxaa.rendergraph` | deferred model with FXAA | `helio.fxaa` |
| `Forward.rendergraph` | forward model, no G-buffer | `helio.forward_opaque`, `helio.forward_only` |
| `<label>.rendergraph` | each pipeline a registered lighting model brings (the lights' `HLFS.rendergraph`, `HLFS_FXAA.rendergraph`) | the model pipeline's id (`helio.hlfs`, ...) |

- **Made for the enabled plugins.** Each file is `helio_presets::graphs::graph_asset`
  over the node registry of the plugins the project enables, so it names every pass
  they place and no other: a project that disables a kind gets files without its
  nodes, never a graph that no longer validates (a static file naming every kind's
  node would fail with `missing node (plugin disabled)`). The folder is made again
  when `pulsar_plugins::generation()` changes; a changed file is reported through
  the folder's capability, the engine registers it again and the renderers running
  it rebuild. No project setting is baked in: the anti-aliasing node
  (`helio.pass.antialiasing`, "Anti-Aliasing: Project Setting") builds TSR at the
  project's quality when the project enables it and its fallback otherwise.
- **Made, not authored.** Generating the files from the presets is the design, not a
  stopgap: a kind's passes join where its feature placed them, so a plugin this repository
  has never heard of joins the shipped pipelines by placing its nodes, and the frame's
  structure has one owner (the spine's presets). A hand-authored file could drop an absent
  kind's nodes but never pick up a new one's, and would be a second copy of the structure.
  (Unity's URP frame is code with Renderer Features injected at events; UE's is code with
  SceneViewExtensions; neither ships a hand-expanded frame asset.)
- **Wired, readable form.** With the pins the engine's probe found
  (`pipelines::current_node_registry`), a file is every pass wired to what supplies its
  inputs (Get/Set buffer nodes beside each pass), laid out in lanes and sections, with the
  pins it was wired from recorded in it (`GraphAsset::pins`). Before the probe has run it is
  the chain form (On Frame Start, every node on one execution chain, the Output), which
  builds the same pipeline. The render graph editor waits for the probe before it reads a
  file, so it always shows the wired form. The swap from one form to the other rebuilds no
  renderer (see "Rebuilds" below).
- **Read-only; make your own from a copy.** Like every plugin folder the files cannot be
  written (`engine_fs::virtual_fs::mount_of`, `ReadOnlyMount`). The render graph editor
  opens them read-only, says which plugin serves them, and its toolbar's **Save a copy to
  the project** (also what Save does there) writes the graph, edits included, to
  `<project>/render_graphs/<Name>.rendergraph` (`render_bps::document::save_copy_to_project`,
  numbered when the name is taken) and goes on editing the copy, as a Save As does. World
  Settings offers the copy beside the shipped files; a level that picks it runs it.
- **Old ids.** The plugin registers a `PipelineIdMigration`: loading a level (editor:
  `WorldSettingsData::migrate_level`; game: `render_pipeline_of_level`) rewrites an
  old id to its file's path. A level saved before pipelines were explicit migrates
  to `helio.deferred` first, then to `Deferred.rendergraph`. With the plugin
  disabled the old id stays and the viewport says the pipeline was not found.
- **Packaged games** ship the packager's snapshot of the folder (`InGame::Snapshot`).
- Tests: `plugins/vendor/default_pipelines/tests/` (the files for a registry);
  `plugins/installed/tests/default_pipelines.rs` (as the editor and a game read them: wired
  once probed, valid with routing and in a game from the pin record, the chain form's build
  key, the render graph editor's loader; a copy saved, edited, selected and built; the editor
  renderer on Vulkan and D3D12; old ids).
- Repository: [Plugin_DefaultPipelines](https://github.com/Far-Beyond-Pulsar/Plugin_DefaultPipelines),
  vendored at `plugins/vendor/default_pipelines`.

## Pin records and rebuilds

**Pin records.** A generated node's pins come from what its passes declare, which takes a
device to find. A graph asset records the pins of the generated node kinds it uses
(`GraphAsset::pins`, written by `GraphAsset::cache_pins`: `graph_asset` and the render graph
editor's save do it), as UE's graphs serialise each node's pins and reconcile them with the
live signature on load. Validation and compile read the registry through
`NodeRegistry::with_pin_fallback(&asset.pins)`: a kind no probe has given pins takes the
record's, so a wired graph validates and builds where nothing probes (a game, a tool);
probed pins win, so a wire the passes no longer declare is an error, not trusted.

**Rebuilds.** The engine bumps a registered graph's revision, which rebuilds the renderers
running it, only when its build key changes (`Validation::build_key`: the executing nodes in
build order with their ids and parameters). Moving nodes, comments, sections, the pin record
and wires that keep the order change nothing a renderer builds.

**Hot-reload swap policy.** Each feature declares which of its pass types shader hot reload
may swap alone (`FeatureRegistrar::swappable`) or together (`swap_group`, `swap_shares`); a
`GraphAssetPipeline` reports the registry's policy for every graph, since independence is a
property of a node's constructor, not of the graph it is in. Helio's own (spine, SSR, planar
reflections, FXAA, perf overlay, debug draw) are declared by their features.

## Layout

`helio_graph_asset::auto_layout` (also the editor's Auto Layout button) is a layered
layout for execution graphs: executing nodes are ranked by a stable topological sort
of the flow and data wires and run left to right; a data-only node is placed in a
column directly left of the first node that uses it; runs of one palette category
form **lanes** (Scene Prep, Geometry, Lighting, Overlays, Post, Output), each a row
that wraps when it gets wide and is surrounded by a labelled comment box. Sizes are
estimated with the editor's own metrics so nothing overlaps. It replaces its own
comments (ids starting `lane:`) and keeps the author's.

Node titles read `Group: Action` (`Shadows: Render Atlas`, `Culling: Depth Pyramid
(Hi-Z)`) so a node says what it is and sorts with its group in the palette.

## Buffers (variables)

A buffer is a variable: a name for a texture, depth target or buffer that exists
only to keep the graph tidy. Wires between passes are not drawn; a pass reads a
buffer through a **Get** node on its left (`core.buffer_get`, param `buffer` =
`<tag>:<name>`) and writes one through a **Set** node on its right
(`core.buffer_set`). Any number of Get and Set nodes can name the same buffer.

`helio_graph_asset::buffers::flatten` removes them before validation and
compile: each Get is wired to the latest Set that runs before the node using it
(by execution rank); a Get with no earlier Set is a history read (last frame) or
a host input. The compiled graph is exactly what direct wires would give.

- Node titles read `Get <name>` / `Set <name>` (`buffers::title_of`, and
  `RenderPresenter` titles the node).
- The editor's left **Buffers** panel (`render_bps/src/buffers_panel.rs`) lists
  every buffer the generated nodes read or write (`buffers::known_buffers`),
  searchable, grouped by kind, each with Get and Set buttons that add a node.
- Auto layout puts a pass's Gets left and Sets right of it; host inputs that are
  declared but unused form a "Host Inputs" group.
- Nodes and lanes collapse into sections (see Sections below).

## Sections (collapsed nodes)

The default graph is grouped into **sections**: one named node per stage of the
frame (Scene Prep, Geometry, Lighting, ...), each holding its passes and their
Get/Set buffer nodes. The main graph is then short: On Frame Start, the sections
in execution order, Output.

- A section is `GraphAsset::sections` (`GraphSection`: id, name, ports, nodes,
  connections) placed by a node of kind `section:<id>`. Inside, the pseudo nodes
  `$entry` / `$exit` stand for its ports (`SECTION_ENTRY` / `SECTION_EXIT`).
- It is organisation only: `sections::expand` puts the nodes back in place and
  joins the wires across the ports before the buffers are flattened, validated and
  built. A section is placed once (its nodes keep their ids).
- In the editor a section is a graph of its own (a `GraphCanvas` per section, document
  node type `section:<id>` as in the asset) placed by one node that opens it (double-click,
  or the left **Sections** tab). The context menu has **Collapse to Section** (selection to a
  new section, the crossing wires becoming ports) and **Expand Section** (`render_bps::collapse`,
  tested to give back the same pipeline as `sections::expand`). An expanded section stays in
  the list so undo can find it. Section ports are the pins of the section graph's entry and
  exit nodes; comments inside a section are not part of the format and are not saved.
- `sections::sectionize` makes the sections from the lane groups of
  `auto_layout`; On Frame Start and Output stay outside. Auto Layout on a graph
  with sections lays out each inside and the main graph as a row.
- The left panel has two tabs: **Sections** (open any section) and **Buffers**.
- Compile and Save check the whole document (main graph and every section) and apply it to
  the preview.

## Resource nodes

`core.texture` / `core.depth` allocate a graph-recycled texture by name.
`core.persistent_texture` / `core.persistent_buffer` own a GPU object that
survives frames and graph rebuilds at the same size (`inherit_persistent_state`)
and publish it by name (`write_texture_binding` / `write_buffer_binding`).
`core.external` declares a host-supplied input.

## Rules

- A graph builder returns a **locked** graph (`graph.lock(w, h)`); `compile` locks.
- A pipeline installs a `GraphRebuilder` so resize and shader reload rebuild it.
  `GraphAssetPipeline` does this.
- A graph that fails to compile is reported (the editor shows the diagnostics from
  Compile); in the engine it is not offered (`refresh_project_pipelines` records why,
  `PipelineState::Invalid`) and a renderer asked to run it shows the reason. No other
  pipeline replaces it.
- New passes register a node with `NodeRegistry::register` (`pass_node` shows the
  shape). `helio-graph-asset` must stay free of pass types.
- A pass that fetches a SceneDB buffer lists it in `RenderPass::scene_buffers`.
  Files with several `RenderPass` impls were declared by hand where the key is used.

## Editor

`render_bps` is a built-in editor, registered at link time like the Blueprint editor
(`pulsar_engine` feature `render-graph`, on by default; it no longer needs `blueprint`).
It has its own editor on the shared Graphy canvas (`.agents/GRAPH_CANVAS.md`, "Render Graph
(Phase 5)") and links nothing of the Blueprint editor.

| File | What |
|---|---|
| `model.rs` | Domain rules, pure: registry, pin type strings and styles, catalogue and node factory, params, dynamic-pin commands, auto layout, checks |
| `document.rs` | `.rendergraph` <-> Graphy graphs (`RenderDocument`, borrowed `DocRef`), the buildable projection |
| `collapse.rs` | Collapse to / expand a section, as graph commands |
| `profile.rs` | `RenderProfile`: `GraphProfile` + `Presenter` for the canvas |
| `editor.rs`, `view.rs` | The editor entity (tabs, undo, save, Compile, picker) and its views |
| `sections_panel.rs`, `buffers_panel.rs`, `preview.rs` | Sections and Buffers lists, the preview |

The **preview** above Properties is a Helio renderer on the editor's shared device,
drawing a private test scene through the graph; Compile and Save apply the graph (a
graph with errors is reported, not applied). It uses the shared frame pacer and
publish-driven repaint pump from `ui_helio_viewport`; scene setup, camera controls,
surface lifecycle, and preview status remain local to this panel. The level editor
keeps its more involved surface lifecycle and frame pump policy local because they
coordinate level loading, thumbnails, and viewport input (issue 1419).

## Lighting models and the unlit stand-in

A built-in preset chooses a lighting model by id (`Preset::Deferred` is
`deferred`, `Preset::Forward` is `forward`, a model's own pipeline is
`Preset::Model(..)`, e.g. HLFS as `helio_light_host::HLFS`). The lights plugin
registers those models. When no installed plugin registered the chosen id (the
lights plugin is disabled), `resolve_model` falls back to the spine's `unlit`
model: its `UnlitPass` draws albedo + emissive over the background, so Forward
and Deferred show their surfaces unlit instead of black or broken. Forward
without the lights is an unlit G-buffer pipeline. The fallback is not silent:
`Preset::model` logs one warning per missing id the first time a graph build
takes it. Helio `docs/architecture/plugin-migration.md` (lighting models) has
the design; `graph_parity` pins it.

## Routing validation

A pipeline once rendered black: a pass's result never reached the output, and
because Helio resolves a read by resource name against earlier passes, the graph
built, locked and ran. Routing validation says so, from declarations only (no
GPU work, no per-frame readback).

One engine, `helio_core::check_routing` (`graph/routing.rs`), takes an ordered
list of units (reads, writes, whether it produces the presented target) and the
host-supplied inputs. Two front ends feed it:

- **The asset** (`helio_graph_asset::routing`): one unit per node from its pins,
  in the compiler's order; `Scene` pins and `scenedb.*` buffers are the host's,
  history and optional inputs need no writer, and the node whose flow wire enters
  the Output writes the final output. `validate` appends the findings, so the
  render graph editor's **Compile** shows them (`render_bps` lists warnings in its
  status, errors stop the compile). A graph with no executing node reports nothing.
- **The built graph** (`RenderGraph::routing_issues`): one unit per pass from
  `PassDeclaration`. `Renderer::construct` logs it for every graph, graph asset or
  legacy builder (`helio::routing`: error for a missing output, warn for the
  rest), and `engine_backend` adds one line with the pipeline id.

Findings: a resource written and never read (or overwritten before any read), a
read nothing writes (not host, history or optional), a unit whose writes cannot
lead to the output, a final output **no pass writes** (error), and a later output
writer that ignores the earlier one. A pass that draws the frame's image into
`ctx.target` says so with `RenderPass::writes_target` (FXAA, TSR unless
intermediate, PostProcess); the target is not a named resource, so this is
declared, not derived. Overlays do not declare it. Pass-level reads carry no
optionality, so a read a pass can do without shows as a warning there.

## Migration of implicit pipelines

Levels saved before pipelines were explicit stored `render_pipeline: null` (or no key) for
"the engine default", which was `helio.deferred`. `world_settings.pipeline_version` marks
what `None` means: absent or 0 is the old meaning, 1 is "no pipeline". Loading a level
migrates version 0 on the JSON, in the data path
(`WorldSettingsData::migrate_level` in the editor, `render_pipeline_of_level` for the
game): a level that named none gets `helio.deferred` (`LEGACY_DEFAULT_PIPELINE`) and the
current version, and saves that way; one that named a pipeline keeps it. A level created
now has no pipeline until one is picked. Every load then runs the enabled plugins'
`PipelineIdMigration`s on the id, so Helio's old built-in ids become the
`pulsar.default_pipelines` files (see above).

## Known gaps

- The shipped files are wired only once the engine's probe has run in the process (the
  render graph editor runs it; a renderer does not). A packaged game snapshots the folder as
  the packager's process had it.
- Plugin passes declare no hot-reload swap policy yet (`FeatureRegistrar::swappable`): a
  shader edit in one rebuilds the graph whole.
- Plugin passes join by slot/phase placement at registration; a plugin node that must sit
  between two kernel nodes inside one slot is ordered by its `order` only (there is no
  per-pin after/before hint).
- `render_bps::model::registry()` keeps its first snapshot of the node registry for the
  process, so the editor's palette does not follow a plugin set that changes after it
  opens (the engine's `graph_node_registry` does).
- Forward, HLFS and simple pipelines are still legacy builders; their passes are not
  all nodes yet (`forward_lit` is).
- Pass construction still hands buffers between passes through `find_pass`. Making
  them resources the graph owns and sizes from the pins is the next step.
- The probe builds every registered pass once: a second or two at first use.
- The preview has no sky, no gizmos or picking, simple pacing.
- Project render graphs (plugin folders included) are compiled and registered by the
  engine the first time a renderer or the picker asks for a pipeline of the open project
  (`pipelines::ensure_project_pipelines`), and again on each `.rendergraph` change (the
  "Render pipeline assets" service); the game registers them in `load_level`. A packaged
  game needs `.rendergraph` files in its content.
- The level stores the asset's project-relative path, not a typed `AssetRef` id: moving
  or renaming the file breaks the reference (the panel says so). A typed `AssetKind` for
  render graphs is the next step.
- Blueprints that declare `IntoGraph` are not yet listed as pipelines.
