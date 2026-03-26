//! DXGI Shared Texture Creation (Windows only).
//!
//! Creates textures at the DXGI/Driver level that can be accessed by both
//! D3D11 and D3D12.  This is true zero-copy — both APIs see the exact same
//! memory in VRAM.

use anyhow::{Context, Result};

use windows::Win32::Graphics::{
    Direct3D12::*,
    Dxgi::Common::*,
};
use windows::Win32::Foundation::HANDLE;
use windows::core::PCWSTR;

/// Information about a DXGI shared texture.
pub struct DxgiSharedTexture {
    /// The D3D12 resource (for blade-graphics / wgpu).
    pub dx12_resource: ID3D12Resource,
    /// Shared NT handle (can be opened in D3D11).
    pub shared_handle: HANDLE,
    pub width: u32,
    pub height: u32,
}

impl DxgiSharedTexture {
    /// Create a new DXGI shared texture accessible by both D3D12 and D3D11.
    ///
    /// # Safety
    /// Calls raw D3D12 APIs.
    pub unsafe fn create(
        device: &ID3D12Device,
        width: u32,
        height: u32,
        format: DXGI_FORMAT,
    ) -> Result<Self> {
        let heap_props = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_DEFAULT,
            CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_UNKNOWN,
            MemoryPoolPreference: D3D12_MEMORY_POOL_UNKNOWN,
            CreationNodeMask: 1,
            VisibleNodeMask: 1,
        };

        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Alignment: 0,
            Width: width as u64,
            Height: height,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
            // KEY: ALLOW_SIMULTANEOUS_ACCESS enables D3D11/D3D12 sharing.
            Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET
                | D3D12_RESOURCE_FLAG_ALLOW_SIMULTANEOUS_ACCESS,
        };

        let mut resource: Option<ID3D12Resource> = None;
        device
            .CreateCommittedResource(
                &heap_props,
                D3D12_HEAP_FLAG_SHARED, // KEY: share across adapters/APIs
                &desc,
                D3D12_RESOURCE_STATE_COMMON,
                None,
                &mut resource,
            )
            .context("Failed to create shared D3D12 resource")?;

        let resource = resource.context("Resource was None after creation")?;

        tracing::debug!(
            "[DXGI-SHARED] D3D12 resource created, requesting shared handle…"
        );

        const GENERIC_ALL: u32 = 0x10000000;
        let shared_handle = device
            .CreateSharedHandle(&resource, None, GENERIC_ALL, PCWSTR::null())
            .context("Failed to create shared DXGI handle")?;

        tracing::debug!(
            "[DXGI-SHARED] Created shared texture {}x{} handle=0x{:X}",
            width,
            height,
            shared_handle.0 as usize
        );

        Ok(Self { dx12_resource: resource, shared_handle, width, height })
    }

    /// Return the raw handle value for passing to a D3D11 device.
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
