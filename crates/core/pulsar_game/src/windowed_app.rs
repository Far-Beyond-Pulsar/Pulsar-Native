//! Main-thread winit `ApplicationHandler` that owns all game windows and their
//! GPU + Helio render state.
//!
//! You never construct this directly.  Call
//! [`TickLoop::run_with_windows`][crate::TickLoop::run_with_windows] — it
//! creates the `PulsarApp`, spawns the ECS tick thread, and hands the main
//! thread to the winit event loop.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use winit::{
    application::ApplicationHandler,
    event::{DeviceEvent, DeviceId, ElementState, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow},
    window::{CursorGrabMode, Window, WindowId},
};

use engine_backend::scene::RuntimeLevel;
use engine_backend::subsystems::render::{DeviceOwner, SceneRenderer, SceneRendererOptions};

use crate::camera_selection::{camera_for_frame, view_target_drives_camera};
use crate::freecam::FreeCam;
use crate::window::{RenderCamera, WindowBridge, WindowCommand, WindowDescriptor, WindowHandle};

// ── Per-window GPU state ──────────────────────────────────────────────────────

struct GameWindow {
    handle: WindowHandle,
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    device: Arc<wgpu::Device>,
    /// wgpu 30 moved `SurfaceTexture::present()` to `Queue::present()`.
    queue: Arc<wgpu::Queue>,
    /// The scene's renderer, as the editor viewport builds it.
    renderer: SceneRenderer,
    /// The renderer as an engine runtime the window's event loop drives: listed
    /// with its frame time, and a paused one draws nothing.
    runtime: engine_backend::services::render_runtimes::DrivenRuntime,
    /// Built-in free-look camera — active when no ECS camera has been set.
    freecam: FreeCam,
    /// Whether the OS says this window has input focus.
    focused: bool,
    /// Earliest time the next frame may be requested. Holds back windows that
    /// cannot present (occluded, minimised) and, with several game copies
    /// running, the ones nobody is looking at.
    next_redraw: Instant,
    /// Built with the editor's overlays (kept to rebuild it on a new device).
    editor_mode: bool,
}

/// Frame interval of an unfocused window when several game copies are running.
const BACKGROUND_FRAME: std::time::Duration = std::time::Duration::from_millis(66);
/// Retry interval of a window that could not present a frame.
const BLOCKED_FRAME: std::time::Duration = std::time::Duration::from_millis(100);

/// Whether this process is one of several game copies launched together.
fn running_beside_copies() -> bool {
    std::env::var("PULSAR_INSTANCE_COUNT")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .is_some_and(|n| n > 1)
}

impl GameWindow {
    /// Initialise a new window's GPU surface and Helio renderer.
    fn new(
        handle: WindowHandle,
        window: Arc<Window>,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        editor_mode: bool,
        scene_store: &engine_backend::scene::SharedScene,
    ) -> Self {
        let surface = instance
            .create_surface(window.clone())
            .expect("Failed to create wgpu surface");

        let caps = surface.get_capabilities(adapter);
        let surface_format = caps
            .formats
            .iter()
            .find(|f| f.is_srgb())
            .copied()
            .unwrap_or(caps.formats[0]);

        let size = window.inner_size();
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: engine_state::settings::global_config()
                .get(
                    engine_state::settings::NS_EDITOR,
                    "renderer",
                    "max_frame_latency",
                )
                .ok()
                .and_then(|value| value.as_int().ok())
                .unwrap_or(1)
                .clamp(1, 4) as u32,
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(&device, &surface_config);

        // The scene's renderer, built as the editor viewport's with the
        // project's settings. `pulsar_game` owns its device (unlike the
        // editor's wgpui-hosted viewport, which shares GPUI's device).
        let renderer = SceneRenderer::new(
            device.clone(),
            queue.clone(),
            scene_store,
            surface_config.width,
            surface_config.height,
            surface_format,
            SceneRendererOptions::game(DeviceOwner::Renderer, editor_mode),
        );
        // The game's device is the engine's: GPU work with no window of its own
        // (the render graph pin probe) uses it, not a second one.
        engine_backend::services::render_runtimes::global()
            .offer_device((*device).clone(), (*queue).clone());
        let runtime = engine_backend::services::render_runtimes::DrivenRuntime::register(
            engine_backend::services::render_runtimes::RuntimeDescription {
                label: format!("Game window {}", handle.id()),
                origin: pulsar_plugins::engine_origin!(),
                scene_label: "game".into(),
                priority: 10,
                mode: engine_backend::services::render_runtimes::UpdateMode::Always,
            },
        );
        runtime.set_size(surface_config.width, surface_config.height);

        Self {
            handle,
            window,
            surface,
            surface_config,
            device,
            queue,
            renderer,
            runtime,
            freecam: FreeCam::default(),
            focused: true,
            next_redraw: Instant::now(),
            editor_mode,
        }
    }

    fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.surface_config.width = width;
        self.surface_config.height = height;
        self.surface.configure(device, &self.surface_config);
        self.renderer.renderer.set_render_size(width, height);
        self.runtime.set_size(width, height);
    }

    fn acquire_after_surface_recovery(&self) -> Option<wgpu::SurfaceTexture> {
        match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => Some(t),
            other => {
                tracing::warn!(
                    window = self.handle.id(),
                    ?other,
                    "Skipping frame after surface recovery"
                );
                None
            }
        }
    }

    /// Render one frame.
    ///
    /// `ecs_camera` is whatever the ECS thread last wrote via
    /// `WindowManager::set_camera`.  If `None`, the built-in freecam is used.
    fn render(&mut self, ecs_camera: Option<RenderCamera>) {
        // A paused runtime draws nothing; the frame's logs are its task's.
        let Some(_frame) = self.runtime.begin_frame() else {
            return;
        };
        let latency_frame = crate::latency::frame_started();
        let _span = self.runtime.task().span().entered();
        let cam = ecs_camera.unwrap_or_else(|| {
            // The freecam stands on the ground's local vertical and
            // flies faster the higher it is, as the editor camera does.
            let ground = self.renderer.camera_ground(self.freecam.position());
            self.freecam.follow_ground(ground);
            self.freecam.to_render_camera()
        });

        // `Renderer::render` tracks its own frame delta internally (wind/TAA/
        // foliage clocks all advance from that), so there is nothing to push
        // in from here.
        let size = self.window.inner_size();
        let aspect = size.width as f32 / size.height.max(1) as f32;

        // wgpu 30 replaced `Result<SurfaceTexture, SurfaceError>` with the
        // `CurrentSurfaceTexture` enum (see wgpui's
        // `platform/cross/renderer.rs` for the template this mirrors).
        // `Suboptimal` is treated the same as `Success` — matching wgpui's
        // choice — rather than forcing a reconfigure after every present;
        // an actually-outdated surface will surface as `Outdated` on the
        // next frame's acquire and get reconfigured then.
        let acquire_scope = profiling::ProfileScope::new_static("Game::acquire_surface");
        let output = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                // The surface no longer matches its last known configuration.
                // Reconfigure once and retry rather than dropping every later
                // frame after a resize or compositor transition.
                self.surface.configure(&self.device, &self.surface_config);
                let Some(texture) = self.acquire_after_surface_recovery() else {
                    return;
                };
                texture
            }
            wgpu::CurrentSurfaceTexture::Timeout => {
                tracing::debug!(
                    window = self.handle.id(),
                    "Skipping frame after surface acquisition timeout"
                );
                self.next_redraw = Instant::now() + BLOCKED_FRAME;
                return;
            }
            wgpu::CurrentSurfaceTexture::Occluded => {
                // Nothing can be shown: don't spin re-requesting frames (or
                // log every one) until the window is visible again.
                tracing::debug!(window = self.handle.id(), "Skipping frame: window occluded");
                self.next_redraw = Instant::now() + BLOCKED_FRAME;
                return;
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                tracing::warn!(
                    window = self.handle.id(),
                    "Skipping frame after surface validation error"
                );
                return;
            }
        };
        drop(acquire_scope);
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        crate::latency::frame_stage(latency_frame, crate::latency::FrameStage::Acquired);

        let frame = {
            profiling::profile_scope!("Game::prepare_frame");
            self.renderer.prepare_frame(
                &cam.frame_view(aspect),
                [self.surface_config.width, self.surface_config.height],
            )
        };
        crate::latency::frame_stage(latency_frame, crate::latency::FrameStage::Prepared);
        {
            profiling::profile_scope!("Game::render");
            if let Err(e) = self.renderer.render(&frame.camera, &view) {
                tracing::error!(window = self.handle.id(), "Render error: {:?}", e);
            }
        }
        crate::latency::frame_stage(latency_frame, crate::latency::FrameStage::Rendered);

        {
            profiling::profile_scope!("Game::present");
            self.queue.present(output);
        }
        crate::latency::frame_presented(latency_frame);
    }
}

// ── Shared GPU context ────────────────────────────────────────────────────────

struct GpuContext {
    instance: wgpu::Instance,
    adapter: Option<wgpu::Adapter>,
    device: Option<Arc<wgpu::Device>>,
    queue: Option<Arc<wgpu::Queue>>,
    /// Whether `device` was lost (`PulsarApp::recover_lost_device`).
    device_loss: Option<engine_backend::services::device_loss::DeviceLoss>,
    power_preference: wgpu::PowerPreference,
}

impl GpuContext {
    fn new(display: winit::event_loop::OwnedDisplayHandle) -> Self {
        let renderer_config = engine_state::settings::global_config();
        let backends = engine_state::settings::renderer_backends();
        let power_preference = renderer_config
            .get(
                engine_state::settings::NS_EDITOR,
                "renderer",
                "gpu_preference",
            )
            .ok()
            .and_then(|value| value.as_str().ok().map(str::to_owned))
            .map(|preference| match preference.as_str() {
                "low_power" => wgpu::PowerPreference::LowPower,
                "auto" => wgpu::PowerPreference::None,
                _ => wgpu::PowerPreference::HighPerformance,
            })
            .unwrap_or(wgpu::PowerPreference::HighPerformance);
        Self {
            instance: wgpu::Instance::new(
                wgpu::InstanceDescriptor {
                    backends,
                    flags: wgpu::InstanceFlags::empty(),
                    ..wgpu::InstanceDescriptor::new_without_display_handle()
                }
                .with_display_handle(Box::new(display)),
            ),
            adapter: None,
            device: None,
            queue: None,
            device_loss: None,
            power_preference,
        }
    }

