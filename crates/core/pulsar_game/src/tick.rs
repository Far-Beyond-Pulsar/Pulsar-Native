use crate::blueprint_runtime::{BlueprintDispatcher, BlueprintEvent};
use crate::window::{WindowBridge, WindowCommand, WindowDescriptor, WindowHandle, WindowManager};
use pulsar_core::{Clock, GameTime, TaskPool, TickMode};
use pulsar_scenedb::{ActorRegistry, Schedule, World};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The main game loop.
///
/// Drives:
/// 1. A `Schedule` — ordered set of ECS systems.
/// 2. An `ActorRegistry` — object lifecycle callbacks.
/// 3. A `TaskPool` — background async tasks.
///
/// ## Headless (no window)
/// ```rust,ignore
/// game.run_blocking();
/// ```
///
/// ## Windowed (Helio renderer, multiple windows supported)
/// ```rust,ignore
/// let event_loop = winit::event_loop::EventLoop::with_user_event().build().unwrap();
/// game.run_with_windows(event_loop);   // blocks on main thread
/// ```
pub struct TickLoop {
    pub world: World,
    pub schedule: Schedule,
    pub actors: ActorRegistry,
    pub tasks: Arc<TaskPool>,
    pub blueprint_dispatcher: Option<Arc<Mutex<BlueprintDispatcher>>>,
    /// Set by [`run_with_windows`][Self::run_with_windows]; game code can
    /// clone this to open/close/configure windows from actors and systems.
    pub window_manager: Option<Arc<WindowManager>>,
    clock: Clock,
    mode: TickMode,
    running: Arc<AtomicBool>,
    /// Shared running flag — lets external code stop the loop.
    pub running_flag: Arc<AtomicBool>,
}

impl TickLoop {
    /// Build a new `TickLoop`.
    ///
    /// - `mode` — tick timing strategy.
    /// - `task_threads` — number of background threads in the `TaskPool`.
    pub fn new(mode: TickMode, task_threads: usize) -> Self {
        let max_delta = match mode {
            TickMode::Fixed { dt } => dt * 5,
            TickMode::Variable { max_delta } => max_delta,
        };
        let running = Arc::new(AtomicBool::new(false));
        Self {
            world: World::new(),
            schedule: Schedule::new(),
            actors: ActorRegistry::new(),
            tasks: Arc::new(TaskPool::new(task_threads)),
            blueprint_dispatcher: None,
            window_manager: None,
            clock: Clock::new(max_delta),
            mode,
            running: running.clone(),
            running_flag: running,
        }
    }

    /// Execute one logical tick.
    ///
    /// Returns the `GameTime` snapshot for this tick.
    pub fn tick_once(&mut self) -> GameTime {
        let time = match self.mode {
            TickMode::Fixed { dt } => {
                let t = self.clock.tick_counter;
                self.clock.tick_counter += 1;
                GameTime {
                    elapsed: dt * t as u32,
                    delta: dt,
                    tick: t,
                }
            }
            TickMode::Variable { .. } => self.clock.tick(),
        };

        profiling::profile_scope!("TickLoop::tick");
        // `pulsar_core::GameTime` and `pulsar_scenedb::GameTime` are two
        // independent, structurally-identical types — a byproduct of the
        // SceneDB extraction (pulsar_scenedb now lives in its own repo and
        // carries its own copy of `GameTime` rather than depending on
        // pulsar_core). This is a type-identity artifact, not a wgpu 30
        // change; converting at this single call site is the minimal fix
        // that avoids touching either crate's source.
        let scenedb_time = pulsar_scenedb::GameTime {
            elapsed: time.elapsed,
            delta: time.delta,
            tick: time.tick,
        };
        self.schedule.run(&mut self.world, scenedb_time);
        self.actors.tick_all(&mut self.world, scenedb_time);

        // Drive runtime blueprint lifecycle + tick events after ECS + actor
        // updates. `begin_play` for newly-registered instances is deferred to
        // here (rather than fired at registration time during level setup) so
        // it observes a fully-initialised window/world/scene — registration
        // happens before the primary window opens, but `tick_once` only runs
        // after `spawn_ecs_thread`, which is called once the window is ready.
        if let Some(dispatcher) = &self.blueprint_dispatcher {
            let mut dispatcher = dispatcher.lock().unwrap();
            dispatcher.dispatch_pending_begin_play();
            let object_ids = dispatcher.instance_ids();
            for object_id in object_ids {
                let _ = dispatcher.dispatch_event(BlueprintEvent::Tick {
                    object_id,
                    delta_time: time.delta.as_secs_f32(),
                });
            }
        }

        time
    }

