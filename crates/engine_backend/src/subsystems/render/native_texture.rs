// Cross-platform native texture handle extraction from wgpu
// This allows GPUI to directly display Bevy's GPU textures with ZERO COPIES
//
// Architecture:
// - Windows: wgpu Texture → ID3D11Texture2D handle
// - macOS: wgpu Texture → MTLTexture handle
// - Linux: wgpu Texture → VkImage handle
//
// NO copying, NO staging buffers - pure pointer casting!

use bevy::render::{
    render_resource::Texture,
    renderer::RenderDevice,
};
use anyhow::{Context, Result};

#[cfg(target_os = "windows")]
use std::sync::OnceLock;

/// Global storage for DXGI shared texture handles
#[cfg(target_os = "windows")]
static SHARED_TEXTURE_HANDLES: OnceLock<Vec<usize>> = OnceLock::new();

/// Store the shared texture handles for later extraction
#[cfg(target_os = "windows")]
pub fn store_shared_handles(handles: Vec<usize>) {
    tracing::debug!("[NATIVE-TEXTURE] 💾 Storing {} shared handles globally", handles.len());
    for (i, h) in handles.iter().enumerate() {
        tracing::debug!("[NATIVE-TEXTURE] 📍 Handle {}: 0x{:X}", i, h);
    }
    SHARED_TEXTURE_HANDLES.set(handles).ok();
}

/// Get the stored shared texture handles
#[cfg(target_os = "windows")]
pub fn get_shared_handles() -> Option<Vec<NativeTextureHandle>> {
    SHARED_TEXTURE_HANDLES.get().map(|handles| {
        handles.iter().map(|&h| NativeTextureHandle::D3D11(h)).collect()
    })
}

/// Native GPU texture handle - platform-specific
#[derive(Clone, Copy, Debug)]
pub enum NativeTextureHandle {
    #[cfg(target_os = "windows")]
    D3D11(usize), // HANDLE (NT handle) for DXGI shared resource - can be opened in both DX11 and DX12

    #[cfg(target_os = "macos")]
    Metal(usize), // MTLTexture* as usize

    #[cfg(target_os = "linux")]
    Vulkan(u64), // VkImageView as u64
}

