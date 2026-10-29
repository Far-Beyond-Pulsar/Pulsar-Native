# Graph canvas (Graphy render + interaction)

The shared node-graph canvas lives in the Graphy submodule
(`crates/third-party/graphy`). The Blueprint (Phase 4) and Render Graph (Phase 5) editors use it, each below; the
Shader editor is migrating onto it (Phase 6 of `docs/graphy-workspace-migration-plan.md`),
and until it moves its own renderer under `plugins/vendor/shader_editor/src/rendering/`
is still the one in use.

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
