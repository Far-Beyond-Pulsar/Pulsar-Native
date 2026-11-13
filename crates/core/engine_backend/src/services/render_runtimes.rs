//! The engine's render runtimes: every Helio renderer the engine runs, owned
//! here and drawn by one scheduler.
//!
//! Helio is a library: it builds a renderer, runs a pipeline and draws a frame
//! into a view, and knows no thread, task or plugin. The engine owns the
//! renderers. Whoever needs one (the level editor's viewports, a plugin's
//! preview, a thumbnail) asks the service for a **runtime**
//! ([`RenderRuntimes::create`]) and gets a [`RuntimeHandle`]: the controls
//! (pause, resume, stop, update mode, visibility, a frame on demand) and the
//! renderer itself, for what the owner drives (camera, selection, its
//! mailbox). The runtime's frames go to its **output** ([`FrameTarget`]): a
//! UI triple buffer (the editor's `ui_helio_viewport::SurfaceTarget`), or an
//! offscreen texture with an async readback ([`OffscreenTarget`]).
//!
//! **One scheduler.** All runtimes are drawn by one service thread, "Helio
//! runtimes" (a persistent task with a heartbeat), in priority order, each at
//! its own pace, on the one device the outputs share. Unreal draws every
//! viewport from its one render thread and Godot's `RenderingServer` draws
//! all viewports in order each frame; the copied per-viewport loops this
//! replaces contended for the same queue and submit lock with no parallelism
//! to gain. The scheduler never waits for a consumer: an output whose last
//! frame was not taken yet ([`FrameTarget::ready`]) is skipped this pass, so a
//! hidden or slow view costs nothing. A renderer's first build (seconds: it
//! compiles pipelines) runs as a pooled task, so the other runtimes keep
//! drawing meanwhile.
//!
//! **States** ([`RuntimeState`]): starting (the first build), running, paused
//! (keeps its GPU resources, draws nothing), stopped (its renderer's GPU
//! resources are freed, the description kept; resuming builds again), lost
//! (its output's device was lost; it builds again on the next output) and
//! failed. **Update modes** ([`UpdateMode`]), as Godot's viewports have them:
//! always, when visible, on demand for N frames, disabled.
//!
//! **Visible.** Each runtime is a persistent task in the editor's Tasks system
//! (`editor_task_queue`): its log (what its renderer logs while it draws), its
//! phase (the state), its heartbeat (one per frame drawn) and its details
//! (owner, scene, size, mode, frame time), which the Tasks window's Renderers
//! view lists. The task system is GPUI-free, so a game build has the same.
//!
//! **Owners.** A runtime carries the origin of the code that asked for it
//! (`pulsar_plugins::origin!()`); when the plugin set changes, the runtimes
//! of a plugin that is no longer enabled are destroyed. Dropping the last
//! handle destroys a runtime; [`RenderRuntimes::shutdown`] stops the scheduler
//! and joins it.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::gpu_renderer::GpuRenderer;

/// How often a runtime draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateMode {
    /// Every frame, visible or not (a capture, a game view that must advance).
    Always,
    /// Every frame while its output is shown; nothing while hidden.
    WhenVisible,
    /// The next `n` frames, then nothing until asked again
    /// ([`RuntimeHandle::request_frames`]).
    OnDemand(u32),
    /// Never.
    Disabled,
}

impl UpdateMode {
    fn label(self) -> String {
        match self {
            Self::Always => "Always".into(),
            Self::WhenVisible => "When visible".into(),
            Self::OnDemand(n) => format!("On demand ({n} left)"),
            Self::Disabled => "Disabled".into(),
        }
    }
}

/// Where a runtime is in its life.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeState {
    /// Waiting for an output, or building its renderer (pipelines compile).
    Starting,
    /// Drawing as its update mode says.
    Running,
    /// Not drawing; its renderer and GPU resources are kept.
    Paused,
    /// Not drawing; its renderer's GPU resources were freed. Resuming builds
    /// them again from the same description.
    Stopped,
    /// Its output's device was lost; it builds again on the next output.
    Lost,
    /// Its renderer could not be built.
    Failed(String),
}

impl RuntimeState {
    fn label(&self) -> String {
        match self {
            Self::Starting => "Starting".into(),
            Self::Running => "Running".into(),
            Self::Paused => "Paused".into(),
            Self::Stopped => "Stopped".into(),
            Self::Lost => "Device lost".into(),
            Self::Failed(reason) => format!("Failed: {reason}"),
        }
    }
}

/// What a runtime draws into. Implemented by each kind of output: a UI
/// triple buffer (the editor's), an offscreen texture.
pub trait FrameTarget: Send + Sync + 'static {
    /// The device and queue frames for it are drawn on.
    fn device(&self) -> wgpu::Device;
    fn queue(&self) -> wgpu::Queue;
    fn format(&self) -> wgpu::TextureFormat;
    /// Its size in pixels; `(0, 0)` while it has none.
    fn size(&self) -> (u32, u32);
    /// Whether its device was lost.
    fn is_lost(&self) -> bool {
        false
    }
    /// Whether it can take a frame now (its consumer took the last one).
    fn ready(&self) -> bool {
        true
    }
    /// How long one frame lasts at its consumer's pace (a display's refresh).
    fn frame_interval(&self) -> Duration {
        Duration::from_secs_f64(1.0 / 60.0)
    }
    /// Draw one frame: `draw` records into the view it is given (with its
    /// size) and returns its submission; the target then publishes it.
    /// Returns whether a frame was published.
    fn draw(
        &self,
        draw: &mut dyn FnMut(&wgpu::TextureView, u32, u32) -> Option<wgpu::SubmissionIndex>,
    ) -> bool;
}

