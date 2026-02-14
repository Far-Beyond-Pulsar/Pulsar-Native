// Rendering subsystem for Pulsar Engine Backend

pub mod helio_renderer;
pub mod dxgi_shared_texture;
pub mod handle_utils;
// pub mod native_texture; // Obsolete - depends on bevy which is removed

pub use helio_renderer::{HelioRenderer, CameraInput, RenderMetrics, GpuProfilerData};
pub use dxgi_shared_texture::*;
pub use handle_utils::{handle_to_usize, usize_to_handle};
// pub use native_texture::{NativeTextureHandle, SharedTextureInfo, TextureFormat};

// Stub for compatibility
pub struct WgpuRenderer;

// Re-export common types
pub struct Framebuffer {
    pub width: u32,
    pub height: u32,
    pub buffer: Vec<u8>,
}

impl Framebuffer {
    pub fn new(width: u32, height: u32) -> Self {
        let buffer_size = (width * height * 4) as usize;
        Self {
            width,
            height,
            buffer: vec![0; buffer_size],
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        let buffer_size = (width * height * 4) as usize;
        self.buffer.resize(buffer_size, 0);
    }

    pub fn clear(&mut self, color: [u8; 4]) {
        for chunk in self.buffer.chunks_exact_mut(4) {
            chunk.copy_from_slice(&color);
        }
    }
}
