# pulsar_script_runtime

Runs script classes bound to entities. It is language-neutral.

- Owns: `ScriptRuntime`, which owns the native registry, loaded native libraries, classes and instances, runs lifecycle and custom events, resumes latent calls, and does transactional hot reload with state migration.
- Entry points: `ScriptRuntime`; modules `latent`, `phase`, `state`.
- Workspace path dependencies, plus SceneDB (Cargo.toml): `engine_fs`, `profiling`, `pulsar_script_vm`, `pulsar_scenedb` (git).
- Layer: kernel. It may depend on kernel and foundation crates only, dev dependencies included, never on a plugin, tool or host crate (`architecture/layers.toml`, `just arch`).
- Must not depend on: any scripting language front end, GPUI, plugins, tools or host crates.
- See: [`.agents/SCRIPT_LANGUAGES.md`](../../../.agents/SCRIPT_LANGUAGES.md).