/// What a runtime is: who asked for it and how it runs.
pub struct RuntimeDescription {
    /// Shown in the Tasks window ("Level viewport", "Render graph preview").
    pub label: String,
    /// The code that asked for it: `pulsar_plugins::origin!()`.
    pub origin: &'static str,
    /// The scene it draws, as the Renderers view names it.
    pub scene_label: String,
    /// Higher draws first in a pass.
    pub priority: i32,
    pub mode: UpdateMode,
}

/// A runtime's renderer: the editor-style Helio renderer (a scene, a camera,
/// its mailbox), shared with the owner.
pub type SharedRenderer = Arc<Mutex<GpuRenderer>>;

struct Runtime {
    id: u64,
    description: Mutex<RuntimeDescription>,
    renderer: SharedRenderer,
    output: Mutex<Option<Arc<dyn FrameTarget>>>,
    state: Mutex<RuntimeState>,
    /// The owner asked to pause or stop it (cleared by resume).
    held: Mutex<Option<RuntimeState>>,
    visible: AtomicBool,
    /// Frames still owed in [`UpdateMode::OnDemand`].
    demanded: AtomicU32,
    /// Called right before each frame (the owner samples its input late).
    before_frame: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    /// The next frame starts its temporal history over.
    reset_history: AtomicBool,
    /// A first build is running on the task pool.
    building: AtomicBool,
    /// Running but drawing nothing on purpose (hidden, no frames asked for):
    /// its task is idle, so the missing heartbeat is no stall.
    quiet: AtomicBool,
    last_frame: Mutex<Option<Instant>>,
    /// The last frame's CPU time, in microseconds.
    frame_us: AtomicU64,
    frames: AtomicU64,
    task: editor_task_queue::PersistentTask,
}

impl Runtime {
    fn set_state(&self, state: RuntimeState) {
        let mut current = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if *current == state {
            return;
        }
        self.task.context().set_phase(state.label());
        let idle = !matches!(state, RuntimeState::Running | RuntimeState::Starting);
        self.task.context().set_idle(idle);
        self.quiet.store(idle, Ordering::Release);
        *current = state;
    }

    /// Running and not drawing on purpose (`true`), or drawing again.
    fn set_quiet(&self, quiet: bool) {
        if self.quiet.swap(quiet, Ordering::AcqRel) != quiet {
            self.task.context().set_idle(quiet);
        }
    }

    fn state(&self) -> RuntimeState {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn publish_details(&self) {
        let description = self.description.lock().unwrap_or_else(|e| e.into_inner());
        let size = self
            .output
            .lock()
            .ok()
            .and_then(|output| output.as_ref().map(|output| output.size()))
            .map_or_else(|| "no output".to_string(), |(w, h)| format!("{w}x{h}"));
        let owner = owner_label(description.origin);
        let frame_ms = self.frame_us.load(Ordering::Relaxed) as f64 / 1000.0;
        // The graph's textures (a renderer that is drawing holds its lock:
        // the last value stays until the next publish).
        let graph_mb = self
            .renderer
            .try_lock()
            .ok()
            .and_then(|renderer| renderer.graph_texture_bytes())
            .map_or_else(
                || "-".to_string(),
                |bytes| format!("{:.0} MB", bytes as f64 / 1_048_576.0),
            );
        self.task.context().set_details(vec![
            ("Owner".into(), owner),
            ("Scene".into(), description.scene_label.clone()),
            ("Size".into(), size),
            ("Mode".into(), description.mode.label()),
            ("State".into(), self.state().label()),
            (
                "Visible".into(),
                self.visible.load(Ordering::Relaxed).to_string(),
            ),
            ("Frame (CPU)".into(), format!("{frame_ms:.2} ms")),
            ("Graph textures".into(), graph_mb),
            (
                "Frames".into(),
                self.frames.load(Ordering::Relaxed).to_string(),
            ),
        ]);
    }

    /// Whether it should draw now, and when it is next due otherwise.
    fn due(&self, now: Instant, interval: Duration) -> Result<(), Instant> {
        let last = *self.last_frame.lock().unwrap_or_else(|e| e.into_inner());
        match last {
            Some(last) if now < last + interval => Err(last + interval),
            _ => Ok(()),
        }
    }

    fn wants_frames(&self) -> bool {
        let mode = self
            .description
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .mode;
        match mode {
            UpdateMode::Always => true,
            UpdateMode::WhenVisible => self.visible.load(Ordering::Acquire),
            UpdateMode::OnDemand(_) => self.demanded.load(Ordering::Acquire) > 0,
            UpdateMode::Disabled => false,
        }
    }
}

/// The controls of one runtime, for its owner. Cheap to clone; when the last
/// clone drops, the runtime is destroyed (its renderer is freed once a frame
/// in flight finished).
#[derive(Clone)]
pub struct RuntimeHandle {
    runtime: Arc<Runtime>,
    _owner: Arc<Owned>,
}

/// Destroys its runtime when the last handle goes.
struct Owned {
    id: u64,
    service: &'static RenderRuntimes,
}

impl Drop for Owned {
    fn drop(&mut self) {
        self.service.destroy(self.id);
    }
}

impl RuntimeHandle {
    pub fn id(&self) -> u64 {
        self.runtime.id
    }

