# pulsar_scene_model

The engine's scene vocabulary: plain components stored directly in SceneDB, plus stateless helpers derived from them.

- Owns: `StableId`, `Name`, `Transform`, `Visibility` and the hierarchy components, component-instance entities (`attachments`), sublevel placements (`sublevels`), frames, and the `SceneWorldExt` helpers (identity lookup, hierarchy, selection). It adds no store around the components.
- Entry points: modules `components`, `attachments`, `sublevels`, `world_ext` (`SceneWorldExt`).
- Workspace path dependencies, plus SceneDB (Cargo.toml): `pulsar_reflection`, `pulsar_scenedb` (git).
- Layer: kernel. It may depend on kernel and foundation crates only, dev dependencies included, never on a plugin, tool or host crate (`architecture/layers.toml`, `just arch`).
- Must not depend on: plugins, tools or host crates.
- See: [`.agents/SCENEDB_MIGRATION.md`](../../../.agents/SCENEDB_MIGRATION.md), [`.agents/FRAMES.md`](../../../.agents/FRAMES.md).
