# pulsar_macros

Proc macros for defining Blueprint nodes in Rust.

- Owns: `#[blueprint]`, `#[blueprint_type]`, `#[bp_import]`, `exec_output!`, `generate_icon_enum!` and `#[derive(IntoPlot)]`.
- Entry points: the macros in `src/lib.rs`.
- Workspace path dependencies, plus SceneDB (Cargo.toml): none.
- Layer: kernel. It may depend on kernel and foundation crates only, dev dependencies included, never on a plugin, tool or host crate (`architecture/layers.toml`, `just arch`).
- Must not depend on: any workspace crate: it is a `proc-macro` crate with no path-crate dependencies.
- See: [`.agents/CRATES.md`](../../../.agents/CRATES.md) and the `/add-blueprint-node` skill.
