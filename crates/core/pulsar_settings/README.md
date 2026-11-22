# pulsar_settings

The registry of all built-in engine and project settings.

- Owns: the schemas for the editor, project and plugin settings groups, registered into the global `pulsar_config::ConfigManager`.
- Entry points: `register_all_settings`; modules `editor`, `project`, `plugin`.
- Workspace path dependencies, plus SceneDB (Cargo.toml): `helio-settings`, `pulsar-config`, `pulsar_plugins`.
- Layer: kernel. It may depend on kernel and foundation crates only, dev dependencies included, never on a plugin, tool or host crate (`architecture/layers.toml`, `just arch`).
- Must not depend on: plugins, tools or host crates.
- See: [`.agents/plans/settings-rebuild/README.md`](../../../.agents/plans/settings-rebuild/README.md).
