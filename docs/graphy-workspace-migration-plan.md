# Graphy Workspace Migration Plan

## Claude Code implementation prompt

You are working in `D:\GitHub\Pulsar-Native`, a Rust game engine whose
editor is also the runtime. Read `AGENTS.md` and the relevant `.agents/`
architecture notes before changing code. The goal is to make Graphy the shared
graph platform used by graph-based tools, then build the animation graph editor
on that platform. **Complete and validate the Graphy migration before starting
the animation asset/runtime refactor.** Do not create another private node
canvas inside the animation plugin.

Treat this as a multi-repository migration. Graphy is a Git submodule at
`crates/third-party/graphy`; Blueprint Editor and Shader Editor are separate
plugin submodules. Follow `.agents/CRATES.md`'s dependency-first merge and
submodule pin order. Preserve existing user changes and branches. Do not reset,
force-push, merge, or close anything without an explicit request.

This is also a consolidation and deletion effort. Do not preserve obsolete
graph implementations as forwarding wrappers after Graphy takes ownership.
Move reusable behavior to Graphy, migrate every consumer directly, and remove
the old copies and their workspace/dependency/documentation entries.

First produce an inventory and a migration sequence. Then implement in small,
reviewable repository-sized changes. Keep animation formats, component fields,
and animation editor work out of the Graphy migration changes. Stop at the
animation gate at the end of this plan and report the result.

## Research findings to preserve

- `crates/third-party/graphy` is already an upstream Graphy submodule, but its
  current package is compiler-oriented. It contains graph/compiler structures,
  type descriptors, analysis, diagnostics, code generation and Rust AST
  helpers. It has no shared GPUI/WGPU graph canvas, node renderer, input system,
  or Pulsar Reflection adapter.
- `plugins/vendor/blueprint_editor/graph` is a separate, GPUI-free
  `blueprint_graph` crate. It owns the persisted Blueprint schema, codecs,
  legacy migration, prefab/library data and view data. It currently has no
  Graphy dependency, so the engine has overlapping graph representations.
- `crates/core/pulsar_game` directly depends on the plugin-local
  `blueprint_graph` package for game-side Blueprint loading. That creates an
  engine-to-editor-submodule dependency and is a concrete candidate to move
  into Graphy's Blueprint schema package. The root workspace and CI also
  explicitly enroll/expect `blueprint_graph` as a vendor workspace member.
- `.agents/CRATES.md` still lists `pulsar_graph` and `pulsar_bp_executor` core
  crates, while the current filesystem/workspace contains neither. Treat these
  as stale ownership documentation to reconcile, not as missing crates to
  recreate.
- `plugins/vendor/blueprint_editor/compiler` consumes `blueprint_graph` and
  Graphy. The Blueprint plugin owns its graph runtime state, rendering,
  hit testing, interaction features and editor shell. The canvas includes a
  custom GPU renderer and WGSL shaders under `src/rendering/gpu/`, plus
  geometry/layout, spatial indexing and input handlers.
- `plugins/vendor/shader_editor` has a second graph state, renderer, input and
  node interaction implementation under its own `src/core/` and
  `src/rendering/`. It depends on `blueprint_graph` and Graphy but also has
  shader-specific types and pin previews. Do not assume its needs are a strict
  subset of Blueprint's.
- `plugins/vendor/render_bps` already reuses Blueprint's editor via
  `blueprint_editor_plugin::profile::EditorProfile`. Its asset model,
  conversion, Helio validation/compiler, side panels and live preview belong
  to the render-graph plugin. It is the first consumer that proves the common
  editor API supports different graph semantics without copying the canvas.
- `plugins/vendor/asset_viewer/src/editor/graph_material.rs` previews Helio
  shader-graph materials through the renderer's graph preview; it is a
  material preview, not another node-canvas implementation. Physics collision
  layer diagrams and the UI widget tree are domain-specific views, not node
  graph editors. Do not migrate them merely because they contain the word
  “graph” or “node.”
- Blueprint nodes already resolve Pulsar types through
  `pulsar_reflection::RUNTIME_TYPE_REGISTRY`, use stable type names for pins,
  and query `CONVERSION_REGISTRY`. Shader graphs also have shader-language
  types such as WGSL vectors and textures that are not Pulsar runtime types.
  The shared type API must support both without inventing a competing global
  runtime type registry.
