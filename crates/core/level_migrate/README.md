# level_migrate

A standalone, best-effort repair tool for `.level` files whose component JSON was saved in the wrong shape.

- Owns: the `pulsar_level_repair` binary. It rebuilds affected components through the editor's reflection path, verifies them with `pulsar_world_registry::hydrate_world_component_for_class`, and backs up the original as `<file>.bak` first.
- Entry points: `main` in `src/main.rs`.
- Workspace path dependencies, plus SceneDB (Cargo.toml): `engine_fs`, `installed_plugins`, `pulsar_class`, `pulsar_reflection`, `pulsar_world_registry`, `pulsar_scenedb` (git).
- Layer: host. A host may depend on any layer (`architecture/layers.toml`).
- Must not depend on: nothing is forbidden here: this is a host tool that links every installed plugin so it can migrate every class.