    fn ensure_device(&mut self, surface: &wgpu::Surface<'_>) {
        if self.device.is_some() {
            return;
        }

        let adapter =
            pollster::block_on(self.instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: self.power_preference,
                compatible_surface: Some(surface),
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            }))
            .expect("No suitable GPU adapter found");

        // The engine's device, as the editor's window gets it: Helio's
        // features and limits, ray queries where the adapter runs them and
        // the user left them on.
        let (device, queue) = engine_backend::services::render_runtimes::request_engine_device(
            &adapter,
            &wgpu::DeviceDescriptor {
                label: Some("Pulsar GPU Device"),
                ..Default::default()
            },
        )
        .expect("Failed to create GPU device");

        let info = adapter.get_info();
        tracing::info!(
            backend = ?info.backend,
            device = %info.name,
            driver = %info.driver,
            "GPU initialised"
        );

        self.device_loss = Some(engine_backend::services::device_loss::DeviceLoss::watch(
            &device,
        ));
        self.adapter = Some(adapter);
        self.device = Some(Arc::new(device));
        self.queue = Some(Arc::new(queue));
    }

    fn adapter(&self) -> &wgpu::Adapter {
        self.adapter.as_ref().expect("GPU not initialised")
    }
    fn device(&self) -> Arc<wgpu::Device> {
        self.device.clone().expect("GPU not initialised")
    }
    fn queue(&self) -> Arc<wgpu::Queue> {
        self.queue.clone().expect("GPU not initialised")
    }
}

// ── PulsarApp ─────────────────────────────────────────────────────────────────

pub struct PulsarApp {
    bridge: Arc<WindowBridge>,
    gpu: GpuContext,
    tick_loop: Option<crate::tick::TickLoop>,
    /// The tick thread once started; joined at shutdown (`join_ecs_thread`).
    ecs_thread: Option<std::thread::JoinHandle<()>>,
    initial_windows: Vec<(WindowHandle, WindowDescriptor)>,
    winit_to_handle: HashMap<WindowId, WindowHandle>,
    windows: HashMap<WindowHandle, GameWindow>,
    /// The game's content (project or packaged); levels and classes load
    /// from it.
    content: pulsar_content::ContentRoot,
    default_scene: Option<PathBuf>,
    /// The ONE shared, SceneDB-owned world: cloned out
    /// of the TickLoop, hydrated with the requested level, and read by every
    /// window's per-frame rebuild. Gameplay mutations land here too, so an
    /// actor-spawned entity renders on the next frame.
    scene_store: engine_backend::scene::SharedScene,
    /// The tick loop's event hub (shared): key and mouse input is published
    /// here and delivered at the next tick's after-input flush.
    events: pulsar_events::EventHub,
    /// Set while a possessed pawn owns the player's input: every window's
    /// free camera stands down (see `possession::PossessionGate`).
    possession: crate::possession::PossessionGate,
    /// The entity the local player's view follows (possession sets it to the
    /// pawn); read by the camera selection.
    view_targets: crate::view_target::ViewTargets,

    /// Which window currently owns the cursor (receives mouse-look).
    focused_window: Option<WindowHandle>,
    /// Whether the cursor is currently captured for mouse-look.
    cursor_captured: bool,
    /// Where the cursor last was, in window pixels, for motion deltas.
    last_cursor: Option<(f64, f64)>,
    /// Time of last `about_to_wait` — used to compute per-frame dt for freecam.
    last_frame: Instant,
    /// When the last frame was drawn, for the profiler's frame markers.
    last_redraw: Option<Instant>,
    /// Several copies of the game are running; unfocused windows draw slowly.
    throttle_background: bool,
    /// The startup level a worker is preparing; the tick thread starts once
    /// it has been applied.
    pending_level: Option<PendingLevel>,
}

/// A level being prepared off the event loop.
struct PendingLevel {
    /// The window whose free camera takes the level's editor camera.
    window: WindowHandle,
    path: PathBuf,
    state: LevelLoadState,
}

/// Where the startup level is: with the worker, or being hydrated a few
/// milliseconds per frame.
enum LevelLoadState {
    Preparing(std::sync::mpsc::Receiver<Result<LoadedLevel, String>>),
    Hydrating {
        hydration: engine_backend::scene::LevelHydration,
        roles: pulsar_traits::WorldRoles,
    },
}

/// What one frame spends hydrating the level, so input and drawing stay
/// responsive while it arrives.
const LEVEL_HYDRATION_BUDGET: std::time::Duration = std::time::Duration::from_millis(4);

