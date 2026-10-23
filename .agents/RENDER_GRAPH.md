# Render pipelines and render graphs

A level chooses its **render pipeline** (World Settings, first section). A pipeline
is anything that implements Helio's `IntoGraph`: Helio's shipped pipelines and
project-authored `.rendergraph` assets are both offered. A render graph is a
graph of **pass nodes**: one node builds one pass, and the node's pins come from
what that pass declares.

## Layers

| Layer | Where | What |
|---|---|---|
| `IntoGraph`, `RenderPipelineRegistry` | `helio` (`renderer/pipeline.rs`) | The extension seam. `RendererBuilder::with_graph(impl IntoGraph)` / `with_boxed_graph`. |
| Graph asset + compiler | `helio-graph-asset` | `GraphAsset` (`.rendergraph` JSON), `NodeRegistry`, `validate`, `compile`, `GraphAssetPipeline` (an `IntoGraph`), `probe`. Knows no pass. |
| Pass nodes | `helio-default-passes` (`graph_nodes.rs`) | One node per pass: how to construct it. Registered once; pins generated. |
| Default graphs | `helio-default-passes` (`graphs.rs`) | `deferred_graph_asset`: the default deferred pipeline *as a node graph*. |
| Legacy builders | `helio-default-passes` (`lib.rs`) | The `add_pass` builders (`build_default_graph_*` and the rest) and `pipelines.rs`. Still the engine default (`helio.deferred`) and the forward/HLFS/simple pipelines; they go away as each is replaced by a graph. |
| Engine registry | `engine_backend::…::helio_renderer::pipelines` | Process-wide pipeline and node registries; the pin probe; project `.rendergraph` registration; the hidden `preview:N` pipelines. |
| Selection | level `world_settings.render_pipeline` | Editor: poller pushes it to every viewport mailbox, which rebuilds the renderer at a frame boundary. Game: `load_level` records it, `game_renderer` builds from it. |
| Editor | `plugins/vendor/render_bps` | The Blueprint graph editor with a profile installed, plus a live preview. |
| Trait | `engine_fs::builtin_traits` | `types/traits/into_graph.trait.json` is written into every project when the trait index rebuilds. |

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

A new `.rendergraph` from the file menu is a marker the editor expands on open into
the wired graph.

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
  `EditorProfile::node_title` in the editor).
- The editor's left **Buffers** panel (`render_bps/src/buffers_panel.rs`) lists
  every buffer the generated nodes read or write (`buffers::known_buffers`),
  searchable, grouped by kind, each with Get and Set buttons that add a node.
- Auto layout puts a pass's Gets left and Sets right of it; host inputs that are
  declared but unused form a "Host Inputs" group.
- Collapsing nodes and lanes is not implemented yet.

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
- A graph that fails to compile is logged and replaced by the fallback (the legacy
  default graph); the editor shows the same diagnostics from Compile.
- New passes register a node with `NodeRegistry::register` (`pass_node` shows the
  shape). `helio-graph-asset` must stay free of pass types.
- A pass that fetches a SceneDB buffer lists it in `RenderPass::scene_buffers`.
  Files with several `RenderPass` impls were declared by hand where the key is used.

## Editor

`render_bps` is a built-in editor, registered at link time like the Blueprint editor
(`pulsar_engine` feature `render-graph`, on by default, which also needs `blueprint`).
It reuses the Blueprint graph editor through an `EditorProfile`
(`blueprint_editor_plugin::profile`): a registry; an editor opened on a file takes the
profile claiming its extension (`.rendergraph`), every other file stays a Blueprint.

A profile supplies: id and extensions, node catalogue, `decode`/`encode`, pin styles,
`validate` (the Compile button), `parameters`/`parameter_changed` (editable
parameters; resource pins are renamed and their wires moved), a `preview_panel`, and
`graph_applied`. The Blueprint side panels do not show.

The **preview** above Properties is a Helio renderer on the editor's shared device,
drawing a private test scene through the graph; Compile and Save apply the graph (a
graph with errors is reported, not applied). It is a small component, not the level
editor's `HelioViewport` (issue 1419).

## Known gaps

- The engine default stays the legacy `helio.deferred`. `helio.deferred_graph` differs:
  FXAA path (no TSR node), no virtual geometry, no debug overlay, and **no voxel
  passes** (the engine's voxel backends add theirs through the legacy builder; they
  need to register nodes). Switch the default when that is verified.
- Forward, HLFS and simple pipelines are still legacy builders; their passes are not
  all nodes yet (`forward_lit` is).
- Pass construction still hands buffers between passes through `find_pass`. Making
  them resources the graph owns and sizes from the pins is the next step.
- The probe builds every registered pass once: a second or two at first use.
- The preview has no sky, no gizmos or picking, simple pacing.
- Project render graphs register when the World Settings panel (editor) or `load_level`
  (game) runs; a packaged game needs `.rendergraph` files in its content.
- Blueprints that declare `IntoGraph` are not yet listed as pipelines.
