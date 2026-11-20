# pulsar_game

The game runtime: tick loop, scripting, native script actors, possession, cameras and view targets.

- Owns: `TickLoop` (fixed or variable timestep, owns the event hub), the component lifecycle traits as seen by game code, `NativeActor` for boxed script actors, game session roles and possession, the scripting driver, and the standalone and embedded game windows.
- Entry points: `tick::TickLoop`, `lifecycle::{Actor, WorldActor, NativeActor}`, modules `possession`, `scripting`, `standalone`.
- Workspace path dependencies, plus SceneDB (Cargo.toml): `editor_task_queue`, `engine_backend`, `engine_fs`, `engine_state`, `helio`, `helio_component`, `profiling`, `pulsar_class`, `pulsar_component_lifecycle`, `pulsar_content`, `pulsar_core`, `pulsar_events`, `pulsar_pie_abi`, `pulsar_plugins`, `pulsar_reflection`, `pulsar_scene`, `pulsar_script_codegen`, `pulsar_script_object_model`, `pulsar_script_runtime`, `pulsar_script_vm`, `pulsar_settings`, `pulsar_std`, `pulsar_traits`, `pulsar_world_registry`, `pulsar_scenedb` (git).
- Layer: kernel. It may depend on kernel and foundation crates only, dev dependencies included, never on a plugin, tool or host crate (`architecture/layers.toml`, `just arch`).
- Must not depend on: plugins, tools or host crates. One exception is recorded and may only shrink: a dev dependency on `installed_plugins` (`architecture/layer-violations.txt`).
- See: [`.agents/ECS.md`](../../../.agents/ECS.md), [`.agents/COMPONENT_LIFECYCLE_PLAN.md`](../../../.agents/COMPONENT_LIFECYCLE_PLAN.md).