- Pulsar plugins are separate Rust `cdylib`s. Plugin libraries remain loaded
  for process lifetime, and world/reflection state is attached through the
  existing runtime tables. Avoid APIs that depend on `TypeId` identity,
  cross-DLL statics, or unloading. Prefer stable type names/IDs and explicit
  host-provided contexts at plugin boundaries.

Verify these findings against the current checkout before implementing; code
may have moved. Relevant starting points:

- `crates/third-party/graphy/src/`
- `plugins/vendor/blueprint_editor/graph/src/`
- `plugins/vendor/blueprint_editor/compiler/src/`
- `plugins/vendor/blueprint_editor/src/rendering/`
- `plugins/vendor/blueprint_editor/src/features/`
- `plugins/vendor/blueprint_editor/src/profile.rs`
- `plugins/vendor/shader_editor/src/core/`
- `plugins/vendor/shader_editor/src/rendering/`
- `plugins/vendor/render_bps/src/model.rs` and `src/lib.rs`
- `crates/external/pulsar-reflection/`
- `.agents/CRATES.md`, `.agents/PLUGIN_SYSTEM.md`, `.agents/REFLECTION.md`,
  `.agents/UI_CRATES.md`, `.agents/RENDER_GRAPH.md`

## Target architecture

Graphy is an upstream **Cargo workspace** and a shared graph platform. It
contains rendering crates, compiler crates, and helper crates for type and
Pulsar Reflection integration. Keep the dependency direction one-way:

```text
Graphy core and graph contracts
   ├── Graphy compiler crates
   ├── Graphy reflection/type adapters
   └── Graphy rendering and interaction crates
          └── domain editor plugins and their surrounding editor UI
```

The precise package names may change after the inventory, but keep package
boundaries explicit and independently testable. A sensible initial split is:

| Workspace crate | Owns | Must not own |
|---|---|---|
| `graphy-core` | Stable graph/node/pin/edge IDs, authored graph contracts, generic type keys and node definitions, invariants, graph edits/change sets | GPUI, WGPU, Blueprint-specific asset fields, Pulsar `TypeId` |
| `graphy-blueprint` | Blueprint document/class schema, local subgraphs/macros, prefab/library metadata and legacy codecs, built on Graphy's shared graph document | Editor UI, GPUI, script VM execution, renderer/compiler implementations |
| `graphy-compiler` | Graph validation/analysis pipeline, diagnostics, compiler interfaces and reusable codegen utilities | Blueprint runtime semantics, Helio pass construction, PSGC shader semantics |
| `graphy-reflection` (or `graphy-pulsar-reflection`) | Adapter from stable Graphy type keys to Pulsar Reflection metadata/conversions and reflected node metadata | A private copy of Pulsar's type registry, process-global state, editor shell |
| `graphy-render` | Shared layout/geometry, view transforms, culling, spatial indexing and renderer-neutral draw data | Asset loading, editor tabs, property inspector, domain compilers |
| `graphy-render-wgpu` | Common WGPU graph drawing, GPU resources/shaders and lifecycle using the host's device/queue | Creating a second engine device, assuming Blueprint node meanings |
| `graphy-gpui` | Reusable GPUI canvas, input-to-command translation, shared interactions and extension/profile API | Project browser, asset tabs, file format policy, Blueprint-only panels |

Do not force every consumer to depend on every crate. Compiler-only builds
must remain headless and avoid GPUI/WGPU. Do not create a re-export-only
facade, a compatibility crate with no owning behavior, or a new indirection
layer just to preserve an old crate name. Keep Graphy's existing `graphy`
package as a substantive owner of compiler/core APIs, or rename its direct
consumers to the new workspace packages in one deliberate migration. Remove
temporary compatibility packages once direct consumers have moved.

Graphy should own reusable graph behavior: drawing and hit geometry must use
the same layout model; graph/screen coordinate transforms, pan/zoom, grid,
selection, marquee, node/comment movement and resize, pin connection drags,
disconnect, reroute, clipboard, context-menu request, snapping, culling and
viewport framing should have one implementation. Express edits as explicit
commands/operations with change sets so an editor can save, mark dirty, undo,
redo, validate or broadcast them. Keep domain-specific policies injectable.

The renderer must support extension points for node title/category, pin shape
and color, variable/dynamic pins, per-node custom body content, inline pin
widgets, previews, comments/sections and overlays. Shader texture/value
previews and Blueprint-specific collapsed subgraphs must remain possible
through these extension points. **Do not simply extract the current renderer
and call the migration complete:** Graphy's shared node and canvas renderer
must receive a deliberate visual redesign. Use
`docs/mockups/animation-editor.html` as the visual-quality reference for the
node cards and graph surface. Keep this work strictly inside the graph canvas:
do not redesign the surrounding editor panels, toolbar, tabs, inspector,
preview layout or editor shell.

