//! Application Handler Module
//!
//! This module contains the main Winit application handler (`WinitGpuiApp`) that manages
//! multiple windows and coordinates between Winit (windowing), GPUI (UI), and D3D11 (rendering).
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────┐
//! │          WinitGpuiApp                       │
//! │   (ApplicationHandler for Winit)            │
//! ├─────────────────────────────────────────────┤
//! │ windows: HashMap<WindowId, WindowState>     │
//! │ engine_state: EngineState                   │
//! │ window_request_rx: Receiver<WindowRequest>  │
//! └─────────────────────────────────────────────┘
//!          │
//!          ├─── window_event() → Process all window events
//!          ├─── resumed() → Create initial window
//!          └─── about_to_wait() → Initialize GPUI & render
//! ```
//!
//! ## Responsibilities
//!
//! - **Window Management**: Create, track, and destroy multiple independent windows
//! - **Event Routing**: Route Winit events to appropriate GPUI handlers
//! - **D3D11 Integration**: Initialize and manage D3D11 rendering pipeline (Windows)
//! - **GPUI Initialization**: Set up GPUI application and windows with proper content
//! - **Lifecycle Management**: Handle window creation requests and cleanup
//!
//! ## Usage
//!
//! ```rust,ignore
//! let event_loop = EventLoop::new()?;
//! let mut app = WinitGpuiApp::new(engine_state, window_rx);
//! event_loop.run_app(&mut app)?;
//! ```

use crate::assets::Assets;
use crate::OpenSettings;  // Import the OpenSettings action from main/root
use ui_core::{PulsarApp, PulsarRoot, ToggleCommandPalette};
use ui_entry::{EntryScreen, ProjectSelected, create_entry_component};
use ui_settings::{SettingsWindow, create_settings_component};
use ui_loading_screen::create_loading_component;
use ui_about::create_about_window;
use ui_documentation::create_documentation_window;
use ui_common::menu::{AboutApp, ShowDocumentation};
use crate::window::{convert_modifiers, convert_mouse_button, WindowState};
use engine_state::{EngineState, WindowRequest};
use gpui::*;
use ui::Root;
use std::collections::HashMap;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton as WinitMouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window as WinitWindow, WindowId};

#[cfg(target_os = "windows")]
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

#[cfg(target_os = "windows")]
use windows::{
    core::*,
    Win32::{
        Foundation::*,
        Graphics::{
            Direct3D::*,
            Direct3D11::*,
            Direct3D::Fxc::*,
            Dxgi::{Common::*, *},
        },
    },
};

/// Main application handler managing multiple windows
///
/// This struct implements the Winit `ApplicationHandler` trait and manages
/// all windows in the application. Each window has independent state including
/// its own GPUI application instance and optional D3D11 rendering pipeline.
///
/// ## Fields
///
/// - `windows` - Map of WindowId to WindowState for all active windows
/// - `engine_state` - Shared engine state for cross-window communication
/// - `window_request_rx` - Channel for receiving window creation requests
/// - `pending_window_requests` - Queue of requests to process on next frame
pub struct WinitGpuiApp {
    windows: HashMap<WindowId, WindowState>,
    engine_state: EngineState,
    window_request_rx: Receiver<WindowRequest>,
    pending_window_requests: Vec<WindowRequest>,
}

impl WinitGpuiApp {
    /// Create a new application handler
    ///
    /// # Arguments
    /// * `engine_state` - Shared engine state
    /// * `window_request_rx` - Channel for receiving window creation requests
    ///
    /// # Returns
    /// New WinitGpuiApp ready to be run
    pub fn new(engine_state: EngineState, window_request_rx: Receiver<WindowRequest>) -> Self {
        Self {
            windows: HashMap::new(),
            engine_state,
            window_request_rx,
            pending_window_requests: Vec::new(),
        }
    }

    // TODO: Refactor window creation into a trait based system for modular window types
    /// Create a new window based on a request
    ///
    /// # Arguments
    /// * `event_loop` - Active event loop for window creation
    /// * `request` - Type of window to create
    fn create_window(&mut self, event_loop: &ActiveEventLoop, request: WindowRequest) {
        let (title, size) = match &request {
            WindowRequest::Entry => ("Pulsar Engine", (1280.0, 720.0)),
            WindowRequest::Settings => ("Settings", (800.0, 600.0)),
            WindowRequest::About => ("About Pulsar Engine", (600.0, 500.0)),
            WindowRequest::Documentation => ("Documentation", (1400.0, 900.0)),
            WindowRequest::ProjectEditor { .. } => ("Pulsar Engine - Project Editor", (1280.0, 800.0)),
            WindowRequest::ProjectSplash { .. } => ("Loading Project...", (960.0, 540.0)),
            WindowRequest::CloseWindow { .. } => return, // Handled elsewhere
        };

        println!("≡ƒ¬ƒ [CREATE-WINDOW] Creating new window: {} (type: {:?})", title, request);

        let mut window_attributes = WinitWindow::default_attributes()
            .with_title(title)
            .with_inner_size(winit::dpi::LogicalSize::new(size.0, size.1))
            .with_transparent(false)
            .with_decorations(true); // Use OS window decorations for proper controls

        // Splash window positioning (centered by default)
        // Position::Automatic doesn't exist in winit, windows are centered by default

        let winit_window = Arc::new(
            event_loop
                .create_window(window_attributes)
                .expect("Failed to create window"),
        );

        let window_id = winit_window.id();
        let mut window_state = WindowState::new(winit_window);
        window_state.window_type = Some(request);

        self.windows.insert(window_id, window_state);
        self.engine_state.increment_window_count();

        println!("Γ£à Window created: {} (total windows: {})", title, self.engine_state.window_count());
    }
}