    /// The renderer, for what the owner drives (camera, selection, mailbox).
    pub fn renderer(&self) -> &SharedRenderer {
        &self.runtime.renderer
    }

    pub fn state(&self) -> RuntimeState {
        self.runtime.state()
    }

    /// Whether its renderer is being built (a first build, a pipeline swap,
    /// changed graph settings): it shows its last frame meanwhile.
    pub fn is_building(&self) -> bool {
        self.runtime.building.load(Ordering::Acquire)
    }

    /// Frames drawn so far.
    pub fn frames(&self) -> u64 {
        self.runtime.frames.load(Ordering::Acquire)
    }

    /// Draw into `output` from now on (a new surface after a resize of the
    /// window's device, a lost device's replacement).
    pub fn set_output(&self, output: Arc<dyn FrameTarget>) {
        *self
            .runtime
            .output
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(output);
        // A new output is a new start for one that lost its device or could
        // not build on the old one.
        if matches!(self.state(), RuntimeState::Lost | RuntimeState::Failed(_)) {
            self.runtime.set_state(RuntimeState::Starting);
        }
        self.runtime.publish_details();
        global().wake();
    }

    /// Stop drawing; keep the renderer and its GPU resources.
    pub fn pause(&self) {
        self.hold(RuntimeState::Paused);
    }

    /// Stop drawing and free the renderer's GPU resources; the description
    /// stays, and [`resume`](Self::resume) builds them again.
    pub fn stop(&self) {
        self.hold(RuntimeState::Stopped);
    }

    /// Draw again after [`pause`](Self::pause) or [`stop`](Self::stop).
    pub fn resume(&self) {
        *self.runtime.held.lock().unwrap_or_else(|e| e.into_inner()) = None;
        global().wake();
    }

    fn hold(&self, state: RuntimeState) {
        *self.runtime.held.lock().unwrap_or_else(|e| e.into_inner()) = Some(state);
        global().wake();
    }

    pub fn set_mode(&self, mode: UpdateMode) {
        if let UpdateMode::OnDemand(n) = mode {
            self.runtime.demanded.store(n, Ordering::Release);
        }
        self.runtime
            .description
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .mode = mode;
        self.runtime.publish_details();
        global().wake();
    }

    /// Ask for `n` more frames (an [`UpdateMode::OnDemand`] runtime).
    pub fn request_frames(&self, n: u32) {
        self.runtime.demanded.fetch_add(n, Ordering::AcqRel);
        global().wake();
    }

    /// Call `callback` right before each frame, on the scheduler's thread
    /// (sample input as late as possible).
    pub fn set_before_frame(&self, callback: Box<dyn Fn() + Send + Sync>) {
        *self
            .runtime
            .before_frame
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(callback);
    }

    /// The next frame starts its temporal history over (the view was hidden
    /// and the camera moved meanwhile).
    pub fn reset_history(&self) {
        self.runtime.reset_history.store(true, Ordering::Release);
    }

    /// Whether its output is shown (a tab in front); a
    /// [`UpdateMode::WhenVisible`] runtime draws only then.
    pub fn set_visible(&self, visible: bool) {
        if self.runtime.visible.swap(visible, Ordering::AcqRel) != visible {
            self.runtime.publish_details();
            global().wake();
        }
    }
}

/// The service: every runtime and the scheduler that draws them.
pub struct RenderRuntimes {
    runtimes: Mutex<Vec<Arc<Runtime>>>,
    next_id: AtomicU64,
    /// Set by anything that changes what the scheduler should do.
    wake: (Mutex<bool>, Condvar),
    scheduler: Mutex<Option<editor_task_queue::ServiceHandle<()>>>,
    stop: AtomicBool,
    device: Mutex<Option<(wgpu::Device, wgpu::Queue)>>,
    /// A renderer build is running. Builds go one at a time: each compiles
    /// many pipelines synchronously in the driver, and several at once (a
    /// split layout's viewports) only contend.
    building: AtomicBool,
}

/// The process's service.
pub fn global() -> &'static RenderRuntimes {
    // Static: the editor, the game and headless tools all draw through it, and
    // a game has no editor context to hang it on.
    static SERVICE: OnceLock<RenderRuntimes> = OnceLock::new();
    SERVICE.get_or_init(|| RenderRuntimes {
        runtimes: Mutex::new(Vec::new()),
        next_id: AtomicU64::new(1),
        wake: (Mutex::new(false), Condvar::new()),
        scheduler: Mutex::new(None),
        stop: AtomicBool::new(false),
        device: Mutex::new(None),
        building: AtomicBool::new(false),
    })
}

/// The scheduler's longest sleep when nothing is due.
const IDLE_WAIT: Duration = Duration::from_millis(250);

impl RenderRuntimes {
    /// Use `device` for the engine's GPU work that has no output of its own
    /// (thumbnails, the render graph pin probe): the editor offers its
    /// window's, the game its own. Only the first offer counts.
    pub fn offer_device(&self, device: wgpu::Device, queue: wgpu::Queue) {
        let mut slot = self.device.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_none() {
            *slot = Some((device, queue));
        }
    }

