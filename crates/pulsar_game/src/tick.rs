use crate::actor::ActorRegistry;
use crate::blueprint_runtime::{BlueprintDispatcher, BlueprintEvent};
use crate::schedule::Schedule;
use crate::task::TaskPool;
use crate::time::{Clock, GameTime};
use crate::window::{WindowBridge, WindowCommand, WindowManager};
use crate::world::World;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Describes how the tick loop advances time.
#[derive(Clone, Copy, Debug)]
pub enum TickMode {
    /// Advance by a fixed `dt` every tick regardless of wall-clock time.
    /// Use for deterministic simulation (physics, rollback netcode).
    Fixed { dt: Duration },
    /// Advance by real elapsed wall-clock time per tick, capped at `max_delta`.
    Variable { max_delta: Duration },
}

impl Default for TickMode {
    fn default() -> Self {
        TickMode::Fixed {
            dt: Duration::from_secs_f64(1.0 / 60.0),
        }
    }
}

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
        self.schedule.run(&mut self.world, time);
        self.actors.tick_all(&mut self.world, time);

        // Drive runtime blueprint tick events after ECS + actor updates.
        if let Some(dispatcher) = &self.blueprint_dispatcher {
            let mut dispatcher = dispatcher.lock().unwrap();
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
    /// This method:
    /// 1. Creates a [`WindowBridge`] from the event loop's proxy.
    /// 2. Injects a [`WindowManager`] into `self.window_manager` so actors can
    ///    call `wm.open(…)` to open their first window.
    /// 3. Spawns the ECS tick loop on a background thread.
    /// 4. **Blocks the calling thread** running the winit event loop.
    ///    On macOS the main thread must own the event loop; always call this
    ///    from `main()`, never from a spawned thread.
    ///
    /// When all game windows are closed (or [`stop`][Self::stop] is called),
    /// both the event loop and the ECS thread exit.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// fn main() {
    ///     let event_loop = winit::event_loop::EventLoop::with_user_event()
    ///         .build()
    ///         .unwrap();
    ///
    ///     let threads = std::thread::available_parallelism()
    ///         .map(|n| n.get()).unwrap_or(4);
    ///     let mut game = TickLoop::new(TickMode::default(), threads);
    ///     engine_main::setup(&mut game).unwrap();
    ///
    ///     game.run_with_windows(event_loop);
    /// }
    /// ```
    pub fn run_with_windows(
        mut self,
        event_loop: winit::event_loop::EventLoop<WindowCommand>,
    ) {
        use crate::windowed_app::PulsarApp;
        use winit::event_loop::EventLoop;

        // Build the bridge using the event loop proxy so the ECS thread can
        // send window commands without polling.
        let proxy = event_loop.create_proxy();
        let bridge = Arc::new(WindowBridge::new(proxy));

        // Inject the WindowManager so game code can reach it.
        let wm = Arc::new(WindowManager::new(Arc::clone(&bridge)));
        self.window_manager = Some(Arc::clone(&wm));

        // Capture the running flag so the main thread can stop the ECS when
        // the last window closes.
        let running_flag = Arc::clone(&self.running_flag);

        // Spawn the ECS tick thread.
        std::thread::Builder::new()
            .name("pulsar-ecs".into())
            .spawn(move || {
                self.run_blocking();
            })
            .expect("Failed to spawn ECS tick thread");

        // Main thread: drive the winit event loop (required on macOS).
        let mut app = PulsarApp::new(bridge);
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
