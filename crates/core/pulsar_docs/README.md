# pulsar_docs

In-editor documentation content: the generated crate docs and markdown, embedded at compile time.

- Owns: the embedded doc assets (the build script populates `target/doc` before this crate compiles), the crate index, and the project parser.
- Entry points: `get_doc_content`, `get_crate_index`, `list_crates`, `docs_available`; module `project_parser`.
- Workspace path dependencies, plus SceneDB (Cargo.toml): `engine_fs`.
- Layer: kernel. It may depend on kernel and foundation crates only, dev dependencies included, never on a plugin, tool or host crate (`architecture/layers.toml`, `just arch`).
- Must not depend on: plugins, tools or host crates.