    /// The engine's device: the one offered, else one made for this process
    /// (headless: a tool or test with no window). `None` without a GPU.
    pub fn device(&self) -> Option<(wgpu::Device, wgpu::Queue)> {
        let mut slot = self.device.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_none() {
            *slot = headless_device();
        }
        slot.clone()
    }

    /// A new runtime drawing `renderer` (a fresh one when `None`) as
    /// `description` says. It starts once it has an output
    /// ([`RuntimeHandle::set_output`]).
    pub fn create(
        &'static self,
        description: RuntimeDescription,
        renderer: SharedRenderer,
    ) -> RuntimeHandle {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let task = editor_task_queue::global().persistent_task(
            editor_task_queue::TaskDescription::persistent(
                format!("Helio runtime: {}", description.label),
                RUNTIME_CATEGORY,
            ),
            // A frame per pass at least every few seconds while running; idle
            // states turn the check off.
            Some(Duration::from_secs(5)),
        );
        let demanded = match description.mode {
            UpdateMode::OnDemand(n) => n,
            _ => 0,
        };
        let runtime = Arc::new(Runtime {
            id,
            description: Mutex::new(description),
            renderer,
            output: Mutex::new(None),
            state: Mutex::new(RuntimeState::Starting),
            held: Mutex::new(None),
            visible: AtomicBool::new(true),
            demanded: AtomicU32::new(demanded),
            before_frame: Mutex::new(None),
            reset_history: AtomicBool::new(false),
            building: AtomicBool::new(false),
            quiet: AtomicBool::new(false),
            last_frame: Mutex::new(None),
            frame_us: AtomicU64::new(0),
            frames: AtomicU64::new(0),
            task,
        });
        runtime
            .task
            .context()
            .set_phase(RuntimeState::Starting.label());
        runtime.task.context().set_idle(true);
        runtime.publish_details();
        self.runtimes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(Arc::clone(&runtime));
        self.ensure_scheduler();
        self.wake();
        RuntimeHandle {
            runtime,
            _owner: Arc::new(Owned { id, service: self }),
        }
    }

