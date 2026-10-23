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

## Known gaps

- Plugin-registered nodes are not visible to `render_bps` (its registry is its
  own copy). Nodes from `helio-default-graphs` are.
- A custom graph does not include the engine's voxel passes (the editor's
  default pipeline does).
- Project render graphs are registered when the World Settings panel (editor)
  or `load_level` (game) runs; a packaged game needs `.rendergraph` files in its
  content.
- Blueprints that declare `IntoGraph` are not yet listed as pipelines.