### Graph canvas visual direction (required)

The current Blueprint renderer already has a substantial custom GPU pipeline;
the issue is its visual language and consistency, not a lack of a renderer.
Study its node, wire, pin, text and grid shaders, then redesign them as a
coherent system inspired by the mockup's clean, layered node cards and quiet
dark canvas. Translate the reference into the engine's GPU renderer rather
than copying CSS literally.

- **Nodes:** create a clear card silhouette with restrained depth, clean corner
  treatment, a distinct but compact title header, an understated category
  accent, readable title/icon hierarchy, deliberate spacing and a calmer body
  surface. Keep category color as an accent instead of flooding large areas.
  Give selected, hovered, active/running, disabled and focused nodes distinct
  readable states that remain legible at every supported zoom level.
- **Pins and rows:** align input/output rows precisely; make pin shape/type,
  labels and inline values readable without crowding. Ensure dynamic pin counts,
  long labels, return values, reroutes and compact nodes have predictable
  geometry. Connection hit targets must remain generous even when the visible
  pin is small.
- **Wires:** refine bezier and angular connections, endpoint joins, stroke
  weight, antialiasing, type color and interaction emphasis. Connected,
  hovered, selected, compatible-target and inactive wires need a deliberate
  visual hierarchy. Prevent dense intersections from becoming a bright,
  tangled mass.
- **Canvas:** make the dark graph surface feel intentional and quiet, with
  subtle infinite minor/major grid lines, consistent pan/zoom anchoring and
  enough contrast to orient the user without competing with nodes. Keep grid
  spacing and intensity stable across zoom and high-DPI scales.
- **Rendering quality:** maintain crisp text, edges, corners and pin glyphs at
  fractional zoom and high DPI. Use zoom-tier detail/LOD intentionally so the
  overview stays clean and the close view stays useful. Preserve instancing,
  culling and batched GPU work; visual depth effects must not add a costly
  per-node UI tree or an unbounded draw-call increase.
- **Visual acceptance:** review side-by-side with the web reference at matching
  canvas sizes and dark theme. Include clean graphs and crowded graphs with
  mixed node categories, many pins, comments, reroutes, texture previews and
  active wires. Capture stable GPU screenshots for normal, hover, selection,
  connection-drag and low/high zoom states. Compare Blueprint, Shader and
  Render Graph so profile-specific visuals feel coherent without erasing
  domain-specific pin and node identity.

The host editor continues to own surrounding panels, properties, palettes,
asset references, preview scenes, file I/O and document lifecycle.

Define a domain-neutral profile/adapter contract for each consumer to supply:

- graph asset decode/encode or conversion to/from its own persisted schema;
- node catalogue and domain type resolver;
- pin appearance, node presentation and custom pin/node widgets;
- dynamic pin refresh and domain validation/compiler diagnostics;
- optional graph-specific side panels, preview panels and post-apply callbacks.

Move the reusable canvas/profile contract out of `blueprint_editor_plugin` so
`render_bps`, Shader Editor and the future animation graph editor depend on
Graphy directly. `render_bps` keeps its own `helio_graph_asset` conversion,
Helio validation, pass probing, buffers/sections panels and pipeline preview.

Pulsar Reflection integration must use stable type names as cross-plugin
identity. Resolve types and conversion candidates through the shared reflection
registries at the host boundary. Permit domain resolvers to augment or override
presentation/compatibility for shader-language types, animation poses and
other domain values. Define behavior for unknown, wildcard, generic, optional,
enum, struct and execution pins. Never equate `TypeId` values from different
plugin DLLs or serialize reflection pointers into graph assets.

Keep persisted domain assets authoritative. Graphy may supply a generic graph
document or editing model, but use adapters/migrations to preserve existing
`.blueprint`, shader/material, and `.rendergraph` formats. Do not silently
rewrite whole assets just to adopt a new in-memory graph type. Separate
authored graph data, editor-persisted layout/view metadata, and transient
selection/drag/preview state.

## Consolidation and deletion rules

The purpose of this migration is to reduce the graph ecosystem to one real
shared implementation in Graphy and remove large amounts of duplicated or
pass-through code from Pulsar-Native and the editor plugins. A successful
migration must leave **fewer owners and fewer graph abstractions**, not the old
systems hidden behind a new Graphy API.

