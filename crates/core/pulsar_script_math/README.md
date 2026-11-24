# pulsar_script_math

glam math types as script value types, with natives.

- Owns: `Vec2`, `Vec3`, `Vec4`, `DVec3`, `Quat` and `Mat4` registered as script value types (value semantics), with constructor, field and arithmetic natives.
- Entry points: the registration functions in `src/natives.rs`.
- Workspace path dependencies, plus SceneDB (Cargo.toml): `pulsar_script_vm`, `pulsar_scenedb` (git).
- Layer: kernel. It may depend on kernel and foundation crates only, dev dependencies included, never on a plugin, tool or host crate (`architecture/layers.toml`, `just arch`).
- Must not depend on: it is linked by the editor and packaged games, never by the VM itself (`pulsar_script_vm` does not depend on it).
- See: [`.agents/SCRIPT_LANGUAGES.md`](../../../.agents/SCRIPT_LANGUAGES.md).
