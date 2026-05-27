//! Main-thread winit `ApplicationHandler` that owns all game windows and their
//! GPU + Helio render state.
//!
//! You never construct this directly.  Call
//! [`TickLoop::run_with_windows`][crate::TickLoop::run_with_windows] — it
//! creates the `PulsarApp`, spawns the ECS tick thread, and hands the main
//! thread to the winit event loop.

use std::collections::HashMap;
use std::sync::Arc;

use winit::{
    application::ApplicationHandler,
    event::{WindowEvent},
    event_loop::ActiveEventLoop,
    window::{Window, WindowId},
};

use helio::{Camera, Renderer, RendererConfig, required_wgpu_features, required_wgpu_limits};

use crate::window::{RenderCamera, WindowBridge, WindowCommand, WindowDescriptor, WindowHandle};

// ── Per-window GPU state ──────────────────────────────────────────────────────

struct GameWindow {
    handle: WindowHandle,
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    surface_format: wgpu::TextureFormat,
    renderer: Renderer,
}

impl GameWindow {
    /// Initialise a new window's GPU surface and Helio renderer.
    ///
    /// Requires that `device` and `queue` are already initialised (the first
    /// window creates them; subsequent windows reuse them).
    fn new(
        handle: WindowHandle,
        window: Arc<Window>,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        desc: &WindowDescriptor,
    ) -> Self {
        // Safety: the window is `Arc`-owned and outlives the surface.
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
        renderer.set_editor_mode(desc.editor_mode);

        Self {
            handle,
            window,
            surface,
            surface_config,
            surface_format,
            renderer,
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

    fn render(&mut self, camera: &RenderCamera) {
        let size = self.window.inner_size();
        let aspect = size.width as f32 / size.height.max(1) as f32;

        let helio_cam = Camera::perspective_look_at(
            glam::Vec3::from_array(camera.position),
            glam::Vec3::from_array(camera.target),
            glam::Vec3::from_array(camera.up),
            camera.fov_y,
            aspect,
            camera.near,
            camera.far,
        );

        let output = match self.surface.get_current_texture() {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(window = self.handle.id(), "Surface error: {:?}", e);
                return;
            }
        };
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        if let Err(e) = self.renderer.render(&helio_cam, &view) {
            tracing::error!(window = self.handle.id(), "Render error: {:?}", e);
        }

        output.present();
    }
}

// ── Shared GPU context ────────────────────────────────────────────────────────

/// Lazily-initialised wgpu adapter + device + queue shared across all windows.
struct GpuContext {
    instance: wgpu::Instance,
    adapter: Option<wgpu::Adapter>,
    device: Option<Arc<wgpu::Device>>,
    queue: Option<Arc<wgpu::Queue>>,
}

impl GpuContext {
    fn new() -> Self {
        Self {
            instance: wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::all(),
                flags: wgpu::InstanceFlags::empty(),
                ..Default::default()
            }),
            adapter: None,
            device: None,
            queue: None,
        }
    }

    /// Initialise the adapter + device from the first compatible surface.
    ///
    /// No-op if already initialised.
    fn ensure_device(&mut self, surface: &wgpu::Surface<'_>) {
        if self.device.is_some() {
            return;
        }

        let adapter = pollster::block_on(self.instance.request_adapter(
            &wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(surface),
                force_fallback_adapter: false,
            },
        ))
        .expect("No suitable GPU adapter found");

        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("Pulsar GPU Device"),
                required_features: required_wgpu_features(adapter.features()),
                required_limits: required_wgpu_limits(adapter.limits()),
                ..Default::default()
            },
        ))
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

/// The main-thread winit application.  Owns all [`GameWindow`]s and the
/// shared GPU context.
pub struct PulsarApp {
    bridge: Arc<WindowBridge>,

    gpu: GpuContext,

    /// Pending open requests that arrived before the event loop was ready
    /// (i.e. before `resumed` fires).
    pending: Vec<(WindowHandle, WindowDescriptor)>,

    /// Map from winit's `WindowId` to our stable `WindowHandle`.
    winit_to_handle: HashMap<WindowId, WindowHandle>,
    /// The live game windows, keyed by our stable handle.
    windows: HashMap<WindowHandle, GameWindow>,
}

impl PulsarApp {
    pub fn new(bridge: Arc<WindowBridge>) -> Self {
        Self {
            bridge,
            gpu: GpuContext::new(),
            pending: Vec::new(),
            winit_to_handle: HashMap::new(),
            windows: HashMap::new(),
        }
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    fn open_window(
        &mut self,
        event_loop: &ActiveEventLoop,
        handle: WindowHandle,
        desc: WindowDescriptor,
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

        // Temporarily create a surface just to prime the GPU on the first window.
        // We'll recreate it properly inside GameWindow::new.
        if self.gpu.device.is_none() {
            let temp_surface = self.gpu.instance.create_surface(window.clone())
                .expect("Failed to create temp surface for GPU init");
            self.gpu.ensure_device(&temp_surface);
            // temp_surface is dropped here; GameWindow::new creates its own.
        }

        let winit_id = window.id();
        let game_window = GameWindow::new(
            handle,
            window,
            &self.gpu.instance,
            self.gpu.adapter(),
            self.gpu.device(),
            self.gpu.queue(),
            &desc,
        );

        self.winit_to_handle.insert(winit_id, handle);
        self.windows.insert(handle, game_window);

        tracing::info!(
            id = handle.id(),
            title = %desc.title,
            "Window opened"
        );
    }

    fn close_window(&mut self, handle: WindowHandle) {
        if let Some(gw) = self.windows.remove(&handle) {
            self.winit_to_handle.remove(&gw.window.id());
            self.bridge.remove_camera(handle);
            tracing::info!(id = handle.id(), "Window closed");
        }
    }
}

impl ApplicationHandler<WindowCommand> for PulsarApp {
    // Called when the event loop is ready to accept window creation.
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        for (handle, desc) in std::mem::take(&mut self.pending) {
            self.open_window(event_loop, handle, desc);
        }
    }

    // Window-creation / close commands from the ECS thread.
    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: WindowCommand) {
        match event {
            WindowCommand::Open { handle, desc } => {
                self.open_window(event_loop, handle, desc);
            }
            WindowCommand::Close { handle } => {
                self.close_window(handle);
                // If all windows are closed, exit the application.
                if self.windows.is_empty() {
                    tracing::info!("All windows closed — exiting");
                    event_loop.exit();
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
            WindowEvent::CloseRequested => {
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

            WindowEvent::RedrawRequested => {
                if let Some(gw) = self.windows.get_mut(&handle) {
                    let camera = self.bridge.camera(handle).unwrap_or_default();
                    gw.render(&camera);
                }
            }

            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        // Drive continuous rendering: request a redraw for every live window.
        for gw in self.windows.values() {
            gw.window.request_redraw();
        }
    }
}
