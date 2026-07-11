# Engine lifecycle

The engine starts in `crates/core/engine/src/main.rs`. The startup is driven
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
├── Anti-debug check (Windows only)
├── Rustls crypto provider init
├── GPU policy check (discrete GPU enforcement)
├── macOS accessibility permission
├── Profiling init (Tracy)
├── Parse args (clap, dotenv)
└── InitGraph execution (ordered by deps):

    1. LOGGING           — Tracy + tracing-subscriber setup
    2. APPDATA           — Config directory creation (~/.pulsar/)
    3. SETTINGS          — Load engine config from PulsarConfig
    4. RUNTIME           — Tokio async runtime (multi-threaded)
    5. BACKEND           — EngineBackend::init()
                          Creates SubsystemRegistry, registers physics, rendering, etc.
                          Sets up the GPUI app and hot-reload watchers
    6. ENGINE_CONTEXT    — EngineContext::new()
                          Creates StateStore, KeyedStore, handles URI project paths
    7. DEV_DETECT        — Auto-detect source build (check if binary is in target/debug/)
                          Stash embedded default.level for editor scenarios
    8. SET_GLOBAL        — EngineContext::set_global() (stores in OnceLock)
    9. DISCORD           — Discord Rich Presence init
    10. URI_REGISTRATION — Register pulsar:// URL scheme

└── GPUI Application::with_wgpu_options()
    └── app.run(move |cx| {
        ├── ui::init(cx)                    — UI system bootstrap
        ├── ui::themes::init(cx)            — Theme system (load .json themes)
        ├── ui_core::init(cx)              — Core editor shell:
        │   ├── PluginManager::new()
        │   ├── Register built-in editor providers
        │   └── load_plugins_from_dir()    — Scan plugins/vendor/*.dylib/.so/.dll
        ├── WindowManager/WindowRegistry   — Global window management
        ├── window_manager::register_all_windows(cx)
        │                               — Process inventory::submit! registrations
        └── Open entry window or loading screen
    })
```

## EngineBackend

`EngineBackend` owns the long-lived runtime systems:

```rust
// From crates/core/engine_backend/src/lib.rs
pub struct EngineBackend {
    pub subsystems: SubsystemRegistry,
    pub gpui_app: Option<Application>,
    watchers: Vec<tokio::task::JoinHandle<()>>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}
```

`EngineBackend::init()` creates the `SubsystemRegistry` and registers:
- `HelioRenderer` — the wgpu-based renderer
- `MeshCache` / `SceneObjectCache` — asset caching
- `LiveKeySet` — stale cleanup tracker
- Project scanner for filesystem changes

## Subsystem lifecycle

The `Subsystem` trait (`crates/core/engine_subsystems/src/lib.rs`) defines the
lifecycle for pluggable runtime services:

```rust
pub trait Subsystem: Send + Sync + 'static {
    fn name(&self) -> &'static str;
    fn init(&mut self, _cx: &mut SubsystemContext) -> Result<(), Box<dyn Error>>;
    fn tick(&mut self, _dt: f32) {}
    fn shutdown(&mut self) {}
}
```

- `init()` is called once during startup
- `tick(dt)` is called each frame with delta time
- `shutdown()` is called during graceful exit

## EngineContext

`EngineContext` is the global context hub, stored in a `OnceLock` after init:

```rust
// From crates/core/engine_state/src/context.rs
pub struct EngineContext {
    pub windows: Arc<DashMap<WindowId, WindowContext>>,
    pub multiuser: ResourceHandle<Option<MultiuserContext>>,
    pub renderers: TypedRendererRegistry,
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
