use crate::time::to_scenedb_time;
use crate::window::{WindowBridge, WindowCommand, WindowDescriptor, WindowHandle, WindowManager};

use parking_lot::RwLock;
use pulsar_core::{Clock, GameTime, TaskPool, TickMode};
use pulsar_scenedb::{ActorRegistry, Schedule};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// The main game loop.
///
/// Drives:
/// 1. A `Schedule` — ordered set of ECS systems.
/// 2. An `ActorRegistry` — object lifecycle callbacks.
/// 3. A `TaskPool` — background async tasks.
///
/// All three run against ONE authoritative world state:
/// `scene_store`, an `engine_backend::scene::SharedScene` wrapping SceneDB --
/// the same store renderers read (Pulsar-Native#634). A mutation made by a
/// system or actor is visible to the renderer's next frame rebuild, and a
/// level hydrated into the store after `setup()` is visible to gameplay on
/// its next tick; there is no second world copy anywhere.
///
/// ## Locking protocol
///
/// `tick_once` acquires the store's write lock ONCE PER PHASE (schedule →
/// actors → script events), dropping it between phases so render/editor
/// threads are never blocked for a whole tick. Phase code must never stash
/// the guard: borrow it, mutate, let it drop.
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
    /// THE authoritative scene state: SceneDB-backed, shared with every
    /// renderer and editor surface that holds the same handle. Systems and
    /// actors receive `&mut World` borrows of `scene_store.write().world`
    /// per phase; anything they spawn/mutate is globally visible.
    pub scene_store: engine_backend::scene::SharedScene,
    pub schedule: Schedule,
    pub actors: ActorRegistry,
    pub tasks: Arc<TaskPool>,
    /// The script phase: the script runtime, following the world's class
    /// instances (see `crate::scripting::ScriptDriver`). `None` runs no
    /// scripts; [`enable_scripting`](Self::enable_scripting) creates it.
    pub scripts: Option<Arc<Mutex<crate::scripting::ScriptDriver>>>,
    /// The session's engine event hub (`pulsar_events::EventHub`): built-in
    /// events, script events and plugin events, flushed at four fixed
    /// points of every tick (see [`tick_once`](Self::tick_once)). The
    /// script driver is attached to it by
    /// [`enable_scripting`](Self::enable_scripting). Publish input with
    /// [`publish_input`](Self::publish_input).
    pub events: pulsar_events::EventHub,
    /// Set by [`run_with_windows`][Self::run_with_windows]; game code can
    /// clone this to open/close/configure windows from actors and systems.
    pub window_manager: Option<Arc<WindowManager>>,
    clock: Clock,
    mode: TickMode,
    running: Arc<AtomicBool>,
    /// Shared running flag — lets external code stop the loop.
    pub running_flag: Arc<AtomicBool>,
    // ── Native hot-reload bookkeeping (#653, see scripts.rs) ────────────────
    /// Surviving [`ScriptTag`]s collected by `begin_script_reload`, consumed
    /// one-per-registration until empty.
    pub(crate) pending_rebinds: Vec<crate::scripts::RebindTarget>,
    /// Actor shells rebound onto existing entities this session; ticked right
    /// after the registry phase.
    pub(crate) rebinding: Vec<crate::scripts::ReboundActor>,
    /// Set once a reload has been armed, even after every tag is claimed.
    pub(crate) reload_armed: bool,
    // ── Pause / step (Play-in-Editor simulate controls, #925) ───────────────
    paused: bool,
    pending_steps: u32,
    /// The last tick's time, which a paused tick reports again.
    last_time: GameTime,
    /// Problems scripts raised, kept for [`take_script_problems`](Self::take_script_problems)
    /// while [`collect_script_problems`](Self::collect_script_problems) is on.
    script_problems: Vec<pulsar_events::ScriptProblem>,
    collect_problems: bool,
    script_stats: ScriptStats,
}

