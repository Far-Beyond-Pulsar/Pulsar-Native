# core

Engine platform crates. These live under `crates/core/` and are the backbone of Pulsar.

| Crate | Purpose |
|---|---|
| `engine` | Binary entry point and main loop |
| `engine_backend` | Window/context/input backend abstraction |
| `engine_class_derive` | Derive macro for engine class system |
| `engine_fs` | Virtual filesystem with asset mounts |
| `friends_engine` | Multiplayer friends backend (presence, invites) |
| `engine_state` | Global engine state and app lifecycle |
| `plugin_editor_api` | Plugin SDK — traits for editor plugins |
| `plugin_manager` | Plugin loading, lifecycle, and registry |
| `profiling` | Performance tracing and profiling |
| `pulsar_auth` | Authentication and session management |
| `pulsar_bp_executor` | Blueprint graph runtime executor |
| `pulsar_core` | Core engine types and utilities |
| `pulsar_docs` | Documentation generation from reflected types |
| `pulsar_ecs` | Entity-Component-System |
| `pulsar_events` | Event bus |
| `pulsar_game` | Game mode abstractions |
| `pulsar_graph` | Blueprint graph data model |
| `pulsar_lsp` | LSP client integration |
| `pulsar_macros` | Proc macros for engine types |
| `pulsar_reflection` | Runtime type reflection |
| `pulsar_reflection_derive` | Derive macro for reflection |
| `pulsar_settings` | Settings store and watcher |
| `pulsar_std` | Standard library for blueprints |
| `pulsar_std_bundle` | Bundled standard library definitions |
| `pulsar-multiplayer-core` | Multiplayer session core |
| `pulsar-relay` | Relay server client |
| `ui_gen_macros` | Procedural macros for UI code generation |
| `window_manager` | Multi-window management |
