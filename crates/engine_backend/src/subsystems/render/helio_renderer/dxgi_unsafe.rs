//! DXGI Shared Handle Creation for Helio/blade-graphics
//! 
//! This module uses unsafe code to access blade-graphics D3D12 backend internals
//! and create DXGI shared texture handles for interop with GPUI/D3D11.
//!
//! ⚠️ WARNING: This module contains unsafe code that depends on blade-graphics internals.
//! It may break if blade-graphics is updated.

#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Direct3D12::*;
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Dxgi::Common::*;
#[cfg(target_os = "windows")]
use windows::Win32::Foundation::*;
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Direct3D::*;

use blade_graphics as gpu;

/// Attempt to extract raw D3D12 device from blade-graphics Context
/// 
/// This uses unsafe code to access internal structures. The approach:
/// 1. Cast Context to its internal representation
/// 2. Access the D3D12 device field
/// 3. Clone/ref the ComPtr
#[cfg(target_os = "windows")]
pub unsafe fn get_d3d12_device_from_context(context: &gpu::Context) -> Option<ID3D12Device> {
    // blade-graphics Context structure (approximate - may need adjustment):
    // pub struct Context {
    //     device: ID3D12Device,
    //     ...
    // }
    
    // Try to access via memory layout
    // This is extremely unsafe and brittle!
    
    // METHOD 1: Try to transmute to a known structure
    // We need to know the exact memory layout of Context
    
    // For now, return None and log a message
    tracing::warn!("[DXGI-UNSAFE] ⚠️ Unable to extract D3D12 device - blade-graphics doesn't expose raw device access");
    tracing::warn!("[DXGI-UNSAFE] Recommended: Submit PR to blade-graphics to add raw_device() method");
    None
}

/// Create a D3D12 texture with DXGI shared handle
#[cfg(target_os = "windows")]
pub unsafe fn create_shared_d3d12_texture(
    device: &ID3D12Device,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
) -> std::result::Result<(ID3D12Resource, HANDLE), String> {
    // Heap properties for GPU-only memory
    let heap_props = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_DEFAULT,
        CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_UNKNOWN,
        MemoryPoolPreference: D3D12_MEMORY_POOL_UNKNOWN,
        CreationNodeMask: 1, // Must be 1 for single-GPU
        VisibleNodeMask: 1,  // Must be 1 for single-GPU
    };

    // Resource description for render target texture
    let resource_desc = D3D12_RESOURCE_DESC {
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
        Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET | D3D12_RESOURCE_FLAG_ALLOW_SIMULTANEOUS_ACCESS,
    };

    // Create committed resource with SHARED flag
    let mut resource: Option<ID3D12Resource> = None;
    device.CreateCommittedResource(
        &heap_props,
        D3D12_HEAP_FLAG_SHARED, // KEY: Enable sharing
        &resource_desc,
        D3D12_RESOURCE_STATE_COMMON,
        None,
        &mut resource,
    ).expect("Failed to create D3D12 resource");

    let resource = resource.ok_or("Resource creation returned None")?;

    // Create shared handle - use correct constant
    const GENERIC_ALL: u32 = 0x10000000;
    
    let shared_handle = device.CreateSharedHandle(
        &resource,
        None, // No security attributes
        GENERIC_ALL,
        windows::core::PCWSTR::null(), // No name
    ).expect("Failed to create shared handle");

    tracing::info!("[DXGI-UNSAFE] ✅ Created shared D3D12 texture with handle: {:?}", shared_handle);

    Ok((resource, shared_handle))
}

/// Workaround: Create shared textures using Windows-rs directly
/// 
/// Since we can't access blade's internal D3D12 device, we create our own
/// D3D12 device just for shared texture creation. This is inefficient but works.
#[cfg(target_os = "windows")]
pub fn create_shared_textures_workaround(
    width: u32,
    height: u32,
) -> std::result::Result<(Vec<HANDLE>, Vec<ID3D12Resource>, ID3D12Device), String> {
    use windows::Win32::Graphics::Dxgi::*;

    unsafe {
        // Create a D3D12 device
        let mut device: Option<ID3D12Device> = None;
        
        // Try to create device
        D3D12CreateDevice(
            None, // Use default adapter
            D3D_FEATURE_LEVEL_11_0,
            &mut device,
        ).expect("Failed to create D3D12 device");

        let device = device.ok_or("D3D12CreateDevice returned None")?;
        tracing::info!("[DXGI-WORKAROUND] ✅ Created standalone D3D12 device for shared textures");

        // Create two shared textures
        let mut handles = Vec::new();
        let mut resources = Vec::new();
        
        for i in 0..2 {
            let (resource, handle) = create_shared_d3d12_texture(
                &device,
                width,
                height,
                DXGI_FORMAT_B8G8R8A8_UNORM,
            )?;
            
            tracing::info!("[DXGI-WORKAROUND] Created shared texture {}: handle 0x{:X}", 
                i, handle.0 as usize);
            
            handles.push(handle);
            resources.push(resource); // Keep resources alive
        }

        Ok((handles, resources, device))
    }
}

/// Get handle value as usize for storage
#[cfg(target_os = "windows")]
pub fn handle_to_usize(handle: HANDLE) -> usize {
    handle.0 as usize
}

/// Convert usize back to HANDLE
#[cfg(target_os = "windows")]
pub fn usize_to_handle(value: usize) -> HANDLE {
    HANDLE(value as *mut core::ffi::c_void)
}

#[cfg(not(target_os = "windows"))]
pub fn create_shared_textures_workaround(_width: u32, _height: u32) -> std::result::Result<(Vec<usize>, ()), String> {
    Err("DXGI shared textures only supported on Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn handle_to_usize(handle: usize) -> usize { handle }

#[cfg(not(target_os = "windows"))]
pub fn usize_to_handle(value: usize) -> usize { value }
