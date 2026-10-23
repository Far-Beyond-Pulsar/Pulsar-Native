# Render pipelines and render graph assets

A level chooses its **render pipeline** (World Settings, first section). A
pipeline is anything that implements Helio's `IntoGraph`; Helio's shipped graphs
and project-authored `.rendergraph` assets are both offered.

## Layers

| Layer | Where | What |
|---|---|---|
| `IntoGraph`, `RenderPipelineRegistry` | `helio` (`renderer/pipeline.rs`) | The extension seam. `RendererBuilder::with_graph(impl IntoGraph)` / `with_boxed_graph`. A plain `FnOnce(PassBuildContext) -> RenderGraph` implements it. |
| Built-in pipelines | `helio-default-graphs/src/pipelines.rs` | Each shipped `build_*_graph_with_context` registered by id (`helio.deferred`, `helio.fxaa`, …). `helio.deferred` is the engine default. `helio.deferred_graph` is the default built from nodes. |
| Graph asset + compiler | `helio-graph-asset` | `GraphAsset` (`.rendergraph` JSON), `NodeRegistry`, validation (`validate`), `compile`, `GraphAssetPipeline` (an `IntoGraph`). Knows no pass. |
| Helio's nodes | `helio-default-graphs/src/nodes.rs` | One node per stage of the default deferred graph, and `default_graph_asset()` that wires them. |
| Engine registry | `engine_backend::…::helio_renderer::pipelines` | Process-wide pipeline + node registries; `refresh_project_pipelines(root)` registers a project's `.rendergraph` files (id = project-relative path). |
| Selection | level `world_settings.render_pipeline` | Editor: poller pushes it to every viewport mailbox, which rebuilds the renderer at a frame boundary. Game: `pulsar_game::standalone::load_level` records it, `game_renderer` builds from it. |
| Editor | `plugins/vendor/render_bps` | The Blueprint graph editor with a profile installed. |
| Trait | `engine_fs::builtin_traits` | `types/traits/into_graph.trait.json` is written into every project when the trait index rebuilds. |

## Pins

`PinType { kind, persistent }`; the editor compares pin type strings, so the
string carries kind, persistence and the Helio resource name
(`tex:pre_aa`, `ptex:history`, `depth:…`, `buf:…`, `pbuf:…`, `scene:…`; flow is
`execution`). Two pins connect only when the strings are equal, because Helio
binds resources by name.

| Kind | Shape | Persistent |
|---|---|---|
| Flow | arrow | – |
| Texture | circle | second outline |
| Depth | diamond | – |
| Buffer | square | second outline |
| Scene (host input) | hexagon | – |

## Pins are generated, not written

No node lists its resources by hand. `RenderPass::scene_buffers` (SceneDB keys a
pass consumes) joins `reads()` / `writes()` / `declare_resources` as what a
pass declares, and `helio_graph_asset::probe` builds a graph on a device and
derives every node's pins from those declarations:

- outputs: everything its passes write, typed by how the pass declares it
  (`write_buffer` is a buffer, colour and group writes are textures, depth is
  depth);
- inputs: what its passes read that an earlier node wrote, or that the host
  supplies (a Scene pin). A resource only a later pass writes is last frame's
  value (Hi-Z reads `depth`) and is not an input; one nothing writes is an
  optional input with a fallback;
- SceneDB: a Buffer input `scenedb.<key>` for each key a pass consumes. The
  SceneDB node outputs every key the mirror registered plus those declared;
- host inputs: a node that calls `declare_external_input` gets a Scene output.

The pin type of a SceneDB buffer is the buffer kind, not its row type. The
default graph's wires are generated the same way (`default_graph_asset`).

`engine_backend` runs the probe once per process on a headless device
(`graph_pin_probe`, started in the background when the editor registers), and
`graph_node_registry` waits for it. With no GPU the stages have flow pins only.
A pass that fetches a SceneDB buffer should list it in `scene_buffers`; passes
with more than one `RenderPass` impl in a file (sky composite, perf overlay
analyser, VG raster) are declared by hand where the key is used.

## Resource nodes

`core.texture` / `core.depth` allocate a graph-recycled texture by name.
`core.persistent_texture` / `core.persistent_buffer` own a GPU object that
survives frames and graph rebuilds at the same size (`inherit_persistent_state`)
and publish it by name through the reflected-binding contract
(`ResourceRegistry::write_texture_binding` / `write_buffer_binding`).
`core.external` declares a host-supplied input.

## Rules

- A graph builder returns a **locked** graph (`graph.lock(w, h)`); executing an
  unlocked one panics. `compile` locks; every new `*_with_context` builder must.
- A pipeline installs a `GraphRebuilder` (`set_graph_data`) so resize and shader
  reload rebuild it. `GraphAssetPipeline` does this.
- A graph that fails to compile is logged and replaced by the fallback (the
  default graph); the editor shows the same diagnostics from its Compile button.
- New nodes are registered by the crate that owns the pass
  (`NodeRegistry::register`); `helio-graph-asset` must stay free of pass types.

## Plugin

`render_bps` is a built-in editor, registered at link time like the Blueprint
editor (`pulsar_engine` feature `render-graph`, on by default, which also needs
`blueprint`; `--no-default-features` drops it). It also builds as a plugin DLL
without the `builtin` feature.

It reuses the Blueprint graph editor through an `EditorProfile`
(`blueprint_editor_plugin::profile`). Profiles are a registry: an editor opened
on a file takes the profile that claims its extension (`.rendergraph`), every
other file stays a Blueprint, so both share one copy of the editor. The editor
holds its profile, the canvas and palette copy it, and nothing reads a global.

A profile supplies: id and file extensions, node catalogue, `decode`/`encode` of
the document, pin styles, `validate` (the Compile button), `parameters` /
`parameter_changed` (editable node parameters; resource pins are renamed and
their wires moved), and whether the Blueprint side panels show (they don't).

## Preview

The editor shows a live preview above the Properties panel (`render_bps/src/preview.rs`):
a full Helio renderer on the editor's own wgpu device (the window's surface), drawing a
private built-in test scene (ground, shapes, sun, point light) through the graph's
pipeline. Compile and Save apply the graph (`EditorProfile::graph_applied`); a graph
with errors is reported in the preview and not applied. The pipeline is registered
under a hidden `preview:N` id and set through the mailbox
(`HelioEditorMailbox::set_render_pipeline`), so it rebuilds like a level does.
Drag orbits, scroll zooms.

The node-pin probe also runs on the editor's shared device when the window offers one
(`offer_probe_device`), and on a headless one otherwise (the game).

## Known gaps

- The probe builds with TSR on (FXAA and TSR are alternatives), so `fxaa_color`
  has no output pin; stage inputs that only exist in the other branch are
  missing the same way.
- A new graph made from the file menu starts as the flow chain (the file is
  written before the probe can finish); `default_graph_template()` has the
  wires.
- A custom graph does not include the engine's voxel passes (the editor's
  default pipeline does).
- The preview is a small separate component, not the level editor's `HelioViewport`
  (no input beyond orbit and zoom, no gizmos, simple frame pacing). Extracting a
  reusable viewport from `ui_level_editor` is tracked as an issue.
- Project render graphs are registered when the World Settings panel (editor)
  or `load_level` (game) runs; a packaged game needs `.rendergraph` files in its
  content.
- Blueprints that declare `IntoGraph` are not yet listed as pipelines.
