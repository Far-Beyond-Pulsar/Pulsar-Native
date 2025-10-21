//! DXGI Shared Texture Creation
//! 
//! Creates textures at the DXGI/Driver level that can be accessed by BOTH DX11 and DX12.
//! This is TRUE zero-copy - both APIs see the exact same memory in VRAM.

use anyhow::{Context, Result};

#[cfg(target_os = "windows")]
pub mod windows_impl {
    use super::*;
    use windows::Win32::Graphics::{
        Direct3D12::*,
        Dxgi::Common::*,
    };
    use windows::Win32::Foundation::HANDLE;
    use windows::core::Interface;

    /// Information about a DXGI shared texture
    pub struct DxgiSharedTexture {
        /// The DX12 resource (for Bevy/wgpu)
        pub dx12_resource: ID3D12Resource,
        /// Shared NT handle (can be opened in DX11)
        pub shared_handle: HANDLE,
        /// Dimensions
        pub width: u32,
        pub height: u32,
    }

    impl DxgiSharedTexture {
        /// Create a new DXGI shared texture accessible by both DX12 and DX11
        pub unsafe fn create(
            device: &ID3D12Device,
            width: u32,
            height: u32,
            format: DXGI_FORMAT,
        ) -> Result<Self> {
            // Heap properties for GPU-only memory
            let heap_props = D3D12_HEAP_PROPERTIES {
                Type: D3D12_HEAP_TYPE_DEFAULT,
                CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_UNKNOWN,
                MemoryPoolPreference: D3D12_MEMORY_POOL_UNKNOWN,
                CreationNodeMask: 0,
                VisibleNodeMask: 0,
            };

            // Resource description with SIMULTANEOUS ACCESS flag
            let desc = D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                Alignment: 0,
                Width: width as u64,
                Height: height,
                DepthOrArraySize: 1,
                MipLevels: 1,
                Format: format,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
                Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET | 
                       D3D12_RESOURCE_FLAG_ALLOW_SIMULTANEOUS_ACCESS, // KEY: Enables DX11/DX12 sharing
            };

            // Create the committed resource with SHARED heap flag
            let mut resource: Option<ID3D12Resource> = None;
            device.CreateCommittedResource(
                &heap_props,
                D3D12_HEAP_FLAG_SHARED, // KEY: Share across adapters/APIs
                &desc,
                D3D12_RESOURCE_STATE_COMMON, // Initial state
                None, // No clear value
                &mut resource,
            ).context("Failed to create shared D3D12 resource")?;

            let resource = resource.context("Resource was None after creation")?;

            // Create a shared NT handle for this resource
            let shared_handle = device.CreateSharedHandle(
                &resource,
                None, // Default security
                0x80000000 | 0x40000000 | 0x00100000, // GENERIC_READ | GENERIC_WRITE | SYNCHRONIZE
                None, // Unnamed
            ).context("Failed to create shared handle")?;

            println!("[DXGI-SHARED] ✅ Created shared texture {}x{} with handle: 0x{:X}", 
                width, height, shared_handle.0 as usize);

            Ok(Self {
                dx12_resource: resource,
                shared_handle,
                width,
                height,
            })
        }

        /// Get the shared handle value for passing to DX11
        pub fn handle_value(&self) -> usize {
            self.shared_handle.0 as usize
        }
    }

    impl Drop for DxgiSharedTexture {
        fn drop(&mut self) {
            unsafe {
                if !self.shared_handle.is_invalid() {
                    let _ = windows::Win32::Foundation::CloseHandle(self.shared_handle);
                }
            }
        }
    }
}

#[cfg(target_os = "windows")]
pub use windows_impl::*;
