# pulsar_lsp

The language-server layer: a generic client, the service that routes a file to its server, and the diagnostics store.

- Owns: `LanguageServer` (one per server and root), `LanguageServers` (routing, on-demand start, live compiler checks, managed installs), `DiagnosticsStore` (by file and source), and the application of workspace edits.
- Entry points: `LanguageServers::server_for`, `DiagnosticsStore`, module `testing` (scripts a server on a pipe for tests).
- Workspace path dependencies, plus SceneDB (Cargo.toml): `editor_task_queue`, `engine_fs`, `gpui-ce`, `pulsar_language`, `pulsar_plugins`, `pulsar_settings`, `ui`.
- Layer: kernel. It may depend on kernel and foundation crates only, dev dependencies included, never on a plugin, tool or host crate (`architecture/layers.toml`, `just arch`).
- Must not depend on: any specific language server: each server's plugin brings its own `LanguageServerSpec` (rust-analyzer's is in `pulsar.rust`).
- See: [`.agents/LANGUAGE_PLUGINS_PLAN.md`](../../../.agents/LANGUAGE_PLUGINS_PLAN.md).
