# pulsar_script_vm

Engine script bytecode and its virtual machine: the only thing the engine core knows about scripting.

- Owns: the module format, `ModuleBuilder` (what a language front end builds a module with), the verifier, linker, typed register interpreter, native function registry with hot-reloadable libraries, the `compiled` code contract and the `migrate` planner.
- Entry points: modules `module`, `module_builder`, `verify`, `link`, `interp`, `native`.
- Workspace path dependencies, plus SceneDB (Cargo.toml): `pulsar_reflection`, `pulsar_scenedb` (git).
- Layer: kernel. It may depend on kernel and foundation crates only, dev dependencies included, never on a plugin, tool or host crate (`architecture/layers.toml`, `just arch`).
- Must not depend on: any scripting language: front ends are plugins that compile to its modules.
- See: [`.agents/SCRIPT_LANGUAGES.md`](../../../.agents/SCRIPT_LANGUAGES.md).
