# pulsar_package

Packages a project as a standalone game and provides the `pulsar` command-line tool.

- Owns: headless script compilation, level and prefab cooking, collecting the assets they use, writing `Content/game.pak` with its asset registry, and building the game binary.
- Entry points: `package(&PackageOptions)`; the `pulsar` binary (`pulsar package`, `pulsar build-scripts`).
- Workspace path dependencies, plus SceneDB (Cargo.toml): `engine_backend`, `engine_fs`, `engine_state`, `installed_plugins`, `plugin_editor_api`, `plugin_manager`, `pulsar_class`, `pulsar_content`, `pulsar_events`, `pulsar_game`, `pulsar_language`, `pulsar_plugins`, `pulsar_script_runtime`, `pulsar_script_vm`, `pulsar_settings`, `pulsar_world_registry`.
- Layer: host. A host may depend on any layer (`architecture/layers.toml`).
- Must not depend on: nothing is forbidden here: this is a host that links every script language and plugin migration it needs.
- See: [`.agents/QUICK_BUILD_PLAN.md`](../../../.agents/QUICK_BUILD_PLAN.md), [`.agents/COOK_GRAPH_PLAN.md`](../../../.agents/COOK_GRAPH_PLAN.md).