    /// Every runtime's id, label and state, for tools and tests.
    pub fn list(&self) -> Vec<(u64, String, RuntimeState)> {
        self.runtimes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|runtime| {
                let label = runtime
                    .description
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .label
                    .clone();
                (runtime.id, label, runtime.state())
            })
            .collect()
    }

    fn destroy(&self, id: u64) {
        let removed = {
            let mut runtimes = self.runtimes.lock().unwrap_or_else(|e| e.into_inner());
            let at = runtimes.iter().position(|runtime| runtime.id == id);
            at.map(|at| runtimes.remove(at))
        };
        if let Some(runtime) = removed {
            runtime.task.context().info("destroyed");
            // Its task ends with its last handle, not when the scheduler,
            // mid-step, lets go of it.
            runtime.task.finish();
        }
        self.wake();
    }

    /// Stops the scheduler and waits for it; runtimes stop drawing.
    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::Release);
        self.wake();
        let handle = self
            .scheduler
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(handle) = handle {
            let _ = handle.join();
        }
    }

    fn wake(&self) {
        let (flag, condvar) = &self.wake;
        *flag.lock().unwrap_or_else(|e| e.into_inner()) = true;
        condvar.notify_all();
    }

    fn ensure_scheduler(&'static self) {
        let mut slot = self.scheduler.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_some() || self.stop.load(Ordering::Acquire) {
            return;
        }
        let spawned = editor_task_queue::spawn_service(
            editor_task_queue::ServiceDescription::new(
                "Helio runtimes",
                "render",
                "Helio Runtimes",
                editor_task_queue::ThreadRole::Render,
            )
            .owner("engine")
            .heartbeat(Duration::from_secs(3)),
            move |ctx| self.run(ctx),
        );
        match spawned {
            Ok(handle) => *slot = Some(handle),
            Err(error) => tracing::error!("The Helio runtimes scheduler did not start: {error}"),
        }
    }

    /// The scheduler's loop.
    fn run(&'static self, ctx: &editor_task_queue::TaskContext) {
        profiling::set_thread_name("Helio Runtimes");
        let mut plugin_generation = pulsar_plugins::generation();
        while !self.stop.load(Ordering::Acquire) {
            ctx.heartbeat();
            let generation = pulsar_plugins::generation();
            if generation != plugin_generation {
                plugin_generation = generation;
                self.destroy_disabled_owners();
            }
            let mut runtimes: Vec<Arc<Runtime>> = self
                .runtimes
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            runtimes.sort_by_key(|runtime| {
                -runtime
                    .description
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .priority
            });
            let now = Instant::now();
            let mut next = now + IDLE_WAIT;
            let mut drew = false;
            for runtime in &runtimes {
                match self.step(runtime, now) {
                    Step::Drew(at) => {
                        drew = true;
                        next = next.min(at);
                    }
                    Step::DueAt(at) => next = next.min(at),
                    Step::Idle => {}
                }
            }
            ctx.set_idle(!drew && runtimes.iter().all(|r| r.state() != RuntimeState::Running));
            self.sleep_until(next);
        }
    }

    fn sleep_until(&self, until: Instant) {
        let (flag, condvar) = &self.wake;
        let mut woken = flag.lock().unwrap_or_else(|e| e.into_inner());
        while !*woken {
            let now = Instant::now();
            if now >= until {
                break;
            }
            woken = condvar
                .wait_timeout(woken, until - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        *woken = false;
    }

    fn destroy_disabled_owners(&self) {
        let mut runtimes = self.runtimes.lock().unwrap_or_else(|e| e.into_inner());
        runtimes.retain(|runtime| {
            let origin = runtime
                .description
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .origin;
            let keep = pulsar_plugins::origin_enabled(origin);
            if !keep {
                runtime
                    .task
                    .context()
                    .warn("destroyed: the plugin that owns it is no longer enabled");
                runtime.set_state(RuntimeState::Stopped);
                runtime.task.finish();
            }
            keep
        });
    }

    /// One runtime's turn in a pass.
    fn step(&self, runtime: &Arc<Runtime>, now: Instant) -> Step {
        let held = runtime
            .held
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        match held {
            Some(RuntimeState::Paused) => {
                runtime.set_state(RuntimeState::Paused);
                return Step::Idle;
            }
            Some(RuntimeState::Stopped) => {
                if runtime.state() != RuntimeState::Stopped {
                    if let Ok(mut renderer) = runtime.renderer.try_lock() {
                        renderer.release_gpu_resources();
                        runtime.set_state(RuntimeState::Stopped);
                        runtime.task.context().info("stopped: GPU resources freed");
                    }
                }
                return Step::Idle;
            }
            _ => {}
        }
        let Some(output) = runtime
            .output
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        else {
            return Step::Idle;
        };
        if output.is_lost() {
            if runtime.state() != RuntimeState::Lost {
                runtime
                    .task
                    .context()
                    .error("the output's GPU device was lost");
                runtime.set_state(RuntimeState::Lost);
            }
            return Step::Idle;
        }
        if matches!(
            runtime.state(),
            RuntimeState::Lost | RuntimeState::Failed(_)
        ) {
            return Step::Idle;
        }
        let (width, height) = output.size();
        if width == 0 || height == 0 {
            return Step::Idle;
        }
        // A build in flight holds the renderer: nothing to do until it is done.
        let Ok(needs_build) = runtime
            .renderer
            .try_lock()
            .map(|mut renderer| renderer.needs_build())
        else {
            return Step::Idle;
        };
        if needs_build {
            // A first build, the one after a stop, a pipeline swap or changed
            // graph settings compiles pipelines: a pooled task, so the other
            // runtimes keep drawing (the one being rebuilt shows its last
            // frame meanwhile).
            self.build_in_background(runtime, &output);
            return Step::Idle;
        }
        if runtime.state() != RuntimeState::Running {
            runtime.set_state(RuntimeState::Running);
        }
        if !runtime.wants_frames() {
            runtime.set_quiet(true);
            return Step::Idle;
        }
        runtime.set_quiet(false);
        let interval = output.frame_interval();
        if let Err(at) = runtime.due(now, interval) {
            return Step::DueAt(at);
        }
        if !output.ready() {
            // The consumer still holds the last frame: try again shortly.
            return Step::DueAt(now + interval / 4);
        }
        self.draw(runtime, &output, now);
        // Next due one frame on, at its output's pace.
        Step::Drew(now + interval)
    }

    fn build_in_background(&self, runtime: &Arc<Runtime>, output: &Arc<dyn FrameTarget>) {
        if runtime.building.load(Ordering::Acquire) || self.building.swap(true, Ordering::AcqRel) {
            return;
        }
        runtime.building.store(true, Ordering::Release);
        let runtime = Arc::clone(runtime);
        let output = Arc::clone(output);
        let label = runtime
            .description
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .label
            .clone();
        // Only a first build is "starting": a rebuild keeps the runtime
        // running on its last frame.
        if runtime
            .renderer
            .try_lock()
            .is_ok_and(|renderer| !renderer.is_initialized())
        {
            runtime.set_state(RuntimeState::Starting);
        }
        editor_task_queue::global().submit(
            editor_task_queue::TaskDescription::new(
                format!("Build Helio renderer: {label}"),
                "render",
                editor_task_queue::TaskDuration::Long,
            ),
            move |task| {
                task.set_phase("Creating the scene renderer and render graph");
                let (device, queue) = (output.device(), output.queue());
                let (width, height) = output.size();
                // A panic in a pass constructor is the build's failure, not the
                // scheduler's: the flags below are released either way.
                let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let _span = runtime.task.context().span().entered();
                    runtime
                        .renderer
                        .lock()
                        .map(|mut renderer| {
                            renderer.prepare_for_render(
                                &device,
                                &queue,
                                width,
                                height,
                                output.format(),
                            )
                        })
                        .unwrap_or(false)
                }));
                runtime.building.store(false, Ordering::Release);
                global().building.store(false, Ordering::Release);
                let result = match built {
                    Ok(true) => {
                        runtime.set_state(RuntimeState::Running);
                        Ok(())
                    }
                    // Its output's device was lost meanwhile: the next pass
                    // says so and a new output builds it again.
                    Ok(false) if output.is_lost() => Ok(()),
                    Ok(false) => Err("the renderer could not be built".to_string()),
                    Err(_) => Err("building the renderer panicked".to_string()),
                };
                if let Err(reason) = &result {
                    runtime.task.context().error(reason);
                    runtime.set_state(RuntimeState::Failed(reason.clone()));
                }
                runtime.publish_details();
                global().wake();
                result
            },
        );
    }

    fn draw(&self, runtime: &Arc<Runtime>, output: &Arc<dyn FrameTarget>, now: Instant) {
        *runtime.last_frame.lock().unwrap_or_else(|e| e.into_inner()) = Some(now);
        // The renderer's own logs while it draws belong to this runtime's task.
        let _span = runtime.task.context().span().entered();
        let Ok(mut renderer) = runtime.renderer.try_lock() else {
            // The owner holds it (an edit, a capture): next pass.
            return;
        };
        let started = Instant::now();
        tracing::trace!(target: "render_runtimes", runtime = runtime.id, "draw: renderer locked");
        if runtime.reset_history.swap(false, Ordering::AcqRel) {
            renderer.reset_taa();
        }
        if let Some(callback) = runtime
            .before_frame
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            callback();
        }
        tracing::trace!(target: "render_runtimes", runtime = runtime.id, "draw: input latched");
        let (device, queue, format) = (output.device(), output.queue(), output.format());
        let (width, height) = output.size();
        // A pipeline swap or project-setting rebuild happens here, at a frame
        // boundary, before the frame takes the output's submit permission.
        if !renderer.prepare_for_render(&device, &queue, width, height, format) {
            return;
        }
        tracing::trace!(target: "render_runtimes", runtime = runtime.id, "draw: prepared");
        let published = output.draw(&mut |view, width, height| {
            tracing::trace!(target: "render_runtimes", "draw: output acquired");
            let submitted =
                renderer.render_frame_to_surface(&device, &queue, view, width, height, format);
            tracing::trace!(target: "render_runtimes", "draw: rendered");
            submitted
        });
        tracing::trace!(target: "render_runtimes", runtime = runtime.id, published, "draw: done");
        drop(renderer);
        let took = started.elapsed();
        runtime
            .frame_us
            .store(took.as_micros() as u64, Ordering::Relaxed);
        if published {
            let frames = runtime.frames.fetch_add(1, Ordering::AcqRel) + 1;
            runtime.task.context().tick(took);
            if let UpdateMode::OnDemand(_) = runtime
                .description
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .mode
            {
                let _ = runtime
                    .demanded
                    .try_update(Ordering::AcqRel, Ordering::Acquire, |left| {
                        left.checked_sub(1)
                    });
            }
            // The details are a snapshot for the Renderers view: a few times
            // a second is plenty.
            if frames % 30 == 1 {
                runtime.publish_details();
            }
        }
    }
}

