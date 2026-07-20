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
    event_loop::ActiveEventLoop,
    window::{CursorGrabMode, Window, WindowId},
};

use helio::{required_wgpu_features, required_wgpu_limits, Camera, Renderer, RendererConfig};

use crate::freecam::FreeCam;
use crate::window::{RenderCamera, WindowBridge, WindowCommand, WindowDescriptor, WindowHandle};

// ── Per-window GPU state ──────────────────────────────────────────────────────

struct GameWindow {
    handle: WindowHandle,
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    device: Arc<wgpu::Device>,
    renderer: Renderer,
    /// Built-in free-look camera — active when no ECS camera has been set.
    freecam: FreeCam,
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
        desc: &WindowDescriptor,
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
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &surface_config);

        let mut renderer = Renderer::new(
            device.clone(),
            queue.clone(),
            RendererConfig::new(surface_config.width, surface_config.height, surface_format),
        );
        // Kill the default helio ambient ([0.05, 0.05, 0.08] @ 1.0).
        // All illumination comes from lights in the scene file — same as editor.
        renderer.set_ambient([0.0, 0.0, 0.0], 0.0);
        renderer.set_editor_mode(desc.editor_mode);

        Self {
            handle,
            window,
            surface,
            surface_config,
            device,
            renderer,
            freecam: FreeCam::default(),
        }
    }

    fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.surface_config.width = width;
        self.surface_config.height = height;
        self.surface.configure(device, &self.surface_config);
        self.renderer.set_render_size(width, height);
    }

    fn acquire_after_surface_recovery(&self) -> Option<wgpu::SurfaceTexture> {
        match self.surface.get_current_texture() {
            Ok(texture) => Some(texture),
            Err(error) => {
                tracing::warn!(
                    window = self.handle.id(),
                    ?error,
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
        let cam = ecs_camera.unwrap_or_else(|| self.freecam.to_render_camera());

        let size = self.window.inner_size();
        let aspect = size.width as f32 / size.height.max(1) as f32;

        let helio_cam = Camera::perspective_look_at(
            glam::Vec3::from_array(cam.position),
            glam::Vec3::from_array(cam.target),
            glam::Vec3::from_array(cam.up),
            cam.fov_y,
            aspect,
            cam.near,
            cam.far,
        );

        let output = match self.surface.get_current_texture() {
            Ok(texture) => texture,
            Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
                // The surface no longer matches its last known configuration.
                // Reconfigure once and retry rather than dropping every later
                // frame after a resize or compositor transition.
                self.surface.configure(&self.device, &self.surface_config);
                let Some(texture) = self.acquire_after_surface_recovery() else {
                    return;
                };
                texture
            }
            Err(wgpu::SurfaceError::Timeout) => {
                tracing::warn!(
                    window = self.handle.id(),
                    "Skipping frame after surface acquisition timeout"
                );
                return;
            }
            Err(wgpu::SurfaceError::OutOfMemory) => {
                tracing::error!(
                    window = self.handle.id(),
                    "Surface acquisition ran out of memory"
                );
                return;
            }
            Err(wgpu::SurfaceError::Other) => {
                tracing::warn!(
                    window = self.handle.id(),
                    "Skipping frame after surface error"
                );
                return;
            }
        };
        let reconfigure_after_present = output.suboptimal;
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        if let Err(e) = self.renderer.render(&helio_cam, &view) {
            tracing::error!(window = self.handle.id(), "Render error: {:?}", e);
        }

        output.present();

        if reconfigure_after_present {
            self.surface.configure(&self.device, &self.surface_config);
        }
    }
}

// ── Shared GPU context ────────────────────────────────────────────────────────

struct GpuContext {
    instance: wgpu::Instance,
    adapter: Option<wgpu::Adapter>,
    device: Option<Arc<wgpu::Device>>,
    queue: Option<Arc<wgpu::Queue>>,
}

impl GpuContext {
    fn new(display: winit::event_loop::OwnedDisplayHandle) -> Self {
        Self {
            instance: wgpu::Instance::new(
                wgpu::InstanceDescriptor {
                    backends: wgpu::Backends::all(),
                    flags: wgpu::InstanceFlags::empty(),
                    ..Default::default()
                }
                .with_display_handle(Box::new(display)),
            ),
            adapter: None,
            device: None,
            queue: None,
        }
    }

    fn ensure_device(&mut self, surface: &wgpu::Surface<'_>) {
        if self.device.is_some() {
            return;
        }

        let adapter =
            pollster::block_on(self.instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(surface),
                force_fallback_adapter: false,
            }))
            .expect("No suitable GPU adapter found");

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("Pulsar GPU Device"),
            required_features: required_wgpu_features(adapter.features()),
            required_limits: required_wgpu_limits(adapter.limits()),
            ..Default::default()
        }))
        .expect("Failed to create GPU device");

        let info = adapter.get_info();
        tracing::info!(
            backend = ?info.backend,
            device = %info.name,
            driver = %info.driver,
            "GPU initialised"
        );

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
    initial_windows: Vec<(WindowHandle, WindowDescriptor)>,
    winit_to_handle: HashMap<WindowId, WindowHandle>,
    windows: HashMap<WindowHandle, GameWindow>,
    project_root: PathBuf,
    default_scene: Option<PathBuf>,

    /// Which window currently owns the cursor (receives mouse-look).
    focused_window: Option<WindowHandle>,
    /// Whether the cursor is currently captured for mouse-look.
    cursor_captured: bool,
    /// Time of last `about_to_wait` — used to compute per-frame dt for freecam.
    last_frame: Instant,
}

