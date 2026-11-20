# pulsar_script_conformance

Conformance tests that run each script module interpreted and as generated Rust, and compare everything observable.

- Owns: the harness and fixtures: results, errors and traces, variables, native calls and waiting. The generated Rust is built by `build.rs` in the profile under test, so run it in debug and release.
- Entry points: `harness::run`; the `just conformance` recipe.
- Workspace path dependencies, plus SceneDB (Cargo.toml): `pulsar_script_codegen`, `pulsar_script_math`, `pulsar_script_vm`, `pulsar_scenedb` (git).
- Layer: kernel. It may depend on kernel and foundation crates only, dev dependencies included, never on a plugin, tool or host crate (`architecture/layers.toml`, `just arch`).
- Must not depend on: plugins, tools or host crates.
