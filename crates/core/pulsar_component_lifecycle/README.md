# pulsar_component_lifecycle

The component lifecycle capability registry: the trait a component class implements to receive construct, begin_play, tick and end_play.

- Owns: the `Actor` and `WorldActor` traits, the per-class `ActorTable`, `ActorRegistration` (gated by the class's plugin) and `ActorIndex`.
- Entry points: `Actor`, `WorldActor`, `ActorTable::of::<T>()`, `ActorRegistration`; a class registers it as `ClassSpec { actor: Some(..), .. }`.
- Workspace path dependencies, plus SceneDB (Cargo.toml): `pulsar_events`, `pulsar_plugins`, `pulsar_scenedb` (git).
- Layer: kernel. It may depend on kernel and foundation crates only, dev dependencies included, never on a plugin, tool or host crate (`architecture/layers.toml`, `just arch`).
- Must not depend on: `pulsar_reflection` and `pulsar_script_vm`, so no lifecycle function can be reached from Blueprints. Its `crate_edges` test pins this, because `just arch` checks layers, not crate pairs.
- See: [`.agents/COMPONENT_LIFECYCLE_PLAN.md`](../../../.agents/COMPONENT_LIFECYCLE_PLAN.md).
