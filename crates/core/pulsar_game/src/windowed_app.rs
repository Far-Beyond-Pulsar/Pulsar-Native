//! Main-thread winit `ApplicationHandler` that owns all game windows and their
//! GPU + Helio render state.
//!
//! You never construct this directly.  Call
//! [`TickLoop::run_with_windows`][crate::TickLoop::run_with_windows] — it
//! creates the `PulsarApp`, spawns the ECS tick thread, and hands the main
//! thread to the winit event loop.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use winit::{
    application::ApplicationHandler,
    event::{DeviceEvent, DeviceId, ElementState, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow},
    window::{CursorGrabMode, Window, WindowId},
};

use engine_backend::subsystems::render::{
    DeviceOwner, PipelineBuildMode, SceneRenderer, SceneRendererOptions,
};

use crate::camera_selection::{camera_for_frame_turned, view_target_drives_camera};
use crate::freecam::FreeCam;
use crate::game_control::{GameControl, InputMode, LevelEvent};
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
    /// Where the scene is drawn: the swapchain image is acquired only after the
    /// frame is recorded and submitted (see [`GameWindow::render`]), so the
    /// scene goes to a texture of its own and is copied to the image.
    scene_target: SceneTarget,
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
    /// The frame-time breakdown `PULSAR_FRAME_STATS` logs (`crate::frame_stats`).
    stats: crate::frame_stats::FrameStats,
}

/// The texture a frame is drawn into before the swapchain image is acquired,
/// the size and format of the surface.
struct SceneTarget {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

impl SceneTarget {
    fn new(device: &wgpu::Device, config: &wgpu::SurfaceConfiguration) -> Self {
        // The window's own video memory (`engine_backend::services::vram`).
        let _vram_owner = helio_core::vram::root_scope("Game window");
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Game scene target"),
            size: wgpu::Extent3d {
                width: config.width,
                height: config.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self { texture, view }
    }
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

/// The profiler scope a window event of this kind runs under (the hot ones, cursor
/// motion and redraws, have none).
fn window_event_kind(event: &WindowEvent) -> Option<&'static str> {
    Some(match event {
        WindowEvent::Resized(_) => "Game::event::Resized",
        WindowEvent::ScaleFactorChanged { .. } => "Game::event::ScaleFactorChanged",
        WindowEvent::Moved(_) => "Game::event::Moved",
        WindowEvent::Focused(_) => "Game::event::Focused",
        WindowEvent::Occluded(_) => "Game::event::Occluded",
        WindowEvent::MouseInput { .. } => "Game::event::MouseInput",
        WindowEvent::KeyboardInput { .. } => "Game::event::KeyboardInput",
        WindowEvent::CloseRequested => "Game::event::CloseRequested",
        _ => return None,
    })
}

/// The present mode the editor setting `renderer.present_mode` names, when the
/// surface offers it; Fifo otherwise (every surface has it).
fn present_mode(offered: &[wgpu::PresentMode]) -> wgpu::PresentMode {
    use pulsar_content::PresentModeSetting as Setting;
    let setting = pulsar_content::current()
        .map(|content| content.settings().window.present_mode)
        .unwrap_or_default();
    let wanted = match setting {
        Setting::Fifo => wgpu::PresentMode::Fifo,
        Setting::FifoRelaxed => wgpu::PresentMode::FifoRelaxed,
        Setting::Mailbox => wgpu::PresentMode::Mailbox,
        Setting::Immediate => wgpu::PresentMode::Immediate,
        Setting::AutoVsync => wgpu::PresentMode::AutoVsync,
        Setting::AutoNoVsync => wgpu::PresentMode::AutoNoVsync,
    };
    let auto = matches!(
        wanted,
        wgpu::PresentMode::AutoVsync | wgpu::PresentMode::AutoNoVsync | wgpu::PresentMode::Fifo
    );
    if auto || offered.contains(&wanted) {
        wanted
    } else {
        tracing::warn!(?wanted, ?offered, "present mode not offered by the display: using Fifo");
        wgpu::PresentMode::Fifo
    }
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
            // The scene is copied into the image once the frame is recorded.
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_DST,
            format: surface_format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: present_mode(&caps.present_modes),
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
        let scene_target = SceneTarget::new(&device, &surface_config);
        tracing::info!(
            present_mode = ?surface_config.present_mode,
            offered = ?caps.present_modes,
            frame_latency = surface_config.desired_maximum_frame_latency,
            "Surface presentation"
        );

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
            SceneRendererOptions {
                // The loading phase draws frames while the pipeline builds: it
                // must not present one frame of hundreds of milliseconds.
                pipeline_build: PipelineBuildMode::Background,
                ..SceneRendererOptions::game(DeviceOwner::Renderer, editor_mode)
            },
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
            scene_target,
            runtime,
            freecam: FreeCam::default(),
            focused: true,
            next_redraw: Instant::now(),
            editor_mode,
            stats: crate::frame_stats::FrameStats::new(),
        }
    }

    fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        // A window reports its size again as it is shown and moved between
        // displays; reconfiguring the surface and rebuilding the render targets
        // costs about ten milliseconds, so only a size that differs is applied.
        if (self.surface_config.width, self.surface_config.height) == (width, height) {
            return;
        }
        self.surface_config.width = width;
        self.surface_config.height = height;
        self.surface.configure(device, &self.surface_config);
        self.scene_target = SceneTarget::new(device, &self.surface_config);
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

    /// The next swapchain image, or when this frame cannot be shown (the reason
    /// is logged) how long to hold the window back before trying again.
    fn acquire_frame(&self) -> Result<wgpu::SurfaceTexture, Option<std::time::Duration>> {
        // wgpu 30 replaced `Result<SurfaceTexture, SurfaceError>` with the
        // `CurrentSurfaceTexture` enum (see wgpui's
        // `platform/cross/renderer.rs` for the template this mirrors).
        // `Suboptimal` is treated the same as `Success` — matching wgpui's
        // choice — rather than forcing a reconfigure after every present;
        // an actually-outdated surface will surface as `Outdated` on the
        // next frame's acquire and get reconfigured then.
        match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => Ok(t),
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                // The surface no longer matches its last known configuration.
                // Reconfigure once and retry rather than dropping every later
                // frame after a resize or compositor transition.
                self.surface.configure(&self.device, &self.surface_config);
                self.acquire_after_surface_recovery().ok_or(None)
            }
            wgpu::CurrentSurfaceTexture::Timeout => {
                tracing::debug!(
                    window = self.handle.id(),
                    "Skipping frame after surface acquisition timeout"
                );
                Err(Some(BLOCKED_FRAME))
            }
            wgpu::CurrentSurfaceTexture::Occluded => {
                // Nothing can be shown: don't spin re-requesting frames (or
                // log every one) until the window is visible again.
                tracing::debug!(window = self.handle.id(), "Skipping frame: window occluded");
                Err(Some(BLOCKED_FRAME))
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                tracing::warn!(
                    window = self.handle.id(),
                    "Skipping frame after surface validation error"
                );
                Err(None)
            }
        }
    }

    /// Render one frame.
    ///
    /// The frame is recorded and submitted before the swapchain image is
    /// acquired. With a present queue of one the acquire is where a frame waits
    /// for the display (and for the GPU to finish the one before), so a frame
    /// that acquired first recorded nothing while that wait ran and the period
    /// was the CPU's time plus the GPU's; the scene goes to a texture of its
    /// own, the GPU starts on it at once, and only the copy into the image
    /// waits for the acquire. `sample` steps the scene and answers the camera
    /// as late as the frame allows (right before the uniform upload); `None`
    /// means the built-in freecam is used.
    fn render(
        &mut self,
        pacer: &crate::frame_pacer::FramePacer,
        sample: impl FnOnce(&mut SceneRenderer) -> Option<RenderCamera>,
    ) {
        // A paused runtime draws nothing; the frame's logs are its task's.
        let Some(_frame) = self.runtime.begin_frame() else {
            return;
        };
        let _span = self.runtime.task().span().entered();
        self.stats.begin();

        // The world as late as possible: right before the camera goes into the
        // frame's uniforms. The input chain's frame starts here: this is the
        // moment it reads what the tick wrote.
        let latency_frame = crate::latency::frame_started();
        pacer.sampling();
        let cam = {
            let ecs_camera = sample(&mut self.renderer);
            ecs_camera.unwrap_or_else(|| {
                // The freecam stands on the ground's local vertical and
                // flies faster the higher it is, as the editor camera does.
                let ground = self.renderer.camera_ground(self.freecam.position());
                self.freecam.follow_ground(ground);
                self.freecam.to_render_camera()
            })
        };
        // `Renderer::render` tracks its own frame delta internally (wind/TAA/
        // foliage clocks all advance from that), so there is nothing to push
        // in from here.
        crate::latency::frame_stage(latency_frame, crate::latency::FrameStage::Sampled);
        // The sample's stages (lock, step, camera) are recorded by the caller.
        self.stats.restart();
        let size = self.window.inner_size();
        let aspect = size.width as f32 / size.height.max(1) as f32;

        let frame = {
            profiling::profile_scope!("Game::prepare_frame");
            self.renderer.prepare_frame(
                &cam.frame_view(aspect),
                [self.surface_config.width, self.surface_config.height],
            )
        };
        crate::latency::frame_stage(latency_frame, crate::latency::FrameStage::Prepared);
        self.stats.mark(crate::frame_stats::PREPARE);
        {
            profiling::profile_scope!("Game::render");
            if let Err(e) = self.renderer.render(&frame.camera, &self.scene_target.view) {
                tracing::error!(window = self.handle.id(), "Render error: {:?}", e);
            }
        }
        crate::latency::frame_stage(latency_frame, crate::latency::FrameStage::Rendered);
        self.stats.mark(crate::frame_stats::RECORD_SUBMIT);

        // The display wait: the GPU is already working on the frame.
        let output = {
            let _acquire = profiling::ProfileScope::new_static("Game::acquire_surface");
            match self.acquire_frame() {
                Ok(output) => output,
                Err(hold) => {
                    if let Some(hold) = hold {
                        self.next_redraw = Instant::now() + hold;
                    }
                    return;
                }
            }
        };
        self.stats.mark(crate::frame_stats::ACQUIRE);
        {
            profiling::profile_scope!("Game::copy_to_surface");
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("Game present copy"),
                });
            // The image can be a frame behind a resize that has not been
            // applied yet: copy what both have.
            let extent = wgpu::Extent3d {
                width: self.scene_target.texture.width().min(output.texture.width()),
                height: self.scene_target.texture.height().min(output.texture.height()),
                depth_or_array_layers: 1,
            };
            encoder.copy_texture_to_texture(
                self.scene_target.texture.as_image_copy(),
                output.texture.as_image_copy(),
                extent,
            );
            self.queue.submit([encoder.finish()]);
        }

        {
            profiling::profile_scope!("Game::present");
            self.queue.present(output);
        }
        {
            profiling::profile_scope!("Game::latency_publish");
            crate::latency::frame_presented(latency_frame);
        }
        self.stats.mark(crate::frame_stats::COPY_PRESENT);
    }

    /// Close the frame's stats with the sample stages the caller timed.
    fn finish_stats(&mut self, sampled: [std::time::Duration; 3]) {
        if !self.stats.enabled() {
            return;
        }
        self.stats.record(crate::frame_stats::SCENE_LOCK, sampled[0]);
        self.stats.record(crate::frame_stats::STEP_SCENE, sampled[1]);
        self.stats.record(crate::frame_stats::CAMERA, sampled[2]);
        if self.stats.end(
            [self.surface_config.width, self.surface_config.height],
            self.renderer.renderer.timing_snapshot(),
        ) {
            crate::frame_stats::log_gpu_memory();
        }
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
    /// What scripts ask of this host (input mode, quit) and the level events.
    control: GameControl,
    /// The input mode the cursor was last set up for.
    input_mode: InputMode,
    /// The first window: its free camera takes each level's editor camera.
    primary_window: Option<WindowHandle>,
    /// The ONE shared, SceneDB-owned world: cloned out
    /// of the TickLoop, hydrated with each level, and read by every
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
    /// Mouse motion and key arrivals taken on the OS-side input thread; the
    /// tick drains the motion where it reads input ([`crate::pointer`]).
    pointer: Arc<crate::pointer::PointerSampler>,
    /// The frame turns the view by the mouse motion no tick has applied yet
    /// (project setting `simulation.late_look`, [`crate::late_look`]).
    late_look: bool,
    /// Told when each frame reads the world, so the tick can end just
    /// before the next one ([`crate::frame_pacer`]).
    pacer: Arc<crate::frame_pacer::FramePacer>,

    /// Which window currently owns the cursor (receives mouse-look).
    focused_window: Option<WindowHandle>,
    /// Whether the cursor is currently captured for mouse-look.
    cursor_captured: bool,
    /// Where the cursor last was, in window pixels, for motion deltas.
    last_cursor: Option<(f64, f64)>,
    /// Buttons whose press the screen claimed (an on-screen button's).
    claimed_buttons: engine_backend::scene::game_hooks::ClaimedButtons,
    /// Time of last `about_to_wait` — used to compute per-frame dt for freecam.
    last_frame: Instant,
    /// When the last frame was drawn, for the profiler's frame markers.
    last_redraw: Option<Instant>,
    /// Several copies of the game are running; unfocused windows draw slowly.
    throttle_background: bool,
}

