# Engine lifecycle

The engine starts in `crates/core/engine/src/main.rs` (the graph and the event loop; the GPUI application setup is in `startup.rs`). The startup is driven
by an explicit dependency graph (`InitGraph`) rather than a linear sequence.

## InitGraph

An `InitGraph` is a DAG of `InitTask` nodes. Each task has an ID, a human
name, a list of dependency task IDs, and an executor closure. The graph is
topologically sorted (Kahn's algorithm) and validated for cycles, missing
deps, and duplicate tasks before execution.

```rust
// From engine/src/init/graph.rs
pub struct InitTask {
    pub id: TaskId,
    pub name: &'static str,
    pub dependencies: Vec<TaskId>,
    pub executor: Box<dyn FnOnce(&mut InitContext)>,
}
```

Tasks are registered with the `init_task!` macro:

```rust
init_task!(graph, LOGGING, "Logging", [], steps::logging::run);
init_task!(graph, BACKEND, "Engine Backend", [RUNTIME], steps::backend::run);
```

## Startup sequence

```
main()
├── dev_bootstrap::run_if_requested()  — developer bootstrap commands exit here
├── prepare_process()
│   ├── Rustls crypto provider
│   ├── dialog helper placement
│   ├── DXC requirement (Windows ARM64)
│   └── macOS accessibility permission
├── Register the main thread; route wgpui frame scopes to the flamegraph
│   (profiling itself stays off until the Flamegraph panel records)
├── Parse args (dotenv, pipeline cache path, clap)
└── InitGraph execution (ordered by deps):

    1. LOGGING           — Tracy + tracing-subscriber setup
    2. APPDATA           — Config directory creation (~/.pulsar/)
    3. SETTINGS          — Load engine config from PulsarConfig
    4. RUNTIME           — Tokio async runtime (multi-threaded)
    5. BACKEND           — EngineBackend::init(), set as the global
                          Plugin components arrive later through `inject_plugin_components`
    6. ENGINE_CONTEXT    — EngineContext::new() (no dependencies: it may run
                          before any of 1-5 in the topological order)
                          Creates StateStore, KeyedStore, handles URI project paths
    7. DEV_DETECT        — Auto-detect source build (check if binary is in target/debug/)
                          Stash embedded default.level for editor scenarios
    8. SET_GLOBAL        — EngineContext::set_global() (stores in OnceLock)
    9. DISCORD           — Discord Rich Presence init
    10. URI_REGISTRATION — Register pulsar:// URL scheme

└── GPU policy check (discrete GPU enforcement; after logging, so it can log)
└── startup::build_gpui_application()
    └── app.run(move |cx| {
        ├── startup::init_ui_stack(cx)
        │   ├── WindowManager / WindowRegistry globals
        │   ├── ui::init(cx)                — UI system bootstrap
        │   ├── ui::themes::init(cx)        — Theme system (load .json themes)
        │   ├── ui_core::init(cx)           — Core editor shell: keybindings, undo keys,
        │   │                                 file manager, editor tools
        │   └── window_manager::register_all_windows(cx)
        │                                   — Process inventory::submit! registrations
        └── startup::open_startup_project — entry window or loading screen
    })
```

## Dynamic plugin libraries

`ui_core::init` does not load plugin libraries. They go through
`plugin_manager::startup`:

1. The loading screen's plugin task calls `plugin_manager::prewarm_plugins("plugins/editor")`,
   which loads each library and checks its integrity on worker threads
   (`PluginManager::prepare_plugin`; no plugin code runs).
2. The loading screen installs them (`install_prepared_plugin`) and
   hands the manager over with `stage_manager`.
3. `ui_core` (`create_plugin_manager`) takes the staged manager. If there is none,
   it makes a fresh one and prepares the libraries itself
   (`prepare_plugins_in_dir`, then `install_prepared_plugin`).

`PluginManager::load_plugins_from_dir` and `load_plugin` have no callers in the
workspace; do not add startup code on them. Project plugins need none of this: they
are linked into the binary (`.agents/PLUGIN_SYSTEM.md`).

## EngineBackend

`EngineBackend` is the global backend handle (`EngineBackend::global()`); it holds the
plugin-provided component factories:

```rust
// From crates/core/engine_backend/src/lib.rs
pub struct EngineBackend {
    plugin_components: component_registry::PluginComponentRegistry,
}
```

`EngineBackend` has no subsystem registry (removed: nothing implemented or
ticked it). Background work is a thread in the engine's thread registry
(`editor_task_queue::spawn_service`, `.agents/EDITOR_TASKS.md`); a plugin's
part in each frame is a SceneDB scene subsystem (`SceneSubsystemRegistration`)
or a class `System` (`.agents/PLUGIN_SYSTEM.md`, "Component classes").

## EngineContext

`EngineContext` is the global context hub, stored in a `OnceLock` after init:

```rust
// From crates/core/engine_state/src/context.rs
pub struct EngineContext {
    pub windows: Arc<DashMap<WindowId, WindowContext>>,
    pub multiuser: ResourceHandle<Option<MultiuserContext>>,
    pub store: StateStore,                     // TypeId-keyed resources
    pub window_state: KeyedStore<WindowId>,    // Per-window resources
}
```

Access pattern:
```rust
let ctx = EngineContext::global().expect("engine initialized");
let handle = ctx.store.get_or_init::<MyResource>();
```

## StateStore and ResourceHandle

`StateStore` is a `TypeId`-keyed table of `ResourceHandle<T>`. Each type `T`
has at most one entry.

`ResourceHandle<T>` is a cheap-clone `Arc` handle with:
- `.read()` — parking_lot `RwLockReadGuard`
- `.write()` — RAII write guard that bumps a version counter on drop
- `.update(f)` — closure-based mutation
- `.changed()` — async listener for the next mutation

No upfront registration. Any `T: Default` can be inserted on first access
via `get_or_init`.

## Startup graph vs linear init

The `InitGraph` exists because some tasks are expensive and could be parallelized
(e.g., `BACKEND` depends only on `RUNTIME`, while `SETTINGS` depends only on
`APPDATA`). Currently execution is sequential because the graph uses `FnOnce`
closures, but the infrastructure supports parallel dispatch for tasks with
independent dependency chains.
