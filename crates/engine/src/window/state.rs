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
//! │                                         │
//! │ Event Tracking:                        │
//! │  - last_cursor_position                │
//! │  - motion_smoother                     │
//! │  - current_modifiers                   │
//! │  - pressed_mouse_buttons               │
//! │  - click_state                         │
//! │                                         │
//! │ D3D11 Rendering (Windows):             │
//! │  - d3d_device, d3d_context             │
//! │  - shared_texture, swap_chain          │
//! │  - shaders, buffers                    │
//! │                                         │
//! │ 3D Rendering:                          │
//! │  - bevy_renderer (optional)            │
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

    // ===== GPU Compositor (Cross-Platform) =====

    /// Platform-specific compositor for zero-copy GPU composition
    /// - Windows: D3D11 compositor
    /// - Linux: Vulkan compositor
    /// - macOS: Metal compositor
    pub compositor: Option<Box<dyn crate::window::compositor::Compositor>>,

    // ===== 3D Rendering =====

    /// Bevy renderer for this window (if it has a 3D viewport)
    pub bevy_renderer: Option<Arc<std::sync::Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>>,
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

            // GPU compositor (will be initialized when window is created)
            compositor: None,

            // Bevy renderer
            bevy_renderer: None,
        }
    }
}
