# pulsar_world_registry

The class-name-keyed registry of reflected components in SceneDB's `World`.

- Owns: factories, boundary decode, clone and erased insert and remove, reflected property and method dispatch, object feeds, change watches, systems, and component lifecycles (`tick_live_components`).
- Entry points: modules `component_lifecycle`, `dispatch`, `object_feed`, `change_watch`, `scene_subsystems`, `systems`; the registration side is `#[register_world_component]` from `engine_class_derive`.
- Workspace path dependencies, plus SceneDB (Cargo.toml): `profiling`, `pulsar_component_lifecycle`, `pulsar_events`, `pulsar_plugins`, `pulsar_reflection`, `pulsar_scene_model`, `pulsar_script_math`, `pulsar_script_vm`, `pulsar_scenedb` (git).
- Layer: kernel. It may depend on kernel and foundation crates only, dev dependencies included, never on a plugin, tool or host crate (`architecture/layers.toml`, `just arch`).
- Must not depend on: plugins, tools or host crates.
- See: [`.agents/SCENEDB_MIGRATION.md`](../../../.agents/SCENEDB_MIGRATION.md), [`.agents/COMPONENT_LIFECYCLE_PLAN.md`](../../../.agents/COMPONENT_LIFECYCLE_PLAN.md).
