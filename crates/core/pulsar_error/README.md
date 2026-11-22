# pulsar_error

The shared error type and error-handling conventions for the engine.

- Owns: `EngineError` and the conventions written in its crate docs: typed `thiserror` errors in libraries, `anyhow` only in binaries and UI glue, no panics on user input, and `tracing` for logging.
- Entry points: `EngineError`; it re-exports `tracing`.
- Workspace path dependencies, plus SceneDB (Cargo.toml): none.
- Layer: kernel. It may depend on kernel and foundation crates only, dev dependencies included, never on a plugin, tool or host crate (`architecture/layers.toml`, `just arch`).
- Must not depend on: plugins, tools or host crates.
