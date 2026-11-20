# plugin_editor_api

The SDK for editor plugins: the traits and types a plugin implements to add editors, file types, menus, status-bar items, scripting and AI tools.

- Owns: the plugin and registrar contracts, the editor element lifecycle, file type and menu metadata, manifests, panel restore, component and asset events, and the runtime-safe cooking and scripting contracts.
- Entry points: modules `plugin`, `registrar`, `editor_element`, `file_types`, `menus`, `statusbar`, `scripting`, `manifest`.
- Workspace path dependencies, plus SceneDB (Cargo.toml): `engine_fs`, `gpui-ce`, `profiling`, `pulsar_events`, `pulsar_language`, `pulsar_lsp`, `pulsar_plugins`, `pulsar_reflection`, `pulsar_script_vm`, `pulsar_world_registry`, `ui`, `ui_types_common`, `window_manager`.
- Layer: kernel. It may depend on kernel and foundation crates only, dev dependencies included, never on a plugin, tool or host crate (`architecture/layers.toml`, `just arch`).
- Must not depend on: GPUI, `ui` and the language server in its default feature set (runtime plugin roots use it headless; editor hosts enable its `editor` feature); also plugins, tools or host crates.
- See: [`.agents/PLUGIN_SYSTEM.md`](../../../.agents/PLUGIN_SYSTEM.md).
