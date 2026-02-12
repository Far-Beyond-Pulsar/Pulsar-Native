//! Window State Management
//!
//! This module defines the per-window state structure that holds all data needed
//! for a single window, including GPUI app, rendering state, and event tracking.
//!
//! ## Architecture
//!
//! Each window in the engine has completely independent state:
//!
//! ```text
//! ┌────────────────────────────────────────┐
//! │          WindowState                   │
//! ├────────────────────────────────────────┤
//! │ Core Components:                       │
//! │  - winit_window: Arc<WinitWindow>      │
//! │  - gpui_app: Application               │
//! │  - gpui_window: WindowHandle<Root>     │
//! │                                        │
//! │ Event Tracking:                        │
//! │  - last_cursor_position                │
//! │  - motion_smoother                     │
//! │  - current_modifiers                   │
//! │  - pressed_mouse_buttons               │
//! │  - click_state                         │
//! │                                        │
//! │ D3D11 Rendering (Windows):             │
//! │  - d3d_device, d3d_context             │
//! │  - shared_texture, swap_chain          │
//! │  - shaders, buffers                    │
//! │                                        │
//! │ 3D Rendering:                          │
//! │  - helio_renderer (optional)            │
//! └────────────────────────────────────────┘
//! ```
//!
//! ## Usage
//!
//! ```rust,ignore
//! let window_state = WindowState::new(winit_window);
//! window_state.window_type = Some(WindowRequest::Settings);
//! ```

use crate::assets::Assets;
use engine_state::WindowRequest;
use crate::window::events::{MotionSmoother, SimpleClickState};
use gpui::*;
use ui::Root;
use std::collections::HashSet;
use std::sync::Arc;
use winit::window::Window as WinitWindow;

#[cfg(target_os = "windows")]
use windows::Win32::Graphics::{Direct3D11::*, Dxgi::*};

/// Per-window state for each independent window
///
/// Each window in the engine has its own independent state, including:
/// - Winit window handle for OS-level window management
/// - GPUI application instance for UI rendering
/// - Direct3D 11 rendering pipeline (Windows only)
/// - Event tracking (mouse, keyboard, click detection)
/// - Optional Bevy renderer for 3D viewports
///
/// ## Lifecycle
///
/// 1. Created when window is created (via `new()`)
/// 2. GPUI components initialized in `about_to_wait()`
/// 3. D3D11 rendering setup (Windows only)
/// 4. Active event processing
/// 5. Cleaned up when window closes
pub struct WindowState {
    // ===== Core Window Components =====

    /// Winit window handle (Arc for cheap cloning)
    pub winit_window: Arc<WinitWindow>,

    /// GPUI application instance (independent per window)
    pub gpui_app: Application,

    /// GPUI window handle (once initialized)
    pub gpui_window: Option<WindowHandle<Root>>,

    /// Whether GPUI window has been initialized
    pub gpui_window_initialized: bool,

    /// Whether this window needs to render on next frame
    pub needs_render: bool,

    /// Type of window (Settings, ProjectEditor, etc.)
    pub window_type: Option<WindowRequest>,

    // ===== Event Tracking State =====

    /// Last known cursor position (logical pixels)
    pub last_cursor_position: Point<Pixels>,

    /// Motion smoother for high-quality mouse input
    pub motion_smoother: MotionSmoother,

    /// Current keyboard modifier state
    pub current_modifiers: Modifiers,

    /// Set of currently pressed mouse buttons
    pub pressed_mouse_buttons: HashSet<MouseButton>,

    /// Click state tracker for double-click detection
    pub click_state: SimpleClickState,

    // ===== Direct3D 11 Rendering State (Windows only) =====

    #[cfg(target_os = "windows")]
    /// D3D11 device for rendering
    pub d3d_device: Option<ID3D11Device>,

    #[cfg(target_os = "windows")]
    /// D3D11 immediate context
    pub d3d_context: Option<ID3D11DeviceContext>,

    #[cfg(target_os = "windows")]
    /// Shared texture from GPUI (for composition)
    pub shared_texture: Option<ID3D11Texture2D>,

    #[cfg(target_os = "windows")]
    /// Whether shared texture has been initialized
    pub shared_texture_initialized: bool,