1. Build an ownership table for every graph-related crate/module, manifest
   entry, feature flag, workspace member, CI allowlist, test/fixture and doc.
   Classify each as one of: move to Graphy, remain domain-owned, temporary
   migration adapter, or delete as stale/unused. Include reverse dependencies
   and prove a file is unused before deleting it.
2. Move generic document structures and graph algorithms into Graphy. Move the
   existing `blueprint_graph` package's reusable Blueprint asset contract into
   a real `graphy-blueprint` workspace crate so both the Blueprint plugin and
   `pulsar_game` use it directly. Keep Blueprint schema behavior there, but do
   not put UI or script execution there.
3. Migrate `pulsar_game` to the Graphy-owned Blueprint schema, then remove the
   root workspace's path dependency on `plugins/vendor/blueprint_editor/graph`
   and stop treating the editor plugin's nested graph package as an engine
   crate. Update root manifests, lockfiles, CI workspace-member expectations,
   tests and docs together.
4. Make Graphy's shared graph document/editor model the sole mutable node/edge
   document during an editing session. Do not keep a Blueprint graph, a Shader
   graph and a Graphy graph synchronized through whole-document copies. Domain
   formats may retain their own metadata and codec, but convert at load/save
   boundaries only. Any boundary adapter must preserve data, have tests, and
   be deleted if it only forwards or mirrors the same fields.
5. Move shared renderer, graph layout/geometry, spatial index, hit testing,
   coordinate transforms, common selection/drag/connect operations and generic
   undoable edit commands into Graphy. Delete the Blueprint/Shader copies after
   their consumers call Graphy directly. Delete any main-repo convenience
   crate/module that only re-exports or forwards those APIs.
6. Keep only domain behavior in domain owners: Blueprint node catalog,
   compiler-to-script lowering and Blueprint-specific editor panels; PSGC,
   shader node types and material/shader preview; Helio render-graph asset
   schema, pass registry/compiler, buffer/section policy and pipeline preview;
   animation semantics in its later animation crates; engine/runtime graph
   execution in its runtime owners.
7. Do **not** move unrelated graph-shaped systems into Graphy: Helio's runtime
   render-frame scheduler/executor/resource lifetime graph, engine startup
   InitGraph, script VM/runtime, SceneDB hierarchy, collision-layer diagrams,
   UI widget trees, profiler flamegraphs and Helio material preview execution
   are not node-canvas infrastructure. Share graph primitives only where a
   real dependency and API fit exists; avoid renaming or wrapping these systems
   just to make every use of “graph” look uniform.
8. Remove stale crate rows, compatibility shims, duplicate shaders, dead
   renderer/input modules, duplicated node/pin types, obsolete dependency
   patches, lockfile packages and CI exceptions after the final consumer moves.
   Do not retain an empty crate or wrapper for an unplanned future use.
9. Report exact files/crates removed, lines removed and moved, dependencies
   eliminated, and any adapters that remain with the reason and removal
   condition. LOC is an audit metric, not the definition of success: behavior,
   formats and tests must remain intact.

## Migration phases

### Phase 0 — Inventory and performance baseline

Before changing implementation:

1. Map the current graph document and compiler types, all renderer/input
   modules, node/pin layout contracts, profile APIs and tests in both Blueprint
   and Shader plugins. Identify exact duplicated and domain-specific code.
2. Trace render graph conversion and all calls from `render_bps` to the
   Blueprint profile/canvas. Trace the shader graph's custom types, dynamic
   pins, texture previews and compiler path through PSGC.
3. Trace all main-repo consumers of `blueprint_graph`, Graphy and graph
   compilers/renderers. Audit workspace membership and CI checks; in
   particular, verify `pulsar_game`'s dependency on the plugin-local graph
   schema and stale `pulsar_graph`/`pulsar_bp_executor` crate documentation.
4. Record serialization fixtures and compatibility paths for Blueprint,
   shader/material and render graph assets.
5. Capture a reproducible baseline for both graph renderers: small (about 100),
   medium (about 1,000) and stress (about 10,000) nodes/connections where the
   existing harness permits. Record machine/GPU/driver/build/profile and
   median/p95 CPU frame preparation, GPU draw time, pan/zoom frame rate,
   hit-test latency, memory and compile latency separately. Do not claim a
   speedup until measured against this baseline.
6. Present the proposed crate dependency graph, file ownership/deletion map,
   expected dependency and line-count reductions, migration risks and exact
   test commands. Do not start animation work.

