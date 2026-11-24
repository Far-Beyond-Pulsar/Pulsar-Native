# pulsar_dialog_embed

Carries the `pulsar-dialog` helper (a separate dialog process) inside the engine executable.

- Owns: the build script that builds the helper for the target and embeds it, and `ensure_helper`, which places it beside the engine at startup (or in the per-user app-data directory when that is read-only).
- Entry points: `ensure_helper`.
- Workspace path dependencies, plus SceneDB (Cargo.toml): `pulsar_install_layout`.
- Layer: kernel. It may depend on kernel and foundation crates only, dev dependencies included, never on a plugin, tool or host crate (`architecture/layers.toml`, `just arch`).
- Must not depend on: plugins, tools or host crates.
