# Graph canvas (Graphy render + interaction)

The shared node-graph canvas lives in the Graphy submodule
(`crates/third-party/graphy`). The Blueprint (Phase 4), Render Graph (Phase 5) and Shader
(Phase 6) editors use it, each below. No editor owns a renderer, layout, hit-testing, spatial
index or input handling of its own.

## Crates

| Crate | Headless | Owns |
|---|---|---|
| `graphy-render` | yes | The single layout model (`NodeSpec` -> `NodeLayout`, rows with per-row heights, header outputs, host rows), `ViewTransform`, R-tree `SpatialIndex`, `Scene` (derived from `GraphDescription`, incremental via `ChangeSet`), hit testing, `DrawData` (culling, zoom-tier LOD, state hierarchy), `Presenter` hooks, `Theme` tokens, sample scenes |
| `graphy-render-wgpu` | no (wgpu) | `GraphRenderer`: draws `DrawData` into the host's pass on the host's device/queue; WGSL for grid, cards, pins, wires, glyph-atlas text; `render_scenes` example |
| `graphy-gpui` | core yes, canvas behind feature `gpui` | `Controller` (input -> `Action`s, all edits as `GraphCommand` batches), `GraphProfile` extension API, clipboard, snapping, `GraphCanvas` GPUI entity |

`graphy-gpui` is **not** a member of the Graphy workspace: its `gpui` feature
needs Pulsar's gpui and wgpu forks, so it has its own `[workspace]` and lockfile
(`cargo test` / `cargo check --features gpui` from `crates/graphy-gpui`).

## Rules

- Drawing, text, wire endpoints and hit-testing all read `NodeLayout`; never
  compute a pin position anywhere else.
- The editing session's only mutable document is the `GraphDescription` (owned
  by `GraphCanvas`). Edits are `GraphCommand`s applied with
  `apply_transaction`; the host receives `CanvasEvent::Edited { changes }` for
  undo/dirty/validate. Undo of a change set is `ChangeSet::undo`.
- Profiles (`GraphProfile`, optionally `GpuiProfile`) are passed explicitly as
  `Arc<dyn ...>`; no registries, no cross-DLL statics, no `TypeId` identity.
- Editor chrome (panels, toolbar, palette, inspector) stays in the host.
- GPU targets may be `*Srgb` (GPUI surfaces are `Bgra8UnormSrgb`); colours are
  authored in display space and linearised in the renderer.

## Extension hooks (what a domain supplies)

`Presenter` (headless): node title/accent/icon/shape/header output/host rows,
pin style + label + inline value chip, row heights, wire colour/routing,
comment and section presentation. `GraphProfile`: connection policy and
`TypeResolver`, catalogue + node factory, reroute factory, dynamic-pin refresh,
macro/subgraph navigation, menu items, overlays (`decorate`). `is_deletable` protects structural nodes. Header double-clicks
reach the host as `Action::CommentActivated`; `CanvasEvent::Edited` carries the commands
and the change set; `graphy::diff_commands` turns "this graph should become that graph" into
commands. `GpuiProfile`:
GPUI content for host rows (texture previews, custom bodies).

## Reviewing the visuals

```
cargo run --release -p graphy-render-wgpu --example render_scenes
```

writes PNGs to `target/graphy-scenes/` (offscreen, no window). See
`crates/graphy-render-wgpu/README.md` for the visual system and LOD tiers.

## Blueprint (Phase 4): the first consumer

`plugins/vendor/blueprint_editor` draws and edits through `GraphCanvas`; it has no
renderer, hit-testing, spatial index or input code of its own.

- Each open tab is a `GraphCanvasPanel` (`editor/graph_canvas_panel.rs`) holding an
  `Entity<GraphCanvas>`, which owns the tab's `graph::GraphDescription`. A tab without a
  canvas yet carries a `TabDoc` that the canvas takes over when mounted.