impl NativeTextureHandle {
    /// Extract native handle from Bevy's wgpu texture
    /// This is UNSAFE but required for zero-copy integration with GPUI
    pub unsafe fn from_wgpu_texture(
        texture: &Texture,
        _device: &RenderDevice,
    ) -> Option<Self> {
        // Use wgpu's as_hal to extract native GPU handles
        // This gives us direct access to the underlying DirectX/Metal/Vulkan objects

        #[cfg(target_os = "windows")]
        {
            // First priority: Use pre-created DXGI shared handles
            if let Some(handles) = SHARED_TEXTURE_HANDLES.get() {
                if !handles.is_empty() {
                    // TODO: Map texture to correct handle index
                    // For now, alternate between the two handles
                    static TEXTURE_INDEX: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
                    let index = TEXTURE_INDEX.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % handles.len();
                    
                    let handle = handles[index];
                    tracing::debug!("[NATIVE-TEXTURE] ✅ Using pre-created DXGI shared handle[{}]: 0x{:X}", index, handle);
                    return Some(NativeTextureHandle::D3D11(handle));
                }
            }

            // Fallback: Try to create shared handle (will fail for wgpu textures)
            use wgpu_hal::api::Dx12;
            
            if let Some(hal_tex) = texture.as_hal::<Dx12>() {
                let dx12_texture = &*hal_tex;
                let resource = dx12_texture.raw_resource();
                
                if let Some(hal_device) = _device.wgpu_device().as_hal::<Dx12>() {
                    use windows::Win32::Graphics::Direct3D12::ID3D12Device;
                    let d3d12_device: &ID3D12Device = hal_device.raw_device();
                    
                    unsafe {
                        // Try to create shared handle
                        // This will fail if texture doesn't have ALLOW_SIMULTANEOUS_ACCESS flag
                        match d3d12_device.CreateSharedHandle(
                            resource,
                            None,
                            0x80000000 | 0x40000000, // GENERIC_READ | GENERIC_WRITE
                            None,
                        ) {
                            Ok(shared_handle) => {
                                let handle_value = shared_handle.0 as usize;
                                tracing::debug!("[NATIVE-TEXTURE] ✅ Created DX12 shared handle: 0x{:X}", handle_value);
                                return Some(NativeTextureHandle::D3D11(handle_value));
                            }
                            Err(_e) => {
                                // Expected error for wgpu-created textures without ALLOW_SIMULTANEOUS_ACCESS
                                // Silently fall through to return None
                            }
                        }
                    }
                } else {
                    tracing::debug!("[NATIVE-TEXTURE] ❌ Failed to get D3D12 device");
                }
            } else {
                tracing::debug!("[NATIVE-TEXTURE] ❌ Not a DX12 texture");
            }
            
            None
        }

        #[cfg(target_os = "macos")]
        {
            use wgpu_hal::api::Metal;
            
            // Get the HAL texture for Metal
            if let Some(hal_tex) = texture.as_hal::<Metal>() {
                let metal_texture = &*hal_tex;
                
                // Get the raw MTLTexture pointer
                let texture_ref = metal_texture.raw_texture();
                let texture_ptr = texture_ref.as_ptr() as usize;
                
                tracing::debug!("[NATIVE-TEXTURE] ✅ Extracted Metal texture: 0x{:X}", texture_ptr);
                Some(NativeTextureHandle::Metal(texture_ptr))
            } else {
                tracing::debug!("[NATIVE-TEXTURE] ❌ Failed to get HAL texture for Metal");
                None
            }
        }

        #[cfg(target_os = "linux")]
        {
            use wgpu_hal::api::Vulkan;
            use ash::vk::Handle;

            // Get the HAL texture for Vulkan
            if let Some(hal_tex) = texture.as_hal::<Vulkan>() {
                let vk_texture = &*hal_tex;

                // Get the raw VkImage handle - this returns ash::vk::Image
                // ash::vk::Image is a newtype wrapper around u64 that implements Handle trait
                // as_raw() extracts the GPU handle (NOT copying texture data - zero-copy!)
                let image_handle = vk_texture.raw_handle();
                let image_handle_u64 = image_handle.as_raw();

                tracing::debug!("[NATIVE-TEXTURE] ✅ Extracted Vulkan image: 0x{:X}", image_handle_u64);
                Some(NativeTextureHandle::Vulkan(image_handle_u64))
            } else {
                tracing::debug!("[NATIVE-TEXTURE] ❌ Failed to get HAL texture for Vulkan");
                None
            }
        }

        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
        {
            tracing::debug!("[NATIVE-TEXTURE] ❌ Unsupported platform");
            None
        }
    }

    /// Convert to raw pointer for GPUI integration
    #[cfg(target_os = "windows")]
    pub unsafe fn as_d3d11_ptr(self) -> Option<*mut std::ffi::c_void> {
        match self {
            NativeTextureHandle::D3D11(ptr) => Some(ptr as *mut std::ffi::c_void),
            _ => None,
        }
    }

    #[cfg(target_os = "macos")]
    pub unsafe fn as_metal_ptr(self) -> Option<*mut std::ffi::c_void> {
        match self {
            NativeTextureHandle::Metal(ptr) => Some(ptr as *mut std::ffi::c_void),
            _ => None,
        }
    }

    #[cfg(target_os = "linux")]
    pub unsafe fn as_vulkan_image(self) -> Option<u64> {
        match self {
            NativeTextureHandle::Vulkan(img) => Some(img),
        }
    }
}

/// Information about a shared texture for GPUI integration
#[derive(Clone, Debug)]
pub struct SharedTextureInfo {
    /// Native GPU handle
    pub handle: NativeTextureHandle,
    /// Texture dimensions
    pub width: u32,
    pub height: u32,
    /// Format (BGRA8UnormSrgb)
    pub format: TextureFormat,
}

#[derive(Clone, Copy, Debug)]
pub enum TextureFormat {
    Bgra8UnormSrgb,
    Rgba8UnormSrgb,
}

impl SharedTextureInfo {
    /// Create from Bevy texture
    pub unsafe fn from_bevy_texture(
        texture: &Texture,
        device: &RenderDevice,
        width: u32,
        height: u32,
    ) -> Option<Self> {
        let handle = NativeTextureHandle::from_wgpu_texture(texture, device)?;

        Some(Self {
            handle,
            width,
            height,
            format: TextureFormat::Bgra8UnormSrgb,
        })
    }
}