// ── Host-driven runtimes ────────────────────────────────────────────────────

/// A runtime whose frames its host draws on its own loop: a game, whose
/// frame is the step of its tick (simulation, then the frame of what it
/// simulated), standalone on the window's event loop or embedded in the
/// editor (Play in Editor). The service lists it like any runtime (a
/// persistent task in the Renderers view, with its frame time) and the
/// controls reach it: a paused one draws nothing
/// ([`DrivenRuntime::begin_frame`] says so).
pub struct DrivenRuntime {
    description: Mutex<RuntimeDescription>,
    /// Width in the high half, height in the low; 0 until set.
    size: AtomicU64,
    paused: AtomicBool,
    frames: AtomicU64,
    task: editor_task_queue::PersistentTask,
}

/// One frame of a [`DrivenRuntime`]: its time is recorded when it drops.
pub struct DrivenFrame<'a> {
    runtime: &'a DrivenRuntime,
    started: Instant,
}

impl Drop for DrivenFrame<'_> {
    fn drop(&mut self) {
        let took = self.started.elapsed();
        let frames = self.runtime.frames.fetch_add(1, Ordering::AcqRel) + 1;
        self.runtime.task.context().tick(took);
        if frames % 30 == 1 {
            self.runtime.publish_details(Some(took));
        }
    }
}

impl DrivenRuntime {
    /// Registers a host-driven runtime.
    #[track_caller]
    pub fn register(description: RuntimeDescription) -> Self {
        let task = editor_task_queue::global().persistent_task(
            editor_task_queue::TaskDescription::persistent(
                format!("Helio runtime: {}", description.label),
                RUNTIME_CATEGORY,
            ),
            None,
        );
        task.context().set_phase(RuntimeState::Running.label());
        let runtime = Self {
            description: Mutex::new(description),
            size: AtomicU64::new(0),
            paused: AtomicBool::new(false),
            frames: AtomicU64::new(0),
            task,
        };
        runtime.publish_details(None);
        runtime
    }