- `io::canvas_doc` is the only conversion between the `.blueprint` schema
  (`graphy_blueprint`) and the editing document: `from_authored` on load, `to_authored`
  for save, compile, prefab and AI snapshots. Comment/connection ids and the
  authored-only fields (`AuthoredSidecar`) survive the round trip.
- Edits are `GraphCommand` batches. `CanvasEvent::Edited` carries the commands and the
  `ChangeSet`; the panel pushes `features::undo::Command` (change set to undo, commands to
  redo) on the tab's host-registered history and marks the editor dirty. Editor-made
  changes that are not user steps (macro interface and event sync) use `apply_system`.
  Whole-document replacement (AI edits) is `graphy::diff_commands`.
- `canvas_profile::BlueprintProfile` is the `GraphProfile`/`Presenter`: catalogue and node
  factory, pin styles, type rules, reroute inference, macro navigation, deletion rules,
  titles (custom events, subgraphs, engine events).
  `ProfileData` carries what it reads from the editor (subgraphs, library, event names,
  running nodes, wire style).
- Blueprint-only chrome stays in the plugin: context-menu entries (breakpoint, collapse),
  quick palette, comment title editing, breakpoint badges, debug HUD, macro pin editor,
  inspector (over `NodeView`/`CommentView` snapshots of the document).

## Render Graph (Phase 5): the second consumer

`plugins/vendor/render_bps` has its own editor on the canvas and shares no code or
registry with the Blueprint editor (the `EditorProfile` contract and its global registry
were deleted; a profile is the `Arc<dyn GpuiProfile>` the editor passes to the canvas).

- **One document.** Each graph of a `.rendergraph` (the main graph and one per section) is a
  `graphy::GraphDescription` owned by its `GraphCanvas`. There is no second editable copy:
  `document::decode` builds the graphs from the asset at load, `document::encode` writes the
  asset from them at save, and validation, Compile and the preview read the same graphs
  through a borrowed `DocRef`. Helio's `validate` and compiler take a `GraphAsset`, so those
  paths ask the `DocRef` for a `buildable_asset` (reroutes joined, sections kept): a derived,
  read-only projection, never edited and never kept.
- **Nothing is lost at the boundary**: node ids and authoring order, params exactly as loaded
  (a param equal to its default stays if the file had it; Helio reads `buffer` raw), params of
  node kinds this build does not know, the wires of such nodes (pins are made from them),
  reroutes (saved with their `type`), comments, section ports (the pins of the section graph's
  entry and exit nodes), `format_version`, name and description.
- `profile::RenderProfile` is the `GraphProfile`/`Presenter`: catalogue and node factory,
  pin styles (shape by kind, ring when persistent), exact-type-string connection rule, section
  nodes opening their graph, protected entry/exit nodes, and `pin_commands` (Graphy hook) for
  dynamic Helio pins: a resource node's pin is named after the resource, so a parameter edit
  returns the commands that move the wires to the renamed pin.
- Render-only chrome stays in the plugin: toolbar (Save, Compile, Comment, Auto Layout), graph
  tabs, node picker, context-menu entries (Collapse to Section, Expand Section), comment title
  editor, the Sections, Buffers and Properties panels and the pipeline preview.

## Shader (Phase 6): the third consumer

`plugins/vendor/shader_editor` (`.material`) edits a Graphy `GraphDescription` on the canvas;
its former canvas stack (GPU renderer, layout, hit-testing, input, the `BlueprintGraph` model
and its conversions, the clipboard/undo/selection features) was deleted.

- **One document, no conversion.** The asset's `main_graph` *is* a `graphy::GraphDescription`, so
  load (`io::save_load::decode`) hands the graph to the `GraphCanvas` as it is and save writes
  the canvas's graph back (`ShaderEditorPanel::to_shader_asset`). The file's other fields
  (`editor_state`, `shader_metadata` incl. fields older files carry, unknown top-level fields)
  are kept in `DocMeta` and written back unchanged; the saved view and shader model are updated.
  Property values stay `serde_json::Value`s (a vec4 default is a JSON array), not text.
  Comments keep their id, colour and members.
