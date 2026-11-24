# ui_gen_macros

Proc macros for UI boilerplate.

- Owns: `#[register_window]`, which submits a window type to the window registry.
- Entry points: `register_window` in `src/lib.rs`.
- Workspace path dependencies, plus SceneDB (Cargo.toml): none.
- Layer: kernel. It may depend on kernel and foundation crates only, dev dependencies included, never on a plugin, tool or host crate (`architecture/layers.toml`, `just arch`).
- Must not depend on: any workspace crate: it is a `proc-macro` crate with no path-crate dependencies.
- See: [`window_manager`](../window_manager/README.md).
