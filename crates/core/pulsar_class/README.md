# pulsar_class

Class assets and placed class instances: a class is a directory under `<project>/src/classes/<Name>/` and this crate gives it identity and turns a placed class into scene state.

- Owns: class GUIDs and the registry from GUID to class directory, the `prefab.json` read model with stable component slot ids, the `ClassInstance` world component, override diffs, instantiation into the scene, the one-language-per-class rule, and the level migration from the older script bindings. It is GPUI-free and shared by the editor, the game runtime and tools.
- Entry points: modules `id` (`ClassId`), `registry`, `prefab`, `language`, `overrides`, `migrate`.
- Workspace path dependencies, plus SceneDB (Cargo.toml): `engine_class_derive`, `engine_fs`, `pulsar_component_lifecycle`, `pulsar_language`, `pulsar_plugins`, `pulsar_reflection`, `pulsar_scene_model`, `pulsar_world_registry`, `pulsar_scenedb` (git).
- Layer: kernel. It may depend on kernel and foundation crates only, dev dependencies included, never on a plugin, tool or host crate (`architecture/layers.toml`, `just arch`).
- Must not depend on: GPUI; plugins, tools or host crates.
- See: [`.agents/SCRIPT_LANGUAGES.md`](../../../.agents/SCRIPT_LANGUAGES.md), [`.agents/COMPONENT_LIFECYCLE_PLAN.md`](../../../.agents/COMPONENT_LIFECYCLE_PLAN.md).