### Phase 1 — Graphy workspace and contracts

1. Convert the Graphy repository into a Cargo workspace with a documented
   dependency DAG and independent headless/compiler versus UI/render features.
2. Decide which current Graphy graph types become core document types versus
   compiler IR. Move the `blueprint_graph` asset contract into
   `graphy-blueprint`; inventory existing callers and design only the
   necessary load/save compatibility adapters. Do not keep a second in-memory
   graph schema after migration.
3. Define stable IDs, graph invariants, generic type resolver interfaces,
   diagnostics, graph edit commands/change sets, rendering inputs and profile
   extension points. Document ownership and serialization boundaries.
4. Keep valuable compiler APIs source-compatible where practical. If package
   or API renaming is necessary, migrate direct consumers in the same planned
   change and remove obsolete compatibility code; do not leave an ambiguous
   half-old/half-new public surface.
5. Add workspace-level tests for stable serialization, IDs, endpoint integrity,
   dynamic pins, deterministic edits, type resolution and diagnostics.

### Phase 2 — Pulsar Reflection adapter

1. Add the Graphy adapter for Pulsar `RuntimeTypeInfo`, methods/properties,
   conversions and node metadata without making `graphy-core` depend on
   Pulsar.
2. Resolve names on demand through the attached runtime registry; do not cache
   stale reflection pointers or duplicate global registries in each plugin.
3. Add domain resolver composition so WGSL types remain valid and distinct
   from reflected Pulsar types.
4. Test primitive, struct, enum, wrapper/generic, wildcard, execution, unknown
   types, conversion insertion, missing registrations and plugin attachment.

### Phase 3 — Shared renderer visual redesign, rendering and interaction

1. Record the visual reference targets and create representative graph scenes
   before changing shaders. Agree the node/card, type/pin, wire and grid visual
   tokens against `docs/mockups/animation-editor.html`. This is a renderer
   redesign, not a CSS-to-WGSL transcription.
2. Port and unify the common geometry/layout contract from Blueprint and
   Shader into Graphy. Make rendered node, label, pin, wire and hit-test
   geometry derive from the same layout model; remove duplicated coordinate
   and pin-position logic.
3. Implement the visual redesign in the shared GPU renderer incrementally:
   grid and canvas, card body/header/accent, dynamic pins and rows,
   bezier/angular wires and joins, comments/sections, text hierarchy,
   selection/hover/active states, zoom-tier LOD, culling and overlays. Inject
   the host GPU device/queue and respect GPUI/plugin lifetimes.
4. Move input translation and hit testing to the same Graphy layout model.
   Shared input produces domain-neutral graph commands; the editor applies
   commands through its document/undo/save path.
5. Add extension hooks for shader inline texture previews and Blueprint macro
   navigation, custom widgets, node menus, and render-graph sections/buffers.
6. Use common interaction tests, GPU screenshot/golden tests and the Phase 0
   performance baseline. Keep editor chrome outside Graphy and iterate until
   the shared canvas has the visual quality of the web reference without
   losing performance or domain-specific affordances.

### Phase 4 — Blueprint Editor migration

1. Make Blueprint the first Graphy canvas consumer. Replace its private graph
   presentation and input implementation with Graphy while preserving its
   Blueprint-specific nodes, compiler integration, macros/subgraphs,
   variables, events, prefabs, properties, AI tools and asset UI.
2. Replace the plugin-local `blueprint_graph` package with
   `graphy-blueprint`. Convert old formats at asset boundaries and avoid
   retaining a plugin-local duplicate schema or a permanent re-export shim.
3. Preserve undo/redo, dirty state, save/load, copy/paste, selection, zoom,
   comments, dynamic pins, reroutes, auto-layout, subgraph tabs and diagnostics.
4. Run the plugin's own tests and workspace tests against current fixture files;
   prove legacy/current assets round-trip without lost fields or ID churn.

### Phase 5 — Render Graph profile migration

1. Move the existing `EditorProfile` contract to Graphy's GPUI/editor adapter.
2. Port `render_bps` to that API while keeping its model adapter, node catalog,
   validation, dynamic Helio pins, `Buffers`/`Sections` panels, preview and
   `.rendergraph` domain semantics in `render_bps`. First determine whether its
   current `model.rs` is doing necessary file-format adaptation or copying a
   second editable graph. Keep only required boundary conversion; edit a
   single Graphy document in memory.
3. Test that render graph nodes/sections, buffer flattening, pin updates,
   compile diagnostics and preview application remain equivalent.

