//! Core `Renderer` trait and its headless extension.

use crate::types::{RenderScene, RenderOutput, RenderMetrics, GpuTextureHandle};
use anyhow::Result;

/// The engine-facing renderer interface.
///
/// Every backend (Helio/D3D12, wgpu, headless) implements this trait so that
/// engine subsystems can drive rendering without knowing the concrete backend.
pub trait Renderer: Send + Sync {
    /// Render one frame described by `scene` and return the result.
    fn render_frame(&mut self, scene: &RenderScene) -> Result<RenderOutput>;

    /// Inform the renderer that the viewport has been resized.
    fn resize(&mut self, width: u32, height: u32);

    /// Return the shared native GPU texture handle for the current read buffer,
    /// or `None` if the backend does not expose a shared texture (e.g. headless).
    fn gpu_texture_handle(&self) -> Option<GpuTextureHandle>;

    /// Return a snapshot of per-frame performance metrics.
    fn metrics(&self) -> RenderMetrics;

    /// Signal the renderer to stop and release GPU resources.
    fn shutdown(&mut self);
}

/// Extension for CPU/software renderers that can expose raw pixel data.
pub trait HeadlessRenderer: Renderer {
    /// Read the last rendered frame as RGBA8 bytes (row-major, top-to-bottom).
    fn read_pixels(&self) -> Vec<u8>;
}
