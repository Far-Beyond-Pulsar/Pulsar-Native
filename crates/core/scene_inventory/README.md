# scene_inventory

The scene-data closure ledger and architecture checks, run as tests.

- Owns: the ledger (`ledger.toml`, one row per component class, GPU schema, buffer key, render pass crate and lifecycle call site) and the code that recomputes its facts from the linked registries and the source tree.
- Entry points: `cargo test -p scene_inventory`.
- Workspace path dependencies, plus SceneDB (Cargo.toml): `engine_backend`, `helio_component`, `installed_plugins`, `pulsar_class`, `pulsar_plugins`, `pulsar_reflection`, `pulsar_scene_model`, `pulsar_world_registry`, `pulsar_scenedb` (git).
- Layer: host. A host may depend on any layer (`architecture/layers.toml`).
- Must not depend on: nothing is forbidden here: this is a host that links every crate registering scene components.
- See: [`.agents/SCENEDB_MIGRATION.md`](../../../.agents/SCENEDB_MIGRATION.md).
