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
//!          ├─── window_event() → Delegates to handlers::events
//!          ├─── resumed() → Delegates to handlers::lifecycle
//!          └─── about_to_wait() → Delegates to handlers::lifecycle
//! ```
//!
//! ## Responsibilities
//!
//! - **Window Management**: Create, track, and destroy multiple independent windows
//! - **Event Routing**: Delegate events to specialized handler modules
//! - **D3D11 Integration**: Coordinate D3D11 rendering pipeline (Windows)
//! - **GPUI Initialization**: Coordinate GPUI application and window setup
//! - **Lifecycle Management**: Handle window creation requests and cleanup
//!
//! ## Modular Architecture
//!
//! Event handling is now delegated to specialized modules:
//! - `handlers::lifecycle` - Application start and idle events
//! - `handlers::events` - Main event dispatcher
//! - `input::keyboard` - Keyboard event handling
//! - `input::mouse` - Mouse event handling
//! - `input::modifiers` - Modifier state tracking
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
use raw_window_handle::HasWindowHandle;
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
use raw_window_handle::RawWindowHandle;

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
///
/// **Note**: Fields are `pub(crate)` to allow access from handler modules within
/// the `window` module while remaining private to external code.
pub struct WinitGpuiApp {
    pub(crate) windows: HashMap<WindowId, WindowState>,
    pub(crate) engine_state: EngineState,
    pub(crate) window_request_rx: Receiver<WindowRequest>,
    pub(crate) pending_window_requests: Vec<WindowRequest>,
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
    ///
    /// **Note**: This method is `pub(crate)` to allow access from lifecycle handlers
    pub(crate) fn create_window(&mut self, event_loop: &ActiveEventLoop, request: WindowRequest) {
        let (title, size) = match &request {
            WindowRequest::Entry => ("Pulsar Engine", (1280.0, 720.0)),
            WindowRequest::Settings => ("Settings", (800.0, 600.0)),
            WindowRequest::About => ("About Pulsar Engine", (600.0, 500.0)),
            WindowRequest::Documentation => ("Documentation", (1400.0, 900.0)),
            WindowRequest::ProjectEditor { .. } => ("Pulsar Engine - Project Editor", (1920.0, 1080.0)),
            WindowRequest::ProjectSplash { .. } => ("Loading Project...", (960.0, 540.0)),
            WindowRequest::CloseWindow { .. } => return, // Handled elsewhere
        };

        tracing::debug!("≡ƒ¬ƒ [CREATE-WINDOW] Creating new window: {} (type: {:?})", title, request);

        let mut window_attributes = WinitWindow::default_attributes()
            .with_title(title)
            .with_inner_size(winit::dpi::LogicalSize::new(size.0, size.1))
            .with_transparent(false)
            .with_decorations(false) // Use custom titlebar instead of OS decorations
            .with_resizable(true); // Enable resize for borderless window

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

        tracing::debug!("Γ£à Window created: {} (total windows: {})", title, self.engine_state.window_count());
    }
}

impl ApplicationHandler for WinitGpuiApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // Delegate to lifecycle handler
        crate::window::handlers::lifecycle::handle_resumed(self, event_loop);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        // Delegate ALL event handling to the modular event dispatcher
        crate::window::handlers::events::dispatch_window_event(self, event_loop, window_id, event);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Delegate to lifecycle handler
        crate::window::handlers::lifecycle::handle_about_to_wait(self, event_loop);
    }
}
