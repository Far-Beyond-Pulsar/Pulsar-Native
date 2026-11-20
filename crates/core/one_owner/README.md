# one_owner

The one-owner rule as a checker: every cross-kind concept has one owner, and nothing else declares a copy of it.

- Owns: a scan of the source tree that reports copies of WGSL structs, constants and functions, and of layouts, declared outside their owner.
- Entry points: `src/lib.rs` (reads the source tree).
- Workspace path dependencies, plus SceneDB (Cargo.toml): none.
- Layer: kernel. It may depend on kernel and foundation crates only, dev dependencies included, never on a plugin, tool or host crate (`architecture/layers.toml`, `just arch`).
- Must not depend on: any workspace crate; it has no dependencies.
