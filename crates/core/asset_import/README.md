# asset_import

Source-asset import: turns raw files such as FBX and OBJ that sit in a project into native assets, and tracks them.

- Owns: the import database `.pulsar/import_db.json`, the project scan, the per-format importer dispatch (each format's importer is a plugin's registration), import options, and the conversions, which run as editor tasks.
- Entry points: modules `scan`, `db`, `importer`, `options`, `service`, `companion`.
- Workspace path dependencies, plus SceneDB (Cargo.toml): `editor_task_queue`, `engine_fs`, `plugin_editor_api`, `pulsar_plugins`, `pulsar_reflection`.
- Layer: kernel. It may depend on kernel and foundation crates only, dev dependencies included, never on a plugin, tool or host crate (`architecture/layers.toml`, `just arch`).
- Must not depend on: a concrete importer; formats arrive through plugin registrations. Also plugins, tools or host crates.
- See: [`.agents/CRATES.md`](../../../.agents/CRATES.md), section "Asset import".