    /// The host starts a frame: `None` while the runtime is paused (draw
    /// nothing). Keep the guard for the frame's work; its time is recorded.
    pub fn begin_frame(&self) -> Option<DrivenFrame<'_>> {
        if self.paused.load(Ordering::Acquire) {
            return None;
        }
        // The frame's logs belong to the runtime's task.
        Some(DrivenFrame {
            runtime: self,
            started: Instant::now(),
        })
    }

    /// The runtime's task, to enter its span around the frame's work.
    pub fn task(&self) -> &editor_task_queue::TaskContext {
        self.task.context()
    }

    pub fn pause(&self) {
        self.paused.store(true, Ordering::Release);
        self.task.context().set_phase(RuntimeState::Paused.label());
        self.task.context().set_idle(true);
    }

    pub fn resume(&self) {
        self.paused.store(false, Ordering::Release);
        self.task.context().set_phase(RuntimeState::Running.label());
        self.task.context().set_idle(false);
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }

    /// Its size, for the Renderers view.
    pub fn set_size(&self, width: u32, height: u32) {
        self.size.store(
            u64::from(width) << 32 | u64::from(height),
            Ordering::Release,
        );
        self.publish_details(None);
    }

    fn publish_details(&self, frame: Option<Duration>) {
        let description = self.description.lock().unwrap_or_else(|e| e.into_inner());
        let size = self.size.load(Ordering::Acquire);
        let mut details = vec![
            ("Owner".into(), owner_label(description.origin)),
            ("Scene".into(), description.scene_label.clone()),
            (
                "Size".into(),
                if size == 0 {
                    "unknown".into()
                } else {
                    format!("{}x{}", size >> 32, size & 0xffff_ffff)
                },
            ),
            ("Mode".into(), "Driven by its host".into()),
            (
                "State".into(),
                if self.is_paused() {
                    "Paused"
                } else {
                    "Running"
                }
                .into(),
            ),
            (
                "Frames".into(),
                self.frames.load(Ordering::Relaxed).to_string(),
            ),
        ];
        if let Some(frame) = frame {
            details.push((
                "Frame (CPU)".into(),
                format!("{:.2} ms", frame.as_secs_f64() * 1000.0),
            ));
        }
        self.task.context().set_details(details);
    }
}

/// Who a runtime's origin is, as the Renderers view names it.
fn owner_label(origin: &'static str) -> String {
    match pulsar_plugins::owner_of(origin) {
        pulsar_plugins::Owner::Plugin(plugin) => plugin.id.clone(),
        pulsar_plugins::Owner::Engine(name) => format!("engine ({name})"),
        pulsar_plugins::Owner::Unknown(origin) => origin.to_string(),
    }
}

/// The Tasks category of every runtime (the Renderers view lists it).
pub const RUNTIME_CATEGORY: &str = editor_task_queue::categories::RENDER_RUNTIME;

enum Step {
    /// Drew a frame; the next is due at the instant given.
    Drew(Instant),
    DueAt(Instant),
    Idle,
}

fn headless_device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    let features = helio::required_wgpu_features(adapter.features());
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("Pulsar engine device"),
        required_features: features,
        required_limits: helio::required_wgpu_limits(adapter.limits()),
        experimental_features: helio::required_experimental_features(features),
        ..Default::default()
    }))
    .map_err(|error| tracing::warn!("No GPU device for the engine: {error}"))
    .ok()
}

// ── Offscreen output ─────────────────────────────────────────────────────────

/// An offscreen output: a texture on the engine's device, read back without
/// blocking the frame (thumbnails, captures). [`OffscreenTarget::take_image`]
/// returns the last frame's pixels once its copy finished.
pub struct OffscreenTarget {
    device: wgpu::Device,
    queue: wgpu::Queue,
    format: wgpu::TextureFormat,
    size: (u32, u32),
    texture: wgpu::Texture,
    readback: wgpu::Buffer,
    padded_row: u32,
    state: Arc<Mutex<Readback>>,
}

#[derive(Default)]
enum Readback {
    #[default]
    Empty,
    Pending,
    Ready(Vec<u8>),
}

impl OffscreenTarget {
    /// A `width` x `height` RGBA8 (sRGB) output on the engine's device.
    pub fn new(width: u32, height: u32) -> Option<Self> {
        let (device, queue) = global().device()?;
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Offscreen runtime output"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let padded_row = (width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Offscreen runtime readback"),
            size: u64::from(padded_row) * u64::from(height),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Some(Self {
            device,
            queue,
            format,
            size: (width, height),
            texture,
            readback,
            padded_row,
            state: Arc::new(Mutex::new(Readback::Empty)),
        })
    }

    /// The last frame's RGBA8 pixels (tightly packed rows), once its readback
    /// finished; `None` before. The device is polled without waiting.
    pub fn take_image(&self) -> Option<Vec<u8>> {
        let _ = self.device.poll(wgpu::PollType::Poll);
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        match std::mem::take(&mut *state) {
            Readback::Ready(pixels) => Some(pixels),
            other => {
                *state = other;
                None
            }
        }
    }
}

impl FrameTarget for OffscreenTarget {
    fn device(&self) -> wgpu::Device {
        self.device.clone()
    }

    fn queue(&self) -> wgpu::Queue {
        self.queue.clone()
    }

    fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    fn size(&self) -> (u32, u32) {
        self.size
    }

    fn ready(&self) -> bool {
        !matches!(
            *self.state.lock().unwrap_or_else(|e| e.into_inner()),
            Readback::Pending
        )
    }

