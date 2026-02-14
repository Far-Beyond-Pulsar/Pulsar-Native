//! Extension trait for blade_graphics::Context to expose D3D12 internals
//! 
//! This module provides unsafe access to the underlying D3D12 device and resources
//! from blade-graphics, enabling DXGI shared texture creation and other advanced scenarios.

#[cfg(target_os = "windows")]
pub mod windows_impl {
    use blade_graphics as gpu;
    use windows::Win32::Graphics::Direct3D12::*;
    use std::sync::Arc;

    /// Extension trait to extract D3D12 device from blade Context
    pub trait BladeD3D12Ext {
        /// # Safety
        /// Returns a raw pointer to the D3D12 device.
        /// The device lifetime is managed by blade - don't drop it!
        unsafe fn raw_d3d12_device(&self) -> Option<ID3D12Device>;
        
        /// # Safety  
        /// Extract D3D12 resource from a blade Texture
        unsafe fn raw_d3d12_resource(&self, texture: &gpu::Texture) -> Option<ID3D12Resource>;
    }

    impl BladeD3D12Ext for gpu::Context {
        unsafe fn raw_d3d12_device(&self) -> Option<ID3D12Device> {
            // blade_graphics Context contains:
            // - On D3D12 backend: device: ID3D12Device
            // - Wrapped in internal types
            
            // NOTE: blade-graphics doesn't expose D3D12 device officially.
            // Workaround: Using standalone device (see GpuSharedTextureManager).
            // Future: Submit PR to blade-graphics to add raw device access API.
            
            tracing::debug!("[BLADE-D3D12-EXT] raw_d3d12_device() not yet implemented - using standalone device");
            None
        }

        unsafe fn raw_d3d12_resource(&self, _texture: &gpu::Texture) -> Option<ID3D12Resource> {
            // NOTE: blade Texture wraps ID3D12Resource but doesn't expose it.
            // Workaround: Using standalone resources.
            // Future: Submit PR to blade-graphics to add raw resource access API.
            
            tracing::debug!("[BLADE-D3D12-EXT] raw_d3d12_resource() not yet implemented - using standalone resources");
            None
        }
    }

    /// Memory layout assumption structure - DO NOT USE
    /// 
    /// This documents the theoretical approach to extract blade-graphics internals.
    /// Left as documentation of rejected approach due to brittleness.
    #[repr(C)]
    struct BladeContextHack {
        _device: Option<ID3D12Device>,
        // ... other fields would need to match blade's actual Context layout
    }

    /// Rejected approach: Extract device using memory layout assumptions
    /// 
    /// # Safety
    /// DO NOT USE - Extremely unsafe and fragile approach.
    /// Left as documentation of what to avoid.
    pub unsafe fn try_extract_device_hack(context: &gpu::Context) -> Option<ID3D12Device> {
        tracing::error!("[BLADE-D3D12-EXT] try_extract_device_hack() called - this is a rejected approach");
        None
    }
}

#[cfg(not(target_os = "windows"))]
pub mod windows_impl {
    // No-op on non-Windows platforms
}