### Phase 6 — Shader Editor migration

1. Replace Shader Editor's second canvas renderer, layout/hit-testing and
   general interaction state with Graphy.
2. Retain shader-specific type resolution, PSGC compilation, shader node
   semantics, asset format, material previews, texture previews and custom
   property controls in Shader Editor or its compiler crates.
3. Test dynamic texture pins, previews, shader-specific compatibility,
   serialization, compilation and rendering on GPU-capable CI/dev machines.

### Phase 7 — Compatibility, performance and animation gate

1. Run all Graphy workspace tests, Blueprint compiler/schema tests, both
   plugin suites, `render_bps` tests, root `just check`, `just test` where
   supported and relevant CI workflows. Report unavailable GPU tests clearly.
2. Compare all Phase 0 metrics on the same machine and build profile. Explain
   regressions, investigate them, and publish measured before/after values;
   do not infer benefits from fewer source lines.
3. Confirm Blueprint, Shader and Render Graph no longer own duplicate general
   graph renderers/coordinate systems/input implementations. Document any
   intentionally specialized renderer code with an owner and reason.
4. Update `.agents/CRATES.md`, relevant Graphy/render graph docs, workspace
   manifests, lockfiles, CI, test commands and submodule pin exceptions.
5. Only when all consumers pass, mark Graphy migration complete and write a
   short animation integration contract. Then begin the separate animation
   refactor using Graphy's profile/canvas. Animation must be a consumer, not a
   new graph framework.

Animation is a separate follow-on after this gate. Its asset, runtime, editor,
and UI scope is specified in
[`animation-system-implementation-plan.md`](animation-system-implementation-plan.md).

## Cross-repository delivery order

Respect the submodule guidance in `.agents/CRATES.md`:

1. Graphy workspace/API PR and tests land in Graphy's default branch.
2. If the adapter requires new Pulsar Reflection APIs, land that dependency
   change in Pulsar-Reflection first, then update Graphy's dependency revision.
3. Blueprint Editor migration PR lands in its default branch.
4. Shader Editor migration PR lands in its default branch.
5. Update/re-pin Graphy and the two plugin submodules in Pulsar-Native; update
   workspace patches and dependency metadata to point all consumers at the same
   Graphy package revisions.
6. Migrate and validate `render_bps` in Pulsar-Native after its shared profile
   API is available, then update architecture docs/CI.
7. Run the animation follow-on only after Graphy, Blueprint, Shader and Render
   Graph consumers meet Phase 7; keep its asset/runtime/editor changes in
   separately reviewable repository changes.

If plugin changes depend on an unmerged Graphy API, use the documented
submodule-pin exception workflow for review and remove the exception after the
Graphy commit reaches its default branch. Never leave main pinned to an
unmerged dependency branch.

## Definition of done

- Graphy is a tested multi-crate workspace with compiler-only consumers free
  of UI/GPU dependencies.
- Reusable graph schemas/algorithms and the Blueprint graph contract have one
  owner under Graphy; `pulsar_game` no longer depends on a graph crate nested
  inside an editor plugin.
- Blueprint and Shader use one shared renderer, layout, hit-testing and common
  interaction implementation.
- The shared Graphy canvas has a reviewed visual redesign for node cards,
  pins, wires and the graph surface; screenshot comparisons cover state and
  zoom variations, and GPU performance is measured against the old renderer.
- Render Graph and future Animation Graph can supply their own asset adapters,
  node/type catalogs, validation/compilation, panels and previews without
  copying a canvas or introducing circular references.
- Pulsar Reflection integration resolves stable names through the shared
  registry and supports domain-specific types without a second runtime type
  system.
- Existing graph assets round-trip with compatibility fixtures and no
  unintended data loss or ID churn.
- Duplicate renderers, mutable graph-model copies, forwarding wrappers and
  obsolete workspace/dependency entries have been removed; every retained
  graph crate has substantive ownership documented.
- Performance is measured against recorded baselines and regressions are
  explained or fixed.
- CI/test commands, documentation, workspace manifests, lockfiles and
  submodule pins match the new workspace boundaries.
- No animation asset, runtime component, montage, notify or animation editor
  refactor starts before this gate. After it, follow
  [`animation-system-implementation-plan.md`](animation-system-implementation-plan.md).

At each phase, report changed repositories, commits/PRs if created, tests,
performance evidence, remaining blockers and the next dependency gate. Do not
claim completion from a successful compile alone.
