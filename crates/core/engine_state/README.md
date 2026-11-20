# engine_state

Typed engine state: the global context and the resource store the rest of the engine reads.

- Owns: `EngineContext` with its per-window state, `StateStore`, `KeyedStore`, `ResourceHandle`, the settings access layer and defaults, the launch, project and playback contexts, and where a project's plugin set is activated.
- Entry points: `EngineContext::global`, `StateStore::get_or_init`, `ResourceHandle`, `set_project_path`, `register_default_settings`.
- Workspace path dependencies, plus SceneDB (Cargo.toml): `engine_fs`, `pulsar-config`, `pulsar-multiplayer-core`, `pulsar_auth`, `pulsar_plugins`, `pulsar_settings`, `ui_types_common`.
- Layer: kernel. It may depend on kernel and foundation crates only, dev dependencies included, never on a plugin, tool or host crate (`architecture/layers.toml`, `just arch`).
- Must not depend on: plugins, tools or host crates.
- See: [`.agents/ENGINE_LIFECYCLE.md`](../../../.agents/ENGINE_LIFECYCLE.md).
