//! Cross-Platform GPU Compositor
//!
//! This module provides a platform-agnostic abstraction for compositing multiple GPU textures
//! into a single window. It handles the zero-copy blending of:
//! - Bevy 3D rendering (from shared GPU texture)
//! - GPUI UI layer (with alpha blending)
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────┐
//! │           Compositor Trait                          │
//! │  (Platform-agnostic interface)                      │
//! ├─────────────────────────────────────────────────────┤
//! │ • init() - Setup rendering pipeline                 │
//! │ • begin_frame() - Clear and prepare frame           │
//! │ • composite_bevy() - Render 3D layer (opaque)       │
//! │ • composite_gpui() - Render UI layer (alpha blend)  │
//! │ • present() - Display final result                  │
//! │ • resize() - Handle window resize                   │
//! └─────────────────────────────────────────────────────┘
//!              ↓              ↓              ↓
//!     ┌────────────┐  ┌────────────┐  ┌────────────┐
//!     │   D3D11    │  │   Metal    │  │   Vulkan   │
//!     │  Windows   │  │   macOS    │  │   Linux    │
//!     └────────────┘  └────────────┘  └────────────┘
//! ```
//!
//! ## Platform Support
//!
//! - **Windows**: Direct3D 11 with swap chain, shader-based alpha blending
//! - **macOS**: Metal with CAMetalLayer, IOSurface texture sharing
//! - **Linux**: Vulkan with DMA-BUF, X11/Wayland presentation
//!
//! ## Usage
//!
//! ```rust,ignore
//! let compositor = Compositor::new(window, size)?;
//!
//! loop {
//!     compositor.begin_frame()?;
//!
//!     if let Some(bevy_handle) = bevy_renderer.get_texture_handle() {
//!         compositor.composite_bevy(bevy_handle)?;
//!     }
//!
//!     if let Some(gpui_handle) = gpui_window.get_shared_texture_handle()? {
//!         compositor.composite_gpui(gpui_handle)?;
//!     }
//!
//!     compositor.present()?;
//! }
//! ```

use anyhow::Result;
use engine_backend::subsystems::render::NativeTextureHandle;
use gpui::SharedTextureHandle;

// Cross-platform wgpu-based implementation
pub mod wgpu;

// Re-export wgpu compositor as the platform compositor
pub use wgpu::WgpuCompositor as PlatformCompositor;

/// Compositor state for tracking render requirements
#[derive(Debug, Clone, Copy)]
pub struct CompositorState {
    /// Width of the rendering surface
    pub width: u32,
    /// Height of the rendering surface
    pub height: u32,
    /// Scale factor for HiDPI displays
    pub scale_factor: f32,
    /// Whether the compositor needs to render this frame
    pub needs_render: bool,
}

/// Cross-platform GPU compositor trait
///
/// Implementations handle platform-specific details of:
/// - Creating rendering surfaces and swap chains
/// - Opening shared textures from GPUI and Bevy
/// - Compositing multiple layers with alpha blending
/// - Presenting final frames to the screen
pub trait Compositor: Send {
    /// Initialize the compositor with platform-specific resources
    ///
    /// # Arguments
    /// * `window` - Platform window handle for creating swap chain/surface
    /// * `width` - Initial width in physical pixels
    /// * `height` - Initial height in physical pixels
    /// * `scale_factor` - Display scale factor for HiDPI
    ///
    /// # Returns
    /// Ok(()) on success, or an error if initialization fails
    fn init(
        window: &(impl raw_window_handle::HasWindowHandle + raw_window_handle::HasDisplayHandle),
        width: u32,
        height: u32,
        scale_factor: f32,
    ) -> Result<Self>
    where
        Self: Sized;

    /// Begin a new frame, clearing the back buffer
    ///
    /// This should be called at the start of each frame before any composition.
    fn begin_frame(&mut self) -> Result<()>;

    /// Composite the Bevy 3D rendering layer (opaque, no blending)
    ///
    /// # Arguments
    /// * `handle` - Native GPU texture handle from Bevy renderer
    ///
    /// # Returns
    /// Ok(()) on success, None if texture not ready yet
    fn composite_bevy(&mut self, handle: &NativeTextureHandle) -> Result<Option<()>>;

    /// Composite the GPUI UI layer (transparent, alpha-blended on top)
    ///
    /// # Arguments
    /// * `handle` - Shared texture handle from GPUI
    /// * `should_render` - Whether GPUI actually rendered this frame (for frame persistence)
    ///
    /// # Returns
    /// Ok(()) on success
    fn composite_gpui(&mut self, handle: &SharedTextureHandle, should_render: bool) -> Result<()>;

    /// Present the composited frame to the screen
    ///
    /// # Returns
    /// Ok(()) on success, or an error if present fails
    fn present(&mut self) -> Result<()>;

    /// Resize the compositor's rendering surface
    ///
    /// Called when the window is resized. This typically recreates the swap chain
    /// and render targets to match the new size.
    ///
    /// # Arguments
    /// * `width` - New width in physical pixels
    /// * `height` - New height in physical pixels
    ///
    /// # Returns
    /// Ok(()) on success
    fn resize(&mut self, width: u32, height: u32) -> Result<()>;

    /// Get the current compositor state
    fn state(&self) -> &CompositorState;

    /// Get mutable compositor state
    fn state_mut(&mut self) -> &mut CompositorState;
}

/// Create a platform-specific compositor instance
///
/// This is a convenience function that creates the appropriate compositor
/// for the current platform.
///
/// # Arguments
/// * `window` - Platform window handle
/// * `width` - Initial width in physical pixels
/// * `height` - Initial height in physical pixels
/// * `scale_factor` - Display scale factor
///
/// # Returns
/// A boxed compositor instance ready for use
pub fn create_compositor(
    window: &(impl raw_window_handle::HasWindowHandle + raw_window_handle::HasDisplayHandle),
    width: u32,
    height: u32,
    scale_factor: f32,
) -> Result<Box<dyn Compositor>> {
    Ok(Box::new(PlatformCompositor::init(
        window,
        width,
        height,
        scale_factor,
    )?))
}