/// Totals of what the script phase did since the loop was created.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct ScriptStats {
    /// Script phases run.
    pub frames: u64,
    /// Instances started (`begin_play` queued).
    pub started: u64,
    /// Instances stopped.
    pub stopped: u64,
    /// Objects built by `world::spawn` / `world::spawn_child`.
    pub spawned: u64,
    /// Objects removed by `world::destroy`.
    pub destroyed: u64,
    /// Script runtime errors.
    pub script_errors: u64,
    /// Class modules that did not load or link.
    pub load_errors: u64,
}

impl ScriptStats {
    fn absorb(&mut self, report: &crate::scripting::DriverReport) {
        self.frames += 1;
        self.started += report.started.len() as u64;
        self.stopped += report.stopped.len() as u64;
        self.spawned += report.spawned.len() as u64;
        self.destroyed += report.destroyed.len() as u64;
        // Log the first errors; a handler failing every tick would flood.
        for error in &report.script_errors {
            if self.script_errors < LOGGED_SCRIPT_ERRORS {
                tracing::error!("script error: {error}");
            } else if self.script_errors == LOGGED_SCRIPT_ERRORS {
                tracing::error!("further script errors are counted but not logged");
            }
            self.script_errors += 1;
        }
        self.load_errors += report.load_errors.len() as u64;
    }
}

/// Script runtime errors logged before the rest are only counted.
const LOGGED_SCRIPT_ERRORS: u64 = 50;

/// Problems kept between two [`TickLoop::take_script_problems`] calls; the
/// oldest are dropped past this.
const MAX_KEPT_PROBLEMS: usize = 256;

/// The simulation step of one [`TickLoop::step`] in variable-timestep mode.
const STEP_DELTA: std::time::Duration = std::time::Duration::from_micros(16_667);

impl TickLoop {
    /// Build a new `TickLoop` with its own fresh scene store.
    ///
    /// - `mode` — tick timing strategy.
    /// - `task_threads` — number of background threads in the `TaskPool`.
    ///
    /// The store starts empty; levels hydrate into it later via
    /// `engine_backend::scene::RuntimeLevel::load_into` (play mode), or
    /// actors register into it during `setup()`. To share this exact store
    /// with a renderer, clone [`Self::scene_store`] before handing the loop
    /// to a runner.
    pub fn new(mode: TickMode, task_threads: usize) -> Self {
        let max_delta = match mode {
            TickMode::Fixed { dt } => dt * 5,
            TickMode::Variable { max_delta } => max_delta,
        };
        let running = Arc::new(AtomicBool::new(false));
        Self {
            scene_store: Arc::new(RwLock::new(engine_backend::scene::new_scene())),
            schedule: Schedule::new(),
            actors: ActorRegistry::new(),
            tasks: Arc::new(TaskPool::new(task_threads)),
            scripts: None,
            events: pulsar_events::EventHub::new(),
            window_manager: None,
            clock: Clock::new(max_delta),
            mode,
            running: running.clone(),
            running_flag: running,
            pending_rebinds: Vec::new(),
            rebinding: Vec::new(),
            reload_armed: false,
            paused: false,
            pending_steps: 0,
            last_time: GameTime { elapsed: std::time::Duration::ZERO, delta: std::time::Duration::ZERO, tick: 0 },
            script_problems: Vec::new(),
            collect_problems: false,
            script_stats: ScriptStats::default(),
        }
    }

