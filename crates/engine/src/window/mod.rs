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
//! - `compositor` - Cross-platform GPU compositor (Windows/macOS/Linux)
//! - `d3d11` - Direct3D 11 rendering (Windows only) [DEPRECATED - use compositor]
//!
//! ## Zero-Copy Composition (Cross-Platform)
//!
//! We use platform-specific shared texture APIs for efficient rendering:
//!
//! **Windows (D3D11)**:
//! 1. **Bevy** renders 3D content to D3D12 shared texture (bottom layer, opaque)
//! 2. **GPUI** renders UI to D3D11 shared texture (top layer, alpha-blended)
//! 3. **D3D11 Compositor** composites both textures to swap chain
//!
//! **macOS (Metal)**:
//! 1. **Bevy** renders to Metal texture backed by IOSurface
//! 2. **GPUI** exposes rendering buffer as IOSurface
//! 3. **Metal Compositor** composites IOSurfaces to CAMetalLayer
//!
//! **Linux (Vulkan)**:
//! 1. **Bevy** exports Vulkan VkImage as DMA-BUF file descriptor
//! 2. **GPUI** exports Blade renderer texture as DMA-BUF
//! 3. **Vulkan Compositor** imports DMA-BUFs and composites to swapchain
//!
//! All platforms achieve true zero-copy - no CPU-GPU data transfers required!
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
pub mod compositor;
pub mod d3d11;
pub mod events;
pub mod state;

pub use app::WinitGpuiApp;
pub use compositor::Compositor;
pub use events::{convert_modifiers, convert_mouse_button, MotionSmoother, SimpleClickState};
pub use state::WindowState;