- **One legacy codec.** Old blueprint-format bytes are recognised by
  `graphy_blueprint::legacy::is_legacy_format` (the shader copy was deleted) and open as an
  empty shader graph, as before.
- `profile::ShaderProfile` is the `GraphProfile`/`Presenter`/`GpuiProfile`: catalogue and node
  factory from `definitions` (PSGC metadata), shader reroutes (`input`/`output` pins), pin and
  node look, and the type rules. WGSL types are their own universe (`types::ShaderTypeResolver`,
  chained behind Graphy's `StructuralResolver`); the reflection registry is only used by the
  Details panel to pick a widget for a pin default.
- **Texture previews are variable-height rows.** With the toolbar's debug toggle on, the presenter
  returns an `ExtraRow` (56 units, min width 220) `AfterRow(i)` for every previewable (`vec3/vec4<f32>`)
  output `i`, slot id `i`. Graphy lays the node out around the row and hit-tests it
  (`Hit::Node { part: NodePart::Custom(slot) }`); `GpuiProfile::render_slot` returns a
  `previews::PinPreview` entity that owns a small GPU surface, compiles the pin's shader on a
  worker (debounced on a graph signature) and draws it every frame. Failures are listed in the
  Compiler panel.
- **PSGC compilation works on a copy** (`features::compilation::lowering`): text defaults become
  numbers/booleans, blank input defaults are dropped, and `TextureSrc` asset references become
  `@group(1)` bindings. The material thumbnail uses the same function.
- Shader-only chrome stays in the plugin: toolbar (Save, Compile, Comment, debug previews, shader
  model, preview mesh), the Compiler, Find (graph index), Details and Material Preview panels,
  the node picker, context menu and comment title editor.
- Not carried over: the minimap and graph-controls overlays (Graphy has neither yet), the comment
  colour picker, and the AI session's `title` arguments and viewport/selection fields (nodes no
  longer store a title; the title comes from the definition).

## Shared picker, context menu and theme (graphy-fixes)

- **Node picker and context menu live in `graphy-gpui`** (`picker.rs`, `catalog.rs`, drawn by
  `GraphCanvas`). They use the app's `ui` toolkit (text input, virtual list, theme colours), so
  they look like the rest of the editor, and no editor has a private copy. A host supplies
  `GraphProfile::catalog()` (`CatalogEntry`: title, category, description, icon, colour, input
  types), `menu_items` / `run_menu_item`, and handles `CanvasEvent::NodePicked` (add the node, using
  `connect_new_node_commands` to wire a dragged wire) and `Action::Custom` (a domain menu item).
  Filtering by a dragged wire's type is `GraphProfile::drag_match` (type resolver by default).
- **Positions**: popups are anchored at the pointer. `Action::ContextMenu.at` and
  `Action::ConnectionDropped.at` are canvas-relative pixels; `ConnectionDropped.graph_at` is the graph
  position for the new node. (`at` used to be the graph position, which hosts mapped as if it were
  canvas-local, so wire-drop popups opened in the wrong place under pan/zoom.)
- **`GpuiProfile::render_slot` receives the document.** It runs while the canvas renders inside its own
  `update`; reading the `GraphCanvas` entity there panics ("cannot read GraphCanvas while it is
  already being updated").
- **Theme**: `graphy_render::Theme::from_colors(&ThemeColors)` derives canvas, card body/header,
  border, grid (minor/major), text tiers and shadows from the host's background/foreground/muted/
  border/accent, pushing text to WCAG 7 / 4.5 / 3 on every surface. `Theme::dark()` (the fallback) has
  canvas `#171717` (the Default Dark `secondary.background`; hosts pass the theme's `secondary` as `ThemeColors::canvas`, node bodies take the theme background, headers and borders are mixed from it, pin labels use the theme text colour). Each editor maps its app theme with a small `theme::graph_theme(cx)` and
  keeps the canvas current with `GraphCanvas::set_theme`.
