//! Helio extensions for D3D12 shared texture support
//!
//! Adds methods to FeatureRenderer to create and render to DXGI shared textures

use blade_graphics as gpu;
use std::sync::Arc;

#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Direct3D12::*;
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Dxgi::Common::*;

/// Extensions for Helio's FeatureRenderer to support DXGI shared textures
pub trait HelioD3D12Extensions {
    /// Get the blade context (gives access to device indirectly)
    fn get_context(&self) -> &Arc<gpu::Context>;
    
    /// Create a render target from an externally-created D3D12 texture
    /// This allows rendering directly to DXGI shared textures
    #[cfg(target_os = "windows")]
    unsafe fn create_render_target_from_d3d12_resource(
        &self,
        resource: &ID3D12Resource,
        format: gpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> Option<gpu::Texture>;
}

#[cfg(target_os = "windows")]
/// Create D3D12 shared textures that can be used by both blade and GPUI
pub unsafe fn create_blade_compatible_shared_texture(
    device: &ID3D12Device,
    width: u32,
    height: u32,
) -> Result<(ID3D12Resource, windows::Win32::Foundation::HANDLE), String> {
    use windows::Win32::Foundation::HANDLE;
    use windows::core::PCWSTR;

    tracing::info!("[HELIO-D3D12] Creating blade-compatible shared texture {}x{}", width, height);

    // Create heap properties
    let heap_props = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_DEFAULT,
        CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_UNKNOWN,
        MemoryPoolPreference: D3D12_MEMORY_POOL_UNKNOWN,
        CreationNodeMask: 1,
        VisibleNodeMask: 1,
    };

    // Resource description - compatible with blade's expectations
    let resource_desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Alignment: 0,
        Width: width as u64,
        Height: height,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM, // Match blade's Bgra8UnormSrgb
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
        Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET | D3D12_RESOURCE_FLAG_ALLOW_SIMULTANEOUS_ACCESS,
    };

    // Create resource
    let mut resource: Option<ID3D12Resource> = None;
    device.CreateCommittedResource(
        &heap_props,
        D3D12_HEAP_FLAG_SHARED,
        &resource_desc,
        D3D12_RESOURCE_STATE_COMMON,
        None,
        &mut resource,
    ).map_err(|e| format!("Failed to create D3D12 resource: {:?}", e))?;

    let resource = resource.ok_or("Resource creation returned None")?;

    // Create shared handle
    const GENERIC_ALL: u32 = 0x10000000;
    let shared_handle = device.CreateSharedHandle(
        &resource,
        None,
        GENERIC_ALL,
        PCWSTR::null(),
    ).map_err(|e| format!("Failed to create shared handle: {:?}", e))?;

    tracing::info!("[HELIO-D3D12] ✅ Created shared texture with handle: 0x{:X}", shared_handle.0 as usize);

    Ok((resource, shared_handle))
}

/// Import an external D3D12 resource as a blade Texture
/// 
/// This uses blade's `external` parameter with the D3D12 resource handle
#[cfg(target_os = "windows")]
pub unsafe fn import_d3d12_resource_as_blade_texture(
    context: &Arc<gpu::Context>,
    resource: &ID3D12Resource,
    format: gpu::TextureFormat,
    width: u32,
    height: u32,
) -> Option<gpu::Texture> {
    tracing::info!("[HELIO-D3D12] Attempting to import D3D12 resource as blade texture");
    
    // Get raw pointer from COM object and convert to isize for Win32 handle
    let resource_ptr = resource as *const _ as isize;
    
    // Use blade's external memory feature to import the D3D12 resource
    // ExternalMemorySource::Win32(Some(handle)) imports an existing resource
    let texture = context.create_texture(gpu::TextureDesc {
        name: "imported_shared",
        format,
        size: gpu::Extent {
            width,
            height,
            depth: 1,
        },
        dimension: gpu::TextureDimension::D2,
        array_layer_count: 1,
        mip_level_count: 1,
        sample_count: 1,
        usage: gpu::TextureUsage::TARGET | gpu::TextureUsage::RESOURCE,
        external: Some(gpu::ExternalMemorySource::Win32(Some(resource_ptr))),
    });

    tracing::info!("[HELIO-D3D12] ✅ Successfully imported resource as blade texture");
    Some(texture)
}