impl ApplicationHandler for WinitGpuiApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // Only create main window if no windows exist
        if !self.windows.is_empty() {
            return;
        }

        println!("Γ£à Creating main entry window...");
        
        // Create the main entry window using the modular system
        self.create_window(event_loop, WindowRequest::Entry);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                println!("\n≡ƒæï Closing window...");
                // Clean up window-specific GPU renderer
                let window_id_u64 = unsafe { std::mem::transmute::<_, u64>(window_id) };
                self.engine_state.remove_window_gpu_renderer(window_id_u64);

                self.windows.remove(&window_id);
                self.engine_state.decrement_window_count();

                // Exit application if no windows remain
                if self.windows.is_empty() {
                    println!("≡ƒæï No windows remain, exiting application...");
                    event_loop.exit();
                }
            }
            _ => {
                // For all other events, we need the window state
                let Some(window_state) = self.windows.get_mut(&window_id) else {
                    return;
                };

                // Extract mutable references to avoid borrow checker issues
                let WindowState {
                    ref winit_window,
                    ref mut gpui_app,
                    ref mut gpui_window,
                    ref mut gpui_window_initialized,
                    ref mut needs_render,
                    window_type: _,
                    ref mut last_cursor_position,
                    ref mut motion_smoother,
                    ref mut current_modifiers,
                    ref mut pressed_mouse_buttons,
                    ref mut click_state,
                    #[cfg(target_os = "windows")]
                    ref mut d3d_device,
                    #[cfg(target_os = "windows")]
                    ref mut d3d_context,
                    #[cfg(target_os = "windows")]
                    ref mut shared_texture,
                    #[cfg(target_os = "windows")]
                    ref mut shared_texture_initialized,
                    #[cfg(target_os = "windows")]
                    ref mut swap_chain,
                    #[cfg(target_os = "windows")]
                    ref mut blend_state,
                    #[cfg(target_os = "windows")]
                    ref mut render_target_view,
                    #[cfg(target_os = "windows")]
                    ref mut vertex_shader,
                    #[cfg(target_os = "windows")]
                    ref mut pixel_shader,
                    #[cfg(target_os = "windows")]
                    ref mut vertex_buffer,
                    #[cfg(target_os = "windows")]
                    ref mut input_layout,
                    #[cfg(target_os = "windows")]
                    ref mut sampler_state,
                    #[cfg(target_os = "windows")]
                    ref mut persistent_gpui_texture,
                    #[cfg(target_os = "windows")]
                    ref mut persistent_gpui_srv,
                    #[cfg(target_os = "windows")]
                    ref mut bevy_texture,
                    #[cfg(target_os = "windows")]
                    ref mut bevy_srv,
                    ref mut bevy_renderer,
                } = window_state;

                // Fetch the GPU renderer for this window from EngineState if not already set
                // If there's a pending renderer (marked with window_id 0), claim it for this window
                if bevy_renderer.is_none() {
                    let window_id_u64 = unsafe { std::mem::transmute::<_, u64>(window_id) };

                    static mut CLAIM_CHECK_COUNT: u32 = 0;
                    unsafe {
                        CLAIM_CHECK_COUNT += 1;
                        if CLAIM_CHECK_COUNT % 60 == 0 {
                            println!("[RENDERER] ≡ƒöì Checking for renderer for window {} (attempt {})...", window_id_u64, CLAIM_CHECK_COUNT);
                        }
                    }

                    // First check if this window already has a renderer
                    if let Some(renderer_handle) = self.engine_state.get_window_gpu_renderer(window_id_u64) {
                        // Try to downcast from Any to the concrete type
                        if let Ok(gpu_renderer) = renderer_handle.clone().downcast::<std::sync::Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>() {
                            *bevy_renderer = Some(gpu_renderer);
                            println!("[RENDERER] ≡ƒÄ« Γ£à Loaded GPU renderer for window {}!", window_id_u64);
                        }
                    }
                    // Otherwise, check if there's a pending renderer we can claim
                    else if let Some(renderer_handle) = self.engine_state.get_window_gpu_renderer(0) {
                        // Try to downcast and claim
                        if let Ok(gpu_renderer) = renderer_handle.clone().downcast::<std::sync::Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>() {
                            self.engine_state.set_window_gpu_renderer(window_id_u64, gpu_renderer.clone() as Arc<dyn std::any::Any + Send + Sync>);
                            self.engine_state.remove_window_gpu_renderer(0);
                            self.engine_state.set_metadata("has_pending_viewport_renderer".to_string(), "false".to_string());
                            
                            *bevy_renderer = Some(gpu_renderer);
                            println!("[RENDERER] ≡ƒÄ» Claimed pending GPU renderer for window {}!", window_id_u64);
                            println!("[RENDERER] Γ£à Bevy will now render to this window's back buffer");
                        }
                    }
                }

                match event {
                WindowEvent::RedrawRequested => {
                    #[cfg(target_os = "windows")]
                    unsafe {
                        // ALWAYS run compositor if we have D3D11 set up (for continuous Bevy rendering)
                        // GPUI only re-renders when needs_render is true
                        let should_render_gpui = *needs_render;
                        
                        // Diagnostic: Show decoupled rendering rates
                        static mut COMPOSITOR_FRAME_COUNT: u32 = 0;
                        static mut GPUI_FRAME_COUNT: u32 = 0;
                        COMPOSITOR_FRAME_COUNT += 1;
                        if should_render_gpui {
                            GPUI_FRAME_COUNT += 1;
                            if GPUI_FRAME_COUNT % 60 == 1 {
                                println!("[DECOUPLED-RENDER] 🎨 GPUI frame {} (compositor frame {})", GPUI_FRAME_COUNT, COMPOSITOR_FRAME_COUNT);
                                println!("[DECOUPLED-RENDER] 📊 Compositor: {} fps, GPUI: {} fps (estimated)", 
                                    COMPOSITOR_FRAME_COUNT / (GPUI_FRAME_COUNT / 60).max(1),
                                    60);
                            }
                        }
                        
                        if should_render_gpui {
                            // First refresh windows (marks windows as dirty)
                            let _ = gpui_app.update(|app| {
                                app.refresh_windows();
                            });
                            // After update finishes, effects are flushed
                            // Now manually trigger drawing
                            let _ = gpui_app.update(|app| {
                                app.draw_windows();
                            });

                            // Reset the flag after rendering
                            *needs_render = false;
                        }

                        // Lazy initialization of shared texture on first render
                        if !*shared_texture_initialized && gpui_window.is_some() && d3d_device.is_some() {
                            let gpui_window_ref = gpui_window.as_ref().unwrap();
                            let device = d3d_device.as_ref().unwrap();

                            // Get the shared texture handle from GPUI using the new update method
                            let handle_result = gpui_app.update(|app| {
                                gpui_window_ref.update(app, |_view, window, _cx| {
                                    window.get_shared_texture_handle()
                                })
                            });

                            if let Ok(handle_ptr) = handle_result {
                                if let Some(handle_ptr) = handle_ptr {
                                    println!("Γ£à Got shared texture handle from GPUI: {:p}", handle_ptr);

                                    // Open the shared texture using OpenSharedResource (legacy API)
                                    // GPUI uses GetSharedHandle() which requires the legacy API
                                    let mut texture: Option<ID3D11Texture2D> = None;
                                    let result = device.OpenSharedResource(
                                        HANDLE(handle_ptr as _),
                                        &mut texture
                                    );

                                    match result {
                                        Ok(_) => {
                                            if let Some(shared_texture_val) = texture {
                                                // Get texture description to create our persistent copy
                                                let mut desc = D3D11_TEXTURE2D_DESC::default();
                                                shared_texture_val.GetDesc(&mut desc);

                                                // Create persistent texture (not shared, just ours)
                                                desc.MiscFlags = D3D11_RESOURCE_MISC_FLAG(0).0 as u32; // Remove shared flag
                                                desc.Usage = D3D11_USAGE_DEFAULT;
                                                desc.BindFlags = D3D11_BIND_SHADER_RESOURCE.0 as u32;

                                                let mut persistent_texture: Option<ID3D11Texture2D> = None;
                                                let create_result = device.CreateTexture2D(&desc, None, Some(&mut persistent_texture));

                                                if create_result.is_ok() && persistent_texture.is_some() {
                                                    let tex = persistent_texture.as_ref().unwrap();

                                                    // CRITICAL: Create SRV once here, not per-frame
                                                    // This prevents memory leaks from allocating SRV every frame
                                                    let mut srv: Option<ID3D11ShaderResourceView> = None;
                                                    let srv_result = device.CreateShaderResourceView(tex, None, Some(&mut srv));

                                                    if srv_result.is_ok() && srv.is_some() {
                                                        *persistent_gpui_srv = srv;
                                                        println!("Γ£à Created cached SRV for persistent texture (no per-frame alloc)");
                                                    } else {
                                                        eprintln!("Γ¥î Failed to create SRV: {:?}", srv_result);
                                                    }

                                                    *persistent_gpui_texture = persistent_texture;
                                                    println!("Γ£à Created persistent GPUI texture buffer!");
                                                } else {
                                                    eprintln!("Γ¥î Failed to create persistent texture: {:?}", create_result);
                                                }

                                                *shared_texture = Some(shared_texture_val);
                                                *shared_texture_initialized = true;
                                                println!("Γ£à Opened shared texture in winit D3D11 device!");
                                            }
                                        }
                                        Err(e) => {
                                            println!("Γ¥î Failed to open shared texture: {:?}", e);
                                            *shared_texture_initialized = true;
                                        }
                                    }
                                } else {
                                    println!("ΓÜá∩╕Å  GPUI hasn't created shared texture yet, will retry next frame");
                                }
                            }
                        }

                        // Note: We don't present here - we'll present once after compositing all layers

                        // GPU-side zero-copy 3-layer composition:
                        // Layer 0 (bottom): Green background (cleared)
                        // Layer 1 (middle): Bevy 3D rendering (opaque, from shared D3D12 texture)
                        // Layer 2 (top): GPUI UI (transparent, alpha-blended)
                        // CRITICAL: Only present frames when we have valid GPUI content to avoid flickering
                        if let (Some(context), Some(shared_texture), Some(persistent_texture), Some(srv), Some(swap_chain), Some(render_target_view), Some(blend_state), Some(vertex_shader), Some(pixel_shader), Some(vertex_buffer), Some(input_layout), Some(sampler_state)) =
                            (d3d_context.as_ref(), &*shared_texture, &*persistent_gpui_texture, &*persistent_gpui_srv, swap_chain.as_ref(), render_target_view.as_ref(), blend_state.as_ref(), vertex_shader.as_ref(), pixel_shader.as_ref(), vertex_buffer.as_ref(), input_layout.as_ref(), sampler_state.as_ref()) {

                            // Periodically check D3D11 device status
                            static mut DEVICE_CHECK_COUNTER: u32 = 0;
                            unsafe {
                                DEVICE_CHECK_COUNTER += 1;
                                if DEVICE_CHECK_COUNTER % 300 == 0 {
                                    if let Some(device) = d3d_device.as_ref() {
                                        let device_reason = device.GetDeviceRemovedReason();
                                        if device_reason.is_err() {
                                            eprintln!("[COMPOSITOR] ⚠️  D3D11 device has been removed! Reason: {:?}", device_reason);
                                            eprintln!("[COMPOSITOR] 🔄 Device recreation would be needed for full recovery");
                                            // Clear Bevy texture cache to prevent using stale handles
                                            *bevy_texture = None;
                                            *bevy_srv = None;
                                        }
                                    }
                                }
                            }

                            // Copy from GPUI's shared texture to our persistent buffer ONLY if GPUI rendered this frame
                            // This preserves the last rendered frame even if GPUI doesn't re-render
                            if should_render_gpui {
                                context.CopyResource(persistent_texture, shared_texture);
                            }
                            // If GPUI didn't render, we just reuse the last frame from persistent_texture

                            // Clear to black (bottom layer) - immediate mode background
                            let black = [0.0f32, 0.0, 0.0, 1.0];
                            context.ClearRenderTargetView(render_target_view, &black);

                            // Set render target
                            context.OMSetRenderTargets(Some(&[Some(render_target_view.clone())]), None);

                            // LAYER 1: Draw Bevy texture to back buffer (BEHIND GPUI)
                            // This window's own Bevy renderer (if it has a 3D viewport)
                            // Debug: Check if we have a renderer
                            static mut CHECK_COUNT: u32 = 0;
                            unsafe {
                                CHECK_COUNT += 1;
                                if CHECK_COUNT % 60 == 0 {
                                    let window_id_u64 = std::mem::transmute::<_, u64>(window_id);
                                    if bevy_renderer.is_none() {
                                        println!("[RENDERER] ΓÅ│ No Bevy renderer for window {} (checked {} times)...", window_id_u64, CHECK_COUNT);
                                    } else {
                                        println!("[RENDERER] Γ£à Have Bevy renderer for window {} (frame {})", window_id_u64, CHECK_COUNT);
                                    }
                                }
                            }

                            if let Some(ref gpu_renderer_arc) = bevy_renderer {
                                if let Ok(gpu_renderer) = gpu_renderer_arc.lock() {
                                    if let Some(ref bevy_renderer_inst) = gpu_renderer.bevy_renderer {
                                        // Get the current native handle from Bevy's read buffer
                                        if let Some(native_handle) = bevy_renderer_inst.get_current_native_handle() {
                                            static mut BEVY_FIRST_RENDER: bool = false;
                                            if !BEVY_FIRST_RENDER {
                                                eprintln!("≡ƒÄ« First Bevy texture found for this window! Starting composition...");
                                                BEVY_FIRST_RENDER = true;
                                            }
                                            // Extract D3D11 handle
                                            if let engine_backend::subsystems::render::NativeTextureHandle::D3D11(handle_ptr) = native_handle {
                                                // Open the shared texture from Bevy using D3D11.1 API (supports NT handles)
                                                let mut bevy_texture_local: Option<ID3D11Texture2D> = None;
                                                let device = d3d_device.as_ref().unwrap();
                                                
                                                // DIAGNOSTIC: Log handle opening
                                                static mut OPEN_ATTEMPT: u32 = 0;
                                                unsafe {
                                                    OPEN_ATTEMPT += 1;
                                                    if OPEN_ATTEMPT == 1 || OPEN_ATTEMPT % 120 == 0 {
                                                        eprintln!("[COMPOSITOR] 🔓 Attempting to open Bevy shared texture handle: 0x{:X} (attempt {})", handle_ptr, OPEN_ATTEMPT);
                                                    }
                                                }
                                                
                                                // Try to cast to ID3D11Device1 for OpenSharedResource1 (supports NT handles)
                                                let open_result: std::result::Result<(), windows::core::Error> = unsafe {
                                                    match device.cast::<ID3D11Device1>() {
                                                        Ok(device1) => {
                                                            // Use OpenSharedResource1 which supports NT handles from CreateSharedHandle
                                                            let result: std::result::Result<ID3D11Texture2D, windows::core::Error> = device1.OpenSharedResource1(
                                                                HANDLE(handle_ptr as *mut _)
                                                            );
                                                            match result {
                                                                Ok(tex) => {
                                                                    bevy_texture_local = Some(tex);
                                                                    Ok(())
                                                                }
                                                                Err(e) => Err(e)
                                                            }
                                                        }
                                                        Err(cast_err) => {
                                                            // Fallback to legacy OpenSharedResource (won't work with NT handles but try anyway)
                                                            eprintln!("[COMPOSITOR] ⚠️  Failed to cast to ID3D11Device1: {:?}, using legacy OpenSharedResource", cast_err);
                                                            device.OpenSharedResource(
                                                                HANDLE(handle_ptr as *mut _),
                                                                &mut bevy_texture_local
                                                            )
                                                        }
                                                    }
                                                };
                                                
                                                if let Err(e) = open_result {
                                                    // Check for device removed/suspended errors
                                                    let hresult = e.code().0;
                                                    let is_device_error = hresult == 0x887A0005_u32 as i32 || // DXGI_ERROR_DEVICE_REMOVED
                                                                         hresult == 0x887A0006_u32 as i32 || // DXGI_ERROR_DEVICE_HUNG
                                                                         hresult == 0x887A0007_u32 as i32 || // DXGI_ERROR_DEVICE_RESET
                                                                         hresult == 0x887A0020_u32 as i32;   // DXGI_ERROR_DRIVER_INTERNAL_ERROR
                                                    
                                                    static mut OPEN_ERROR_COUNT: u32 = 0;
                                                    static mut LAST_WAS_DEVICE_ERROR: bool = false;
                                                    unsafe {
                                                        OPEN_ERROR_COUNT += 1;
                                                        
                                                        if is_device_error {
                                                            if !LAST_WAS_DEVICE_ERROR || OPEN_ERROR_COUNT % 600 == 1 {
                                                                eprintln!("[COMPOSITOR] ❌ GPU DEVICE REMOVED/SUSPENDED: {:?}", e);
                                                                eprintln!("[COMPOSITOR] 💡 This is usually caused by:");
                                                                eprintln!("[COMPOSITOR]    - GPU driver crash/timeout (TDR)");
                                                                eprintln!("[COMPOSITOR]    - GPU overheating");
                                                                eprintln!("[COMPOSITOR]    - Power management suspending GPU");
                                                                eprintln!("[COMPOSITOR]    - Unexpected power dip to the GPU");
                                                                eprintln!("[COMPOSITOR]    - Display driver update in progress");
                                                                eprintln!("[COMPOSITOR] 🔄 Continuing with GPUI-only rendering...");
                                                                LAST_WAS_DEVICE_ERROR = true;
                                                            }
                                                            // Invalidate Bevy texture cache to force retry after device recovery
                                                            *bevy_texture = None;
                                                            *bevy_srv = None;
                                                        } else {
                                                            LAST_WAS_DEVICE_ERROR = false;
                                                            if OPEN_ERROR_COUNT == 1 || OPEN_ERROR_COUNT % 60 == 0 {
                                                                eprintln!("[COMPOSITOR] ❌ Failed to open Bevy shared resource: {:?} (error count: {})", e, OPEN_ERROR_COUNT);
                                                            }
                                                        }
                                                    }
                                                }

                                                if let Some(ref bevy_tex) = bevy_texture_local {
                                                    unsafe {
                                                        if OPEN_ATTEMPT == 1 || OPEN_ATTEMPT % 120 == 0 {
                                                            eprintln!("[COMPOSITOR] ✅ Successfully opened Bevy texture");
                                                        }
                                                    }
                                                    
                                                    // Create or reuse SRV for Bevy texture
                                                    if bevy_texture.is_none() || bevy_texture.as_ref().map(|t| t.as_raw()) != Some(bevy_tex.as_raw()) {
                                                        // Create new SRV - MUST match Bevy's BGRA8UnormSrgb format!
                                                        let srv_desc = D3D11_SHADER_RESOURCE_VIEW_DESC {
                                                            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                                                            ViewDimension: D3D11_SRV_DIMENSION_TEXTURE2D,
                                                            Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
                                                                Texture2D: D3D11_TEX2D_SRV {
                                                                    MostDetailedMip: 0,
                                                                    MipLevels: 1,
                                                                },
                                                            },
                                                        };

                                                        let mut new_srv: Option<ID3D11ShaderResourceView> = None;
                                                        let srv_result = device.CreateShaderResourceView(
                                                            bevy_tex,
                                                            Some(&srv_desc),
                                                            Some(&mut new_srv)
                                                        );
                                                        
                                                        if let Err(e) = srv_result {
                                                            // Check for device removed errors
                                                            let hresult = e.code().0;
                                                            let is_device_error = hresult == 0x887A0005_u32 as i32 || 
                                                                                 hresult == 0x887A0006_u32 as i32 ||
                                                                                 hresult == 0x887A0007_u32 as i32;
                                                            
                                                            static mut SRV_ERROR_COUNT: u32 = 0;
                                                            unsafe {
                                                                SRV_ERROR_COUNT += 1;
                                                                if is_device_error {
                                                                    if SRV_ERROR_COUNT == 1 || SRV_ERROR_COUNT % 600 == 0 {
                                                                        eprintln!("[COMPOSITOR] ❌ GPU device error creating SRV: {:?}", e);
                                                                        eprintln!("[COMPOSITOR] 🔄 Falling back to GPUI-only rendering");
                                                                    }
                                                                    // Clear cache
                                                                    *bevy_texture = None;
                                                                    *bevy_srv = None;
                                                                } else if SRV_ERROR_COUNT == 1 || SRV_ERROR_COUNT % 60 == 0 {
                                                                    eprintln!("[COMPOSITOR] ❌ Failed to create SRV for Bevy texture: {:?} (error count: {})", e, SRV_ERROR_COUNT);
                                                                }
                                                            }
                                                        } else {
                                                            unsafe {
                                                                if OPEN_ATTEMPT == 1 || OPEN_ATTEMPT % 120 == 0 {
                                                                    eprintln!("[COMPOSITOR] ✅ Created SRV for Bevy texture");
                                                                }
                                                            }
                                                        }

                                                        *bevy_texture = Some(bevy_tex.clone());
                                                        *bevy_srv = new_srv;
                                                    }

                                                    // Draw Bevy texture to back buffer (opaque, no blending)
                                                    if let Some(ref bevy_shader_view) = &*bevy_srv {
                                                        // Disable blending for opaque Bevy render
                                                        context.OMSetBlendState(None, None, 0xffffffff);

                                                        // Set shaders
                                                        context.VSSetShader(vertex_shader, None);
                                                        context.PSSetShader(pixel_shader, None);

                                                        // Set input layout
                                                        context.IASetInputLayout(input_layout);

                                                        // Set vertex buffer (fullscreen quad)
                                                        let stride = 16u32;
                                                        let offset = 0u32;
                                                        context.IASetVertexBuffers(0, 1, Some(&Some(vertex_buffer.clone())), Some(&stride), Some(&offset));

                                                        // Set topology
                                                        context.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);

                                                        // Set Bevy texture and sampler
                                                        context.PSSetShaderResources(0, Some(&[Some(bevy_shader_view.clone())]));
                                                        context.PSSetSamplers(0, Some(&[Some(sampler_state.clone())]));

                                                        // Set viewport
                                                        let size = winit_window.inner_size();
                                                        let viewport = D3D11_VIEWPORT {
                                                            TopLeftX: 0.0,
                                                            TopLeftY: 0.0,
                                                            Width: size.width as f32,
                                                            Height: size.height as f32,
                                                            MinDepth: 0.0,
                                                            MaxDepth: 1.0,
                                                        };
                                                        context.RSSetViewports(Some(&[viewport]));

                                                        // Draw Bevy's 3D rendering (opaque)
                                                        context.Draw(4, 0);

                                                        static mut BEVY_FRAME_COUNT: u32 = 0;
                                                        BEVY_FRAME_COUNT += 1;
                                                        if BEVY_FRAME_COUNT % 120 == 1 {
                                                            eprintln!("≡ƒÄ« Bevy layer composited to back buffer (frame {})", BEVY_FRAME_COUNT);
                                                        }
                                                    }
                                                }
                                            }
                                        } else {
                                            // DIAGNOSTIC: Texture handle not available yet
                                            static mut HANDLE_CHECK_COUNT: u32 = 0;
                                            unsafe {
                                                HANDLE_CHECK_COUNT += 1;
                                                if HANDLE_CHECK_COUNT % 120 == 1 {
                                                    eprintln!("[RENDERER] ⚠️  Bevy renderer exists but texture handle is None (checked {} times)", HANDLE_CHECK_COUNT);
                                                    eprintln!("[RENDERER] 💡 This means Bevy hasn't created shared textures yet - waiting for first render...");
                                                }
                                            }
                                        }
                                    } else {
                                        // DIAGNOSTIC: GpuRenderer has no bevy_renderer
                                        static mut NO_BEVY_COUNT: u32 = 0;
                                        unsafe {
                                            NO_BEVY_COUNT += 1;
                                            if NO_BEVY_COUNT % 120 == 1 {
                                                eprintln!("[RENDERER] ⚠️  GpuRenderer exists but bevy_renderer is None (checked {} times)", NO_BEVY_COUNT);
                                                eprintln!("[RENDERER] 💡 This means BevyRenderer initialization failed or timed out");
                                            }
                                        }
                                    }
                                } else {
                                    // DIAGNOSTIC: Failed to lock GpuRenderer
                                    static mut LOCK_FAIL_COUNT: u32 = 0;
                                    unsafe {
                                        LOCK_FAIL_COUNT += 1;
                                        if LOCK_FAIL_COUNT % 120 == 1 {
                                            eprintln!("[RENDERER] ⚠️  Failed to lock GpuRenderer (contended {} times)", LOCK_FAIL_COUNT);
                                        }
                                    }
                                }
                            }

                            // LAYER 2: Draw GPUI texture with alpha blending (ON TOP of Bevy)
                            // Set blend state for alpha blending (top layer)
                            let blend_factor = [0.0f32, 0.0, 0.0, 0.0];
                            context.OMSetBlendState(Some(blend_state), Some(&blend_factor), 0xffffffff);

                            // Use cached SRV (no per-frame allocation!)
                            {
                                static mut FRAME_COUNT: u32 = 0;
                                FRAME_COUNT += 1;
                                if FRAME_COUNT % 60 == 1 {
                                    eprintln!("≡ƒÄ¿ Compositing GPUI texture (frame {})", FRAME_COUNT);
                                }

                                // Set shaders
                                context.VSSetShader(vertex_shader, None);
                                context.PSSetShader(pixel_shader, None);

                                // Set input layout
                                context.IASetInputLayout(input_layout);

                                // Set vertex buffer (fullscreen quad)
                                let stride = 16u32; // 2 floats pos + 2 floats tex = 16 bytes
                                let offset = 0u32;
                                context.IASetVertexBuffers(0, 1, Some(&Some(vertex_buffer.clone())), Some(&stride), Some(&offset));

                                // Set topology
                                context.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);

                                // Set GPUI texture and sampler
                                context.PSSetShaderResources(0, Some(&[Some(srv.clone())]));
                                context.PSSetSamplers(0, Some(&[Some(sampler_state.clone())]));

                                // Set viewport - must use physical pixels
                                // inner_size() already returns physical pixels (logical ├ù scale factor)
                                let size = winit_window.inner_size();
                                let viewport = D3D11_VIEWPORT {
                                    TopLeftX: 0.0,
                                    TopLeftY: 0.0,
                                    Width: size.width as f32,
                                    Height: size.height as f32,
                                    MinDepth: 0.0,
                                    MaxDepth: 1.0,
                                };
                                context.RSSetViewports(Some(&[viewport]));

                                // Draw fullscreen quad with GPUI texture (transparent UI layer on top)
                                context.Draw(4, 0);

                                // ONLY present when we successfully composited GPUI content
                                // This prevents flickering of green-only frames
                                let present_result = swap_chain.Present(1, DXGI_PRESENT(0));
                                
                                // Handle present failures gracefully  
                                if present_result.is_err() {
                                    let e = present_result.unwrap();
                                    // The error type is (), so we can't get an HRESULT.
                                    // Instead, just log the error and continue.
                                    static mut PRESENT_ERROR_COUNT: u32 = 0;
                                    unsafe {
                                        PRESENT_ERROR_COUNT += 1;
                                        if PRESENT_ERROR_COUNT == 1 || PRESENT_ERROR_COUNT % 600 == 0 {
                                            eprintln!("[COMPOSITOR] ❌ Present failed - error: {:?}", e);
                                            eprintln!("[COMPOSITOR] 🔄 Continuing (device may recover)...");
                                        }
                                    }
                                }
                            }
                        } else {
                            // Don't present if we don't have GPUI texture ready yet
                            // This shows the last valid frame instead of flickering
                            static mut SKIP_COUNT: u32 = 0;
                            SKIP_COUNT += 1;
                            if SKIP_COUNT <= 3 || SKIP_COUNT % 60 == 0 {
                                eprintln!("ΓÅ¡∩╕Å  Skipping frame {} - waiting for GPUI texture to be ready", SKIP_COUNT);
                            }
                        }
                    }

                    // Request continuous redraws if we have a Bevy renderer (for max FPS viewport)
                    // GPUI will only re-render when needed, but Bevy layer updates continuously
                    if bevy_renderer.is_some() {
                        winit_window.request_redraw();
                    }
                }
                // Handle keyboard events - request redraw
                WindowEvent::KeyboardInput { event, .. } => {
                    println!("≡ƒ¬ƒ Keyboard event: {:?}, repeat: {}", event.physical_key, event.repeat);

                    // Forward keyboard events to GPUI
                    if let Some(gpui_window_ref) = gpui_window.as_ref() {
                        // Store event and create keystroke before borrowing
                        let current_modifiers_val = *current_modifiers;
                        
                        // Get the key string
                        let keystroke_opt = match &event.physical_key {
                            PhysicalKey::Code(code) => {
                                if let Some(key) = Self::keycode_to_string_static(*code) {
                                    let key_char = match &event.text {
                                        Some(text) if !text.is_empty() => Some(text.chars().next().map(|c| c.to_string()).unwrap_or_default()),
                                        _ => None,
                                    };
                                    
                                    Some(Keystroke {
                                        modifiers: current_modifiers_val,
                                        key,
                                        key_char,
                                    })
                                } else {
                                    None
                                }
                            }
                            PhysicalKey::Unidentified(_) => None,
                        };
                        
                        if let Some(keystroke) = keystroke_opt {
                            let gpui_event = match event.state {
                                ElementState::Pressed => {
                                    println!("≡ƒôñ KeyDown: {:?}", keystroke);

                                    PlatformInput::KeyDown(KeyDownEvent {
                                        keystroke,
                                        is_held: event.repeat,
                                    })
                                }
                                ElementState::Released => {
                                    println!("≡ƒôñ KeyUp: {:?}", keystroke);

                                    PlatformInput::KeyUp(KeyUpEvent { keystroke })
                                }
                            };

                            let _ = gpui_app.update(|cx| gpui_window_ref.inject_input_event(cx, gpui_event));
                        }
                    }
                    
                    *needs_render = true;
                    /* winit_window already available */ {
                        winit_window.request_redraw();
                    }
                }
                WindowEvent::ModifiersChanged(new_modifiers) => {
                    // Update tracked modifiers
                    *current_modifiers = convert_modifiers(&new_modifiers.state());
                    
                    // Forward modifier changes to GPUI
                    if let Some(gpui_window_ref) = gpui_window.as_ref() {
                        let gpui_event = PlatformInput::ModifiersChanged(ModifiersChangedEvent {
                            modifiers: *current_modifiers,
                            capslock: Capslock { on: false }, // TODO: Track capslock state
                        });

                        let _ = gpui_app.update(|cx| gpui_window_ref.inject_input_event(cx, gpui_event));
                    }
                    
                    *needs_render = true;
                    /* winit_window already available */ {
                        winit_window.request_redraw();
                    }
                }
                // Handle window resize - resize GPUI renderer and request redraw
                WindowEvent::Resized(new_size) => {
                    // Tell GPUI to resize its internal rendering buffers AND update logical size
                    if let Some(gpui_window_ref) = gpui_window.as_ref() {
                        let scale_factor = winit_window.scale_factor() as f32;

                        // Physical pixels for renderer (what GPU renders at)
                        let physical_size = gpui::size(
                            gpui::DevicePixels(new_size.width as i32),
                            gpui::DevicePixels(new_size.height as i32),
                        );

                        // Logical pixels for GPUI layout (physical / scale)
                        let logical_size = gpui::size(
                            gpui::px(new_size.width as f32 / scale_factor),
                            gpui::px(new_size.height as f32 / scale_factor),
                        );

                        let _ = gpui_app.update(|app| {
                            let _ = gpui_window_ref.update(app, |_view, window, _cx| {
                                #[cfg(target_os = "windows")]
                                {
                                    // Resize renderer (GPU buffers)
                                    if let Err(e) = window.resize_renderer(physical_size) {
                                        eprintln!("Γ¥î Failed to resize GPUI renderer: {:?}", e);
                                    } else {
                                        println!("Γ£à Resized GPUI renderer to {:?}", physical_size);

                                        // CRITICAL: GPUI recreates its texture when resizing, so we need to re-obtain the shared handle
                                        // Mark for re-initialization on next frame
                                        *shared_texture_initialized = false;
                                        *shared_texture = None;
                                        *persistent_gpui_texture = None;
                                        *persistent_gpui_srv = None; // Also clear cached SRV
                                        println!("≡ƒöä Marked shared texture for re-initialization after GPUI resize");
                                    }

                                    // Update logical size (for UI layout)
                                    window.update_logical_size(logical_size);
                                    println!("Γ£à Updated GPUI logical size to {:?} (scale {})", logical_size, scale_factor);

                                    // CRITICAL: Mark window as dirty to trigger UI re-layout
                                    // This is what GPUI's internal windows do in bounds_changed()
                                    window.refresh();
                                    println!("≡ƒÄ¿ Marked window for refresh/re-layout");
                                }
                            });
                        });
                    }

                    // CRITICAL: Resize the swap chain to match the new window size
                    // This is why the green background was stuck at the original size!
                    if let Some(swap_chain_ref) = swap_chain.as_ref() {
                        unsafe {
                            // Release the render target view before resizing
                            *render_target_view = None;

                            // Resize swap chain buffers
                            let resize_result = swap_chain_ref.ResizeBuffers(
                                0,  // Keep current buffer count
                                new_size.width,
                                new_size.height,
                                DXGI_FORMAT_UNKNOWN,  // Keep current format
                                DXGI_SWAP_CHAIN_FLAG(0),  // No flags
                            );

                            if let Err(e) = resize_result {
                                eprintln!("Γ¥î Failed to resize swap chain: {:?}", e);
                            } else {
                                println!("Γ£à Resized swap chain to {}x{}", new_size.width, new_size.height);

                                // Recreate render target view
                                if let Some(device) = d3d_device.as_ref() {
                                    let back_buffer: Option<ID3D11Texture2D> = swap_chain_ref.GetBuffer(0).ok();
                                    if let Some(back_buffer) = back_buffer {
                                        let mut new_rtv: Option<ID3D11RenderTargetView> = None;
                                        let result = device.CreateRenderTargetView(&back_buffer, None, Some(&mut new_rtv));
                                        if result.is_ok() {
                                            *render_target_view = new_rtv;
                                            println!("Γ£à Recreated render target view");
                                        } else {
                                            eprintln!("Γ¥î Failed to create render target view: {:?}", result);
                                        }
                                    } else {
                                        eprintln!("Γ¥î Failed to get back buffer after resize");
                                    }
                                }
                            }
                        }
                    }

                    *needs_render = true;
                    /* winit_window already available */ {
                        winit_window.request_redraw();
                    }
                }
                WindowEvent::CursorMoved { position, .. } => {
                    // Update cursor position tracking
                    /* winit_window already available */ {
                        let scale_factor = winit_window.scale_factor() as f32;
                        let logical_x = position.x as f32 / scale_factor;
                        let logical_y = position.y as f32 / scale_factor;
                        *last_cursor_position = point(px(logical_x), px(logical_y));
                    }
                    
                    // Forward mouse move events to GPUI using inject_input_event
                    if let Some(gpui_window_ref) = gpui_window.as_ref() {
                        /* winit_window already available */
                        let scale_factor = winit_window.scale_factor() as f32;

                        // Convert physical position to logical position
                        let logical_x = position.x as f32 / scale_factor;
                        let logical_y = position.y as f32 / scale_factor;

                        // Determine which button is pressed (if any)
                        let pressed_button = if pressed_mouse_buttons.contains(&MouseButton::Left) {
                            Some(MouseButton::Left)
                        } else if pressed_mouse_buttons.contains(&MouseButton::Right) {
                            Some(MouseButton::Right)
                        } else if pressed_mouse_buttons.contains(&MouseButton::Middle) {
                            Some(MouseButton::Middle)
                        } else {
                            None
                        };

                        let gpui_event = PlatformInput::MouseMove(MouseMoveEvent {
                            position: point(px(logical_x), px(logical_y)),
                            pressed_button,
                            modifiers: *current_modifiers,
                        });

                        let result = gpui_app.update(|cx| gpui_window_ref.inject_input_event(cx, gpui_event));

                        // Request redraw for cursor updates
                        *needs_render = true;
                        winit_window.request_redraw();
                    }
                }
                WindowEvent::MouseInput { state, button, .. } => {
                    // Forward mouse button events to GPUI
                    if let Some(gpui_window_ref) = gpui_window.as_ref() {
                        let gpui_button = convert_mouse_button(button);
                        // Use actual cursor position for clicks, not smoothed position!
                        let position = *last_cursor_position;

                        match state {
                            ElementState::Pressed => {
                                eprintln!("≡ƒû▒∩╕Å MouseInput PRESSED: {:?} at {:?}", button, position);
                                
                                // Track pressed button
                                pressed_mouse_buttons.insert(gpui_button);
                                
                                // Update click count
                                let click_count = click_state.update(gpui_button, position);
                                
                                let gpui_event = PlatformInput::MouseDown(MouseDownEvent {
                                    button: gpui_button,
                                    position,
                                    modifiers: *current_modifiers,
                                    click_count,
                                    first_mouse: false,
                                });

                                eprintln!("≡ƒôñ Injecting MouseDown event...");
                                let result = gpui_app.update(|cx| gpui_window_ref.inject_input_event(cx, gpui_event));
                                eprintln!("≡ƒôÑ MouseDown result: {:?}", result);
                            }
                            ElementState::Released => {
                                eprintln!("≡ƒû▒∩╕Å MouseInput RELEASED: {:?}", button);
                                
                                // Remove pressed button
                                pressed_mouse_buttons.remove(&gpui_button);
                                
                                let gpui_event = PlatformInput::MouseUp(MouseUpEvent {
                                    button: gpui_button,
                                    position,
                                    modifiers: *current_modifiers,
                                    click_count: click_state.current_count,
                                });

                                eprintln!("≡ƒôñ Injecting MouseUp event...");
                                let result = gpui_app.update(|cx| gpui_window_ref.inject_input_event(cx, gpui_event));
                                eprintln!("≡ƒôÑ MouseUp result: {:?}", result);
                            }
                        }

                        // Request redraw for click feedback
                        *needs_render = true;
                        /* winit_window already available */ {
                            winit_window.request_redraw();
                        }
                    }
                }
                WindowEvent::MouseWheel { delta, .. } => {
                    // Forward mouse wheel events to GPUI
                    if let Some(gpui_window_ref) = gpui_window.as_ref() {
                        /* winit_window already available */

                        // Convert delta
                        let scroll_delta = match delta {
                            winit::event::MouseScrollDelta::LineDelta(x, y) => {
                                ScrollDelta::Lines(point(x, y))
                            }
                            winit::event::MouseScrollDelta::PixelDelta(pos) => {
                                let scale_factor = winit_window.scale_factor() as f32;
                                ScrollDelta::Pixels(point(
                                    px(pos.x as f32 / scale_factor),
                                    px(pos.y as f32 / scale_factor),
                                ))
                            }
                        };

                        // Use actual cursor position for scroll events
                        let position = *last_cursor_position;

                        let gpui_event = PlatformInput::ScrollWheel(ScrollWheelEvent {
                            position,
                            delta: scroll_delta,
                            modifiers: *current_modifiers,
                            touch_phase: TouchPhase::Moved,
                        });

                        let _ = gpui_app.update(|cx| gpui_window_ref.inject_input_event(cx, gpui_event));

                        // Request redraw for scroll updates
                        *needs_render = true;
                        winit_window.request_redraw();
                    }
                }
                _ => {}
            }
        }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // LAZY CHECK: If GPUI windows need rendering, request redraw
        // This happens once per event loop iteration, not blocking
        for (_window_id, window_state) in &mut self.windows {
            if let Some(gpui_window_ref) = &window_state.gpui_window {
                // Only check if we're not already waiting for a redraw
                if !window_state.needs_render {
                    let gpui_needs_render = window_state.gpui_app.update(|cx| {
                        gpui_window_ref.needs_render(cx)
                    });
                    if gpui_needs_render {
                        window_state.needs_render = true;
                        window_state.winit_window.request_redraw();
                    }
                }
            }
        }
        
        // Check for window creation requests
        while let Ok(request) = self.window_request_rx.try_recv() {
            self.pending_window_requests.push(request);
        }

        // Process pending window requests (collect first to avoid borrow issues)
        let requests: Vec<_> = self.pending_window_requests.drain(..).collect();
        for request in requests {
            match request {
                WindowRequest::CloseWindow { window_id } => {
                    // Find and close the window with this ID
                    let window_id_native = unsafe { std::mem::transmute::<u64, WindowId>(window_id) };
                    if self.windows.remove(&window_id_native).is_some() {
                        println!("Γ£à Closed window with ID: {:?}", window_id);
                        self.engine_state.decrement_window_count();
                    }
                }
                _ => {
                    self.create_window(event_loop, request);
                }
            }
        }

        // Initialize any uninitialized GPUI windows
        for (window_id, window_state) in self.windows.iter_mut() {
        if !window_state.gpui_window_initialized {
            let winit_window = window_state.winit_window.clone();
            let scale_factor = winit_window.scale_factor() as f32;
            let size = winit_window.inner_size();

            // GPUI bounds must be in LOGICAL pixels (physical / scale)
            // inner_size() returns physical pixels
            let logical_width = size.width as f32 / scale_factor;
            let logical_height = size.height as f32 / scale_factor;

            let bounds = Bounds {
                origin: point(px(0.0), px(0.0)),
                size: gpui::size(px(logical_width), px(logical_height)),
            };

            println!("≡ƒÄ» Creating GPUI window: physical {}x{}, scale {}, logical {}x{}",
                size.width, size.height, scale_factor, logical_width, logical_height);

            let gpui_raw_handle = winit_window
                .window_handle()
                .expect("Failed to get window handle")
                .as_raw();

            let external_handle = ExternalWindowHandle {
                raw_handle: gpui_raw_handle,
                bounds,
                scale_factor,
                surface_handle: None,
            };

            println!("Γ£à Opening GPUI window on external winit window...");

            // Initialize GPUI components (fonts, themes, keybindings)
            let app = &mut window_state.gpui_app;

            // Clone engine_state for use in closures
            let engine_state_for_actions = self.engine_state.clone();

            // Load custom fonts
            app.update(|app| {
                if let Some(font_data) = Assets::get("fonts/JetBrainsMono-Regular.ttf") {
                    match app.text_system().add_fonts(vec![font_data.data]) {
                        Ok(_) => println!("Successfully loaded JetBrains Mono font"),
                        Err(e) => println!("Failed to load JetBrains Mono font: {:?}", e),
                    }
                } else {
                    println!("Could not find JetBrains Mono font file");
                }

                // Initialize GPUI components
                ui::init(app);
                crate::themes::init(app);
                ui_terminal::init(app);

                // Setup keybindings
                app.bind_keys([
                    KeyBinding::new("ctrl-,", OpenSettings, None),
                    KeyBinding::new("ctrl-space", ToggleCommandPalette, None),
                    KeyBinding::new("ctrl-n", ui_editor::tabs::blueprint_editor::OpenAddNodeMenu, Some("BlueprintGraph")),
                ]);

                let engine_state = engine_state_for_actions.clone();
                app.on_action(move |_: &OpenSettings, _app_cx| {
                    println!("ΓÜÖ∩╕Å  Settings window requested - creating new window!");
                    engine_state.request_window(WindowRequest::Settings);
                });

                let engine_state = engine_state_for_actions.clone();
                app.on_action(move |_: &AboutApp, _app_cx| {
                    println!("ΓäÅ  About window requested - creating new window!");
                    engine_state.request_window(WindowRequest::About);
                });

                let engine_state = engine_state_for_actions.clone();
                app.on_action(move |_: &ShowDocumentation, _app_cx| {
                    println!("≡ƒôÜ Documentation window requested - creating new window!");
                    engine_state.request_window(WindowRequest::Documentation);
                });

                app.activate(true);
            });

            println!("Γ£à GPUI components initialized");

            // Store window_id in EngineState metadata BEFORE opening GPUI window
            // so that views created during open_window_external can access it
            let window_id_u64 = unsafe { std::mem::transmute::<_, u64>(*window_id) };
            println!("[WINDOW-INIT] ≡ƒôì Window ID for this window: {}", window_id_u64);
            self.engine_state.set_metadata("latest_window_id".to_string(), window_id_u64.to_string());

            // If this is a project editor window, also store it with a special key
            if matches!(&window_state.window_type, Some(WindowRequest::ProjectEditor { .. })) {
                self.engine_state.set_metadata("current_project_window_id".to_string(), window_id_u64.to_string());
                println!("[WINDOW-INIT] ≡ƒÄ» This is a ProjectEditor window with ID: {}", window_id_u64);
            }

            // Capture window_id_u64 and engine_state for use in the closure
            let captured_window_id = window_id_u64;
            let engine_state_for_events = self.engine_state.clone();
            println!("[WINDOW-INIT] ≡ƒôª Captured window_id for closure: {}", captured_window_id);

            // Open GPUI window using external window API with appropriate view
            let gpui_window = app.open_window_external(external_handle.clone(), |window, cx| {
                match &window_state.window_type {
                    Some(WindowRequest::Entry) => {
                        create_entry_component(window, cx, &engine_state_for_events)
                    }
                    Some(WindowRequest::Settings) => {
                        create_settings_component(window, cx, &engine_state_for_events)
                    }
                    Some(WindowRequest::About) => {
                        create_about_window(window, cx)
                    }
                    Some(WindowRequest::Documentation) => {
                        create_documentation_window(window, cx)
                    }
                    Some(WindowRequest::ProjectSplash { project_path }) => {
                        // Create loading screen for project loading
                        create_loading_component(
                            PathBuf::from(project_path),
                            captured_window_id,
                            window,
                            cx
                        )
                    }
                    Some(WindowRequest::ProjectEditor { project_path }) => {
                        // Use the captured window_id to ensure consistency
                        // Create the actual PulsarApp editor with the project
                        let app = cx.new(|cx| PulsarApp::new_with_project_and_window_id(
                            std::path::PathBuf::from(project_path),
                            captured_window_id,
                            window,
                            cx
                        ));
                        let pulsar_root = cx.new(|cx| PulsarRoot::new("Pulsar Engine", app, window, cx));
                        cx.new(|cx| ui::Root::new(pulsar_root.into(), window, cx))
                    }
                    Some(WindowRequest::CloseWindow { .. }) | None => {
                        // Fallback to entry screen if window_type is None or CloseWindow
                        create_entry_component(window, cx, &engine_state_for_events)
                    }
                }
            }).expect("Failed to open GPUI window");

            window_state.gpui_window = Some(gpui_window);

            // Initialize D3D11 for blitting on Windows
            #[cfg(target_os = "windows")]
            unsafe {
                println!("Γ£à Initializing D3D11 for GPU blitting...");

                let mut device = None;
                let mut context = None;
                let mut feature_level = Default::default();

                let result = D3D11CreateDevice(
                    None,
                    D3D_DRIVER_TYPE_HARDWARE,
                    HMODULE(std::ptr::null_mut()),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    None,
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    Some(&mut feature_level),
                    Some(&mut context),
                );

                if result.is_ok() && device.is_some() {
                    window_state.d3d_device = device.clone();
                    window_state.d3d_context = context;
                    println!("Γ£à D3D11 device created successfully!");

                    // Create swap chain for the winit window
                    let parent_raw = winit_window.window_handle().unwrap().as_raw();
                    let hwnd = match parent_raw {
                        RawWindowHandle::Win32(h) => HWND(h.hwnd.get() as isize as *mut _),
                        _ => {
                            println!("Γ¥î Failed to get HWND");
                            return;
                        }
                    };

                    let dxgi_device: IDXGIDevice = device.as_ref().unwrap().cast().unwrap();
                    let adapter = dxgi_device.GetAdapter().unwrap();
                    let dxgi_factory: IDXGIFactory2 = adapter.GetParent().unwrap();

                    // Swap chain must use physical pixels
                    // inner_size() already returns physical pixels (logical ├ù scale factor)
                    let physical_width = size.width;
                    let physical_height = size.height;
                    println!("≡ƒÄ» Creating swap chain: physical {}x{}, scale {}",
                        physical_width, physical_height, winit_window.scale_factor());

                    let swap_chain_desc = DXGI_SWAP_CHAIN_DESC1 {
                        Width: physical_width,
                        Height: physical_height,
                        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                        Stereo: FALSE,
                        SampleDesc: DXGI_SAMPLE_DESC {
                            Count: 1,
                            Quality: 0,
                        },
                        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                        BufferCount: 2,
                        Scaling: DXGI_SCALING_NONE,
                        SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                        AlphaMode: DXGI_ALPHA_MODE_IGNORE,  // Ignore alpha for solid window
                        Flags: 0,
                    };

                    let swap_chain = dxgi_factory.CreateSwapChainForHwnd(
                        device.as_ref().unwrap(),
                        hwnd,
                        &swap_chain_desc,
                        None,
                        None,
                    );
                    if let Ok(swap_chain) = swap_chain {
                        window_state.swap_chain = Some(swap_chain.clone());
                        println!("Γ£à Swap chain created for winit window!");

                        // Create render target view from swap chain back buffer
                        if let Ok(back_buffer) = swap_chain.GetBuffer::<ID3D11Texture2D>(0) {
                            let mut rtv: Option<ID3D11RenderTargetView> = None;
                            if device.as_ref().unwrap().CreateRenderTargetView(&back_buffer, None, Some(&mut rtv as *mut _)).is_ok() {
                                window_state.render_target_view = rtv;
                                println!("Γ£à Render target view created!");
                            }
                        }

                        // Create blend state for alpha blending
                        let blend_desc = D3D11_BLEND_DESC {
                            AlphaToCoverageEnable: FALSE,
                            IndependentBlendEnable: FALSE,
                            RenderTarget: [
                                D3D11_RENDER_TARGET_BLEND_DESC {
                                    BlendEnable: TRUE,
                                    SrcBlend: D3D11_BLEND_SRC_ALPHA,
                                    DestBlend: D3D11_BLEND_INV_SRC_ALPHA,
                                    BlendOp: D3D11_BLEND_OP_ADD,
                                    SrcBlendAlpha: D3D11_BLEND_ONE,
                                    DestBlendAlpha: D3D11_BLEND_ZERO,
                                    BlendOpAlpha: D3D11_BLEND_OP_ADD,
                                    RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
                                },
                                D3D11_RENDER_TARGET_BLEND_DESC::default(),
                                D3D11_RENDER_TARGET_BLEND_DESC::default(),
                                D3D11_RENDER_TARGET_BLEND_DESC::default(),
                                D3D11_RENDER_TARGET_BLEND_DESC::default(),
                                D3D11_RENDER_TARGET_BLEND_DESC::default(),
                                D3D11_RENDER_TARGET_BLEND_DESC::default(),
                                D3D11_RENDER_TARGET_BLEND_DESC::default(),
                            ],
                        };

                        let mut blend_state = None;
                        if device.as_ref().unwrap().CreateBlendState(&blend_desc, Some(&mut blend_state as *mut _)).is_ok() {
                            window_state.blend_state = blend_state;
                            println!("Γ£à Blend state created for alpha composition!");
                        }

                        // Create shaders for GPU alpha blending by compiling HLSL at runtime
                        println!("≡ƒö¿ Compiling shaders at runtime...");

                        // Vertex shader source: passthrough with position and texcoord
                        let vs_source = r#"
struct VS_INPUT {
    float2 pos : POSITION;
    float2 tex : TEXCOORD0;
};

struct PS_INPUT {
    float4 pos : SV_POSITION;
    float2 tex : TEXCOORD0;
};

PS_INPUT main(VS_INPUT input) {
    PS_INPUT output;
    output.pos = float4(input.pos, 0.0f, 1.0f);
    output.tex = input.tex;
    return output;
}
"#;

                        // Pixel shader source: sample texture with alpha
                        let ps_source = r#"
Texture2D gpuiTexture : register(t0);
SamplerState samplerState : register(s0);

struct PS_INPUT {
    float4 pos : SV_POSITION;
    float2 tex : TEXCOORD0;
};

float4 main(PS_INPUT input) : SV_TARGET {
    return gpuiTexture.Sample(samplerState, input.tex);
}
"#;

                        // Compile vertex shader
                        let vs_bytecode_blob = {
                            let mut blob: Option<ID3DBlob> = None;
                            let mut error_blob: Option<ID3DBlob> = None;
                            let result = D3DCompile(
                                vs_source.as_ptr() as *const _,
                                vs_source.len(),
                                None,
                                None,
                                None,
                                s!("main"),
                                s!("vs_5_0"),
                                0,
                                0,
                                &mut blob,
                                Some(&mut error_blob),
                            );

                            if result.is_err() {
                                if let Some(err) = error_blob {
                                    let err_msg = std::slice::from_raw_parts(
                                        err.GetBufferPointer() as *const u8,
                                        err.GetBufferSize(),
                                    );
                                    println!("Γ¥î VS compile error: {}", String::from_utf8_lossy(err_msg));
                                }
                            }
                            blob
                        };

                        // Compile pixel shader
                        let ps_bytecode_blob = {
                            let mut blob: Option<ID3DBlob> = None;
                            let mut error_blob: Option<ID3DBlob> = None;
                            let result = D3DCompile(
                                ps_source.as_ptr() as *const _,
                                ps_source.len(),
                                None,
                                None,
                                None,
                                s!("main"),
                                s!("ps_5_0"),
                                0,
                                0,
                                &mut blob,
                                Some(&mut error_blob),
                            );

                            if result.is_err() {
                                if let Some(err) = error_blob {
                                    let err_msg = std::slice::from_raw_parts(
                                        err.GetBufferPointer() as *const u8,
                                        err.GetBufferSize(),
                                    );
                                    println!("Γ¥î PS compile error: {}", String::from_utf8_lossy(err_msg));
                                }
                            }
                            blob
                        };

                        let vs_bytecode = if let Some(blob) = &vs_bytecode_blob {
                            std::slice::from_raw_parts(
                                blob.GetBufferPointer() as *const u8,
                                blob.GetBufferSize(),
                            )
                        } else {
                            &[] as &[u8]
                        };

                        let ps_bytecode = if let Some(blob) = &ps_bytecode_blob {
                            std::slice::from_raw_parts(
                                blob.GetBufferPointer() as *const u8,
                                blob.GetBufferSize(),
                            )
                        } else {
                            &[] as &[u8]
                        };

                        if vs_bytecode.is_empty() || ps_bytecode.is_empty() {
                            println!("Γ¥î Shader compilation failed!");
                        }

                        // Create D3D11 shader objects from compiled bytecode
                        let mut vertex_shader = None;
                        let mut pixel_shader = None;

                        let vs_result = if !vs_bytecode.is_empty() {
                            device.as_ref().unwrap().CreateVertexShader(vs_bytecode, None, Some(&mut vertex_shader as *mut _))
                        } else {
                            Err(Error::from(E_FAIL))
                        };

                        let ps_result = if !ps_bytecode.is_empty() {
                            device.as_ref().unwrap().CreatePixelShader(ps_bytecode, None, Some(&mut pixel_shader as *mut _))
                        } else {
                            Err(Error::from(E_FAIL))
                        };

                        if vs_result.is_ok() && ps_result.is_ok() {
                            window_state.vertex_shader = vertex_shader;
                            window_state.pixel_shader = pixel_shader;
                            println!("Γ£à Shaders created from bytecode!");
                        } else {
                            println!("Γ¥î Failed to create shaders - VS: {:?}, PS: {:?}", vs_result, ps_result);
                        }

                        if window_state.vertex_shader.is_some() && window_state.pixel_shader.is_some() {

                            // Create input layout that matches the vertex shader
                            // VS_INPUT has: float2 pos : POSITION; float2 tex : TEXCOORD0;
                            let layout = [
                                D3D11_INPUT_ELEMENT_DESC {
                                    SemanticName: s!("POSITION"),
                                    SemanticIndex: 0,
                                    Format: DXGI_FORMAT_R32G32_FLOAT,
                                    InputSlot: 0,
                                    AlignedByteOffset: 0,
                                    InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                                    InstanceDataStepRate: 0,
                                },
                                D3D11_INPUT_ELEMENT_DESC {
                                    SemanticName: s!("TEXCOORD"),
                                    SemanticIndex: 0,
                                    Format: DXGI_FORMAT_R32G32_FLOAT,
                                    InputSlot: 0,
                                    AlignedByteOffset: 8,
                                    InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                                    InstanceDataStepRate: 0,
                                },
                            ];

                            let mut input_layout = None;
                            if device.as_ref().unwrap().CreateInputLayout(&layout, vs_bytecode, Some(&mut input_layout as *mut _)).is_ok() {
                                window_state.input_layout = input_layout;
                                println!("Γ£à Input layout created!");
                            } else {
                                println!("Γ¥î Failed to create input layout");
                            }
                        }

                        // Create vertex buffer for fullscreen quad
                        #[repr(C)]
                        struct Vertex {
                            pos: [f32; 2],
                            tex: [f32; 2],
                        }

                        let vertices = [
                            Vertex { pos: [-1.0, -1.0], tex: [0.0, 1.0] },
                            Vertex { pos: [-1.0,  1.0], tex: [0.0, 0.0] },
                            Vertex { pos: [ 1.0, -1.0], tex: [1.0, 1.0] },
                            Vertex { pos: [ 1.0,  1.0], tex: [1.0, 0.0] },
                        ];

                        let vb_desc = D3D11_BUFFER_DESC {
                            ByteWidth: std::mem::size_of_val(&vertices) as u32,
                            Usage: D3D11_USAGE_DEFAULT,
                            BindFlags: D3D11_BIND_VERTEX_BUFFER.0 as u32,
                            CPUAccessFlags: 0,
                            MiscFlags: 0,
                            StructureByteStride: 0,
                        };

                        let vb_data = D3D11_SUBRESOURCE_DATA {
                            pSysMem: vertices.as_ptr() as *const _,
                            SysMemPitch: 0,
                            SysMemSlicePitch: 0,
                        };

                        let mut vertex_buffer = None;
                        if device.as_ref().unwrap().CreateBuffer(&vb_desc, Some(&vb_data), Some(&mut vertex_buffer as *mut _)).is_ok() {
                            window_state.vertex_buffer = vertex_buffer;
                            println!("Γ£à Vertex buffer created!");
                        }

                        // Create sampler state
                        let sampler_desc = D3D11_SAMPLER_DESC {
                            Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                            AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                            AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                            AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                            MipLODBias: 0.0,
                            MaxAnisotropy: 1,
                            ComparisonFunc: D3D11_COMPARISON_NEVER,
                            BorderColor: [0.0, 0.0, 0.0, 0.0],
                            MinLOD: 0.0,
                            MaxLOD: f32::MAX,
                        };

                        let mut sampler_state = None;
                        if device.as_ref().unwrap().CreateSamplerState(&sampler_desc, Some(&mut sampler_state as *mut _)).is_ok() {
                            window_state.sampler_state = sampler_state;
                            println!("Γ£à Sampler state created!");
                        }
                    } else {
                        println!("Γ¥î Failed to create swap chain");
                    }

                    // Note: We'll get the shared texture handle lazily on first render
                    // GPUI creates the shared texture during its first draw call
                    println!("≡ƒÆí Shared texture will be retrieved on first render");
                } else {
                    println!("Γ¥î Failed to create D3D11 device: {:?}", result);
                }
            }

            window_state.gpui_window_initialized = true;
            println!("Γ£à GPUI window opened! Ready for GPU composition!\n");
        }
        }
    }
}

