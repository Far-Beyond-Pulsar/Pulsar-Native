# Plugin API v2: self-contained plugins (proposal)

Status: **Phase 0 and Phase 1 done** (see below); Phases 2-6 are proposals. Numbers come from
line counts of `*.rs` files (see "Move-out estimate"), not from a coupling
analysis, so treat them as upper bounds until Phase 0 is done.

## Goal

A plugin is a complete, self-contained extension of the engine. One plugin can
contribute, end to end:

```
file type -> editor -> component -> GPU data -> render pass / graph node
```

and can depend on other plugins, so a plugin builds on a component another
plugin defines. The engine core keeps only what plugins plug *into*.

## What exists today (and the gaps)

| Need | Today | Gap |
|---|---|---|
| File types, editors, panel restorers | `EditorPlugin`, `EditorPluginEditor`, `PermanentLibrary` | Works. Keep. |
| Typed World components in a DLL | `#[register_world_component]` plus world-runtime attach (#1083) | Works. Plugin gets typed access only to its own types. |
| Component *definitions* | `EditorPluginComponents` hands back `Box<dyn EngineClass>` | Only a property-panel factory. No GPU schema, no systems, no requirements. |
| GPU data from a component | `#[derive(SceneStore)]` `#[gpu]` fields; buffer keys in `scene_buffers` | Schemas and buffer keys are not registered *by plugins*; `scene_inventory/ledger.toml` is one central list. |
| Render passes | `NodeRegistry::register` (`helio-default-passes/graph_nodes.rs`), `IntoGraph`, `RenderPass::scene_buffers` | The registry is built in the host. A plugin cannot add a node or a pass. `helio-graph-asset` is already pass-free, which is the right seam. |
| Plugin dependencies | none (`PluginMetadata` has id, name, version, author) | No `requires`, no load order, no version check between plugins. |
| Importers, thumbnails, companions | link-time `inventory`, host registries | "Not yet wired for DLL plugins" (PLUGIN_SYSTEM.md). |
| Higher-order components | component-instance entities (`attachments`) exist | No way to declare "this class is made of those classes". |

The shape of `EditorPluginFull` is the root problem: every new capability adds
a trait to a combined super-trait, which changes the vtable. v2 replaces that
growth path.

## Design

### 1. One entry point: a registrar, not a super-trait

```rust
pub trait Plugin: Send + Sync {
    fn manifest(&self) -> &'static PluginManifest;
    fn build(&'static self, p: &mut PluginBuilder);
}
```

`PluginBuilder` exposes one section per capability. Adding a section never
changes the FFI surface (`_plugin_create` returns one object):

| Section | Registers |
|---|---|
| `p.files()` | file types, thumbnails, importers, import companions |
| `p.editors()` | editor factories, panel restorers, statusbar, menus |
| `p.components()` | component classes (below) |
| `p.systems()` | lifecycle and tick systems, with ordering labels |
| `p.gpu()` | GPU schemas and named scene buffers |
| `p.render()` | graph nodes, passes, pipelines, host inputs |
| `p.scripting()` | script natives, trait roles |
| `p.ai()` / `p.settings()` / `p.events()` | as today |

The existing `EditorPlugin*` traits stay as a compatibility shim: a blanket
adapter turns an old plugin into a `build` call, so the six vendored plugins
migrate one at a time.

### 2. Manifest and dependencies

```toml
[plugin]
id = "pulsar.character"
version = "0.1.0"
api = "2"

[requires]
"pulsar.skeletal"  = "^0.1"
"pulsar.physics"   = "^0.1"

[optional]
"pulsar.input"     = "^0.1"
```

- The host builds a graph from `requires`, loads in topological order, rejects
  cycles and unmet versions with a named error, and **disables dependents** of
  a failed plugin instead of crashing.
- Missing plugin at project open: its components load as `UnresolvedComponent`
  and its tabs stay dormant. Both mechanisms already exist; this reuses them for
  plugins.
- A plugin reaches another plugin's types by depending on that plugin's **API
  crate** (plain rlib: components, traits, no statics). See "The hard part".

### 3. Components, including higher-order ones

```rust
p.components().class::<Character>(ClassSpec {
    requires: &[class::<SkeletalMesh>(), class::<CharacterController>()],
    gpu: None,
    systems: &[character_tick],
    presentation: Facade,   // one inspector card, merged properties
});
```

- `requires` is resolved when the class is placed: the host attaches the
  required classes as component-instance entities (the `attachments` model from
  SCENEDB_MIGRATION.md), owned by the parent, hidden or nested per
  `presentation`.
- `Facade` presents one monolithic component in the inspector and in scripts
  while the data stays split across the real components. That gives the
  user-visible "character is one component" with the code split they want.
- A dependency's required class must come from a plugin listed in `requires`;
  the loader checks it.

### 4. GPU data

- A component's `#[gpu]` schema registers through `p.gpu()` with an owner
  (the plugin id) and a buffer key. Keys are namespaced: `character.pose_rows`.
- `scene_inventory` becomes a per-plugin manifest check instead of one ledger:
  each plugin's tests assert its own rows; the core ledger keeps core rows only.
- Passes still declare `scene_buffers`; the render probe already derives graph
  pins from that, so a plugin buffer shows up as a `scenedb.<key>` node with no
  extra editor work.

### 5. Render passes

```rust
p.render().pass_node(PassNode::new("character.skin_resolve")
    .group(Group::Geometry)
    .params(&[...])
    .build(|ctx| SkinResolvePass::new(ctx)));
p.render().pipeline(...);      // optional, an IntoGraph
```

- The host owns the render graph (allowed by the brief). `helio-graph-asset`
  stays pass-free; plugins add `NodeDescriptor`s to the process registry before
  the one-time probe runs. The probe then generates pins for plugin passes the
  same way as for built-ins.
- Default pipelines reference nodes by string id. A graph naming a node from an
  absent plugin reports the missing plugin by name and falls back (existing
  fallback path), mirroring dormant tabs.
- `RenderPass` and its context types move to a thin **`helio-pass-api`** crate
  with no statics, so a plugin links only that. The Helio renderer itself stays
  in the host. Anything with process-wide state (shader hot reload registry,
  `helio-core/src/shader/hot.rs`) needs the shared-runtime table treatment, or
  stays host-side behind the API.
- This also resolves the open item in RENDER_GRAPH.md: voxel passes are added
  by the legacy builder because they cannot register nodes. They become the
  first plugin-registered nodes.

### 6. Dual build: DLL in the editor, linked in a shipped game

The same `Plugin` impl builds as a `cdylib` (editor) or links statically
(packaged game); `physics_plugin` already has a `builtin` feature. The registrar
makes this a non-event: the host calls `build` either way. Without it, a shipped
game would need the DLLs and the rustc-lockstep rule.

## The hard part: sharing types between plugins

Each plugin statically links its own copy of shared crates; #1083 solved this
for the four world crates by attaching a runtime table. Plugin-to-plugin needs
the same answer, and the existing rule helps: **component ids follow type name
and layout, not `TypeId`**. So plugin B that compiles plugin A's API crate gets
the same component ids and can read A's components with typed access.

Needed work:

1. A layout fingerprint for each API crate, checked at load against the version
   the dependency exported, like the world-runtime ABI check (`WorldRuntimeRefused`).
2. A rule and a test (extend the `world_state` tests) that API crates contain no
   `static`, `thread_local!` or `inventory::collect!`.
3. Traits across plugins: dispatch through registered function tables or by class
   name, never `TypeId` downcasts.

If this proves too fragile, the fallback is **by-name access only** across plugin
boundaries (what plugins already do for engine types), with typed access
inside each plugin. Phase 2 tests which is needed.

## Phases

| # | Work | Proof |
|---|---|---|
| 0 | Audit: for each central crate list who depends on it, tag it core / movable / blocked. Replace guesses below with real numbers. | table in this doc |
| 1 | `Plugin` / `PluginBuilder` / manifest / dependency graph / load order. Adapter for old traits. No behaviour change. | six vendored plugins load through the adapter; a two-plugin dependency fixture loads in order and a cycle is rejected |
| 2 | Full component API: `requires`, facade, systems, per-plugin GPU schema, plugin-level inventory check. | fixture plugin B places A's component, reads it typed, round-trips a level |
| 3 | Render registration + `helio-pass-api` split. Move voxel passes to register as plugin nodes. | a fixture plugin adds a pass and a buffer; it appears in the editor with probed pins and renders |
| 4 | Importers, thumbnails, companions, and script natives over the registrar for DLLs. | shader plugin's material companion works as DLL |
| 5 | Move-outs, one domain at a time (below), each leaving the tree green. | each move deletes more than it adds in the core |
| 6 | First product: `pulsar.character` (skeletal mesh + controller + camera rig) built purely on the API. | the original motivating case |

Per the project's "finish all phases, then fix issues" preference, log
non-blocking problems in this doc and clean up after Phase 6.

## Move-out estimate

Rust lines only (`find ... -name '*.rs' | xargs cat | wc -l`). "Candidate"
means plausibly movable once the API exists, not that it has no coupling.

| Area | Lines now | Move to plugins | Notes |
|---|---|---|---|
| `helio/crates/passes` (about 50 pass crates) | about 174k | most of it | Core keeps `RenderPass` API, a minimal deferred set (gbuffer, light, post, output). Sky, water, foliage, voxel, VG, SSR, portals, decals, lens and the like become feature plugins. Biggest single win; also shrinks default Helio compile time. |
| `helio-default-passes` | 8.8k | split | `graph_nodes.rs` registration moves next to each pass; only the default graph stays. |
| `helio-component` components | 29.6k | most of it | Camera, light, mesh stay core. Voxel (about 4k with editors), skeletal (about 6k with tests), water, spline, foliage, particle, decal, portal, sprite become plugins. |
| `engine_backend` | 23.1k | about 30-40% | `scene/` voxel frame/presets, sublevels, portals, authored LOD; `subsystems/physics`; networking (about 2k+) as plugins. Core keeps window, GPU, SharedScene. |
| `pulsar_physics` | 2.1k | all | Into `physics_plugin`. |
| `pulsar_std` / `pulsar_std_bundle` | 10.5k+ | most | Blueprint standard library per domain, shipped by the plugin that owns the domain. |
| `pulsar_game` | 13.9k | partial | Freecam, camera selection and view targets are gameplay features; scripting stays. |
| `ui_level_editor` | 44.3k | about 20-30% | Per-component editing UI (voxel, foliage, spline, lens) moves with its component. |
| `helio-vg-*`, `helio-mats`, `helio-bake`, `helio-lens-optics`, `helio-xr` | about 15k | all | Each follows its pass or component. |

See "Phase 0 audit" below for the measured numbers that replace this estimate.

Stays central: reflection, SceneDB, `pulsar_world_registry`,
`pulsar_scene_model`, the graph compiler and asset (`helio-graph-asset`), the
renderer core and frame graph, shader hot reload, virtual FS, project/asset
indexing, editor shell and docking, the plugin host itself.

## Phase 0 audit (measured)

Method: `cargo metadata --offline` over the root manifest (it also resolves the
excluded Helio path crates), non-dev edges between the 171 local crates
(1.18M Rust lines), plus `grep` for component type names outside Helio. Scripts
are throwaway; rerun by dumping metadata and counting reverse edges.

### Findings that change the plan

1. **Passes are mostly leaves.** 24 of 44 pass crates (52k lines) are depended
   on by `helio-default-passes` only. They move once node registration moves
   next to the pass; no consumer changes.
2. **Voxel is the outlier.** `helio-pass-voxel-planet` is 48k lines (29% of all
   pass code) and `engine_backend` names it 30 times (`voxel_backend.rs`,
   2.1k lines, plus `scene/voxel_*`). It needs the render API *and* a component
   API that lets the backend stop naming it.
3. **`helio_component` is data + editor UI + runtime in one crate** (32k lines).
   It depends on `gpui-ce`, `ui`, `ui_common` and `plugin_editor_api`
   (voxel/stack/generator editors live in it), and on `skeletal_assets`, which
   is a *plugin-repo* crate. Ten crates depend on it, and the references are
   by name from `ui_level_editor` and `engine_backend`. Splitting it by
   component is feasible only after the level editor edits components through
   reflection instead of naming types.
4. **The "plugins" are not plugins at link time.** `ui_core` links
   `physics_plugin` and `skeletal_animation_plugin` directly, and `scene_inventory`
   links `physics_plugin`. The `builtin` mode is the real mode today; the DLL
   path is exercised by the fixtures and some editors. v2's dual build is
   therefore cheaper than feared, but the core cannot name plugin crates.
5. **`pulsar_physics` (2.1k) is a core crate the engine, level editor, level
   migrator and `scene_inventory` all call**, though `physics_plugin` owns the
   behavior. It is the cleanest first move-out to prove the pattern.
6. **No `catch_unwind` anywhere in `plugin_editor_api` / `plugin_manager`.**
   A panic in a plugin entry point crosses `extern "C"`. Add this in Phase 1.

### Component coupling (files naming the type outside Helio)

| Component | Files | Mostly in |
|---|---|---|
| StaticMesh | 44 | `ui_level_editor` 16, `engine_backend` 12, `pulsar_game` 4 |
| Voxel | 46 | `ui_level_editor` 11, `engine_backend` 9 |
| SkeletalMesh | 31 | `blueprint_editor` 3, spread thin |
| Atmosphere | 17 | `ui_level_editor` 10, `engine_backend` 6 |
| Foliage | 19 | `engine_backend` 5 |
| Water, Spline, PostProcess, Portal | 11-14 each | level editor + backend |
| ParticleEmitter, Decal, Sprite, ReflectionCapture, CameraLens, Fog | 2-5 each | trivially movable |

StaticMesh, Transform-like and Camera/Light stay core. Everything with 5 or
fewer files can move as soon as the registrar exists.

### Pass crates by move difficulty (lines)

| Tier | Passes | Lines | Blocker |
|---|---|---|---|
| A, leaf | vg-dense/cull/raster/shade/bvh, hlfs, gi, occlusion-cull, shadow, shadow-cull/dirty, foliage-gbuffer, flare, perf-overlay, transparent, hiz, ssr, sprite-cull, simple-cube, debug-overlay, indirect-dispatch, fxaa, light-cull, planar-reflection (24 crates) | 52k | only `helio-default-passes` registration |
| B, tied to a component or backend | voxel-planet, lens, water-sim, foliage-place, volumetric-fog, sky, corona, sprite-batch, decal (9 crates) | 85k | move with their component (`helio_component`, `engine_backend`) |
| C, core deferred set (`helio` depends on them) | gbuffer, deferred-light, forward-lit, postprocess, tsr, object-batch, shadow-matrix, portal-cull/instances, billboard (10 crates) | 30k | stay core, or become optional features of `helio` |
| ui | buffer-view (used by `ui_level_editor`) | 0.3k | ok |

Tier C includes passes that are optional features in practice (TSR, portals);
that is a Phase 3 decision, not a Phase 0 one.

### Revised move-out estimate

| Move | Lines | When |
|---|---|---|
| Tier A passes | about 52k | after Phase 3, no consumer changes |
| Tier B passes with their components | about 85k passes + about 15-20k component code | after Phases 2-3 and level-editor decoupling |
| `pulsar_physics` into `physics_plugin` | 2.1k | first proof, Phase 2 |
| Voxel backend in `engine_backend` | about 3-4k | with voxel-planet |
| Level-editor component panels | about 10-15k | with each component |
| `pulsar_std` per-domain bundles | up to about 10k | later, once plugins can ship Blueprint nodes |

That is roughly **150-170k lines, about 13-14% of the repo**, and about 80% of
it is pass code. This replaces the earlier 150-250k guess. It does not count
`gpui-ce`/`ui` (not ours to move).

## Phase 1 as built

- `plugin_editor_api::manifest`: `PluginManifest`, `PluginDependency` (semver requirements), `satisfies`.
- `plugin_editor_api::registrar`: `Plugin`, `PluginBuilder` with `files`, `editors`, `components`, `statusbar` and `subsystems` sections, `PluginAdapter` (presents a registrar plugin through the old traits), `export_plugin_v2!`. The GPU, render, scripting and AI sections arrive with their phases.
- `EditorPlugin::manifest` (default: no dependencies) and the `_plugin_manifest` export.
- `plugin_manager::dependencies::plan_load_order` (pure, 9 unit tests) wired into `prepare_plugins_in_dir`, `load_plugins_from_dir` and `install_prepared_plugin`.
- `catch_unwind` on `_plugin_create` and `_plugin_manifest`; `PluginCreate` now returns an `Option`.
- Tests: `plugin_manager/tests/plugin_dependencies.rs` over three real cdylib fixtures (`tests/fixtures/dependency_plugins`).
- Not done: the six vendored plugin repos still use the old traits (they compile unchanged); none declares dependencies yet. The adapter's section set is deliberately small.

## Risks and open questions

- **Duplicate statics** are the main correctness risk; every shared crate a
  plugin links needs the runtime-table pattern or must be state-free.
- **rustc lockstep and no unloading** stay: it rules out hot-reloading a plugin.
  Plugin iteration means restarting the editor. Acceptable, but say it.
- **Build cost:** each cdylib links its own copy of the world crates; dozens of
  plugins mean larger disk and memory use. The dual build in section 6 limits
  this for shipped games; for the editor, consider grouping first-party plugins
  into a few bundles (`pulsar-std-render`, `pulsar-std-gameplay`) that expose
  many plugins from one DLL.
- **Pass ordering across plugins:** nodes order by graph wires, so this mostly
  disappears for graph pipelines; the legacy `add_pass` builders cannot take
  plugin passes and should be retired first.
- **Panics across the boundary:** confirmed missing in Phase 0 (no
  `catch_unwind` in `plugin_editor_api` or `plugin_manager`). Add it to the
  `extern "C"` entry points in Phase 1.
- Open: should a plugin be allowed to *replace* a core pass node (override by
  id)? File types already allow it. Proposal: yes, explicit `replaces = ` in the
  manifest, logged.
