# pulsar_project

Game project generation: the one owner of every file the engine generates in a game project.

- Owns: the project model (`Pulsar.toml`, scripts crates, classes), the engine SDK mirrored into `.pulsar/sdk`, the generated Cargo workspace and game crate, and `sync`, which brings them up to date and records what it wrote.
- Entry points: modules `descriptor`, `sdk`, `model`, `render`, `apply`, `sync` (`sync`, `SyncOptions`).
- Workspace path dependencies, plus SceneDB (Cargo.toml): `engine_fs`, `pulsar_class`, `pulsar_install_layout`, `pulsar_language`, `pulsar_plugins`, `pulsar_script_codegen`, `pulsar_script_vm`.
- Layer: kernel. It may depend on kernel and foundation crates only, dev dependencies included, never on a plugin, tool or host crate (`architecture/layers.toml`, `just arch`).
- Must not depend on: plugins, tools or host crates. Note that no other crate depends on it yet (see the plan).
- See: [`.agents/PROJECT_GENERATION_PLAN.md`](../../../.agents/PROJECT_GENERATION_PLAN.md).
