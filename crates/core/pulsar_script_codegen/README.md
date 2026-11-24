# pulsar_script_codegen

Rust code generation for script modules, used by the standalone Rust export. It is GPUI-free.

- Owns: turning a verified `Module` into Rust source: one compiled step function per module function, implementing `CompiledCode` and running on the same VM as interpreted code.
- Entry points: `generate`.
- Workspace path dependencies, plus SceneDB (Cargo.toml): `pulsar_script_vm`.
- Layer: kernel. It may depend on kernel and foundation crates only, dev dependencies included, never on a plugin, tool or host crate (`architecture/layers.toml`, `just arch`).
- Must not depend on: GPUI; any scripting language front end.
- See: [`.agents/SCRIPT_LANGUAGES.md`](../../../.agents/SCRIPT_LANGUAGES.md).