impl PulsarApp {
    pub fn new(
        bridge: Arc<WindowBridge>,
        mut tick_loop: crate::tick::TickLoop,
        initial_windows: Vec<(WindowHandle, WindowDescriptor)>,
        content: pulsar_content::ContentRoot,
        display: winit::event_loop::OwnedDisplayHandle,
    ) -> Self {
        // The tick loop's store handle is cloned BEFORE the loop moves into
        // `self.tick_loop` -- renderer and gameplay then share one world.
        let scene_store = tick_loop.scene_store.clone();
        let events = tick_loop.events.clone();
        let possession = tick_loop.possession_gate();
        let view_targets = tick_loop.view_targets();
        let control = tick_loop.control();
        let primary_window = initial_windows.first().map(|(handle, _)| *handle);
        let pacer = crate::frame_pacer::FramePacer::new();
        tick_loop.set_frame_pacer(pacer.clone());
        let pointer = crate::pointer::PointerSampler::new();
        Self {
            bridge,
            gpu: GpuContext::new(display),
            tick_loop: Some(tick_loop),
            ecs_thread: None,
            initial_windows,
            winit_to_handle: HashMap::new(),
            windows: HashMap::new(),
            content,
            control,
            input_mode: InputMode::Game,
            primary_window,
            scene_store,
            events,
            possession,
            view_targets,
            pointer,
            late_look: pulsar_content::SimulationSettings::default().late_look,
            pacer,
            focused_window: None,
            cursor_captured: false,
            last_cursor: None,
            claimed_buttons: Default::default(),
            last_frame: Instant::now(),
            last_redraw: None,
            throttle_background: running_beside_copies(),
        }
    }

