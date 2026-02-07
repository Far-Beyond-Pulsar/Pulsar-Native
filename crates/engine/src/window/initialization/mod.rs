//! Window and rendering initialization module
//!
//! This module handles initialization of GPUI windows, D3D11 rendering pipelines,
//! and window-type-specific content creation.
//!
//! # New Trait-Based System (Phase 1)
//!
//! The `WindowInitializer` trait provides a modular, plugin-extensible way to create windows.
//! This will eventually replace the procedural initialization functions below.

pub mod gpui;
pub mod d3d11;
pub mod window_content;

// Old initialization functions (to be deprecated in Phase 3)
pub use gpui::initialize_gpui_window;
pub use d3d11::initialize_d3d11_pipeline;
pub use window_content::create_window_content;

// --- New Trait-Based System ---

use crate::window::state::WindowState;
use engine_state::{EngineContext, WindowRequest};
use winit::event_loop::ActiveEventLoop;
use thiserror::Error;

/// Errors that can occur during window initialization
#[derive(Debug, Error)]
pub enum WindowInitError {
    #[error("Failed to create Winit window: {0}")]
    WinitCreationFailed(String),

    #[error("Failed to initialize GPUI: {0}")]
    GpuiInitFailed(String),

    #[error("Failed to initialize D3D11: {0}")]
    D3d11InitFailed(String),

    #[error("Failed to create window content: {0}")]
    ContentCreationFailed(String),

    #[error("Window request type not supported by this initializer")]
    UnsupportedWindowType,

    #[error("Generic initialization error: {0}")]
    Other(String),
}

/// Context provided to window initializers
///
/// Contains all the dependencies needed to create and initialize a window.
pub struct WindowInitContext<'a> {
    /// Winit event loop (for creating windows)
    pub event_loop: &'a ActiveEventLoop,

    /// Engine context (for accessing project, renderers, etc.)
    pub engine_context: &'a EngineContext,

    /// Window ID map (for converting WindowId to u64)
    pub window_id_map: &'a crate::window::WindowIdMap,
}

/// Trait for window initialization strategies
///
/// Implementations of this trait can handle specific window types (Entry, Settings, ProjectEditor, etc.)
/// and are responsible for the complete initialization pipeline:
/// 1. Create Winit window
/// 2. Initialize GPUI (fonts, keybindings, actions)
/// 3. Initialize rendering pipeline (D3D11, etc.)
/// 4. Create window content
///
/// This allows plugins to register custom window types by implementing this trait.
///
/// # Example
///
/// ```ignore
/// struct CustomWindowInitializer;
///
/// impl WindowInitializer for CustomWindowInitializer {
///     fn supports_request(&self, request: &WindowRequest) -> bool {
///         matches!(request, WindowRequest::Custom { .. })
///     }
///
///     fn initialize(&self, request: WindowRequest, context: &WindowInitContext) -> Result<WindowState, WindowInitError> {
///         // Create window state with custom initialization
///         Ok(window_state)
///     }
/// }
/// ```
pub trait WindowInitializer: Send + Sync {
    /// Check if this initializer can handle the given window request
    fn supports_request(&self, request: &WindowRequest) -> bool;

    /// Initialize a window from a request
    ///
    /// This should perform all initialization in a single pass:
    /// - Create the Winit window
    /// - Initialize GPUI
    /// - Initialize rendering pipeline
    /// - Create window content
    ///
    /// Returns a fully initialized WindowState ready to render.
    fn initialize(
        &self,
        request: WindowRequest,
        context: &WindowInitContext,
    ) -> Result<WindowState, WindowInitError>;
}

/// Registry for window initializers
///
/// Allows multiple initializers to be registered (including from plugins)
/// and dispatches to the appropriate one based on the window request type.
pub struct WindowInitializerRegistry {
    initializers: Vec<Box<dyn WindowInitializer>>,
}

impl WindowInitializerRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            initializers: Vec::new(),
        }
    }

    /// Register a window initializer
    pub fn register(&mut self, initializer: Box<dyn WindowInitializer>) {
        self.initializers.push(initializer);
    }

    /// Initialize a window using the first matching initializer
    pub fn initialize(
        &self,
        request: WindowRequest,
        context: &WindowInitContext,
    ) -> Result<WindowState, WindowInitError> {
        for initializer in &self.initializers {
            if initializer.supports_request(&request) {
                return initializer.initialize(request, context);
            }
        }

        Err(WindowInitError::UnsupportedWindowType)
    }
}

impl Default for WindowInitializerRegistry {
    fn default() -> Self {
        Self::new()
    }
}
