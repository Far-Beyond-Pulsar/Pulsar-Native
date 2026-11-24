# engine_fs

The filesystem library every file in the engine, the editor and the plugins goes through.

- Owns: `virtual_fs` (local disk, remote studio and peer-to-peer providers, with plugin folders mounted over them), change events, the project file index with its reference edges, and the file watchers.
- Entry points: `virtual_fs::read_file`, `write_file`, `write_file_atomically`, `open_read`, `manifest`; modules `providers`, `events`, `file_index`.
- Workspace path dependencies, plus SceneDB (Cargo.toml): `editor_task_queue`, `pulsar-multiplayer-core`, `pulsar_events`, `pulsar_plugins`, `ui_types_common`.
- Layer: kernel. It may depend on kernel and foundation crates only, dev dependencies included, never on a plugin, tool or host crate (`architecture/layers.toml`, `just arch`).
- Must not depend on: GPUI, plugins, tools or host crates. Elsewhere in the workspace `std::fs` is a disallowed method (root `clippy.toml`), so callers use this crate.
- See: [`.agents/FILESYSTEM.md`](../../../.agents/FILESYSTEM.md).
