//! DXGI Shared Texture creation for Helio renderer using blade-graphics
//! 
//! This module handles creating D3D12 shared textures that can be accessed
//! by both the Helio renderer (blade-graphics) and GPUI (D3D11 compositor)

use blade_graphics as gpu;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, AtomicU64, Ordering};

use super::core::SharedGpuTextures;
use super::dxgi_unsafe;
use gpui::GpuTextureHandle;

pub const RENDER_WIDTH: u32 = 1600;
pub const RENDER_HEIGHT: u32 = 900;

#[cfg(target_os = "windows")]
pub struct HelioSharedTextures {
    pub textures: [gpu::Texture; 2],
    pub native_handles: [GpuTextureHandle; 2],
    pub write_index: Arc<AtomicUsize>,
    pub read_index: Arc<AtomicUsize>,
    pub frame_number: Arc<AtomicU64>,
    /// Keep the D3D12 device alive for shared textures
    _d3d12_device: Option<windows::Win32::Graphics::Direct3D12::ID3D12Device>,
    /// Keep the D3D12 resources alive
    _d3d12_resources: Vec<windows::Win32::Graphics::Direct3D12::ID3D12Resource>,
}

#[cfg(target_os = "windows")]
impl HelioSharedTextures {
    /// Create double-buffered DXGI shared textures for Helio/GPUI interop
    pub fn new(context: &Arc<gpu::Context>) -> Result<Self, String> {
        tracing::info!("[HELIO-DXGI] Creating DXGI shared textures {}x{}", RENDER_WIDTH, RENDER_HEIGHT);

        // Create standalone D3D12 device for shared texture creation
        let (shared_handles, d3d12_resources, d3d12_device) = dxgi_unsafe::create_shared_textures_workaround(
            RENDER_WIDTH,
            RENDER_HEIGHT,
        )?;

        tracing::info!("[HELIO-DXGI] ✅ Created {} DXGI shared handles", shared_handles.len());

        // Store handles for GPUI compositor
        let handle_values: Vec<usize> = shared_handles.iter()
            .map(|h| dxgi_unsafe::handle_to_usize(*h))
            .collect();
        
        // Store handles for GPUI compositor (disabled - native_texture module removed)
        // crate::subsystems::render::native_texture::store_shared_handles(handle_values.clone());
        tracing::info!("[HELIO-DXGI] Shared handles: 0x{:X}, 0x{:X}", handle_values[0], handle_values[1]);

        // Import shared handles into Vulkan using external memory
        // ExternalMemorySource::Win32(Some(handle)) imports a Win32 NT handle
        // This is the CORRECT way - pass the shared HANDLE, not the resource pointer!
        tracing::info!("[HELIO-DXGI] Importing shared NT handles into Vulkan textures...");
        
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
            external: Some(gpu::ExternalMemorySource::Win32(Some(handle_values[0] as isize))),
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
            external: Some(gpu::ExternalMemorySource::Win32(Some(handle_values[1] as isize))),
        });

        // Create GpuTextureHandle wrappers for GPUI
        let handle_0 = GpuTextureHandle::new(handle_values[0] as isize, RENDER_WIDTH, RENDER_HEIGHT);
        let handle_1 = GpuTextureHandle::new(handle_values[1] as isize, RENDER_WIDTH, RENDER_HEIGHT);

        tracing::info!("[HELIO-DXGI] ✅ Imported shared NT handles into Vulkan!");
        tracing::info!("[HELIO-DXGI] TRUE zero-copy: Vulkan renders → D3D12 shared memory ← D3D11 GPUI reads");

        Ok(Self {
            textures: [texture_0, texture_1],
            native_handles: [handle_0, handle_1],
            write_index: Arc::new(AtomicUsize::new(0)),
            read_index: Arc::new(AtomicUsize::new(1)),
            frame_number: Arc::new(AtomicU64::new(0)),
            _d3d12_device: Some(d3d12_device),
            _d3d12_resources: d3d12_resources,
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
    pub fn get_current_native_handle(&self) -> GpuTextureHandle {
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
