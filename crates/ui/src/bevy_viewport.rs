/// Zero-Copy Bevy Viewport for GPUI
///
/// This viewport displays Bevy-rendered content using GPUI's gpu_canvas element.
/// It achieves true zero-copy rendering by sharing GPU textures between Bevy (DX12)
/// and GPUI (DX11) using DXGI shared resources.
///
/// Architecture:
/// - Bevy renders to double-buffered shared DX12 textures in background thread
/// - GPUI opens these textures via NT handles and displays them directly
/// - No CPU copies, no staging buffers - pure GPU-to-GPU sharing
/// - Cross-platform ready (Windows: DXGI, macOS: IOSurface, Linux: DMA-BUF)

use gpui::*;
use gpui::prelude::FluentBuilder;
use std::sync::Arc;

// Re-export GPU canvas types from gpui
pub use gpui::{GpuCanvasSource, GpuTextureHandle, gpu_canvas as gpu_canvas_element};

/// Bevy viewport state that can be shared across threads
#[derive(Clone)]
pub struct BevyViewportState {
    /// The GPU canvas source providing double-buffered textures
    canvas_source: Option<GpuCanvasSource>,
    /// Viewport dimensions
    width: u32,
    height: u32,
}

impl BevyViewportState {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            canvas_source: None,
            width,
            height,
        }
    }

    /// Initialize with shared texture handles from Bevy renderer
    /// Universal zero-copy texture initialization (cross-platform!)
    ///
    /// # Arguments
    /// * `handle0` - Native GPU memory handle for buffer 0 (NT handle/IOSurface ID/dma-buf FD)
    /// * `handle1` - Native GPU memory handle for buffer 1 (NT handle/IOSurface ID/dma-buf FD)
    /// * `width` - Texture width
    /// * `height` - Texture height
    ///
    /// All platforms use the same RGBA8 byte format - handles are just different OS-level
    /// ways to reference the same GPU memory!
    pub fn initialize_shared_textures(&mut self, handle0: isize, handle1: isize, width: u32, height: u32) {
        // Create universal GPU texture handles
        let buffer0 = GpuTextureHandle::new(handle0, width, height);
        let buffer1 = GpuTextureHandle::new(handle1, width, height);

        self.canvas_source = Some(GpuCanvasSource::new(buffer0, buffer1));
        self.width = width;
        self.height = height;

        #[cfg(target_os = "windows")]
        tracing::debug!("[BEVY-VIEWPORT] ✅ Initialized with DirectX shared textures {}x{}", width, height);
        #[cfg(target_os = "macos")]
        tracing::debug!("[BEVY-VIEWPORT] ✅ Initialized with Metal IOSurface {}x{}", width, height);
        #[cfg(target_os = "linux")]
        tracing::debug!("[BEVY-VIEWPORT] ✅ Initialized with Vulkan dma-buf {}x{}", width, height);

        tracing::debug!("[BEVY-VIEWPORT] 🔥 Zero-copy GPU rendering enabled! (Universal RGBA8 format)");
    }

    /// Notify that Bevy has finished rendering a frame (swaps the active buffer)
    pub fn swap_buffers(&self) {
        if let Some(ref source) = self.canvas_source {
            source.swap_buffers();
        }
    }

    /// Get the canvas source for rendering
    pub fn canvas_source(&self) -> Option<&GpuCanvasSource> {
        self.canvas_source.as_ref()
    }
    
    /// Set which buffer index to read from (0 or 1)
    pub fn set_active_buffer(&mut self, index: usize) {
        if let Some(ref source) = self.canvas_source {
            source.set_active_buffer(index);
        }
    }

    /// Resize the viewport
    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
    }

    /// Get viewport dimensions
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

/// Bevy Viewport Component
/// 
/// A GPUI element that displays Bevy-rendered content with zero CPU copies.
/// This is a standard GPUI component that integrates seamlessly with the UI.
pub struct BevyViewport {
    state: Arc<parking_lot::RwLock<BevyViewportState>>,
    object_fit: ObjectFit,
    focus_handle: FocusHandle,
}

impl BevyViewport {
    /// Create a new Bevy viewport
    pub fn new<V: 'static>(width: u32, height: u32, cx: &mut Context<V>) -> Self {
        tracing::debug!("[BEVY-VIEWPORT] 🚀 Creating viewport {}x{}", width, height);
        
        Self {
            state: Arc::new(parking_lot::RwLock::new(BevyViewportState::new(width, height))),
            object_fit: ObjectFit::Contain,
            focus_handle: cx.focus_handle(),
        }
    }

    /// Get shared state for background thread access
    /// Use this to initialize textures and signal frame swaps from the Bevy render thread
    pub fn shared_state(&self) -> Arc<parking_lot::RwLock<BevyViewportState>> {
        self.state.clone()
    }

    /// Set how the rendered content should fit within the viewport bounds
    pub fn object_fit(mut self, fit: ObjectFit) -> Self {
        self.object_fit = fit;
        self
    }

    /// Resize the viewport
    pub fn resize(&mut self, width: u32, height: u32) {
        self.state.write().resize(width, height);
    }

    /// Get current dimensions
    pub fn dimensions(&self) -> (u32, u32) {
        self.state.read().dimensions()
    }
}

impl Focusable for BevyViewport {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissEvent> for BevyViewport {}

impl Render for BevyViewport {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Request continuous animation frames for live Bevy rendering updates
        // This creates a smooth render loop at the display's refresh rate
        window.request_animation_frame();
        
        let state = self.state.read();
        
        // Return a TRANSPARENT div so we can see through to the winit green background!
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            // TRANSPARENT - no background! This creates a "hole" to see winit's green bg
            .track_focus(&self.focus_handle)
            // Make this element explicitly focusable for keyboard input
            .id("bevy_viewport")
            // NO mouse event handlers here - let them pass through to parent div in viewport.rs!
            .child(
                // TRANSPARENT DIV - Bevy renders to winit back buffer BEHIND this layer
                // This creates a "hole" in the GPUI UI to see through to Bevy's rendering
                div()
                    .size_full()
                    .into_any_element()
            )
    }
}

/// Example integration with Bevy backend:
/// 
/// ```rust,ignore
/// // In your UI code:
/// let viewport = BevyViewport::new(1600, 900, cx);
/// let viewport_state = viewport.shared_state();
/// 
/// // Start Bevy renderer in background thread:
/// std::thread::spawn(move || {
///     let helio_renderer = BevyRenderer::new(1600, 900).await;
///     
///     // Get the shared texture handles (platform-specific)
///     #[cfg(target_os = "windows")]
///     let (handle0, handle1) = helio_renderer.get_shared_nt_handles();
///     
///     // Initialize the viewport with these handles
///     viewport_state.write().initialize_shared_textures(
///         handle0, handle1, 1600, 900
///     );
///     
///     // Main render loop
///     loop {
///         helio_renderer.render_frame();
///         viewport_state.read().swap_buffers();
///         std::thread::sleep(Duration::from_millis(16)); // ~60 FPS
///     }
/// });
/// 
/// // Use viewport in your UI:
/// viewport.object_fit(ObjectFit::Cover)
/// ```
#[allow(dead_code)]
const _EXAMPLE_DOC: () = ();
