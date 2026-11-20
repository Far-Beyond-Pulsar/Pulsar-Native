# pulsar_content

Shipped game content: what a packaged game reads at runtime. It is GPUI- and renderer-free.

- Owns: the `game.toc` and `game_NNNN.pak` archive format, the content root resolver (a packaged `Content/` directory or a development project root), the virtual-filesystem provider that serves a pak, the asset registry, and the runtime project settings (`Pulsar/project.json`).
- Entry points: modules `pak`, `provider`, `registry`, `root`, `settings`, `packer` (`ContentPacker`).
- Workspace path dependencies, plus SceneDB (Cargo.toml): `engine_fs`, `pulsar_plugins`.
- Layer: kernel. It may depend on kernel and foundation crates only, dev dependencies included, never on a plugin, tool or host crate (`architecture/layers.toml`, `just arch`).
- Must not depend on: GPUI and the renderer.
- See: [`.agents/QUICK_BUILD_PLAN.md`](../../../.agents/QUICK_BUILD_PLAN.md) (the pack step and the pak set).
