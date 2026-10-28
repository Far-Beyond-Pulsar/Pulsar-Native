# Graph canvas (Graphy render + interaction)

The shared node-graph canvas lives in the Graphy submodule
(`crates/third-party/graphy`). Blueprint, Shader and Render Graph editors are
migrating onto it (Phases 4-6 of `docs/graphy-workspace-migration-plan.md`); until
a consumer moves, its own renderer under `plugins/vendor/*/src/rendering/` is
still the one in use.

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
macro/subgraph navigation, menu items, overlays (`decorate`). `GpuiProfile`:
GPUI content for host rows (texture previews, custom bodies).

## Reviewing the visuals

```
cargo run --release -p graphy-render-wgpu --example render_scenes
```

writes PNGs to `target/graphy-scenes/` (offscreen, no window). See
`crates/graphy-render-wgpu/README.md` for the visual system and LOD tiers.

## Blueprint (Phase 4, in progress)

`plugins/vendor/blueprint_editor` is the first consumer. Landed so far: the
authored-schema boundary `io::canvas_doc` (`from_authored` / `to_authored`, the only
conversion between `.blueprint` and the editing `graphy::GraphDescription`) and
`canvas_profile::BlueprintProfile` (catalogue, node factory, pin styles, type rules,
reroute inference, subgraph navigation). The editor still draws and edits through its
private `BlueprintGraph`/`rendering/` until the canvas is mounted; the removal order is
in the Phase 4 hand-over note. Core `GraphComment` now carries `id`, `color` and
`contained_node_ids`, and `Connection` an optional `id`, so authored assets round-trip
without id churn.