impl WinitGpuiApp {
    // Helper to convert KeyCode to string (static so it can be used without &self borrow)
    fn keycode_to_string_static(code: KeyCode) -> Option<String> {
        use KeyCode::*;
        Some(match code {
            // Letters
            KeyA => "a",
            KeyB => "b",
            KeyC => "c",
            KeyD => "d",
            KeyE => "e",
            KeyF => "f",
            KeyG => "g",
            KeyH => "h",
            KeyI => "i",
            KeyJ => "j",
            KeyK => "k",
            KeyL => "l",
            KeyM => "m",
            KeyN => "n",
            KeyO => "o",
            KeyP => "p",
            KeyQ => "q",
            KeyR => "r",
            KeyS => "s",
            KeyT => "t",
            KeyU => "u",
            KeyV => "v",
            KeyW => "w",
            KeyX => "x",
            KeyY => "y",
            KeyZ => "z",
            
            // Numbers
            Digit0 => "0",
            Digit1 => "1",
            Digit2 => "2",
            Digit3 => "3",
            Digit4 => "4",
            Digit5 => "5",
            Digit6 => "6",
            Digit7 => "7",
            Digit8 => "8",
            Digit9 => "9",
            
            // Special keys
            Space => "space",
            Enter => "enter",
            Tab => "tab",
            Backspace => "backspace",
            Escape => "escape",
            Delete => "delete",
            Insert => "insert",
            Home => "home",
            End => "end",
            PageUp => "pageup",
            PageDown => "pagedown",
            
            // Arrow keys
            ArrowUp => "up",
            ArrowDown => "down",
            ArrowLeft => "left",
            ArrowRight => "right",
            
            // Function keys
            F1 => "f1",
            F2 => "f2",
            F3 => "f3",
            F4 => "f4",
            F5 => "f5",
            F6 => "f6",
            F7 => "f7",
            F8 => "f8",
            F9 => "f9",
            F10 => "f10",
            F11 => "f11",
            F12 => "f12",
            
            // Punctuation and symbols
            Minus => "-",
            Equal => "=",
            BracketLeft => "[",
            BracketRight => "]",
            Backslash => "\\",
            Semicolon => ";",
            Quote => "'",
            Comma => ",",
            Period => ".",
            Slash => "/",
            Backquote => "`",
            
            _ => return None,
        }.to_string())
    }
}

struct DemoView {
    counter: usize,
}

impl DemoView {
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self { counter: 0 }
    }
}

impl Render for DemoView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.counter += 1;

        // Transparent background - let Winit's green show through
        div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .size_full()
            .gap_4()
            .child(
                // Small blue square to show GPUI is rendering
                div()
                    .size(px(200.0))
                    .bg(rgb(0x4A90E2))
                    .rounded_lg()
                    .shadow_lg()
                    .border_2()
                    .border_color(rgb(0xFFFFFF))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .text_2xl()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(0xFFFFFF))
                            .child("GPUI"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .text_xl()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(rgb(0x333333))
                            .child(format!("Frame: {}", self.counter)),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0x666666))
                            .child("Γ£à GPUI rendering on Winit window!"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0x666666))
                            .child("≡ƒÄ¿ Zero-copy GPU composition"),
                    ),
            )
    }
}
