# pulsar_plugins

Which plugins a project has: descriptors, enablement, contract resolution, and the gate every registration passes.

- Owns: `pulsar-plugin.toml` descriptors, the project `[plugins]` list in `Pulsar.toml`, resolution of provided and required contracts, `PluginBuilder`, and the registry and gate (a disabled plugin's registrations are absent). Also the library export used by dynamic plugins.
- Entry points: `PluginDescriptor`, `PluginBuilder`, `resolve`, `activate_project`, `registered::<T>()`, `export_library!`, `installed!`.
- Workspace path dependencies, plus SceneDB (Cargo.toml): none.
- Layer: kernel. It may depend on kernel and foundation crates only, dev dependencies included, never on a plugin, tool or host crate (`architecture/layers.toml`, `just arch`).
- Must not depend on: any specific plugin or kind.
- See: [`.agents/PLUGIN_SYSTEM.md`](../../../.agents/PLUGIN_SYSTEM.md).
