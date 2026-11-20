# pulsar_script_object_model

The script-facing object model: handles that address live scene state with validation at every use.

- Owns: `ActorRef` and `ComponentRef` handles, property routing with parity to the properties panel, and typed errors that never panic (`ScriptRefError`).
- Entry points: modules `refs`, `access`, `dispatch`, `errors`, `world_host`.
- Workspace path dependencies, plus SceneDB (Cargo.toml): `pulsar_reflection`, `pulsar_scene_model`, `pulsar_world_registry`, `pulsar_scenedb` (git).
- Layer: kernel. It may depend on kernel and foundation crates only, dev dependencies included, never on a plugin, tool or host crate (`architecture/layers.toml`, `just arch`).
- Must not depend on: plugins, tools or host crates.
- See: [`.agents/SCRIPT_LANGUAGES.md`](../../../.agents/SCRIPT_LANGUAGES.md).
