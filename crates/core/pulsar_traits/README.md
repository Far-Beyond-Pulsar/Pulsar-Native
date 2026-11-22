# pulsar_traits

Traits and gameplay roles: a trait is a named contract of methods (`*.trait.json`), a role is a job a level hands to an implementer of a trait.

- Owns: trait records, the resolved `TraitDb`, the one trait index over every implementer source, conformance checks, and the role registry.
- Entry points: `TraitDb`, `TraitRecord`, `TraitImplementerSource`, `NativeTraitImpl`, `RoleRegistration`.
- Workspace path dependencies, plus SceneDB (Cargo.toml): `engine_fs`, `pulsar_language`, `pulsar_plugins`, `pulsar_reflection`.
- Layer: kernel. It may depend on kernel and foundation crates only, dev dependencies included, never on a plugin, tool or host crate (`architecture/layers.toml`, `just arch`).
- Must not depend on: plugins, tools or host crates.
- See: [`.agents/TRAITS.md`](../../../.agents/TRAITS.md).
