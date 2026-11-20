# pulsar_install_layout

The contract between the Hub (which installs engines, plugins and themes) and the engine (which loads them): where each file goes.

- Owns: the per-install plugin directory and its integrity manifest, the shared themes directory and the per-user app-data directory. Both sides name every path through this crate.
- Entry points: the path functions in `src/lib.rs`.
- Workspace path dependencies, plus SceneDB (Cargo.toml): none.
- Layer: kernel. It may depend on kernel and foundation crates only, dev dependencies included, never on a plugin, tool or host crate (`architecture/layers.toml`, `just arch`).
- Must not depend on: plugins, tools or host crates.
