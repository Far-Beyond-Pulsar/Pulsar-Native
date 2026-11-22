# engine_backend

The engine's runtime backend: the global backend handle, the shared scene, Helio rendering, and the seams plugins plug into.

- Owns: `EngineBackend`, the `SharedScene` handle and its Helio bridge (`scene`), the Helio renderers (`subsystems/render`), the render runtimes service and other editor-side services (`services`), and the component registry.
- Entry points: `EngineBackend::init` and `global`, `scene::SharedScene`, `scene::helio_bridge`, `subsystems::render::helio_renderer::SceneRenderer`, `services::render_runtimes`.
- Workspace path dependencies, plus SceneDB (Cargo.toml): `editor_task_queue`, `engine_fs`, `engine_state`, `gpui-ce`, `helio`, `helio-core`, `helio-feature-api`, `helio-frame-api`, `helio-graph-asset`, `helio-pass-postprocess`, `helio-pass-tsr`, `helio-presets`, `helio-scene-derivations`, `helio-scene-join`, `helio_component`, `profiling`, `pulsar_class`, `pulsar_component_lifecycle`, `pulsar_content`, `pulsar_events`, `pulsar_pie_abi`, `pulsar_plugins`, `pulsar_reflection`, `pulsar_scene`, `pulsar_scene_model`, `pulsar_script_object_model`, `pulsar_settings`, `pulsar_traits`, `pulsar_world_registry`, `scenedb-inspector-agent`, `ui`, `ui_gen_macros`, `pulsar_scenedb` (git).
- Layer: kernel. It may depend on kernel and foundation crates only, dev dependencies included, never on a plugin, tool or host crate (`architecture/layers.toml`, `just arch`).
- Must not depend on: any plugin (the layer gate). Code here that names a concrete component type is legacy to move into that component's plugin.
- See: [`.agents/SCENEDB_MIGRATION.md`](../../../.agents/SCENEDB_MIGRATION.md), [`.agents/RENDER_RUNTIMES.md`](../../../.agents/RENDER_RUNTIMES.md), [`.agents/ENGINE_LIFECYCLE.md`](../../../.agents/ENGINE_LIFECYCLE.md).
