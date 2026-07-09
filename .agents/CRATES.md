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
  graphics/          Vendored wgpu fork (submodule, still a git dep)
  third-party/       Vendored smaller deps (pbgc, graphy, pulsar-config)
```

## core/ — Engine platform

The heart of the engine. 26 crates with no UI dependencies.

| Crate | Responsibility |
|---|---|
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
| `pulsar_ecs` | Archetype-based ECS with `World`, `Query`, `Schedule` |
| `pulsar_events` | Event bus |
| `pulsar_game` | Game mode abstractions |
| `pulsar_graph` | Blueprint graph data model |
| `pulsar_lsp` | LSP client integration |
| `pulsar_macros` | Various derive macros |
| `pulsar_reflection` | Runtime type system — `Reflectable`, `RuntimeTypeInfo`, `EngineClass`, `Subsystems` |
| `pulsar_reflection_derive` | `#[derive(Reflectable)]` proc macro |
| `pulsar_settings` | Settings store (wraps `pulsar-config`) |
| `pulsar_std` | Blueprint standard library |
| `pulsar_std_bundle` | Bundled std definitions |
| `pulsar-multiplayer-core` | Multiplayer protocol and state |
| `pulsar-relay` | Relay server client |
| `ui_gen_macros` | Proc macros for UI boilerplate |
| `window_manager` | Multi-window management, window definitions |

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
| `ui_plugin_manager` | Plugin browser |
| `ui_problems` | Errors / warnings panel |
| `ui_settings` | Settings editor |
| `ui_type_debugger` | Runtime type inspector |
| `ui_types_common` | Shared type definitions |

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

Changes to these submodules are committed directly in Pulsar-Native. They are
not workspace members (Cargo nested workspace limitation).

## third-party/ — Vendored deps

| Submodule | Path dep? | Notes |
|---|---|---|
| `pbgc/` | Yes | Pulsar Blueprint Graph Compiler |
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
