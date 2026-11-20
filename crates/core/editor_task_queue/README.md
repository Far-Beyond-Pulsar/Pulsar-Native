# editor_task_queue

The editor's task system: every piece of editor work that is not a single GPUI callback runs here so it can be listed, logged, audited and stopped.

- Owns: async and blocking tasks, per-task span-scoped logs, progress and cancellation, stall diagnosis, the registry of editor threads and long-lived services, and the pool setup.
- Entry points: `TaskQueue::submit_async`, `TaskQueue::submit`, `tracked`, `spawn_service`, `register_current_thread` with `ThreadSpec`.
- Workspace path dependencies, plus SceneDB (Cargo.toml): `helio-tasks`.
- Layer: kernel. It may depend on kernel and foundation crates only, dev dependencies included, never on a plugin, tool or host crate (`architecture/layers.toml`, `just arch`).
- Must not depend on: game or runtime work (its crate docs forbid submitting it here); also plugins, tools or host crates.
- See: [`.agents/EDITOR_TASKS.md`](../../../.agents/EDITOR_TASKS.md).
