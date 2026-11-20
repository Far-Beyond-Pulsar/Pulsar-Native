# helio_component

The shared rendering-side engine pieces that no single object kind owns.

- Owns: the generic asset seams (which component an asset becomes, who loads an asset kind, what a changed file reloads), texture assets, cooking and residency, cameras, movability and its motion gate, skinning helpers, and sublevel displays.
- Entry points: modules `asset_component`, `cooked_texture`, `texture_assets`, `texture_detail`, `motion_gate`, `skinning`, `static_move_watch`.
- Workspace path dependencies, plus SceneDB (Cargo.toml): `engine_class_derive`, `engine_fs`, `engine_state`, `helio`, `helio-core`, `pulsar_events`, `pulsar_plugins`, `pulsar_reflection`, `pulsar_scene_model`, `pulsar_world_registry`, `ui_types_common`, `pulsar_scenedb` (git).
- Layer: kernel. It may depend on kernel and foundation crates only, dev dependencies included, never on a plugin, tool or host crate (`architecture/layers.toml`, `just arch`).
- Must not depend on: any object kind's component, since every kind lives in its own plugin (its crate docs say none lives here); also plugins, tools or host crates.
- See: [`.agents/MEMORY.md`](../../../.agents/MEMORY.md), [`.agents/CRATES.md`](../../../.agents/CRATES.md).
