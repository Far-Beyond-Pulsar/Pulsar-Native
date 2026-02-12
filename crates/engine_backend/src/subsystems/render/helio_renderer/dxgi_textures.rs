//! DXGI Shared Texture creation for Helio renderer using blade-graphics
//! 
//! This module handles creating D3D12 shared textures that can be accessed
//! by both the Helio renderer (blade-graphics) and GPUI (D3D11 compositor)

use blade_graphics as gpu;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, AtomicU64, Ordering};

use super::core::SharedGpuTextures;
use crate::subsystems::render::NativeTextureHandle;

pub const RENDER_WIDTH: u32 = 1600;
pub const RENDER_HEIGHT: u32 = 900;

#[cfg(target_os = "windows")]
pub struct HelioSharedTextures {
    pub textures: [gpu::Texture; 2],
    pub native_handles: [NativeTextureHandle; 2],
    pub write_index: Arc<AtomicUsize>,
    pub read_index: Arc<AtomicUsize>,
    pub frame_number: Arc<AtomicU64>,
}

#[cfg(target_os = "windows")]
impl HelioSharedTextures {
    /// Create double-buffered DXGI shared textures for Helio/GPUI interop
    pub fn new(context: &Arc<gpu::Context>) -> Result<Self, String> {
        use windows::Win32::Graphics::Direct3D12::*;
        use windows::Win32::Graphics::Dxgi::Common::*;
        use windows::Win32::Foundation::*;
        
        tracing::info!("[HELIO-DXGI] Creating DXGI shared textures {}x{}", RENDER_WIDTH, RENDER_HEIGHT);

        // For now, create regular textures
        // TODO: Access raw D3D12 device from blade-graphics to create shared resources
        // This is a temporary implementation that creates non-shared textures
        
        let texture_0 = context.create_texture(gpu::TextureDesc {
            name: "helio_shared_0",
            format: gpu::TextureFormat::Bgra8UnormSrgb,
            size: gpu::Extent {
                width: RENDER_WIDTH,
                height: RENDER_HEIGHT,
                depth: 1,
            },
            dimension: gpu::TextureDimension::D2,
            array_layer_count: 1,
            mip_level_count: 1,
            sample_count: 1,
            usage: gpu::TextureUsage::TARGET | gpu::TextureUsage::RESOURCE,
            external: None,
        });
        
        let texture_1 = context.create_texture(gpu::TextureDesc {
            name: "helio_shared_1",
            format: gpu::TextureFormat::Bgra8UnormSrgb,
            size: gpu::Extent {
                width: RENDER_WIDTH,
                height: RENDER_HEIGHT,
                depth: 1,
            },
            dimension: gpu::TextureDimension::D2,
            array_layer_count: 1,
            mip_level_count: 1,
            sample_count: 1,
            usage: gpu::TextureUsage::TARGET | gpu::TextureUsage::RESOURCE,
            external: None,
        });

        // For now, create placeholder handles
        // TODO: Extract actual D3D12 resource handles once blade-graphics exposes them
        let handle_0 = NativeTextureHandle::D3D11(0);
        let handle_1 = NativeTextureHandle::D3D11(0);

        tracing::warn!("[HELIO-DXGI] ⚠️ Using placeholder handles - DXGI sharing not yet fully implemented");
        tracing::info!("[HELIO-DXGI] Textures created, waiting for blade-graphics D3D12 resource access API");

        Ok(Self {
            textures: [texture_0, texture_1],
            native_handles: [handle_0, handle_1],
            write_index: Arc::new(AtomicUsize::new(0)),
            read_index: Arc::new(AtomicUsize::new(1)),
            frame_number: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Swap write and read buffers for double-buffering
    pub fn swap_buffers(&self) {
        let current_write = self.write_index.load(Ordering::Acquire);
        let current_read = self.read_index.load(Ordering::Acquire);
        
        self.write_index.store(current_read, Ordering::Release);
        self.read_index.store(current_write, Ordering::Release);
        
        self.frame_number.fetch_add(1, Ordering::Release);
    }

    /// Get the texture to render to (write buffer)
    pub fn get_write_texture(&self) -> gpu::Texture {
        let idx = self.write_index.load(Ordering::Acquire);
        self.textures[idx]
    }

    /// Get the texture to display (read buffer)
    pub fn get_read_texture(&self) -> gpu::Texture {
        let idx = self.read_index.load(Ordering::Acquire);
        self.textures[idx]
    }

    /// Get native handle for current read buffer (for GPUI display)
    pub fn get_current_native_handle(&self) -> NativeTextureHandle {
        let idx = self.read_index.load(Ordering::Acquire);
        self.native_handles[idx].clone()
    }

    /// Convert to SharedGpuTextures for compatibility with existing code
    pub fn to_shared_gpu_textures(&self) -> SharedGpuTextures {
        SharedGpuTextures {
            native_handles: Arc::new(std::sync::Mutex::new(Some(self.native_handles.clone()))),
            write_index: self.write_index.clone(),
            read_index: self.read_index.clone(),
            frame_number: self.frame_number.clone(),
            width: RENDER_WIDTH,
            height: RENDER_HEIGHT,
        }
    }
}

// TODO: Implement actual DXGI sharing once blade-graphics exposes:
// 1. Raw D3D12 device access
// 2. Raw D3D12 resource pointers from textures
// 3. Or built-in shared texture creation APIs
//
// The path forward:
// - Check blade-graphics source for D3D12 backend internals
// - Either use unsafe to extract raw pointers
// - Or submit PR to blade-graphics to add shared texture support
// - For now, we can test with non-shared textures (won't display in GPUI)