    fn draw(
        &self,
        draw: &mut dyn FnMut(&wgpu::TextureView, u32, u32) -> Option<wgpu::SubmissionIndex>,
    ) -> bool {
        let view = self.texture.create_view(&Default::default());
        let (width, height) = self.size;
        if draw(&view, width, height).is_none() {
            return false;
        }
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Offscreen runtime readback"),
            });
        encoder.copy_texture_to_buffer(
            self.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = Readback::Pending;
        let state = Arc::clone(&self.state);
        let buffer = self.readback.clone();
        let padded_row = self.padded_row as usize;
        let row = width as usize * 4;
        self.readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let pixels = result.ok().and_then(|()| {
                    let pixels = buffer.slice(..).get_mapped_range().ok().map(|mapped| {
                        let mut pixels = Vec::with_capacity(row * height as usize);
                        for line in mapped.chunks(padded_row) {
                            pixels.extend_from_slice(&line[..row]);
                        }
                        pixels
                    });
                    buffer.unmap();
                    pixels
                });
                *state.lock().unwrap_or_else(|e| e.into_inner()) = match pixels {
                    Some(pixels) => Readback::Ready(pixels),
                    None => Readback::Empty,
                };
            });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::gpu_renderer::GpuRendererBuilder;

    fn description(label: &str, mode: UpdateMode) -> RuntimeDescription {
        RuntimeDescription {
            label: label.into(),
            origin: pulsar_plugins::engine_origin!(),
            scene_label: "test".into(),
            priority: 0,
            mode,
        }
    }

    fn renderer() -> SharedRenderer {
        Arc::new(Mutex::new(GpuRendererBuilder::new(1, 1).build()))
    }

    fn task_of(label: &str) -> Option<editor_task_queue::TaskSnapshot> {
        let title = format!("Helio runtime: {label}");
        editor_task_queue::global()
            .snapshots()
            .into_iter()
            .find(|task| task.title == title)
    }

    #[test]
    fn a_runtime_is_a_task_listed_with_its_details_until_its_last_handle_drops() {
        let label = "unit: lifecycle";
        let handle = global().create(description(label, UpdateMode::WhenVisible), renderer());
        assert!(global().list().iter().any(|(id, _, _)| *id == handle.id()));
        // No output yet: it waits to start, and its task says so (idle).
        assert_eq!(handle.state(), RuntimeState::Starting);
        let task = task_of(label).expect("the runtime's task");
        assert_eq!(task.category, RUNTIME_CATEGORY);
        assert!(task.idle);
        let details: Vec<&str> = task.details.iter().map(|(k, _)| k.as_str()).collect();
        for key in ["Owner", "Scene", "Size", "Mode", "State"] {
            assert!(details.contains(&key), "{key} in {details:?}");
        }
        assert!(task.details.contains(&("Size".into(), "no output".into())));

        let clone = handle.clone();
        let id = handle.id();
        drop(handle);
        assert!(global().list().iter().any(|(listed, _, _)| *listed == id));
        drop(clone);
        assert!(!global().list().iter().any(|(listed, _, _)| *listed == id));
        assert!(task_of(label).unwrap().status.is_finished());
    }

    #[test]
    fn update_modes_decide_whether_a_runtime_wants_frames() {
        let handle = global().create(
            description("unit: modes", UpdateMode::WhenVisible),
            renderer(),
        );
        let runtime = &handle.runtime;
        assert!(runtime.wants_frames(), "shown by default");
        handle.set_visible(false);
        assert!(!runtime.wants_frames(), "hidden");
        handle.set_mode(UpdateMode::Always);
        assert!(runtime.wants_frames(), "always, hidden or not");
        handle.set_mode(UpdateMode::OnDemand(0));
        assert!(!runtime.wants_frames());
        handle.request_frames(2);
        assert!(runtime.wants_frames());
        handle.set_mode(UpdateMode::Disabled);
        assert!(!runtime.wants_frames());
    }

    #[test]
    fn a_paused_driven_runtime_draws_nothing() {
        let runtime = DrivenRuntime::register(description("unit: driven", UpdateMode::Always));
        {
            let _frame = runtime.begin_frame().expect("running");
        }
        runtime.pause();
        assert!(runtime.begin_frame().is_none());
        runtime.resume();
        assert!(runtime.begin_frame().is_some());
        let task = task_of("unit: driven").expect("its task");
        assert!(task.beats >= 1, "a frame is a heartbeat");
    }

    /// The scheduler draws an offscreen runtime on the engine's device and its
    /// readback arrives without blocking; stopping frees the renderer and
    /// resuming builds it again.
    #[test]
    fn the_scheduler_draws_an_offscreen_runtime_and_stop_frees_its_renderer() {
        let Some(target) = OffscreenTarget::new(32, 32) else {
            eprintln!("GPU_VALIDATION_SKIPPED_NO_ADAPTER: render runtimes");
            return;
        };
        let target = Arc::new(target);
        let handle = global().create(
            description("unit: offscreen", UpdateMode::Always),
            renderer(),
        );
        handle.set_output(target.clone());
        let deadline = Instant::now() + Duration::from_secs(120);
        let mut image = None;
        while image.is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
            image = target.take_image();
        }
        let image = image.expect("a frame was drawn and read back");
        assert_eq!(image.len(), 32 * 32 * 4);
        assert_eq!(handle.state(), RuntimeState::Running);

        handle.stop();
        let deadline = Instant::now() + Duration::from_secs(10);
        while handle.state() != RuntimeState::Stopped && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(handle.state(), RuntimeState::Stopped);
        assert!(!handle.renderer().lock().unwrap().is_initialized());

        let drawn = handle.frames();
        handle.resume();
        let deadline = Instant::now() + Duration::from_secs(120);
        while handle.frames() == drawn && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(handle.frames() > drawn, "resumed: built again and drawing");
    }
}