    // ── What the game asks of the window ──────────────────────────────────────

    /// Set the cursor up for the input mode a script set, follow level
    /// changes (the render pipeline, the free camera's start) and quit when
    /// asked.
    fn follow_game_control(&mut self, event_loop: &ActiveEventLoop) {
        let mode = self.control.input_mode();
        if mode != self.input_mode {
            self.input_mode = mode;
            match mode {
                InputMode::Ui => self.release_cursor(),
                InputMode::Game => {
                    let focused = self
                        .focused_window
                        .or(self.primary_window)
                        .filter(|handle| self.windows.get(handle).is_some_and(|w| w.focused));
                    if let Some(handle) = focused {
                        self.capture_cursor(handle);
                    }
                }
            }
        }
        for event in self.control.take_level_events() {
            match event {
                LevelEvent::Begun { .. } | LevelEvent::Failed { .. } => {
                    for window in self.windows.values_mut() {
                        window.renderer.follow_level_pipeline();
                    }
                }
                LevelEvent::Loaded { editor_camera, .. } => {
                    let primary = self
                        .primary_window
                        .and_then(|handle| self.windows.get_mut(&handle));
                    // The editor camera saved in the level file, so the first
                    // frame without a game camera matches the editor view.
                    if let (Some(cam), Some(window)) = (editor_camera, primary) {
                        window.freecam = FreeCam::default().place(
                            glam::DVec3::from_array(cam.position),
                            cam.yaw,
                            cam.pitch,
                        );
                    }
                }
            }
        }
        if self.control.quit_requested() {
            self.release_cursor();
            tracing::info!("The game quit: closing");
            event_loop.exit();
        }
    }

    // ── Cursor capture ────────────────────────────────────────────────────────