impl PulsarApp {
    pub fn new(
        bridge: Arc<WindowBridge>,
        tick_loop: crate::tick::TickLoop,
        initial_windows: Vec<(WindowHandle, WindowDescriptor)>,
        project_root: PathBuf,
        default_scene: Option<PathBuf>,
        display: winit::event_loop::OwnedDisplayHandle,
    ) -> Self {
        Self {
            bridge,
            gpu: GpuContext::new(display),
            tick_loop: Some(tick_loop),
            initial_windows,
            winit_to_handle: HashMap::new(),
            windows: HashMap::new(),
            project_root,
            default_scene,
            focused_window: None,
            cursor_captured: false,
            last_frame: Instant::now(),
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
            let temp_surface = self
                .gpu
                .instance
                .create_surface(window.clone())
                .expect("Failed to create surface for GPU init");
            self.gpu.ensure_device(&temp_surface);
        }

        let winit_id = window.id();
        let mut game_window = GameWindow::new(
            handle,
            window,
            &self.gpu.instance,
            self.gpu.adapter(),
            self.gpu.device(),
            self.gpu.queue(),
            &desc,
        );

        self.winit_to_handle.insert(winit_id, handle);

        // Load the scene into this window's renderer if one was requested.
        if let Some(ref path) = scene_path {
            tracing::info!(scene = %path.display(), window = handle.id(), "Loading scene into window");
            match pulsar_scene::SceneLoader::load_file(
                path,
                &self.project_root,
                &mut game_window.renderer,
            ) {
                Ok(()) => {
                    tracing::info!(
                        window = handle.id(),
                        scene = %path.display(),
                        "Scene loaded into window"
                    );

                    // Seed the freecam from the editor camera stored in the
                    // scene file, if present, so the first frame matches the
                    // editor view.
                    if let Ok(cam) = editor_camera_from_file(path) {
                        game_window.freecam = cam;
                        tracing::info!(
                            window = handle.id(),
                            "FreeCam seeded from scene editor camera"
                        );
                    }
                }
                Err(e) => tracing::warn!(
                    window = handle.id(),
                    scene = %path.display(),
                    "Failed to load scene: {e}"
                ),
            }
        }

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
            std::thread::Builder::new()
                .name("pulsar-ecs".into())
                .spawn(move || tick_loop.run_blocking())
                .expect("Failed to spawn ECS tick thread");
            tracing::info!("ECS tick thread started");
        }
    }
}

impl ApplicationHandler<WindowCommand> for PulsarApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let initial = std::mem::take(&mut self.initial_windows);
        let default_scene = self.default_scene.take();

        for (i, (handle, desc)) in initial.into_iter().enumerate() {
            let scene = if i == 0 { default_scene.clone() } else { None };
            self.open_window_with_scene(event_loop, handle, desc, scene);
        }

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
                use winit::keyboard::Key;

                let pressed = key_event.state == ElementState::Pressed;

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

            // ── Mouse click — capture cursor ──────────────────────────────────
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: winit::event::MouseButton::Left,
                ..
            } => {
                if !self.cursor_captured {
                    self.capture_cursor(handle);
                }
            }

            // ── Render ────────────────────────────────────────────────────────
            WindowEvent::RedrawRequested => {
                // ECS camera takes priority; freecam is the fallback.
                let ecs_camera = self.bridge.camera(handle);
                if let Some(gw) = self.windows.get_mut(&handle) {
                    gw.render(ecs_camera);
                }
            }

            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;

        // Advance every window's freecam and request a redraw.
        for gw in self.windows.values_mut() {
            gw.freecam.update(dt);
            gw.window.request_redraw();
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Try to extract the editor camera from a scene file and return a [`FreeCam`]
/// seeded with that position + orientation.
fn editor_camera_from_file(path: &std::path::Path) -> Result<FreeCam, ()> {
    let text = std::fs::read_to_string(path).map_err(|_| ())?;
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|_| ())?;

    let cam = v.get("editor").and_then(|e| e.get("camera")).ok_or(())?;

    let pos = cam
        .get("position")
        .and_then(|p| p.as_array())
        .and_then(|a| {
            if a.len() >= 3 {
                Some(glam::Vec3::new(
                    a[0].as_f64()? as f32,
                    a[1].as_f64()? as f32,
                    a[2].as_f64()? as f32,
                ))
            } else {
                None
            }
        })
        .ok_or(())?;

    let yaw = cam
        .get("yaw")
        .and_then(|v| v.as_f64())
        .map(|v| v as f32)
        .unwrap_or(0.0);
    let pitch = cam
        .get("pitch")
        .and_then(|v| v.as_f64())
        .map(|v| v as f32)
        .unwrap_or(0.0);

    Ok(FreeCam::default().place(pos, yaw, pitch))
}