    /// Build a `TickLoop` over an EXISTING shared store -- the ABI v2 PIE
    /// path (#635): the guest adopts the HOST's authoritative world instead
    /// of constructing its own, so editor edits and gameplay mutations hit
    /// one world. The handle must be the same `Arc` the host transferred;
    /// see `pulsar_pie_abi`'s module doc for the single-count transfer rule.
    pub fn with_scene_store(
        scene_store: engine_backend::scene::SharedScene,
        mode: TickMode,
        task_threads: usize,
    ) -> Self {
        let max_delta = match mode {
            TickMode::Fixed { dt } => dt * 5,
            TickMode::Variable { max_delta } => max_delta,
        };
        let running = Arc::new(AtomicBool::new(false));
        Self {
            scene_store,
            schedule: Schedule::new(),
            actors: ActorRegistry::new(),
            tasks: Arc::new(TaskPool::new(task_threads)),
            scripts: None,
            events: pulsar_events::EventHub::new(),
            window_manager: None,
            clock: Clock::new(max_delta),
            mode,
            running: running.clone(),
            running_flag: running,
            pending_rebinds: Vec::new(),
            rebinding: Vec::new(),
            reload_armed: false,
            paused: false,
            pending_steps: 0,
            last_time: GameTime { elapsed: std::time::Duration::ZERO, delta: std::time::Duration::ZERO, tick: 0 },
            script_problems: Vec::new(),
            collect_problems: false,
            script_stats: ScriptStats::default(),
        }
    }

    /// Execute one logical tick against the shared world.
    ///
    /// Returns the `GameTime` snapshot for this tick.
    ///
    /// While [paused](Self::set_paused) nothing runs (no systems, actors,
    /// scripts or event flushes) and the last tick's time comes back with a
    /// zero delta, unless a [`step`](Self::step) is pending: then one tick
    /// runs with a fixed delta (the fixed timestep, or 1/60 s).
    pub fn tick_once(&mut self) -> GameTime {
        if self.paused {
            if self.pending_steps == 0 {
                return GameTime { delta: std::time::Duration::ZERO, ..self.last_time };
            }
            self.pending_steps -= 1;
            let delta = match self.mode {
                TickMode::Fixed { dt } => dt,
                TickMode::Variable { .. } => STEP_DELTA,
            };
            self.clock.tick_counter += 1;
            let time = GameTime {
                elapsed: self.last_time.elapsed + delta,
                delta,
                tick: self.last_time.tick + 1,
            };
            return self.run_tick(time);
        }
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
        self.run_tick(time)
    }

    /// Pause or resume the simulation (Play-in-Editor's pause button).
    /// Rendering is the host's business and goes on.
    pub fn set_paused(&mut self, paused: bool) {
        if self.paused == paused {
            return;
        }
        self.paused = paused;
        if !paused {
            self.pending_steps = 0;
            // The paused wall time is not game time.
            self.clock.skip_to_now();
        }
    }

    /// Whether the simulation is paused.
    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// While paused: run `frames` more ticks, one per [`tick_once`](Self::tick_once)
    /// call. Ignored while running.
    pub fn step(&mut self, frames: u32) {
        if self.paused {
            self.pending_steps = self.pending_steps.saturating_add(frames);
        }
    }

    /// Ticks run so far (paused ticks do not count).
    pub fn ticks(&self) -> u64 {
        self.last_time.tick
    }

    /// Keep the problems scripts raise (errors, link errors, dropped
    /// waiting calls, as editor problems) for [`take_script_problems`](Self::take_script_problems).
    /// Play-in-Editor turns it on; without it they are only logged.
    pub fn collect_script_problems(&mut self, on: bool) {
        self.collect_problems = on;
        if !on {
            self.script_problems.clear();
        }
    }

    /// The problems collected since the last call.
    pub fn take_script_problems(&mut self) -> Vec<pulsar_events::ScriptProblem> {
        std::mem::take(&mut self.script_problems)
    }

    /// What the script phase did so far (instances started, objects
    /// spawned, errors).
    pub fn script_stats(&self) -> &ScriptStats {
        &self.script_stats
    }

