# pulsar_pie_abi

The shared C-ABI contract for Play-In-Editor: the structures that cross the boundary between the editor and a game compiled as a dynamic library.

- Owns: the `#[repr(C)]` structs and function-pointer types (`EngineContext`, `PieView`, `InputEvent`, session flags and kinds, control codes). The crate is `no_std` and depends only on `core::ffi`, so no `wgpu` or other layout can drift into it.
- Entry points: the types in `src/lib.rs`.
- Workspace path dependencies, plus SceneDB (Cargo.toml): none.
- Layer: kernel. It may depend on kernel and foundation crates only, dev dependencies included, never on a plugin, tool or host crate (`architecture/layers.toml`, `just arch`).
- Must not depend on: anything that could change its layout: it is `no_std` and uses only `core::ffi` (crate docs).
