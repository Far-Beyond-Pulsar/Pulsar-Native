# engine

The editor binary (package `pulsar_engine`): process setup, the startup graph, and creation of the GPUI application.

- Owns: `main`, the `InitGraph` (`src/init`) with its startup task files (`src/steps`), argument parsing, the GPU policy check, and URI scheme registration.
- Entry points: `main` in `src/main.rs`, the GPUI application setup in `src/startup.rs`; `init::InitGraph` and the `init_task!` macro.
- Workspace path dependencies, plus SceneDB (Cargo.toml): `agent_provider_anthropic`, `agent_provider_aws_bedrock`, `agent_provider_demo_random`, `agent_provider_docker_model_runner`, `agent_provider_gemini`, `agent_provider_github_copilot`, `agent_provider_openai`, `agent_provider_opencode`, `agent_provider_vertex_ai`, `editor_task_queue`, `engine_backend`, `engine_fs`, `engine_state`, `gpui-ce`, `installed_plugins`, `profiling`, `pulsar_auth`, `pulsar_dialog_embed`, `pulsar_install_layout`, `pulsar_package`, `pulsar_std`, `ui`, `ui_common`, `ui_core`, `ui_fab_search`, `ui_flamegraph`, `ui_git_manager`, `ui_loading_screen`, `ui_log_viewer`, `window_manager`.
- Layer: host. A host may depend on any layer (`architecture/layers.toml`).
- Must not depend on: nothing is forbidden here: this is the host that links the installed plugins and the optional tools.
- See: [`.agents/ENGINE_LIFECYCLE.md`](../../../.agents/ENGINE_LIFECYCLE.md).
