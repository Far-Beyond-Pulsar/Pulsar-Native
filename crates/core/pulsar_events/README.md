# pulsar_events

Engine-wide event plumbing on Gamma v2.

- Owns: the per-session `EventHub` (built-in, script and plugin events, delivered at fixed flush points, with a bounded debug tap), the process-wide host bus plugins reach through Gamma's FFI, and asset-update notifications.
- Entry points: `EventHub`, `FlushPoint`, `EventCategory`, `AssetUpdated`, `publish_asset_updated`, `subscribe_asset_updates`.
- Workspace path dependencies, plus SceneDB (Cargo.toml): `profiling`, `ui_types_common`.
- Layer: kernel. It may depend on kernel and foundation crates only, dev dependencies included, never on a plugin, tool or host crate (`architecture/layers.toml`, `just arch`).
- Must not depend on: plugins, tools or host crates.