    fn run_tick(&mut self, time: GameTime) -> GameTime {
        self.last_time = time;
        profiling::profile_scope!("TickLoop::tick");
        let scenedb_time = to_scenedb_time(time);

        // Flush 1 (after input): input published since the last tick
        // (`publish_input`, PIE input forwarding) and anything else queued
        // between ticks.
        self.events.flush(pulsar_events::FlushPoint::AfterInput);

        // Phase 1: ECS systems. Short write scope -- the renderer takes this
        // same lock every frame to rebuild its draw lists (see
        // HelioRenderer::sync_scene_delta's phase docs), so nothing here may
        // hold it across phases.
        {
            let mut store = self.scene_store.write();
            self.schedule.run(&mut store.world, scenedb_time);
        }

        // Phase 2: actor lifecycle ticks (`Actor::tick` is deliberately
        // time-free -- see `pulsar_scenedb::actor`'s trait doc). Separate
        // lock acquisition from phase 1 on purpose: a system that queued
        // work for actors can't starve the render thread for two phases'
        // worth of mutation, and vice versa. Hot-reload-rebound shells
        // (#653) tick in the same phase and scope, right after the registry
        // — same callbacks, same world, one lock acquisition.
        {
            let mut store = self.scene_store.write();
            self.actors.tick_all(&mut store.world);
            for shell in &mut self.rebinding {
                shell.tick(&mut store.world);
            }
        }

        // Flush 2 (after physics): physics runs in the ECS schedule; its
        // hits and overlaps (and whatever systems and actors published)
        // are delivered before the script phase, which runs the script
        // handlers they queued.
        self.events.flush(pulsar_events::FlushPoint::AfterPhysics);

        // Phase 3: the script phase, AFTER ECS + actor updates: the driver
        // starts/stops script instances to match the world's class
        // instances, runs `begin_play` for the ones it started, `tick` for
        // all, then applies the spawns/destroys scripts queued. Deferring
        // `begin_play` to here (rather than firing it at registration during
        // level setup) means it observes a fully initialised window/world/
        // scene: `tick_once` only runs after `spawn_ecs_thread`, once the
        // window is ready. Errors are per instance and logged.
        if let Some(driver) = &self.scripts {
            let mut driver = driver.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let report = {
                let mut store = self.scene_store.write();
                driver.run_frame(&mut store.world, time.delta.as_secs_f64())
            };
            self.script_stats.absorb(&report);
            if self.collect_problems && report.has_problems() {
                for mut problem in driver.problems(&report) {
                    problem.frame = Some(time.tick);
                    self.script_problems.push(problem);
                }
                let excess = self.script_problems.len().saturating_sub(MAX_KEPT_PROBLEMS);
                self.script_problems.drain(..excess);
            }
        }

        // Flush 3 (after scripts): what scripts sent (`event::*`,
        // BeginPlay, LevelLoaded, spawns...). Neither the world nor the
        // driver is locked, so host subscribers may use both; script
        // handlers are queued for the next script phase. Flush 4 (end of
        // frame) delivers anything published after that, e.g. by host
        // handlers of flush 3 on another thread.
        self.events.flush(pulsar_events::FlushPoint::AfterScripts);
        self.events.flush(pulsar_events::FlushPoint::EndOfFrame);

        // The world's change history only covers the current frame.
        engine_backend::scene::end_change_window(&self.scene_store.read().world);

        time
    }

    /// Queue an input event (a `pulsar_events::builtin::KeyDown`, ...) on
    /// the global channel; delivered at the next tick's first flush.
    pub fn publish_input<T: pulsar_events::gamma::Event + Send>(&self, event: T) {
        self.events.publish(pulsar_events::gamma::Channel::Global, event);
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

        // Loop is shutting down — give script instances a chance to run
        // their `end_play` teardown logic, mirroring `ActorRegistry`'s
        // begin_play/end_play contract for native actors.
        self.end_scripts();
    }

    /// Turn on the script phase for the project at `project_root`: a script
    /// driver on a fresh runtime (every engine native), following this
    /// loop's world. Idempotent; returns the driver. The generated
    /// `engine_main::setup()` calls this.
    pub fn enable_scripting(
        &mut self,
        project_root: impl Into<std::path::PathBuf>,
    ) -> Arc<Mutex<crate::scripting::ScriptDriver>> {
        let project_root = project_root.into();
        let events = self.events.clone();
        Arc::clone(self.scripts.get_or_insert_with(|| {
            tracing::info!(project = %project_root.display(), "Script driver enabled");
            let mut driver = crate::scripting::new_driver(project_root);
            driver.attach_events(events);
            Arc::new(Mutex::new(driver))
        }))
    }

