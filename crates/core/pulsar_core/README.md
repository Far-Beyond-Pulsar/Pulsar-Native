# pulsar_core

Game runtime core primitives: event channels, game time, task pool and tick mode.

- Owns: `EventBuffer`, `EventReader`, `EventWriter`, `TaskPool`, `TickMode`, `Clock` and `GameTime`.
- Entry points: the re-exports in `src/lib.rs`.
- Workspace path dependencies, plus SceneDB (Cargo.toml): `editor_task_queue`, `profiling`.
- Layer: kernel. It may depend on kernel and foundation crates only, dev dependencies included, never on a plugin, tool or host crate (`architecture/layers.toml`, `just arch`).
- Must not depend on: plugins, tools or host crates.
