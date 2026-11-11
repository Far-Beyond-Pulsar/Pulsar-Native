# Crate layout

All crates live under `crates/` in category directories. The root `Cargo.toml`
defines a flat workspace — every crate is a peer, even though they're organized
in subdirectories.

```
crates/
  core/              Engine platform — everything that makes the engine tick
  editor/            GPUI-based editor panels — the workspace chrome
  subsystems/        Integration crates — physics, rendering, scene graph
  agent-providers/   AI provider backends — one crate per LLM provider
  ui/                Vendored GPUI repos (submodules with path deps)
  graphics/          Graphics-related docs and integration notes
  third-party/       Vendored smaller deps (graphy, pulsar-config)
```

## The layers (`architecture/layers.toml`, `just arch`)

Core and editor know no kind, subsystem or tool (Pulsar-Native#1605). Every crate
has a layer by directory: **kernel** (`crates/core`, `crates/editor`,
`crates/subsystems`, `crates/profiling`, and the chat's `agent_chat_core` /
`agent_chat_tools`), **tool** (the optional editor tools: `ui_git_manager`,
`ui_flamegraph`, `ui_log_viewer`, `ui_fab_search`, the agent providers), **plugin**
(`plugins/vendor`), **host** (the `engine` binary, `plugins/installed`,
`pulsar_package`, `level_migrate`, `scene_inventory`) and **foundation** (UI,
vendored, external crates, Helio). A kernel crate depends on kernel and foundation
crates only, dev dependencies included; a test that needs plugins is a host's
(`plugins/installed/tests`). Tools register through `ui_common::editor_tools` and
`agent_chat_core::AgentProviderRegistration`, and the editor binary links them.
Helio's checker enforces it, with the shrink-only `architecture/layer-violations.txt`
and `architecture/kind-names.txt` (kernel code naming a plugin's words, each with
its reason).

## core/ — Engine platform

The heart of the engine.

| Crate | Responsibility |
|---|---|
| `asset_import` | Source-asset import (FBX, OBJ, …): import database, project scan, per-format importers, conversions as editor tasks (see below) |
| `editor_task_queue` | Background task queue behind the Tasks window: progress, errors, cancellation |
| `engine` | Binary entry point, startup graph, GPUI `App` creation |
| `engine_backend` | Window and rendering backend; subsystem lifecycle; the plugins' engine seams (render features, scene rows, game hooks, level rules) |
| `engine_class_derive` | `#[derive(EngineClass)]` proc macro |
| `engine_fs` | The filesystem library every file goes through: `virtual_fs` (local, remote, P2P providers, with the plugin folders composed over them: `plugins/<id>/`, provided and changed only by their owner), the project file index and its watcher. `std::fs` is a disallowed method elsewhere (root `clippy.toml`) |
| `pulsar_traits` | Traits and gameplay roles: trait records and database, the one trait index over every implementer source (`TraitImplementerSource`, `NativeTraitImpl`), conformance, the role registry (`RoleRegistration`), the engine's trait files (its `plugins/pulsar/` folder) and roles |
| `engine_state` | Typed resource system — `StateStore`, `ResourceHandle`, `EngineContext` |
| `engine_subsystems` | Subsystem trait and registry |
| `friends_engine` | Multiplayer friends backend |
| `plugin_editor_api` | Plugin SDK — traits, types, `export_plugin!` macro |
| `plugin_manager` | Plugin host — DLL loading, registries, tool bridge |
| `profiling` | Performance tracing (Tracy) |
| `pulsar_auth` | Authentication |
| `pulsar_core` | Core types, math, utilities |
| `pulsar_docs` | Doc generation from reflected types |
| `pulsar_events` | Event bus |
| `pulsar_game` | The game runtime: tick loop, scripting, possession (plugins join through `game_hooks::GameSessionRegistration`), cameras and view targets |
| `pulsar_lsp` | LSP client integration. One engine-owned rust-analyzer; editors attach with `ensure_started` (never `start`), share documents by refcount, sync incrementally (`did_change_file_incremental`), and receive per-file `AnalyzerEvent::FileDiagnostics` |
| `pulsar_macros` | Various derive macros |
| `pulsar_reflection` | Runtime type system — `Reflectable`, `RuntimeTypeInfo`, `EngineClass`, `Subsystems` |
| `pulsar_reflection_derive` | `#[derive(Reflectable)]` proc macro |
| `pulsar_scene_model` | Scene object components (`StableId`, `Name`, `Transform`, `Visibility`, hierarchy), component-instance entities (`attachments`) and sublevel placements (`sublevels`) |
| `pulsar_world_registry` | Typed component classes in SceneDB's `World`: registration, boundary decode, object feeds, change watches, component lifecycles |
| `pulsar_settings` | Settings store (wraps `pulsar-config`) |
| `pulsar_script_vm` | Language-neutral script bytecode: module format, verifier, linker, VM, natives, shared `exec` semantics, `compiled` code contract, `migrate` planner |
| `pulsar_script_runtime` | Runs script classes on entities: lifecycle, events, transactional hot reload with state migration, saved state |
| `pulsar_script_math` | glam math types (Vec2/3/4, DVec3, Quat, Mat4) as script value types with natives |
| `pulsar_script_codegen` | Rust export: generates compiled step functions (and the `Actor` wrapper) from a script module |
| `pulsar_script_conformance` | Runs modules interpreted and as generated Rust and compares everything observable (`just conformance`) |
| `pulsar_script_ts` | TypeScript subset compiled to script modules (oxc parser, type checking against the native registry, `.d.ts` generation, field identity) |
| `pulsar_std` | Blueprint standard library (kind-neutral domains; a kind's script API is its component methods) |
| `pulsar-multiplayer-core` | Multiplayer protocol and state |
| `pulsar-relay` | Relay server client |
| `scene_inventory` | Scene-data closure ledger (the engine's rows; each plugin's in its `ledger.toml`) and architecture checks (`cargo test -p scene_inventory`); a host |
| `ui_gen_macros` | Proc macros for UI boilerplate |
| `window_manager` | Multi-window management, window definitions |

### Asset import

A project can hold raw source files next to the native assets built from
them. `asset_import` handles them:

- `scan`: importable sources with no record, linked sources whose file changed,
  and records whose source is gone. The file manager runs it as a task when a
  drawer opens and again (debounced) when an importable file changes. Each
  source is offered to the user once per session.
- `db`: the import database, `.pulsar/import_db.json`. One record per *linked*
  source: its project-relative path, content hash and size, the native asset
  it produced and the importer id. Sources the user declined are listed as
  ignored.
- `importer`: the per-format conversion, chosen by extension.
- `service`: every conversion runs as an `editor_task_queue` task. In
  `ImportMode::Link` the source stays and gets a record, so later edits are
  detected and reimported with the same options. In
  `ImportMode::ConvertInPlace` the native asset replaces the source, which
  moves to `.pulsar/trash/<timestamp>/`, and no record is kept.

Linked imports store their importer options on the corresponding record in
`.pulsar/import_db.json`. Existing linked records migrate options from the
legacy `.pulsar/import_options.json` on load. Legacy entries for convert-in-place
assets remain available as a fallback because those assets have no link record.

**Mesh default materials.** A native `.mesh` carries one `MeshMaterialSlot`
per material slot (`helio_component::mesh_cache`). `material_asset` is the
slot's default material, authored in the mesh viewer with
`mesh_cache::set_default_materials`; a reimport keeps it, matching slots by
source material index, then by name. A placed `StaticMeshComponent` slot
inherits the default unless it assigns its own material
(`StaticMeshMaterialSlot::effective_material_asset`). The component resolves
this before anything reaches Helio. Helio's passes only ever see the
effective material, never whether it was a default or an assignment.

**Rigged sources.** A model with skinned mesh nodes imports as one linked asset
set per skin (`asset_import::rig`): a `.skeleton`, a `.skmesh` skinned to it,
and one `.animseq` per animation. Each asset has a fresh id and refers to the
others by id, never path. The formats, ids, math and CPU skinning live in the
`skeletal_assets` crate (inside the `skm_editor` plugin repo; the plugin edits
the same files). `ImportRecord::asset` is the first `.skmesh`, and the optional
`outputs` list records every file (path, kind, id, source skin index, and source
animation name for sequences). A reimport reads those back, so each file keeps
its id: an output that was moved or renamed is found again by its id and
rewritten where it now is, a deleted one is recreated with the same id, and the
material slots and preview meshes chosen since are kept. A static source is
imported exactly as before.

**Animation assets.** The rest of the animation family (`.blendspace`, `.montage`,
`.animgraph`, `.animretarget`, `.ikrig`, `.ikretarget`, `.posedb`) is defined in
the same `skeletal_assets` crate, with typed `pulsar_reflection::AssetRef`
markers; `engine_fs::ProjectFileIndex` records their reference edges
(`asset_deps`) for Used By queries. The runtime (`skeletal_assets::runtime`: graph, Montages,
notify playback tracker, root motion) is headless; `helio_component::SkeletalMeshComponent`
owns the per-entity state, delivers notifies and Montage events, applies root motion and
recompiles graphs when a dependency file changes. See `.agents/ANIMATION.md`.

The animation editors are in `skm_editor` (`src/anim_editors/`: per-asset `*_model.rs` + `views/`,
`notify_edit`, `bone_mask_edit`, `skeleton_masks` (the skeleton's named mask library), `skin_preview`, `pose_db_build`); the Sequence editor and its GPU
timeline are `src/editor/` and `src/rendering/timeline/`. Shared pieces elsewhere:
`ui_common::asset_ref_editor` (typed Skeletal Mesh / Animation Graph pickers, registered through
the reflected property editor registry), `pulsar_reflection::asset_refs` (JSON codecs for the
optional refs), `graphy::authored` (the authored <-> canvas conversion used by the Blueprint and
Animation Graph editors) and `helio_component`'s `SkeletalMeshComponent` properties.
Named bone masks live on the Skeleton asset (`skeletal_assets::bone_mask::NamedBoneMask`, migration in
`skeletal_assets::mask_library`); state machine events, whole-skeleton physics blend and root-motion
authority are `helio_component` / `SkeletalMeshComponent` features (see `.agents/ANIMATION.md`).
The Helio branch for this work is `animation-phase4-clean` (animation commits plus the user's
`helio-graph-asset` zip-tie commit on top of Helio `main`; no water changes).
`helio_asset_compat::extract_rigs` reads the skeletons, weights and animations out
of the loaded scene (`ConvertedScene::rigs`). Meshes shared by nodes using the
same skin reuse geometry; different skins keep separate joint palettes and
inverse binds. Unskinned node instances still produce static `.mesh` outputs.
The glTF decoder combines duplicate joints, retains the strongest four
influences across all `JOINTS_n`/`WEIGHTS_n` pairs, and renormalizes them. A
warning per source mesh counts vertices that lost positive influences.

## editor/ — Editor panels

20 crates, one per editor panel or subsystem. Previously `ui-crates/`. Each
provides a piece of the GPUI-based editor UI shell.

| Crate | Panel / Feature |
|---|---|
| `ui_about` | About / credits dialog |
| `ui_common` | Shared widgets, i18n, styling primitives; the generic asset picker and typed asset-reference editor; the editor tools' seams (`editor_tools`) |
| `ui_core` | Editor shell — tab management, plugin wiring, statusbar (the tools' buttons from `ui_common::editor_tools`); layout persistence (`app/layout/`, `app/chrome.rs`, see `LAYOUT.md`) |
| `ui_documentation` | In-editor doc viewer |
| `ui_entry` | Window entry creation and setup |
| `ui_fab_search` | Fab asset search (a tool) |
| `ui_file_manager` | File tree and project browser |
| `ui_flamegraph` | CPU/GPU flamegraph viewer (a tool) |
| `ui_friends` | Friends list |
| `ui_git_manager` | Git integration (a tool: status-bar button, title-bar sync state, Git Settings, hook sync on load) |
| `ui_level_editor` | Level / scene editor |
| `ui_helio_viewport` | Shared Helio frame pacing and publish-driven repaint pump |
| `ui_loading_screen` | Splash + loading progress |
| `ui_log_viewer` | Mission Control (a tool) and the editor's log and memory tracking |
| `ui_multiplayer` | Multiplayer session UI and its network side (`net`: the relay client, the file sync) |
| `ui_multiuser_status` | Presence indicators |
| `ui_plugin_manager` | Plugin browser |
| `ui_problems` | Errors / warnings panel |
| `ui_settings` | Settings editor |
| `ui_type_debugger` | Runtime type inspector |
| `ui_types_common` | Shared type definitions |

### Asset thumbnails

`ui_common::thumbnails` renders and caches file-browser thumbnails, with one
renderer per extension. `ui_common::asset_thumbnails::register_mesh_thumbnail_renderer`
installs Helio's model renderer, plus every `ThumbnailRendererRegistration`
linked into the binary. A crate that knows a format `ui_common` cannot depend
on contributes its renderer at link time:

```rust
pulsar_reflection::inventory::submit! {
    ui_common::asset_thumbnails::ThumbnailRendererRegistration {
        extension: "mesh", // lower-case, no dot
        render: render_thumbnail, // fn(&Path) -> Option<image::RgbaImage>
    }
}
```

`helio_component::mesh_thumbnail` is the reference implementation (`.mesh`).

## subsystems/ — Integration crates

Kernel crates for subsystem lifecycle and scene management. What a kind
simulates (physics, input, animation) is its plugin's.

| Crate | Responsibility |
|---|---|
| `engine_subsystems` | Lifecycle orchestration — the `Subsystem` trait and `SubsystemRegistry` |
| `pulsar_scene` | Scene graph — transforms, prefab instantiation, object hierarchy |

## agent-providers/ — AI providers

One crate per LLM backend (a tool). Each implements `ProviderCrate` from
`agent_chat_core` and registers itself (`AgentProviderRegistration`); the editor
binary links the ones it ships and the chat panel names none. Tool execution is
centralized in `agent_chat_tools`. `agent_chat_core` and `agent_chat_tools` are
the editor's (kernel).

Providers: anthropic, aws_bedrock, demo_random, docker_model_runner, gemini,
github_copilot, openai, opencode, vertex_ai.

## ui/ — Vendored UI framework

Two git submodules referenced as path deps:

| Submodule | Provides | Path dep for |
|---|---|---|
| `wgpui/` | `gpui-ce` crate | The GPU-accelerated UI framework (fork of Zed) |
| `wgpui-component/` | `ui` + `ui-macros` crates | Rich UI primitives built on GPUI |

Changes to these submodules are committed in their own repositories and
pinned here (see "Submodule pins" below). They are not workspace members
(Cargo nested workspace limitation).

## third-party/ — Vendored deps

| Submodule | Path dep? | Notes |
|---|---|---|
| `graphy/` | Yes | Own Cargo workspace (excluded from the root one): `graphy` (graph IR, the `authored` bridge to the `graphy-blueprint` schema, stable ids, invariants, type resolver, edit commands, diagnostics, analysis/codegen) and `graphy-blueprint` (authored Blueprint asset schema, codecs, legacy readers, library/prefab); plus `graphy-reflection` (the only crate that knows Pulsar: `TypeResolver` and node metadata over `pulsar_reflection`); all headless, consumed as path deps. Phase 3 adds `graphy-render` (headless canvas model), `graphy-render-wgpu` (shared GPU renderer) and `graphy-gpui` (interaction engine, `GraphProfile`, GPUI canvas behind feature `gpui`; its own workspace) - see `.agents/GRAPH_CANVAS.md` |
| `pulsar-config/` | Yes | Configuration management |
| `toolbelt/` | No | Tool registry + macros (has its own workspace) |
| `psgc/` | No | Shader Graph Compiler (has its own workspace) |

## plugins/vendor/ — Plugins

Every plugin that ships with the engine lives at `plugins/vendor/<dir>/` and
describes itself in a `pulsar-plugin.toml` (its id, what it requires, its
root crate). The root crate, the package at the plugin's directory, is the
one place the plugin registers what it contributes, its editor registrations
behind its `editor` feature. The installed set (`plugins/installed`,
generated from those descriptors) links the roots; a project enables or
disables each plugin in its `Pulsar.toml` (`.agents/PLUGIN_SYSTEM.md`).

Each object kind is a plugin that owns its SceneDB components and methods,
its contracts (`api`), the Helio feature that renders it (`feature`), its
editor crate (`editor`, `<kind>_editor`), its root (`<kind>_plugin`) and its
demos (`examples`), and the rows of the scene-data closure ledger for what it
ships (`ledger.toml`, checked by `scene_inventory`); Helio knows no kind
(Helio's `docs/architecture/plugin-migration.md`). Since p18 every plugin but
two editor tools is its own repository, a git submodule at the same path. The
plugins' crates stay members of this workspace (`workspace = true`
dependencies; the root `Cargo.toml` names each one by path, never by a glob),
so they build only inside a
Pulsar-Native checkout. Clone with `--recursive` (or `just submodule-init`):
cargo cannot load the workspace without them.

| Path | Repository | Plugin |
|---|---|---|
| `materials` | [Plugin_Materials](https://github.com/Far-Beyond-Pulsar/Plugin_Materials) | `pulsar.materials`: material assets and instances, the material table, templates, the graph compiler and graph editor |
| `meshes` | [Plugin_Meshes](https://github.com/Far-Beyond-Pulsar/Plugin_Meshes) | `pulsar.meshes`: static, skeletal and virtual-geometry meshes, their drawers, culling, import and LOD |
| `lights` | [Plugin_Lights](https://github.com/Far-Beyond-Pulsar/Plugin_Lights) | `pulsar.lights`: lights and reflection captures, the light list, shadows, the lighting models |
| `sky` | [Plugin_Sky](https://github.com/Far-Beyond-Pulsar/Plugin_Sky) | `pulsar.sky`: the atmosphere, clouds and the sky's lighting inputs (`.agents/SKY.md`) |
| `gi` | [Plugin_GI](https://github.com/Far-Beyond-Pulsar/Plugin_GI) | `pulsar.gi`: the GI probe clipmap and its tracers' interface |
| `fog` | [Plugin_Fog](https://github.com/Far-Beyond-Pulsar/Plugin_Fog) | `pulsar.fog`: global and local fog media, volumetric fog |
| `post` | [Plugin_Post](https://github.com/Far-Beyond-Pulsar/Plugin_Post) | `pulsar.post`: post-process volumes and camera baselines |
| `lens` | [Plugin_Lens](https://github.com/Far-Beyond-Pulsar/Plugin_Lens) | `pulsar.lens`: the camera lens and flare (`.agents/LENS.md`) |
| `water` | [Plugin_Water](https://github.com/Far-Beyond-Pulsar/Plugin_Water) | `pulsar.water`: water volumes, their simulation, caustics |
| `foliage` | [Plugin_Foliage](https://github.com/Far-Beyond-Pulsar/Plugin_Foliage) | `pulsar.foliage`: foliage and the level's wind |
| `voxel` | [Plugin_Voxel](https://github.com/Far-Beyond-Pulsar/Plugin_Voxel) | `pulsar.voxel`: destructible voxel worlds (`.agents/VOXEL_STREAMING.md`) |
| `decals` | [Plugin_Decals](https://github.com/Far-Beyond-Pulsar/Plugin_Decals) | `pulsar.decals`: projected decals |
| `particles` | [Plugin_Particles](https://github.com/Far-Beyond-Pulsar/Plugin_Particles) | `pulsar.particles`: GPU particle emitters (Corona) |
| `sprites` | [Plugin_Sprites](https://github.com/Far-Beyond-Pulsar/Plugin_Sprites) | `pulsar.sprites`: 2D sprites over the scene |
| `portals` | [Plugin_Portals](https://github.com/Far-Beyond-Pulsar/Plugin_Portals) | `pulsar.portals`: portals, their pairing, projection and draws |
| `splines` | [Plugin_Splines](https://github.com/Far-Beyond-Pulsar/Plugin_Splines) | `pulsar.splines`: authored curves and their editor lines |
| `physics_plugin` | [Plugin_Physics](https://github.com/Far-Beyond-Pulsar/Plugin_Physics) | `pulsar.physics`: physics components and the Rapier simulation, the first model's components (`legacy`), the `.physasset` schema (`api`, `physics_api`) |
| `skm_editor` | [Plugin_SKM](https://github.com/Far-Beyond-Pulsar/Plugin_SKM) | Animation: skeletal assets, the animator (`animation_component`) and its editors (`.agents/ANIMATION.md`) |
| `input_plugin` | [Plugin_Input](https://github.com/Far-Beyond-Pulsar/Plugin_Input) | Input actions and mappings, their editors and runtime |
| `ui_editor` | [Plugin_UI](https://github.com/Far-Beyond-Pulsar/Plugin_UI) | Widget assets and the WYSIWYG widget editor |
| `blueprint_editor` | [Plugin_Blueprints](https://github.com/Far-Beyond-Pulsar/Plugin_Blueprints) | Blueprint visual scripting, plus its `compiler/` crate |
| `code_editor` | [Plugin_CodeEditor](https://github.com/Far-Beyond-Pulsar/Plugin_CodeEditor) | Script/code editor, a host for Mockaco (`WgpuiEditorView`); `text_editor/lsp.rs` answers Mockaco's requests through the shared analyzer |
| `table_editor` | [Plugin_DBTable](https://github.com/Far-Beyond-Pulsar/Plugin_DBTable) | Database table editor |
| `matter_editor` | [Plugin_Matter](https://github.com/Far-Beyond-Pulsar/Plugin_Matter) | Texture painting in the Pulsar Image Format (`.pif`) |
| `asset_viewer` | in-tree | Viewers for model files, meshes and images |
| `character` | in-tree (p22; its own repository is the user's decision) | `pulsar.character`: a character as one component (a facade) over a skeletal mesh, its animator, a character controller and the player's input, with its movement system |
| `render_bps` | in-tree | Render graph editor (`.rendergraph`) on the Graphy canvas, no Blueprint code (`.agents/RENDER_GRAPH.md`) |
| `trait_editor` | in-tree | `pulsar.trait_editor`: the trait definition editor (`.trait.json`, `.agents/TRAITS.md`) |
| `typescript` | in-tree | `pulsar.typescript`: TypeScript as a scripting language (`class.ts` compiles to script modules) |

`plugins/installed` (the generated installed set) is in-tree too; its tests are the
editor's and the game's tests with every plugin. The editor-tool plugins (the
asset viewer, the render graph, trait and Blueprint editors, the code, matter and
table editors, TypeScript) stay plugins of their own (Pulsar-Native#1605); the
optional editor tools in `crates/` (git, flamegraph, Mission Control, Fab search,
the chat's providers) are tools the editor binary links (above, "The layers").

A change to a plugin is committed in its repository and pinned here, like any
submodule (below). Its repository's CI checks it inside a Pulsar-Native
checkout (rustfmt, clippy, nextest's `ci` profile); Pulsar's CI does not
format-check submodules (`scripts/fmt-check.sh`) but still lints and tests
them as workspace members. The rules for the kinds come from Helio's
`helio-architecture` skill (`AGENTS.md`). The one-owner check
(`crates/core/one_owner`), the installed set, `absent_plugins` and the
`scene_inventory` ledger read the checked-out plugins at these paths.

## Submodule pins

Every submodule (Helio, the `third-party/` and `ui/` repos, Pulsar-Reflection,
the plugins) must pin a commit on its upstream default branch; CI's "submodule
pins are on upstream main" job (`just check-submodule-pins`) fails otherwise.
A change that spans a dependency and Pulsar-Native merges in this order:

1. merge the dependency PR into the dependency's default branch;
2. re-pin the submodule here to that default-branch commit;
3. merge the Pulsar-Native PR.

Pinning a dependency's feature branch is fine while a PR is in review, but not
on main. Pins still waiting on an upstream PR are listed, with that PR, in
`.github/submodule-pin-exceptions.txt`; the check flags a line as stale once
its pin lands upstream, and the line should then be deleted.

**Mesh bounds.** Native PMSH v5 stores a validated local-space AABB computed
from the final converted vertex payload. The geometry content id still hashes
only vertex/index bytes. v1-v4 remain readable and derive the same AABB during
decode; the component's GPU culling sphere is derived from the AABB. The level
editor uses resident CPU geometry and a content-id keyed BVH for triangle-true
viewport picking, with the bounds serving only as broad phase.