/// What the worker hands back: the parsed level and the roles it selects.
struct LoadedLevel {
    prepared: engine_backend::scene::PreparedLevelFile,
    roles: pulsar_traits::WorldRoles,
}

impl PulsarApp {
    pub fn new(
        bridge: Arc<WindowBridge>,
        tick_loop: crate::tick::TickLoop,
        initial_windows: Vec<(WindowHandle, WindowDescriptor)>,
        content: pulsar_content::ContentRoot,
        default_scene: Option<PathBuf>,
        display: winit::event_loop::OwnedDisplayHandle,
    ) -> Self {
        // The tick loop's store handle is cloned BEFORE the loop moves into
        // `self.tick_loop` -- renderer and gameplay then share one world.
        let scene_store = tick_loop.scene_store.clone();
        let events = tick_loop.events.clone();
        let possession = tick_loop.possession_gate();
        let view_targets = tick_loop.view_targets();
        Self {
            bridge,
            gpu: GpuContext::new(display),
            tick_loop: Some(tick_loop),
            ecs_thread: None,
            initial_windows,
            winit_to_handle: HashMap::new(),
            windows: HashMap::new(),
            content,
            default_scene,
            scene_store,
            events,
            possession,
            view_targets,
            focused_window: None,
            cursor_captured: false,
            last_cursor: None,
            last_frame: Instant::now(),
            last_redraw: None,
            throttle_background: running_beside_copies(),
            pending_level: None,
        }
    }

    // ── Cursor capture ────────────────────────────────────────────────────────

    fn capture_cursor(&mut self, handle: WindowHandle) {
        let Some(gw) = self.windows.get(&handle) else {
            return;
        };
        // Try confined first (stays in window), fall back to locked (OS-locked).
        let ok = gw
            .window
            .set_cursor_grab(CursorGrabMode::Confined)
            .or_else(|_| gw.window.set_cursor_grab(CursorGrabMode::Locked))
            .is_ok();
        if ok {
            gw.window.set_cursor_visible(false);
            self.cursor_captured = true;
            self.focused_window = Some(handle);
        }
    }

    fn release_cursor(&mut self) {
        if let Some(handle) = self.focused_window {
            if let Some(gw) = self.windows.get(&handle) {
                let _ = gw.window.set_cursor_grab(CursorGrabMode::None);
                gw.window.set_cursor_visible(true);
            }
        }
        self.cursor_captured = false;
    }

    // ── Device loss ───────────────────────────────────────────────────────────

    /// After the GPU device is lost (a driver reset, a removed GPU): make a
    /// new one and rebuild every window's surface and renderer on it. The
    /// shared world's GPU copy moves to the new device as the first renderer
    /// is built (`ensure_gpu_mirror`), uploading the scene again from the
    /// data SceneDB holds. Each window keeps its camera and focus.
    fn recover_lost_device(&mut self) {
        tracing::warn!("GPU device lost; recreating it and rebuilding the game's windows");
        let old: Vec<GameWindow> = self.windows.drain().map(|(_, window)| window).collect();
        let Some(first) = old.first() else {
            self.gpu.device = None;
            self.gpu.queue = None;
            self.gpu.adapter = None;
            self.gpu.device_loss = None;
            return;
        };
        let probe_window = first.window.clone();
        // Surfaces and renderers on the dead device go first.
        let kept: Vec<(WindowHandle, Arc<Window>, FreeCam, bool, bool)> = old
            .into_iter()
            .map(|gw| (gw.handle, gw.window, gw.freecam, gw.focused, gw.editor_mode))
            .collect();
        self.gpu.device = None;
        self.gpu.queue = None;
        self.gpu.adapter = None;
        self.gpu.device_loss = None;
        let probe = match self.gpu.instance.create_surface(probe_window) {
            Ok(surface) => surface,
            Err(error) => {
                tracing::error!("Could not make a surface to find a new GPU: {error}");
                return;
            }
        };
        self.gpu.ensure_device(&probe);
        drop(probe);
        for (handle, window, freecam, focused, editor_mode) in kept {
            let mut game_window = GameWindow::new(
                handle,
                window,
                &self.gpu.instance,
                self.gpu.adapter(),
                self.gpu.device(),
                self.gpu.queue(),
                editor_mode,
                &self.scene_store,
            );
            game_window.freecam = freecam;
            game_window.focused = focused;
            game_window.window.request_redraw();
            self.windows.insert(handle, game_window);
        }
    }

    // ── Window lifecycle ──────────────────────────────────────────────────────

    fn open_window(
        &mut self,
        event_loop: &ActiveEventLoop,
        handle: WindowHandle,
        desc: WindowDescriptor,
    ) {
        self.open_window_with_scene(event_loop, handle, desc, None);
    }