    /// Block the calling thread, running the tick loop at the target rate.
    ///
    /// Returns when `stop()` is called or the running flag is cleared externally.
    pub fn run_blocking(&mut self) {
        self.running.store(true, Ordering::SeqCst);
        self.clock.reset();

        let target_dt = match self.mode {
            TickMode::Fixed { dt } => dt,
            TickMode::Variable { max_delta } => max_delta,
        };

        while self.running.load(Ordering::Relaxed) {
            let start = std::time::Instant::now();
            self.tick_once();
            let elapsed = start.elapsed();
            if elapsed < target_dt {
                std::thread::sleep(target_dt - elapsed);
            }
        }

        // Loop is shutting down — give VM blueprint instances a chance to run
        // their `end_play` teardown logic, mirroring `ActorRegistry`'s
        // begin_play/end_play contract for native actors.
        if let Some(dispatcher) = &self.blueprint_dispatcher {
            dispatcher.lock().unwrap().dispatch_end_play_all();
        }
    }

    /// Signal the loop to stop after the current tick.
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    /// `true` while `run_blocking` is executing.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    /// Start a windowed game session with Helio rendering.
    ///
    /// Opens `primary_window` **before** the ECS tick thread starts, so
    /// `begin_play` is guaranteed to fire after the window's GPU context
    /// exists.  Additional windows can be opened at any time via
    /// [`WindowManager::open`][crate::window::WindowManager::open].
    ///
    /// **Must be called from `main()`** — winit requires the event loop on the
    /// main thread on macOS (and most other platforms).
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// fn main() {
    ///     let event_loop = winit::event_loop::EventLoop::with_user_event()
    ///         .build()
    ///         .unwrap();
    ///
    ///     let mut game = TickLoop::new(TickMode::default(), threads);
    ///     engine_main::setup(&mut game).unwrap();
    ///
    ///     game.run_with_windows(event_loop, WindowDescriptor {
    ///         title: "My Game".into(),
    ///         width: 1280,
    ///         height: 720,
    ///         editor_mode: false,
    ///     });
    /// }
    /// ```
    /// `project_root` must be the directory that contains the project's
    /// `Cargo.toml` and `.pulsar/` settings tree.  Pass
    /// `std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))` from the game
    /// project's `main.rs` so the macro expands in the right crate context.
    pub fn run_with_windows(
        mut self,
        event_loop: winit::event_loop::EventLoop<WindowCommand>,
        primary_window: WindowDescriptor,
        project_root: std::path::PathBuf,
    ) {
        use crate::windowed_app::PulsarApp;

        // Build the bridge using the event loop proxy so the ECS thread can
        // send window commands without polling.
        let proxy = event_loop.create_proxy();
        let bridge = Arc::new(WindowBridge::new(proxy));

        // Inject the WindowManager so game code can reach it.
        let wm = Arc::new(WindowManager::new(Arc::clone(&bridge)));
        self.window_manager = Some(Arc::clone(&wm));

        // Pre-allocate a handle for the primary window so `engine_main` can
        // record it before the level starts (e.g. store it in an actor).
        let primary_handle = WindowHandle::next();
        let initial_windows = vec![(primary_handle, primary_window)];

        // Capture running_flag so the app can stop the ECS after exit.
        let running_flag = Arc::clone(&self.running_flag);

        // Register all schema definitions so the config manager knows about them.
        tracing::info!(root = %project_root.display(), "Loading project settings");
        pulsar_settings::register_all_settings(engine_state::settings::global_config());

        // Load persisted project settings from <project_root>/.pulsar/
        let default_scene: Option<std::path::PathBuf> = {
            let ps_result = engine_state::settings::ProjectSettings::new(&project_root);
            tracing::debug!(ok = ps_result.is_some(), "ProjectSettings::new");

            let raw_value = ps_result.and_then(|ps| {
                ps.load_all();
                let v = ps.get("project", "default_map");
                tracing::debug!(found = v.is_some(), "settings key project.default_map");
                v
            });

            let raw_path = raw_value
                .as_ref()
                .and_then(|v| v.as_str().ok())
                .map(|s| s.to_owned());
            tracing::info!(raw_map = ?raw_path, "default_map from settings");

            raw_value
                .and_then(|v| v.as_str().ok().map(|s| project_root.join(s)))
                .and_then(|p| {
                    tracing::info!(scene = %p.display(), exists = p.exists(), "Checking default scene path");
                    if p.exists() {
                        tracing::info!(scene = %p.display(), "Default scene found — will load");
                        Some(p)
                    } else {
                        // Try common fallback locations
                        let fallbacks = [
                            project_root.join("scene/default.level"),
                            project_root.join("scenes/default_level.json"),
                            project_root.join("Pulsar/level.json"),
                        ];
                        for fb in &fallbacks {
                            if fb.exists() {
                                tracing::warn!(
                                    configured = %p.display(),
                                    fallback = %fb.display(),
                                    "Configured scene not found — using fallback"
                                );
                                return Some(fb.clone());
                            }
                        }
                        tracing::warn!(
                            scene = %p.display(),
                            "Default scene not found on disk and no fallback matched — starting with empty world"
                        );
                        None
                    }
                })
        };

        // Set up EngineContext globally before any scene loading or
        // component sync (e.g. ScriptComponent::sync_component calls
        // script_registry() which reads EngineContext::global()).
        let engine_ctx = engine_state::EngineContext::new();
        engine_ctx.clone().set_global();

        // PulsarApp owns the TickLoop; it spawns the ECS thread in `resumed()`
        // *after* all initial windows are open.
        let display = event_loop.owned_display_handle();
        let mut app = PulsarApp::new(
            bridge,
            self,
            initial_windows,
            project_root,
            default_scene,
            display,
        );

        // Main thread: drive the winit event loop (required on macOS).
        event_loop
            .run_app(&mut app)
            .expect("Winit event loop error");

        // The event loop has exited (all windows closed) — stop the ECS thread.
        running_flag.store(false, Ordering::SeqCst);
    }
}

/// A `TickLoop` wrapped in `Arc<Mutex<…>>` for sharing between threads.
///
/// Use `spawn_thread` to run the loop on a dedicated OS thread.
pub struct SharedTickLoop(pub Arc<Mutex<TickLoop>>);

impl SharedTickLoop {
    pub fn new(mode: TickMode, task_threads: usize) -> Self {
        Self(Arc::new(Mutex::new(TickLoop::new(mode, task_threads))))
    }

    /// Spawn a dedicated OS thread that runs the tick loop until stopped.
    pub fn spawn_thread(&self, name: impl Into<String>) -> std::thread::JoinHandle<()> {
        let shared = self.0.clone();
        let name = name.into();
        std::thread::Builder::new()
            .name(name)
            .spawn(move || {
                let mut guard = shared.lock().unwrap();
                guard.run_blocking();
            })
            .expect("failed to spawn tick thread")
    }

    pub fn stop(&self) {
        self.0.lock().unwrap().stop();
    }
}
