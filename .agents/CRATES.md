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
  third-party/       Vendored smaller deps (pbgc, graphy, pulsar-config)
```

## core/ — Engine platform

The heart of the engine. 26 crates with no UI dependencies.

| Crate | Responsibility |
|---|---|
| `asset_import` | Source-asset import (FBX, OBJ, …): import database, project scan, per-format importers, conversions as editor tasks (see below) |
| `editor_task_queue` | Background task queue behind the Tasks window: progress, errors, cancellation |
| `engine` | Binary entry point, startup graph, GPUI `App` creation |
| `engine_backend` | Window, input, and rendering backend; subsystem lifecycle |
| `engine_class_derive` | `#[derive(EngineClass)]` proc macro |
| `engine_fs` | Virtual filesystem abstraction — local, remote, P2P |
| `engine_state` | Typed resource system — `StateStore`, `ResourceHandle`, `EngineContext` |
| `engine_subsystems` | Subsystem trait and registry |
| `friends_engine` | Multiplayer friends backend |
| `plugin_editor_api` | Plugin SDK — traits, types, `export_plugin!` macro |
| `plugin_manager` | Plugin host — DLL loading, registries, tool bridge |
| `profiling` | Performance tracing (Tracy) |
| `pulsar_auth` | Authentication |
| `pulsar_bp_executor` | Blueprint graph runtime executor |
| `pulsar_core` | Core types, math, utilities |
| `pulsar_docs` | Doc generation from reflected types |
| `pulsar_events` | Event bus |
| `pulsar_game` | Game mode abstractions |
| `pulsar_graph` | Blueprint graph data model |
| `pulsar_lsp` | LSP client integration |
| `pulsar_macros` | Various derive macros |
| `pulsar_reflection` | Runtime type system — `Reflectable`, `RuntimeTypeInfo`, `EngineClass`, `Subsystems` |
| `pulsar_reflection_derive` | `#[derive(Reflectable)]` proc macro |
| `pulsar_scene_model` | Scene object components (`StableId`, `Name`, `Transform`, `Visibility`, hierarchy) and component-instance entities (`attachments`) |
| `pulsar_world_registry` | Typed component classes in SceneDB's `World`: registration, boundary decode, object feeds, change watches, component lifecycles |
| `pulsar_settings` | Settings store (wraps `pulsar-config`) |
| `pulsar_script_vm` | Language-neutral script bytecode: module format, verifier, linker, VM, natives, shared `exec` semantics, `compiled` code contract, `migrate` planner |
| `pulsar_script_runtime` | Runs script classes on entities: lifecycle, events, transactional hot reload with state migration, saved state |
| `pulsar_script_math` | glam math types (Vec2/3/4, DVec3, Quat, Mat4) as script value types with natives |
| `pulsar_script_codegen` | Rust export: generates compiled step functions (and the `Actor` wrapper) from a script module |
| `pulsar_script_conformance` | Runs modules interpreted and as generated Rust and compares everything observable (`just conformance`) |
| `pulsar_script_ts` | TypeScript subset compiled to script modules (oxc parser, type checking against the native registry, `.d.ts` generation, field identity) |
| `pulsar_std` | Blueprint standard library |
| `pulsar_std_bundle` | Bundled std definitions |
| `pulsar-multiplayer-core` | Multiplayer protocol and state |
| `pulsar-relay` | Relay server client |
| `scene_inventory` | Scene-data closure ledger and architecture checks (`cargo test -p scene_inventory`) |
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

Import options are still stored separately, in `.pulsar/import_options.json`
(`engine_fs::import_options`), keyed by native asset.

**Mesh default materials.** A native `.mesh` carries one `MeshMaterialSlot`
per material slot (`helio_component::mesh_cache`). `material_asset` is the
slot's default material, authored in the mesh viewer with
`mesh_cache::set_default_materials`; a reimport keeps it, matching slots by
source material index, then by name. A placed `StaticMeshComponent` slot
inherits the default unless it assigns its own material
(`StaticMeshMaterialSlot::effective_material_asset`). The component resolves
this before anything reaches Helio. Helio's passes only ever see the
effective material, never whether it was a default or an assignment.

## editor/ — Editor panels

20 crates, one per editor panel or subsystem. Previously `ui-crates/`. Each
provides a piece of the GPUI-based editor UI shell.

| Crate | Panel / Feature |
|---|---|
| `ui_about` | About / credits dialog |
| `ui_common` | Shared widgets, i18n, styling primitives |
| `ui_core` | Editor shell — tab management, plugin wiring, statusbar |
| `ui_documentation` | In-editor doc viewer |
| `ui_entry` | Window entry creation and setup |
| `ui_fab_search` | Floating action button + search |
| `ui_file_manager` | File tree and project browser |
| `ui_flamegraph` | CPU/GPU flamegraph viewer |
| `ui_friends` | Friends list |
| `ui_git_manager` | Git integration |
| `ui_level_editor` | Level / scene editor |
| `ui_loading_screen` | Splash + loading progress |
| `ui_log_viewer` | Log output |
| `ui_multiplayer` | Multiplayer session UI |
| `ui_multiuser_status` | Presence indicators |
| `plugin_typescript` | TypeScript scripting-language plugin (registers at link time; compiles `class.ts`) |
| `ui_plugin_manager` | Plugin browser |
| `ui_problems` | Errors / warnings panel |
| `ui_settings` | Settings editor |
| `ui_type_debugger` | Runtime type inspector |
| `ui_types_common` | Shared type definitions |

### Asset thumbnails

`engine_fs::thumbnails` renders and caches file-browser thumbnails, with one
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

These bridge the engine with external runtimes: physics (rapier3d), rendering
(wgpu + helio), and scene management.

| Crate | Responsibility |
|---|---|
| `engine_subsystems` | Lifecycle orchestration — the `Subsystem` trait and `SubsystemRegistry` |
| `pulsar_physics` | Physics — rapier3d integration, colliders, rigid bodies |
| `pulsar_rendering` | GPU rendering pipeline — mesh cache, scene objects, material system |
| `pulsar_scene` | Scene graph — transforms, prefab instantiation, object hierarchy |

## agent-providers/ — AI providers

One crate per LLM backend. Each implements the `ChatProvider` trait from
`agent_chat_core`. Tool execution is centralized in `agent_chat_tools`.

22 providers: anthropic, aws_bedrock, azure_openai, cohere, deepseek,
demo_random, docker_model_runner, fireworks, gemini, github_copilot, groq,
llama_cpp, lmstudio, mistral, ollama, openai, openrouter, perplexity,
together, vertex_ai, vllm, xai.

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
| `pbgc/` | Yes | pulsar_std node catalogue as Graphy metadata (the old graph compiler, bytecode VM and graph-to-Rust backend were removed) |
| `graphy/` | Yes | Graph data model and compiler types |
| `pulsar-config/` | Yes | Configuration management |
| `toolbelt/` | No | Tool registry + macros (has its own workspace) |
| `psgc/` | No | Shader Graph Compiler (has its own workspace) |

## plugins/vendor/ — Editor plugin repos

These are the built-in editor plugins, loaded as DLLs at runtime. Each is a
git submodule compiled as `cdylib`:

- `blueprint_editor` — Blueprint visual scripting editor
- `code_editor` — Script/code editor
- `shader_editor` — Shader graph editor
- `table_editor` — Database table editor
- `matter_editor` — Material editor

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
