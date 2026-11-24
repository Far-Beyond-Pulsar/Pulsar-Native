# engine_class_derive

Proc macros for engine classes: reflection, SceneDB storage wiring and GPU mirroring generated from one struct definition.

- Owns: `#[engine_class]` and `#[derive(EngineClass)]`, `#[world_component]`, `#[register_world_component]`, `#[component_events]`, and the `#[property]`, `#[sub_props]` and `#[gpu]` field attributes they read.
- Entry points: the attribute and derive macros in `src/lib.rs`.
- Workspace path dependencies, plus SceneDB (Cargo.toml): `pulsar_scenedb` (git).
- Layer: kernel. It may depend on kernel and foundation crates only, dev dependencies included, never on a plugin, tool or host crate (`architecture/layers.toml`, `just arch`).
- Must not depend on: any engine crate at build time: it is a `proc-macro` crate with no non-dev path-crate dependencies.
- See: [`.agents/REFLECTION.md`](../../../.agents/REFLECTION.md), [`.agents/PLUGIN_SYSTEM.md`](../../../.agents/PLUGIN_SYSTEM.md) ("Component classes").