    fn open_window_with_scene(
        &mut self,
        event_loop: &ActiveEventLoop,
        handle: WindowHandle,
        desc: WindowDescriptor,
        scene_path: Option<PathBuf>,
    ) {
        profiling::profile_scope!("Game::open_window");
        let attrs = Window::default_attributes()
            .with_title(&desc.title)
            .with_inner_size(winit::dpi::LogicalSize::new(desc.width, desc.height));

        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                tracing::error!("Failed to create window '{}': {:?}", desc.title, e);
                return;
            }
        };

        if self.gpu.device.is_none() {
            profiling::profile_scope!("Game::init_gpu_device");
            let temp_surface = self
                .gpu
                .instance
                .create_surface(window.clone())
                .expect("Failed to create surface for GPU init");
            self.gpu.ensure_device(&temp_surface);
        }

        // The renderer takes the level's pipeline when it is made, so choose
        // it first; the level itself is hydrated below, once the renderer's
        // GPU mirror is attached.
        {
            profiling::profile_scope!("Game::select_level_pipeline");
            crate::standalone::select_level_pipeline(self.content.root(), scene_path.as_deref());
        }

        let winit_id = window.id();
        let game_window = {
            profiling::profile_scope!("Game::create_window_renderer");
            GameWindow::new(
                handle,
                window,
                &self.gpu.instance,
                self.gpu.adapter(),
                self.gpu.device(),
                self.gpu.queue(),
                desc.editor_mode,
                &self.scene_store,
            )
        };

        self.winit_to_handle.insert(winit_id, handle);

        // Load the scene into the SHARED world + wire this window's renderer
        // to it. The store already exists (the tick
        // loop's), so the level merges INTO it -- setup-time-registered
        // actors survive; no pulsar_scene::SceneLoader's second world.
        // The level is read, parsed and migrated by a worker; the window is
        // up and its event loop answering while that runs, and the level is
        // hydrated by `poll_pending_level` once it is ready.
        if let Some(path) = scene_path {
            self.begin_level_load(handle, path);
        }

        self.windows.insert(handle, game_window);
        tracing::info!(id = handle.id(), title = %desc.title, "Window opened");
    }

    /// Start preparing the level at `path` on a worker. Its assets (meshes)
    /// load through the renderer's per-frame pump; their sources resolve
    /// against the engine global, which `standalone::prepare_engine` set before
    /// the event loop ran. Placed classes resolve against the content's classes
    /// (its class index when packaged).
    fn begin_level_load(&mut self, window: WindowHandle, path: PathBuf) {
        tracing::info!(scene = %path.display(), window = window.id(), "Preparing scene");
        let (send, receive) = std::sync::mpsc::channel();
        let root = self.content.root().to_path_buf();
        let worker_path = path.clone();
        let spawned = editor_task_queue::spawn_thread(
            editor_task_queue::ThreadSpec::new("level-prepare", editor_task_queue::ThreadRole::Io)
                .owner("pulsar_game"),
            move || {
                let registry = pulsar_class::ClassRegistry::scan(&root);
                let loaded = RuntimeLevel::prepare(&worker_path, &registry)
                    .map(|prepared| {
                        let roles = match engine_fs::virtual_fs::read_to_string(&worker_path) {
                            Ok(json) => pulsar_traits::WorldRoles::for_level(&json, &root),
                            Err(_) => pulsar_traits::WorldRoles::default(),
                        };
                        LoadedLevel { prepared, roles }
                    })
                    .map_err(|error| error.to_string());
                let _ = send.send(loaded);
            },
        );
        match spawned {
            Ok(_) => {
                self.pending_level = Some(PendingLevel {
                    window,
                    path,
                    state: LevelLoadState::Preparing(receive),
                });
            }
            Err(error) => {
                tracing::warn!(scene = %path.display(), "No thread to load the scene: {error}")
            }
        }
    }

    /// Advance the startup level: pick up what the worker prepared, then
    /// hydrate it a few milliseconds per call (`LEVEL_HYDRATION_BUDGET`), so
    /// the window keeps drawing and answering input however large the level
    /// is. Objects appear as they are spawned and their assets stream in
    /// through the renderer's pump. Nothing here reads a file or waits for an
    /// asset. The tick thread starts when the level is complete, so the session
    /// begins with all of it in place (its roles and world settings first).
    fn poll_pending_level(&mut self) {
        let Some(pending) = &mut self.pending_level else {
            return;
        };
        if let LevelLoadState::Preparing(loaded) = &pending.state {
            let loaded = match loaded.try_recv() {
                Ok(loaded) => loaded,
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Err("the loader thread ended without a result".to_string())
                }
            };
            let begun = loaded.and_then(|LoadedLevel { prepared, roles }| {
                prepared
                    .begin_hydration()
                    .map(|hydration| LevelLoadState::Hydrating { hydration, roles })
                    .map_err(|error| error.to_string())
            });
            match begun {
                Ok(state) => pending.state = state,
                Err(error) => {
                    let PendingLevel { window, path, .. } = self.pending_level.take().unwrap();
                    self.finish_level_load(window, path, Err(error));
                    return;
                }
            }
        }
        let LevelLoadState::Hydrating { hydration, .. } = &mut pending.state else {
            return;
        };
        let stepped = {
            let mut store = self.scene_store.write();
            hydration.step(&mut store.world, LEVEL_HYDRATION_BUDGET)
        };
        match stepped {
            Ok(engine_backend::scene::HydrationProgress::More) => {}
            Ok(engine_backend::scene::HydrationProgress::Done(extras)) => {
                let PendingLevel {
                    window,
                    path,
                    state,
                } = self.pending_level.take().unwrap();
                if let LevelLoadState::Hydrating { roles, .. } = state {
                    // Displays of other levels are requested from workers.
                    engine_backend::scene::sublevel_display::sync_level_instances_deferred(
                        &mut self.scene_store.write().world,
                    );
                    self.finish_level_load(window, path, Ok((extras, roles)));
                }
            }
            Err(error) => {
                let PendingLevel { window, path, .. } = self.pending_level.take().unwrap();
                self.finish_level_load(window, path, Err(error.to_string()));
            }
        }
    }

    /// The level is in (or could not be): seed the camera, adopt its roles
    /// and settings, and start the game.
    fn finish_level_load(
        &mut self,
        window: WindowHandle,
        path: PathBuf,
        result: Result<
            (
                engine_backend::scene::LevelExtras,
                pulsar_traits::WorldRoles,
            ),
            String,
        >,
    ) {
        match result {
            Ok((extras, roles)) => {
                // Seed the freecam from the editor camera saved in the level
                // file, if present, so the first frame matches the editor view.
                if let (Some(cam), Some(game_window)) =
                    (extras.editor_camera, self.windows.get_mut(&window))
                {
                    game_window.freecam = FreeCam::default().place(
                        glam::DVec3::from_array(cam.position),
                        cam.yaw,
                        cam.pitch,
                    );
                    tracing::info!(
                        window = window.id(),
                        "FreeCam seeded from scene editor camera"
                    );
                }
                // The level's roles and world settings (gravity,
                // `physics_enabled`, ...) are the session's: the loop is still
                // ours (its thread starts below).
                if let Some(tick_loop) = self.tick_loop.as_mut() {
                    crate::standalone::adopt_prepared_level(tick_loop, &self.content, &path, roles);
                }
                // Scripts need nothing else here: the tick loop's script driver
                // finds the level's class instances in the shared world at its
                // first reconcile.
                tracing::info!(
                    window = window.id(),
                    scene = %path.display(),
                    "Scene hydrated into shared world"
                );
            }
            Err(error) => tracing::warn!(
                window = window.id(),
                scene = %path.display(),
                "Failed to load scene: {error}"
            ),
        }
        // Loaded or not, the game runs.
        self.spawn_ecs_thread();
    }

    fn close_window(&mut self, handle: WindowHandle) {
        if let Some(gw) = self.windows.remove(&handle) {
            self.winit_to_handle.remove(&gw.window.id());
            self.bridge.remove_camera(handle);
            if self.focused_window == Some(handle) {
                self.focused_window = None;
                self.cursor_captured = false;
            }
            tracing::info!(id = handle.id(), "Window closed");
        }
    }

    fn spawn_ecs_thread(&mut self) {
        if let Some(mut tick_loop) = self.tick_loop.take() {
            let handle = editor_task_queue::spawn_thread(
                editor_task_queue::ThreadSpec::new(
                    "pulsar-ecs",
                    editor_task_queue::ThreadRole::Service,
                )
                .owner("pulsar_game"),
                move || tick_loop.run_blocking(),
            )
            .expect("Failed to spawn ECS tick thread");
            self.ecs_thread = Some(handle);
            tracing::info!("ECS tick thread started");
        }
    }

    /// Stop the tick thread and wait for it: its loop ends the session
    /// (`end_play` of every live component, the physics thread joined) before
    /// it returns, and the process must not exit before that. `running` is
    /// cleared again until the thread is done, since a thread that had not
    /// started its loop yet would set it.
    pub(crate) fn join_ecs_thread(&mut self, running: &std::sync::atomic::AtomicBool) {
        let Some(handle) = self.ecs_thread.take() else {
            return;
        };
        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        while !handle.is_finished() && Instant::now() < deadline {
            running.store(false, std::sync::atomic::Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        if handle.is_finished() {
            let _ = handle.join();
            tracing::info!("ECS tick thread ended its session");
        } else {
            tracing::error!("ECS tick thread did not end its session within 10 s");
        }
    }
}

impl ApplicationHandler<WindowCommand> for PulsarApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        profiling::profile_scope!("Game::resumed");
        let initial = std::mem::take(&mut self.initial_windows);
        let default_scene = self.default_scene.take();

        for (i, (handle, desc)) in initial.into_iter().enumerate() {
            let scene = if i == 0 { default_scene.clone() } else { None };
            self.open_window_with_scene(event_loop, handle, desc, scene);
        }

        // The simulation ticks as often as the display refreshes (unless the
        // project says otherwise): an input waits for the next tick, which at
        // 60 Hz on a faster display is the largest part of the input delay.
        let rate = tick_rate_hz(event_loop);
        if let Some(tick_loop) = self.tick_loop.as_mut() {
            tick_loop.set_tick_rate_hz(rate);
        }
        tracing::info!(tick_rate_hz = rate, "Simulation tick rate");

        // With a level on its way, the tick thread starts when it is applied
        // (`poll_pending_level`).
        if self.pending_level.is_none() {
            self.spawn_ecs_thread();
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: WindowCommand) {
        match event {
            WindowCommand::Open { handle, desc } => {
                self.open_window(event_loop, handle, desc);
            }
            WindowCommand::Close { handle } => {
                self.close_window(handle);
                if self.windows.is_empty() {
                    tracing::info!("All windows closed — exiting");
                    event_loop.exit();
                }
            }
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: DeviceId,
        event: DeviceEvent,
    ) {
        // Raw mouse motion — only forwarded to the freecam of the focused window
        // when the cursor is captured.
        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event {
            if self.cursor_captured {
                profiling::profile_scope!("Game::mouse_motion");
                crate::latency::note_input();
                let (x, y) = self.last_cursor.unwrap_or((0.0, 0.0));
                self.events.publish(
                    pulsar_events::gamma::Channel::Global,
                    pulsar_events::builtin::MouseMoved { x, y, dx, dy },
                );
                if let Some(handle) = self.focused_window {
                    if let Some(gw) = self.windows.get_mut(&handle) {
                        gw.freecam.on_mouse_delta(dx, dy);
                    }
                }
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        winit_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(&handle) = self.winit_to_handle.get(&winit_id) else {
            return;
        };

        let _event_scope = match &event {
            WindowEvent::RedrawRequested => None,
            _ => Some(profiling::ProfileScope::new_static("Game::window_event")),
        };

        match event {
            // ── Window management ─────────────────────────────────────────────
            WindowEvent::CloseRequested => {
                self.release_cursor();
                self.close_window(handle);
                if self.windows.is_empty() {
                    event_loop.exit();
                }
            }

            WindowEvent::Resized(size) => {
                if let Some(gw) = self.windows.get_mut(&handle) {
                    if let Some(device) = &self.gpu.device {
                        gw.resize(device, size.width, size.height);
                    }
                }
            }

            // ── Focus ─────────────────────────────────────────────────────────
            WindowEvent::Focused(focused) => {
                if let Some(gw) = self.windows.get_mut(&handle) {
                    gw.focused = focused;
                    gw.next_redraw = Instant::now();
                }
                if focused {
                    self.focused_window = Some(handle);
                    // Re-capture when regaining focus (if we were captured before).
                    if self.cursor_captured {
                        self.capture_cursor(handle);
                    }
                } else {
                    // Always release on blur — other apps need the cursor.
                    self.release_cursor();
                }
            }

            // ── Keyboard ──────────────────────────────────────────────────────
            WindowEvent::KeyboardInput {
                event: key_event, ..
            } => {
                let pressed = key_event.state == ElementState::Pressed;
                if !key_event.repeat {
                    crate::latency::note_input();
                    if let winit::keyboard::PhysicalKey::Code(code) = key_event.physical_key {
                        let key = code as i64;
                        let channel = pulsar_events::gamma::Channel::Global;
                        if pressed {
                            self.events
                                .publish(channel, pulsar_events::builtin::KeyDown { key });
                        } else {
                            self.events
                                .publish(channel, pulsar_events::builtin::KeyUp { key });
                        }
                    }
                }

                // Escape releases the cursor.
                if pressed {
                    if let winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape) =
                        &key_event.logical_key
                    {
                        self.release_cursor();
                        return;
                    }
                }

                if let Some(gw) = self.windows.get_mut(&handle) {
                    gw.freecam.on_key(&key_event.physical_key, pressed);
                }
            }

            // ── Cursor and wheel — published as input events. While the
            //    cursor is captured the raw motion in `device_event` is used
            //    instead, so the two never double up.
            WindowEvent::CursorMoved { position, .. } => {
                let (x, y) = (position.x, position.y);
                let (dx, dy) = self
                    .last_cursor
                    .map_or((0.0, 0.0), |(lx, ly)| (x - lx, y - ly));
                self.last_cursor = Some((x, y));
                if !self.cursor_captured && (dx != 0.0 || dy != 0.0) {
                    self.events.publish(
                        pulsar_events::gamma::Channel::Global,
                        pulsar_events::builtin::MouseMoved { x, y, dx, dy },
                    );
                }
            }

            WindowEvent::MouseWheel { delta, .. } => {
                let (x, y) = match delta {
                    winit::event::MouseScrollDelta::LineDelta(x, y) => (f64::from(x), f64::from(y)),
                    // About 40 pixels is a notch on a precision touchpad.
                    winit::event::MouseScrollDelta::PixelDelta(p) => (p.x / 40.0, p.y / 40.0),
                };
                self.events.publish(
                    pulsar_events::gamma::Channel::Global,
                    pulsar_events::builtin::MouseWheel { x, y },
                );
            }

            // ── Mouse buttons — published as input events; a left click
            //    also captures the cursor ─────────────────────────────────────
            WindowEvent::MouseInput { state, button, .. } => {
                let index = match button {
                    winit::event::MouseButton::Left => 0,
                    winit::event::MouseButton::Right => 1,
                    winit::event::MouseButton::Middle => 2,
                    winit::event::MouseButton::Back => 3,
                    winit::event::MouseButton::Forward => 4,
                    winit::event::MouseButton::Other(n) => i64::from(n),
                };
                let channel = pulsar_events::gamma::Channel::Global;
                if state == ElementState::Pressed {
                    self.events.publish(
                        channel,
                        pulsar_events::builtin::MouseButtonDown { button: index },
                    );
                    if button == winit::event::MouseButton::Left && !self.cursor_captured {
                        self.capture_cursor(handle);
                    }
                } else {
                    self.events.publish(
                        channel,
                        pulsar_events::builtin::MouseButtonUp { button: index },
                    );
                }
            }

            // ── Render ────────────────────────────────────────────────────────
            WindowEvent::RedrawRequested => {
                profiling::profile_scope!("Game::Frame");
                // Frame markers drive the profiler's frame-time graph.
                let now = Instant::now();
                if let Some(last) = self.last_redraw.replace(now) {
                    profiling::record_frame_time(now.duration_since(last).as_secs_f32() * 1000.0);
                }
                // Advance the shared world's authoritative SceneDB state and
                // flush its GPU mirror. Helio's scene join reads the mirrored
                // rows directly -- there is no renderer-owned frame projection
                // or CPU object cache to rebuild here (same seam the editor
                // viewport renderer uses). Bind the meshes that finished
                // loading first, so they upload with this frame (SceneDB#84).
                if let Some(gw) = self.windows.get_mut(&handle) {
                    profiling::profile_scope!("Game::step_scene");
                    gw.renderer.step_scene(&mut self.scene_store.write());
                }

                // Camera precedence: a gameplay-pushed bridge camera wins,
                // then the SHARED world's (the view target's camera, the best
                // CameraComponent, a Camera-typed entity; moving the
                // camera's object moves the view next frame), then the
                // freecam. See `camera_selection`.
                let ecs_camera = {
                    profiling::profile_scope!("Game::camera_for_frame");
                    camera_for_frame(
                        self.bridge.camera(handle),
                        &self.scene_store.read(),
                        self.view_targets.local(),
                    )
                };
                let background = self.throttle_background;
                if let Some(gw) = self.windows.get_mut(&handle) {
                    gw.render(ecs_camera);
                    if background && !gw.focused {
                        gw.next_redraw = gw.next_redraw.max(Instant::now() + BACKGROUND_FRAME);
                    }
                }
            }

            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self
            .gpu
            .device_loss
            .as_ref()
            .is_some_and(engine_backend::services::device_loss::DeviceLoss::is_lost)
        {
            self.recover_lost_device();
        }
        self.poll_pending_level();
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;

        // Advance every window's freecam and request a redraw for the ones
        // that are due; sleep until the earliest of the rest instead of
        // spinning.
        let mut wake: Option<Instant> = None;
        // The free camera stands down only while a possessed pawn's camera is
        // the view: a pawn with no camera leaves the view to the free camera.
        let pawn_view = self.possession.is_possessed()
            && view_target_drives_camera(&self.scene_store.read(), self.view_targets.local());
        for gw in self.windows.values_mut() {
            gw.freecam.set_suspended(pawn_view);
            let start = gw.freecam.position();
            gw.freecam.update(dt);
            let end = gw.freecam.position();
            if start != end {
                if let Some((position, rotation, space)) = gw.renderer.traverse_camera(
                    start,
                    end,
                    gw.freecam.coordinate_space.unwrap_or_default(),
                ) {
                    gw.freecam.pose.transport(position, rotation);
                    gw.freecam.coordinate_space = Some(space);
                }
            }
            if now >= gw.next_redraw {
                gw.window.request_redraw();
            } else {
                wake = Some(wake.map_or(gw.next_redraw, |w| w.min(gw.next_redraw)));
            }
        }
        // While a level is being prepared, look again soon.
        if self.pending_level.is_some() {
            let soon = now + std::time::Duration::from_millis(16);
            wake = Some(wake.map_or(soon, |w| w.min(soon)));
        }
        event_loop.set_control_flow(match wake {
            Some(at) => ControlFlow::WaitUntil(at),
            None => ControlFlow::Wait,
        });
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// The simulation rate: project setting `simulation.tick_rate_hz`, or 0 for
/// the primary display's refresh rate, clamped to 60..=240.
fn tick_rate_hz(event_loop: &ActiveEventLoop) -> f64 {
    let configured = match engine_state::settings::global_config().get(
        engine_state::settings::NS_PROJECT,
        "simulation",
        "tick_rate_hz",
    ) {
        Ok(engine_state::settings::ConfigValue::Int(hz)) => hz,
        _ => 0,
    };
    if configured > 0 {
        return configured.clamp(30, 1000) as f64;
    }
    event_loop
        .primary_monitor()
        .and_then(|monitor| monitor.refresh_rate_millihertz())
        .map_or(60.0, |millihertz| (f64::from(millihertz) / 1000.0).round())
        .clamp(60.0, 240.0)
}
