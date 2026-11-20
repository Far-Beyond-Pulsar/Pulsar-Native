# plugin_manager

The host side of dynamic editor plugins: loading native plugin libraries, and the registries and catalog the editor reads.

- Owns: library loading (never unloaded, with a compiler-version check and an integrity check), load ordering from plugin dependencies, the file-type and editor registries, the plugin catalog, panel restorers, startup staging for the splash screen, and the AI tool bridge.
- Entry points: `PluginManager`, `PermanentLibrary`, `plan_load_order`, `EditorRegistry`, `FileTypeRegistry`, `PluginToolBridge`, `attach_world_runtime`.
- Workspace path dependencies, plus SceneDB (Cargo.toml): `editor_task_queue`, `engine_fs`, `gpui-ce`, `plugin_editor_api`, `profiling`, `pulsar_class`, `pulsar_events`, `pulsar_language`, `pulsar_plugins`, `pulsar_world_registry`, `ui`.
- Layer: kernel. It may depend on kernel and foundation crates only, dev dependencies included, never on a plugin, tool or host crate (`architecture/layers.toml`, `just arch`).
- Must not depend on: any specific plugin; its tests use fixtures under `tests/fixtures`. Also tools or host crates.
- See: [`.agents/PLUGIN_SYSTEM.md`](../../../.agents/PLUGIN_SYSTEM.md), section "Dynamic editor plugins".