    /// Turn on the script phase for the game's content: the content root
    /// the process installed (`pulsar_content::current`, set by the
    /// standalone launcher and by Play-in-Editor), with the script limits
    /// and capability allowlist of its project settings. Idempotent. The
    /// generated `engine_main::setup()` calls this; nothing about the
    /// project's location is compiled into the game.
    pub fn enable_project_scripting(&mut self) -> Result<Arc<Mutex<crate::scripting::ScriptDriver>>, String> {
        if let Some(driver) = &self.scripts {
            return Ok(Arc::clone(driver));
        }
        let content = match pulsar_content::current() {
            Some(content) => content,
            None => {
                let content = pulsar_content::ContentRoot::discover()?;
                content.install();
                content
            }
        };
        let events = self.events.clone();
        tracing::info!(content = %content.root().display(), "Script driver enabled");
        let mut driver = crate::scripting::new_content_driver(&content);
        driver.attach_events(events);
        let driver = Arc::new(Mutex::new(driver));
        self.scripts = Some(Arc::clone(&driver));
        Ok(driver)
    }

    /// Run `end_play` on every running script instance (shutdown). Script
    /// subscriptions, queued handler calls and timers are dropped and the
    /// hub's queue is drained (see `EventHub::drain_queued`): a stopped
    /// session leaves nothing on its hub.
    pub fn end_scripts(&mut self) {
        if let Some(driver) = &self.scripts {
            let mut driver = driver.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut store = self.scene_store.write();
            driver.end_play_all(&mut store.world);
        }
        self.events.drain_queued();
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
    /// `project_root` is the project directory (loose dev files). Prefer
    /// [`run_with_content`](Self::run_with_content), which also serves
    /// packaged content; [`crate::standalone::run`] picks the content for
    /// you.
    pub fn run_with_windows(
        self,
        event_loop: winit::event_loop::EventLoop<WindowCommand>,
        primary_window: WindowDescriptor,
        project_root: std::path::PathBuf,
    ) {
        let content = match pulsar_content::current() {
            Some(content) if content.root() == project_root => content,
            _ => {
                let content = pulsar_content::ContentRoot::project(project_root);
                content.install();
                content
            }
        };
        self.run_with_content(event_loop, primary_window, content);
    }

    /// Start a windowed game session on `content` (a project, or a
    /// packaged game's `Content/`): opens `primary_window`, loads the
    /// startup level ([`crate::standalone::startup_level`]) into the shared
    /// world and starts the tick thread. See
    /// [`run_with_windows`](Self::run_with_windows).
    pub fn run_with_content(
        mut self,
        event_loop: winit::event_loop::EventLoop<WindowCommand>,
        primary_window: WindowDescriptor,
        content: pulsar_content::ContentRoot,
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

        // Settings schemas, the global EngineContext (component hydration
        // and runtime behaviours may read it) and the asset root, all
        // before any scene loading.
        tracing::info!(root = %content.root().display(), packaged = content.is_packaged(), "Loading game content");
        if let Err(error) = crate::standalone::prepare_engine(&content) {
            tracing::error!("{error}");
        }
        let default_scene = crate::standalone::startup_level(&content);
        match &default_scene {
            Some(scene) => tracing::info!(scene = %scene.display(), "Startup level"),
            None => tracing::warn!("No startup level found; starting with an empty world"),
        }

        // PulsarApp owns the TickLoop; it spawns the ECS thread in `resumed()`
        // *after* all initial windows are open. The renderer shares THIS
        // loop's scene store (cloned handle) -- one world for gameplay and
        // rendering (Pulsar-Native#634).
        let display = event_loop.owned_display_handle();
        let mut app = PulsarApp::new(
            bridge,
            self,
            initial_windows,
            content,
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
