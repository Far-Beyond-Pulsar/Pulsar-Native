//! Window Management Module
//!
//! This module handles the integration between Winit (OS windowing) and GPUI (UI framework).
//! It provides a multi-window architecture with zero-copy GPU composition on Windows.
//!
//! ## Architecture
//!
//! ```text
//! ┌──────────────────────────────────────────┐
//! │         WinitGpuiApp                     │
//! │  (ApplicationHandler for Winit)          │
//! ├──────────────────────────────────────────┤
//! │ windows: HashMap<WindowId, WindowState>  │
//! │ engine_state: EngineState                │
//! │ window_request_rx: Receiver              │
//! └──────────────────────────────────────────┘
//!              │
//!              ├─── WindowState (per window)
//!              │    ├─ Winit window handle
//!              │    ├─ GPUI application
//!              │    ├─ D3D11 rendering state
//!              │    └─ Event tracking
//!              │
//!              └─── Event Flow:
//!                   Winit → Conversion → Motion Smoothing → GPUI
//! ```
//!
//! ## Modules
//!
//! - `state` - Per-window state management
//! - `app` - Main application handler (WinitGpuiApp)
//! - `events` - Event conversion and utilities
//! - `d3d11` - Direct3D 11 rendering (Windows only)
//! - `handlers` - Application lifecycle and event dispatching
//! - `input` - Input event handling (keyboard, mouse, modifiers)
//! - `rendering` - Rendering and composition (will contain compositor, Bevy integration)
//! - `initialization` - Window and rendering initialization logic
//!
//! ## Zero-Copy Composition (Windows)
//!
//! On Windows, we use Direct3D 11 shared textures for efficient rendering:
//!
//! 1. **Bevy** renders 3D content to D3D12 shared texture (bottom layer, opaque)
//! 2. **GPUI** renders UI to D3D11 shared texture (top layer, alpha-blended)
//! 3. **D3D11** composites both textures to swap chain back buffer
//! 4. No CPU-GPU data transfers required
//!
//! ## Usage
//!
//! ```rust,ignore
//! use window::WinitGpuiApp;
//!
//! let event_loop = EventLoop::new()?;
//! let mut app = WinitGpuiApp::new(engine_state, window_rx);
//! event_loop.run_app(&mut app)?;
//! ```

pub mod app;
pub mod d3d11;
pub mod events;
pub mod state;

// New modular structure
pub mod handlers;
pub mod input;
pub mod rendering;
pub mod initialization;
mod window_id_map;

pub use app::WinitGpuiApp;
pub use events::{
    convert_modifiers, convert_mouse_button, MotionSmoother, SimpleClickState,
    ToGpuiMouseButton, ToGpuiModifiers,
};
pub use state::WindowState;
pub use window_id_map::WindowIdMap;

