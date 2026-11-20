# pulsar_std

The Blueprint standard library: built-in nodes written as Rust functions.

- Owns: nodes declared with `#[blueprint]` (kind-neutral domains in `engine/`), the node registry, type constructors and script generics.
- Entry points: modules `registry`, `engine`, `type_constructors`, `script_generics`.
- Workspace path dependencies, plus SceneDB (Cargo.toml): `engine_fs`, `pulsar_macros`, `pulsar_reflection`, `pulsar_script_vm`, `pulsar_scenedb` (git).
- Layer: kernel. It may depend on kernel and foundation crates only, dev dependencies included, never on a plugin, tool or host crate (`architecture/layers.toml`, `just arch`).
- Must not depend on: a specific object kind: a kind's script API is its component methods, not a node here.
- See: the `/add-blueprint-node` and `/blueprint-system` skills in `.claude/commands/`.
