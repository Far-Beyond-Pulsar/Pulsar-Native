//! Compatibility shim for GPUI types that were removed upstream.
//!
//! We replicate minimal behavior so that the rest of the engine can continue
//! using the same API even though the official `gpui` crate no longer exports
//! these definitions.  Eventually these can be replaced with a more robust
//! solution or upstreamed back into gpui.

use gpui;
use gpui::IntoElement; // needed for `.into_any_element()`

/// Simple handle wrapper representing a native GPU texture that can be shared
/// with a GPUI-like canvas.
#[derive(Clone, Debug)]
pub struct GpuTextureHandle {
    /// Native OS handle (NT handle, IOSurface ID, dma-buf fd, etc.)
    pub handle: isize,
    /// Width of the texture in pixels.
    pub width: u32,
    /// Height of the texture in pixels.
    pub height: u32,
}

impl GpuTextureHandle {
    /// Construct a new handle wrapper.  This mirrors the old GPUI API so our
    /// code can remain unchanged.
    pub fn new(handle: isize, width: u32, height: u32) -> Self {
        Self { handle, width, height }
    }
}

/// Source of two double‑buffered textures that a canvas element can read from.
///
/// The real GPUI implementation did a fair amount of VULKAN/DXGI/Metal magic;
/// here we provide a no‑op stub that satisfies the interface until we add a
/// proper renderer side.
#[derive(Clone, Debug)]
pub struct GpuCanvasSource {
    handles: [GpuTextureHandle; 2],
    active: usize,
}

impl GpuCanvasSource {
    /// Create a new source with the two texture handles.
    pub fn new(h0: GpuTextureHandle, h1: GpuTextureHandle) -> Self {
        Self { handles: [h0, h1], active: 0 }
    }

    /// Swap the active buffer (no‑op for now).
    pub fn swap_buffers(&self) {
        // previously this told GPUI to swap which texture it was reading from;
        // our stub is empty because we don't actually render through GPUI here.
    }

    /// Choose which buffer index the canvas should read from (0 or 1).
    pub fn set_active_buffer(&self, _index: usize) {
        // stubbed
    }
}

/// Replacement for the old `gpui::gpu_canvas()` helper.  The real function
/// returned a specialised canvas element; we simply return an empty div so that
/// the type checks pass.
pub fn gpu_canvas() -> impl gpui::IntoElement {
    gpui::div().into_any_element()
}