    fn capture_cursor(&mut self, handle: WindowHandle) {
        profiling::profile_scope!("Game::capture_cursor");
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
            self.pointer.set_captured(true);
        }
    }

    /// Mouse motion for the focused window's free camera.
    fn drive_freecam(&mut self, dx: f64, dy: f64) {
        if let Some(handle) = self.focused_window {
            if let Some(gw) = self.windows.get_mut(&handle) {
                gw.freecam.on_mouse_delta(dx, dy);
            }
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
        self.pointer.set_captured(false);
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

    /// Open a window. Its renderer takes the pipeline selected for the level
    /// loading or loaded (or the fallback while there is none).
    fn open_window(
        &mut self,
        event_loop: &ActiveEventLoop,
        handle: WindowHandle,
        desc: WindowDescriptor,
    ) {
        profiling::profile_scope!("Game::open_window");
        let mut attrs = Window::default_attributes()
            .with_title(&desc.title)
            .with_inner_size(winit::dpi::LogicalSize::new(desc.width, desc.height));
        if desc.fullscreen {
            attrs = attrs.with_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
        }

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
        self.windows.insert(handle, game_window);
        tracing::info!(id = handle.id(), title = %desc.title, "Window opened");
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
        for (handle, desc) in std::mem::take(&mut self.initial_windows) {
            self.open_window(event_loop, handle, desc);
        }

        // The simulation ticks as often as the display refreshes (unless the
        // project says otherwise): an input waits for the next tick, which at
        // 60 Hz on a faster display is the largest part of the input delay.
        let rate = tick_rate_hz(event_loop);
        if let Some(tick_loop) = self.tick_loop.as_mut() {
            tick_loop.set_tick_rate_hz(rate);
        }
        tracing::info!(tick_rate_hz = rate, "Simulation tick rate");
        self.late_look = self.content.settings().simulation.late_look;
        tracing::info!(late_look = self.late_look, "Late look");

        // Mouse motion is taken where the OS delivers it and drained by the tick
        // where it reads input; without the OS thread the event loop feeds the
        // same accumulator.
        let raw = self.pointer.start_raw_input();
        if let Some(tick_loop) = self.tick_loop.as_mut() {
            tick_loop.set_pointer_sampler(self.pointer.clone());
        }
        tracing::info!(raw_input_thread = raw, "Pointer sampling");

        // The tick thread loads the levels (`tick::levels`): nothing of
        // gameplay runs while one is partly in the world.
        self.spawn_ecs_thread();
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
                // With the OS-side thread the raw motion reaches the sampler
                // (and, through it, the free camera in `about_to_wait`) and the
                // process's raw mouse registration is that thread's: winit sends
                // no motion at all. Without it this is the earliest the game
                // sees the motion.
                if !self.pointer.has_raw_feed() {
                    self.pointer
                        .add_motion(dx, dy, crate::latency::stamp());
                    self.drive_freecam(dx, dy);
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
        // Which kind of event it was, so a slow one is named in a recording.
        let _kind_scope = window_event_kind(&event).map(profiling::ProfileScope::new_static);

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
                self.pointer.set_focused(focused);
                if let Some(gw) = self.windows.get_mut(&handle) {
                    gw.focused = focused;
                    gw.next_redraw = Instant::now();
                }
                if focused {
                    self.focused_window = Some(handle);
                    // Re-capture when regaining focus (if we were captured before).
                    if self.cursor_captured && self.control.input_mode() == InputMode::Game {
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
                    if self.pointer.has_raw_feed() {
                        self.pointer.handled_key();
                    } else {
                        crate::latency::note_input();
                    }
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
                self.pointer.set_cursor(x, y);
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
                // With a free cursor, something on screen may take the press
                // (an on-screen button): gameplay does not see it then, and
                // the cursor stays free.
                let (x, y) = self.last_cursor.unwrap_or_default();
                if state == ElementState::Pressed {
                    let claimed = !self.cursor_captured
                        && self.last_cursor.is_some()
                        && engine_backend::scene::game_hooks::pointer_claimed(
                            &self.scene_store.read().world,
                            x,
                            y,
                        );
                    self.claimed_buttons.pressed(index, claimed);
                    if claimed {
                        self.events.publish(
                            channel,
                            pulsar_events::builtin::PointerClaimedDown {
                                button: index,
                                x,
                                y,
                            },
                        );
                    } else {
                        self.events.publish(
                            channel,
                            pulsar_events::builtin::MouseButtonDown { button: index },
                        );
                        // A menu (`InputMode::Ui`) keeps the cursor free.
                        if button == winit::event::MouseButton::Left
                            && !self.cursor_captured
                            && self.control.input_mode() == InputMode::Game
                        {
                            self.capture_cursor(handle);
                        }
                    }
                } else if self.claimed_buttons.released(index) {
                    self.events.publish(
                        channel,
                        pulsar_events::builtin::PointerClaimedUp {
                            button: index,
                            x,
                            y,
                        },
                    );
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
                // The frame acquires its surface first (the wait for the
                // display), then `sample` runs: it advances the shared world's
                // SceneDB state and flushes its GPU mirror (Helio's scene join
                // reads the mirrored rows directly; same seam the editor
                // viewport renderer uses, binding the meshes that finished
                // loading so they upload with this frame, SceneDB#84), and
                // answers the camera. Camera precedence: a gameplay-pushed
                // bridge camera wins, then the SHARED world's (the view
                // target's camera, the best CameraComponent, a Camera-typed
                // entity), then the freecam. See `camera_selection`.
                let background = self.throttle_background;
                let scene_store = &self.scene_store;
                let bridge = &self.bridge;
                let view_targets = &self.view_targets;
                let pacer = &self.pacer;
                let pointer = &self.pointer;
                // A menu turns no view.
                let late_look = self.late_look && self.control.input_mode() == InputMode::Game;
                if let Some(gw) = self.windows.get_mut(&handle) {
                    // Lock wait, scene step and camera, for `frame_stats`.
                    let mut sampled = [std::time::Duration::ZERO; 3];
                    gw.render(pacer, |renderer| {
                        let started = Instant::now();
                        {
                            profiling::profile_scope!("Game::step_scene");
                            let mut store = scene_store.write();
                            sampled[0] = started.elapsed();
                            renderer.step_scene(&mut store);
                        }
                        let stepped = Instant::now();
                        sampled[1] = stepped - started - sampled[0];
                        // The view as the last tick left it, turned by the mouse
                        // motion no tick has consumed yet (`late_look`).
                        profiling::profile_scope!("Game::camera_for_frame");
                        let (camera, late) =
                            crate::late_look::read_with_pending_motion(pointer, |motion| {
                                let store = scene_store.read();
                                let turn = match (motion.filter(|_| late_look), view_targets.local()) {
                                    (Some(motion), Some(pawn)) if motion != [0.0; 2] => {
                                        engine_backend::scene::game_hooks::pending_view_turn(
                                            &store.world,
                                            pawn,
                                            motion,
                                        )
                                    }
                                    _ => None,
                                };
                                let camera = camera_for_frame_turned(
                                    bridge.camera(handle),
                                    &store,
                                    view_targets.local(),
                                    turn.as_ref(),
                                );
                                (camera, turn.is_some())
                            });
                        if late {
                            crate::latency::look_shown(pointer.take_unshown_motion());
                        }
                        sampled[2] = stepped.elapsed();
                        camera
                    });
                    gw.finish_stats(sampled);
                    if background && !gw.focused {
                        gw.next_redraw = gw.next_redraw.max(Instant::now() + BACKGROUND_FRAME);
                    }
                }
            }

            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // The event loop has pumped its queue: how long raw motion waited for
        // it, and the free camera takes it.
        if self.pointer.has_raw_feed() {
            self.pointer.handled_motion();
            if let Some((dx, dy)) = self.pointer.take_view_motion() {
                self.drive_freecam(dx, dy);
            }
        }
        if self
            .gpu
            .device_loss
            .as_ref()
            .is_some_and(engine_backend::services::device_loss::DeviceLoss::is_lost)
        {
            self.recover_lost_device();
        }
        self.follow_game_control(event_loop);
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;

        // Advance every window's freecam and request a redraw for the ones
        // that are due; sleep until the earliest of the rest instead of
        // spinning.
        let mut wake: Option<Instant> = None;
        // The free camera stands down only while a possessed pawn's camera is
        // the view: a pawn with no camera leaves the view to the free camera.
        // A menu suspends it too.
        let suspended = self.control.input_mode() == InputMode::Ui
            || (self.possession.is_possessed()
                && view_target_drives_camera(&self.scene_store.read(), self.view_targets.local()));
        for gw in self.windows.values_mut() {
            gw.freecam.set_suspended(suspended);
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