    #[cfg(target_os = "windows")]
    /// Swap chain for presenting to window
    pub swap_chain: Option<IDXGISwapChain1>,

    #[cfg(target_os = "windows")]
    /// Blend state for alpha compositing
    pub blend_state: Option<ID3D11BlendState>,

    #[cfg(target_os = "windows")]
    /// Render target view (swap chain back buffer)
    pub render_target_view: Option<ID3D11RenderTargetView>,

    #[cfg(target_os = "windows")]
    /// Vertex shader for fullscreen quad
    pub vertex_shader: Option<ID3D11VertexShader>,

    #[cfg(target_os = "windows")]
    /// Pixel shader for texture sampling
    pub pixel_shader: Option<ID3D11PixelShader>,

    #[cfg(target_os = "windows")]
    /// Vertex buffer for fullscreen quad
    pub vertex_buffer: Option<ID3D11Buffer>,

    #[cfg(target_os = "windows")]
    /// Input layout for vertex shader
    pub input_layout: Option<ID3D11InputLayout>,

    #[cfg(target_os = "windows")]
    /// Sampler state for texture filtering
    pub sampler_state: Option<ID3D11SamplerState>,

    #[cfg(target_os = "windows")]
    /// Persistent GPUI texture (cached copy)
    pub persistent_gpui_texture: Option<ID3D11Texture2D>,

    #[cfg(target_os = "windows")]
    /// Shader resource view for GPUI texture (cached)
    pub persistent_gpui_srv: Option<ID3D11ShaderResourceView>,

    #[cfg(target_os = "windows")]
    /// Bevy 3D texture (if viewport active)
    pub bevy_texture: Option<ID3D11Texture2D>,

    #[cfg(target_os = "windows")]
    /// Shader resource view for Bevy texture
    pub bevy_srv: Option<ID3D11ShaderResourceView>,

    // ===== 3D Rendering =====

    /// Bevy renderer for this window (if it has a 3D viewport)
    pub helio_renderer: Option<Arc<std::sync::Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>>,
}

impl WindowState {
    /// Create a new window state with default initialization
    ///
    /// Sets up a new window with:
    /// - GPUI application instance with embedded assets
    /// - Default event tracking state
    /// - Uninitialized rendering state (will be setup later)
    ///
    /// # Arguments
    /// * `winit_window` - Arc to the Winit window handle
    ///
    /// # Returns
    /// New WindowState ready for initialization
    pub fn new(winit_window: Arc<WinitWindow>) -> Self {
        Self {
            // Core components
            winit_window,
            gpui_app: Application::new().with_assets(Assets),
            gpui_window: None,
            gpui_window_initialized: false,
            needs_render: true,
            window_type: None,

            // Event tracking
            last_cursor_position: point(px(0.0), px(0.0)),
            motion_smoother: MotionSmoother::new(),
            current_modifiers: Modifiers::default(),
            pressed_mouse_buttons: HashSet::new(),
            click_state: SimpleClickState::new(),

            // D3D11 rendering state (Windows)
            #[cfg(target_os = "windows")]
            d3d_device: None,
            #[cfg(target_os = "windows")]
            d3d_context: None,
            #[cfg(target_os = "windows")]
            shared_texture: None,
            #[cfg(target_os = "windows")]
            shared_texture_initialized: false,
            #[cfg(target_os = "windows")]
            swap_chain: None,
            #[cfg(target_os = "windows")]
            blend_state: None,
            #[cfg(target_os = "windows")]
            render_target_view: None,
            #[cfg(target_os = "windows")]
            vertex_shader: None,
            #[cfg(target_os = "windows")]
            pixel_shader: None,
            #[cfg(target_os = "windows")]
            vertex_buffer: None,
            #[cfg(target_os = "windows")]
            input_layout: None,
            #[cfg(target_os = "windows")]
            sampler_state: None,
            #[cfg(target_os = "windows")]
            persistent_gpui_texture: None,
            #[cfg(target_os = "windows")]
            persistent_gpui_srv: None,
            #[cfg(target_os = "windows")]
            bevy_texture: None,
            #[cfg(target_os = "windows")]
            bevy_srv: None,

            // Bevy renderer
            helio_renderer: None,
        }
    }
}
