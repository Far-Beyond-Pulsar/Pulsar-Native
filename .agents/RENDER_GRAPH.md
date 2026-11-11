# Render pipelines and render graphs

A level chooses its **render pipeline** (World Settings, first section) by picking
an **asset**: the editor's shared asset picker lists the project's `.rendergraph`
files and, beside them, the pipelines a provider offers (Helio's shipped ones,
listed by id with their origin). A pipeline is anything that implements Helio's
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
| Presets, default graphs | `helio-presets` (`presets.rs`, `graphs.rs`, `pipelines.rs`) | `Preset::nodes`: the shipped pipelines as node lists over the frame's slots and phases. `graph_asset` / `deferred_graph_asset(_for)`: the same list as a laid-out, sectioned `GraphAsset`. There is no `add_pass` builder any more. |
| Engine registry | `engine_backend::…::helio_renderer::pipelines` | Process-wide pipeline and node registries; the pin probe; project `.rendergraph` registration; the hidden `preview:N` pipelines. |
| Selection | level `world_settings.render_pipeline` (+ `pipeline_version`) | An asset picked in World Settings: a project-relative `.rendergraph` path or a provider's id; `None` is no pipeline. Editor: poller pushes it to every viewport mailbox, which rebuilds the renderer at a frame boundary (also when the asset is saved again: `pipelines::PipelineState::Assigned.revision`). Game: `load_level` records it, `game_renderer` builds from it. |
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
(the project's plugin set), as are the built-in pipelines and the pin probe.

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
- Test: `plugins/vendor/voxel/tests/graph_node.rs`.

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
registering order does not matter and nothing re-registers when a probe finishes.
`engine_backend` runs the probe once per process, on the editor's shared device when
the window offers one (`offer_probe_device`) and a headless one otherwise, and
`graph_node_registry` waits for it. With no GPU the nodes have flow pins only
(graphs still build). `Compile` reports why when it failed.

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
built-in `helio.deferred_graph` pipeline uses that form, so it needs no probe at
runtime.

The graph **is** the preset: both are `Preset::nodes` (`graph_asset(preset, registry,
config)` takes the instances with their parameters), so the nodes features placed at the
frame's slots and phases join it in their place with no preset naming them, and
`sectionize` puts them in the lane of their category. `deferred_graph_asset_for(registry,
config)` has the **TSR node** (`quality` from the config, `transparency_reactivity`) where
the config asks for temporal resolve and **FXAA** otherwise; `helio.deferred_graph` is a
made pipeline over it (`GraphAssetPipeline::made`), so it follows the project's config as
`helio.deferred` does. The first node of the chain is **Frame: Lighting Model**
(`helio.frame.setup`, `features::FRAME_SETUP`, params `model`, `reactive_mask`): it
publishes the lighting model's id, the draw phases (`PhaseList`), specular layers and
lighting-input order, and the reactive mask, as the settings the nodes after it read. It
used to be code in the pipeline closure; as a node a `.rendergraph` states it as data.
A model nobody registered resolves to the unlit stand-in with one warning.

The golden files `crates/renderer/helio/crates/helio-presets/assets/deferred.rendergraph`
and `deferred_tsr.rendergraph` are the sectioned kernel graph (no plugin nodes, unwired
chain form) generated from code only to bootstrap them
(`cargo test -p helio-presets regenerate_the_golden_graphs -- --ignored`); a test fails when
code and files drift, and a GPU test builds each and compares the pass list with the
preset's (`graph_parity`).

A new `.rendergraph` from the file menu is **only an On Frame Start node flowing into an
Output node** (`render_bps::document::new_graph_asset`, also the file type's creation
template). Nothing else is placed: every pass is added from the palette, which lists every
registered node (`model::catalog` is `NodeRegistry::list`), including the ones the default
graph uses. A file with no nodes (the old `{"template": "deferred"}` marker) opens as
that new graph. The default graph above is what a provider ships as an asset, not what the
editor creates.

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
now has no pipeline until one is picked. When the default-graphs plugin ships the
shipped pipelines as assets, a migration rewrites those ids to the asset paths the same way.

## Known gaps

- The shipped pipelines (`helio.deferred` etc.) are still registered by Helio as code
  (`Preset::nodes`). They are offered in the picker as assets of their provider (origin
  shown) and old levels keep working through them (see Migration). The default-graphs
  plugin, when it lands, ships the golden `.rendergraph` files as assets and the built-in
  registrations go; `helio.deferred_graph` is the same nodes as data and is pinned equal
  to the preset (FXAA, TSR at each quality, SSR; `graph_parity`).
- The golden files are the unwired chain form. The editor's wired (Get/Set) form needs
  probed pins, so it is generated on a device and not pinned yet.
- The debug overlay (`helio-pass-debug-overlay`) has no node: its state
  (`DebugOverlayState`) is the host's and only examples build the pass. It needs a node
  that requires that setting.
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
- Project render graphs are compiled and registered when the World Settings panel (editor)
  or `load_level` (game) runs, and again on each `.rendergraph` change (through the VFS);
  a viewport whose level names a project graph shows "not found" until the panel exists.
  A packaged game needs `.rendergraph` files in its content.
- The level stores the asset's project-relative path, not a typed `AssetRef` id: moving
  or renaming the file breaks the reference (the panel says so). A typed `AssetKind` for
  render graphs is the next step.
- Blueprints that declare `IntoGraph` are not yet listed as pipelines.
