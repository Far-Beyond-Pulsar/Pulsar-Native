//! CRITICAL: DXGI shared texture creation and management
//! 
//! ⚠️ WARNING: This module contains fragile rendering logic. Changes here can break frame display.
//! The texture creation order and timing is critical for proper DXGI shared resource setup.

use bevy::prelude::*;
use bevy::render::{
    render_asset::RenderAssets,
    renderer::RenderDevice,
    texture::GpuImage,
};
use std::sync::atomic::{AtomicU64, Ordering};
use super::core::{SharedTexturesResource, SharedGpuTextures};

pub const RENDER_WIDTH: u32 = 1600;
pub const RENDER_HEIGHT: u32 = 900;

/// Create DXGI shared textures BEFORE scene setup
/// This must run first so the Images exist when the camera is created
#[cfg(target_os = "windows")]
pub fn create_shared_textures_startup(
    shared_textures: Res<SharedTexturesResource>,
    mut images: ResMut<Assets<Image>>,
) {
    tracing::info!("[BEVY] 🔧 create_shared_textures_startup() CALLED - Creating placeholder Images...");
    tracing::info!("[BEVY] 📍 This runs in STARTUP schedule (main world) BEFORE rendering begins");

    // Check if already created
    if let Ok(lock) = shared_textures.0.lock() {
        if let Some(ref textures) = *lock {
            if let Ok(native_lock) = textures.native_handles.lock() {
                if native_lock.is_some() {
                    tracing::info!("[BEVY] ⚠️  Textures already created, skipping");
                    return;
                }
            }
        }
    }

    tracing::info!("[BEVY] 📦 Creating placeholder Images (actual DXGI textures created in render world)...");
    let bytes_per_pixel = 4; // BGRA8
    let texture_size = (RENDER_WIDTH * RENDER_HEIGHT * bytes_per_pixel) as usize;
    
    let mut image_0 = Image {
        texture_descriptor: bevy::render::render_resource::TextureDescriptor {
            label: Some("DXGI Shared Render Target 0"),
            size: bevy::render::render_resource::Extent3d {
                width: RENDER_WIDTH,
                height: RENDER_HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: bevy::render::render_resource::TextureDimension::D2,
            format: bevy::render::render_resource::TextureFormat::Bgra8UnormSrgb,
            usage: bevy::render::render_resource::TextureUsages::RENDER_ATTACHMENT | bevy::render::render_resource::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        ..default()
    };
    image_0.data = Some(vec![0u8; texture_size]); // Allocate proper buffer
    let render_target_0 = images.add(image_0);

    let mut image_1 = Image {
        texture_descriptor: bevy::render::render_resource::TextureDescriptor {
            label: Some("DXGI Shared Render Target 1"),
            size: bevy::render::render_resource::Extent3d {
                width: RENDER_WIDTH,
                height: RENDER_HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: bevy::render::render_resource::TextureDimension::D2,
            format: bevy::render::render_resource::TextureFormat::Bgra8UnormSrgb,
            usage: bevy::render::render_resource::TextureUsages::RENDER_ATTACHMENT | bevy::render::render_resource::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        ..default()
    };
    image_1.data = Some(vec![0u8; texture_size]); // Allocate proper buffer
    let render_target_1 = images.add(image_1);

    // Store handles - these will be replaced with DXGI-backed GpuImages in render world
    if let Ok(mut lock) = shared_textures.0.lock() {
        *lock = Some(SharedGpuTextures {
            textures: std::sync::Arc::new([render_target_0.clone(), render_target_1.clone()]),
            native_handles: std::sync::Arc::new(std::sync::Mutex::new(None)),
            write_index: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            read_index: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(1)),
            frame_number: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            width: RENDER_WIDTH,
            height: RENDER_HEIGHT,
        });
    }

    tracing::info!("[BEVY] ✅ Placeholder render target Images created");
}

/// Create shared textures for non-Windows platforms (stub for now)
#[cfg(not(target_os = "windows"))]
pub fn create_shared_textures_startup(
    shared_textures: Res<SharedTexturesResource>,
    mut images: ResMut<Assets<Image>>,
) {
    tracing::info!("[BEVY] 🔧 Creating shared textures (non-Windows)...");

    // Check if already created
    if let Ok(lock) = shared_textures.0.lock() {
        if lock.is_some() {
            tracing::info!("[BEVY] ⚠️ Textures already created");
            return;
        }
    }

    // Create placeholder render targets for Metal/Vulkan
    let bytes_per_pixel = 4; // BGRA8
    let texture_size = (RENDER_WIDTH * RENDER_HEIGHT * bytes_per_pixel) as usize;

    let mut image_0 = Image {
        texture_descriptor: bevy::render::render_resource::TextureDescriptor {
            label: Some("Shared Render Target 0"),
            size: bevy::render::render_resource::Extent3d {
                width: RENDER_WIDTH,
                height: RENDER_HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: bevy::render::render_resource::TextureDimension::D2,
            format: bevy::render::render_resource::TextureFormat::Bgra8UnormSrgb,
            usage: bevy::render::render_resource::TextureUsages::RENDER_ATTACHMENT | bevy::render::render_resource::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        ..default()
    };
    image_0.data = Some(vec![0u8; texture_size]);
    let render_target_0 = images.add(image_0);

    let mut image_1 = Image {
        texture_descriptor: bevy::render::render_resource::TextureDescriptor {
            label: Some("Shared Render Target 1"),
            size: bevy::render::render_resource::Extent3d {
                width: RENDER_WIDTH,
                height: RENDER_HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: bevy::render::render_resource::TextureDimension::D2,
            format: bevy::render::render_resource::TextureFormat::Bgra8UnormSrgb,
            usage: bevy::render::render_resource::TextureUsages::RENDER_ATTACHMENT | bevy::render::render_resource::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        ..default()
    };
    image_1.data = Some(vec![0u8; texture_size]);
    let render_target_1 = images.add(image_1);

    // Store handles
    if let Ok(mut lock) = shared_textures.0.lock() {
        *lock = Some(SharedGpuTextures {
            textures: std::sync::Arc::new([render_target_0.clone(), render_target_1.clone()]),
            native_handles: std::sync::Arc::new(std::sync::Mutex::new(None)),
            write_index: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            read_index: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(1)),
            frame_number: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            width: RENDER_WIDTH,
            height: RENDER_HEIGHT,
        });
    }

    tracing::info!("[BEVY] ✅ Render target Images created");
}

/// Create DXGI shared textures and inject them into Bevy's render pipeline
/// This replaces the GPU backing texture of the render targets with DXGI shared textures
#[cfg(target_os = "windows")]
pub fn create_shared_textures(
    shared_textures: Res<SharedTexturesResource>,
    mut gpu_images: ResMut<RenderAssets<GpuImage>>,
    render_device: Res<RenderDevice>,
) {
    use wgpu_hal::api::Dx12;
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;

    tracing::info!("[BEVY] 🔧 create_shared_textures() CALLED - Replacing render targets with DXGI shared textures...");
    tracing::info!("[BEVY] 📍 This should only run ONCE on the first render cycle");

    let texture_handles = match shared_textures.0.lock().ok().and_then(|l| l.as_ref().map(|t| t.textures.clone())) {
        Some(handles) => handles,
        None => {
            tracing::info!("[BEVY] ❌ No texture handles available");
            return;
        }
    };
    
    // Check if we already have DXGI textures created
    if let Ok(lock) = shared_textures.0.lock() {
        if let Some(ref textures) = *lock {
            if let Ok(native_lock) = textures.native_handles.lock() {
                if native_lock.is_some() {
                    // Already created, don't recreate
                    return;
                }
            }
        }
    }

    // Get D3D12 device from wgpu
    let d3d12_device = unsafe {
        match render_device.wgpu_device().as_hal::<Dx12>() {
            Some(hal_device) => hal_device.raw_device().clone(),
            None => {
                tracing::info!("[BEVY] ❌ Failed to get D3D12 device");
                return;
            }
        }
    };

    // Create 2 DXGI shared textures
    let tex_0 = unsafe {
        match crate::subsystems::render::DxgiSharedTexture::create(&d3d12_device, RENDER_WIDTH, RENDER_HEIGHT, DXGI_FORMAT_B8G8R8A8_UNORM) {
            Ok(t) => t,
            Err(e) => {
                tracing::info!("[BEVY] ❌ Failed to create texture 0: {}", e);
                return;
            }
        }
    };

    let tex_1 = unsafe {
        match crate::subsystems::render::DxgiSharedTexture::create(&d3d12_device, RENDER_WIDTH, RENDER_HEIGHT, DXGI_FORMAT_B8G8R8A8_UNORM) {
            Ok(t) => t,
            Err(e) => {
                tracing::info!("[BEVY] ❌ Failed to create texture 1: {}", e);
                return;
            }
        }
    };

    let handle_0 = tex_0.handle_value();
    let handle_1 = tex_1.handle_value();

    tracing::info!("[BEVY] ✅ Created DXGI textures: 0x{:X}, 0x{:X}", handle_0, handle_1);
    tracing::info!("[BEVY] 🎯 Handles will now be stored for GPUI compositor access");

    // Store handles for GPUI - both in global storage and in SharedGpuTextures
    crate::subsystems::render::native_texture::store_shared_handles(vec![handle_0, handle_1]);
    
    // Store native handles in SharedGpuTextures for indexed access
    if let Ok(mut lock) = shared_textures.0.lock() {
        if let Some(ref mut textures) = *lock {
            if let Ok(mut handles_lock) = textures.native_handles.lock() {
                *handles_lock = Some([
                    crate::subsystems::render::NativeTextureHandle::D3D11(handle_0),
                    crate::subsystems::render::NativeTextureHandle::D3D11(handle_1),
                ]);
                tracing::info!("[BEVY] 💾 Stored native handles in SharedGpuTextures");
            }
        }
    }

    // Wrap D3D12 textures as wgpu textures and inject into Bevy
    unsafe {
        let hal_tex_0 = <Dx12 as wgpu_hal::Api>::Device::texture_from_raw(
            tex_0.dx12_resource.clone(),
            wgpu::TextureFormat::Bgra8UnormSrgb,
            wgpu::TextureDimension::D2,
            wgpu::Extent3d {
                width: RENDER_WIDTH,
                height: RENDER_HEIGHT,
                depth_or_array_layers: 1,
            },
            1, // mip_level_count
            1, // sample_count
        );

        let hal_tex_1 = <Dx12 as wgpu_hal::Api>::Device::texture_from_raw(
            tex_1.dx12_resource.clone(),
            wgpu::TextureFormat::Bgra8UnormSrgb,
            wgpu::TextureDimension::D2,
            wgpu::Extent3d {
                width: RENDER_WIDTH,
                height: RENDER_HEIGHT,
                depth_or_array_layers: 1,
            },
            1, // mip_level_count
            1, // sample_count
        );

        let wgpu_desc = wgpu::TextureDescriptor {
            label: Some("DXGI Shared Texture"),
            size: wgpu::Extent3d {
                width: RENDER_WIDTH,
                height: RENDER_HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bgra8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        };

        let wgpu_tex_0 = render_device.wgpu_device().create_texture_from_hal::<Dx12>(hal_tex_0, &wgpu_desc);
        let mut desc1 = wgpu_desc.clone();
        desc1.label = Some("DXGI Shared Texture 1");
        let wgpu_tex_1 = render_device.wgpu_device().create_texture_from_hal::<Dx12>(hal_tex_1, &desc1);

        // Create texture views before moving textures
        let view_0 = wgpu_tex_0.create_view(&Default::default());
        let view_1 = wgpu_tex_1.create_view(&Default::default());

        // Create GpuImage and inject
        let gpu_img_0 = GpuImage {
            texture: bevy::render::render_resource::Texture::from(wgpu_tex_0),
            texture_view: bevy::render::render_resource::TextureView::from(view_0),
            texture_format: wgpu::TextureFormat::Bgra8UnormSrgb,
            sampler: render_device.create_sampler(&wgpu::SamplerDescriptor::default()),
            size: bevy::render::render_resource::Extent3d {
                width: RENDER_WIDTH,
                height: RENDER_HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
        };

        let gpu_img_1 = GpuImage {
            texture: bevy::render::render_resource::Texture::from(wgpu_tex_1),
            texture_view: bevy::render::render_resource::TextureView::from(view_1),
            texture_format: wgpu::TextureFormat::Bgra8UnormSrgb,
            sampler: render_device.create_sampler(&wgpu::SamplerDescriptor::default()),
            size: bevy::render::render_resource::Extent3d {
                width: RENDER_WIDTH,
                height: RENDER_HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
        };

        // CRITICAL: Inject our textures into Bevy's render assets
        tracing::info!("[BEVY] 📍 Injecting DXGI texture into asset ID 0: {:?}", texture_handles[0].id());
        tracing::info!("[BEVY] 📍 Injecting DXGI texture into asset ID 1: {:?}", texture_handles[1].id());
        gpu_images.insert(&texture_handles[0], gpu_img_0);
        gpu_images.insert(&texture_handles[1], gpu_img_1);

        tracing::info!("[BEVY] ✅ Injected DXGI textures into Bevy - Rendering DIRECTLY to shared GPU memory!");

        // Keep textures alive
        std::mem::forget(tex_0);
        std::mem::forget(tex_1);
    }
}

/// Extract native GPU handles for GPUI
pub fn extract_native_handles(
    shared_textures: Res<SharedTexturesResource>,
    _gpu_images: Res<RenderAssets<GpuImage>>,
) {
    static FRAME: AtomicU64 = AtomicU64::new(0);
    let f = FRAME.fetch_add(1, Ordering::Relaxed);
    
    if f % 120 != 0 {
        return; // Extract once per second
    }

    let _texture_handles = match shared_textures.0.lock().ok().and_then(|l| l.as_ref().map(|t| t.textures.clone())) {
        Some(h) => h,
        None => return,
    };

    // TODO: Extract actual GPU texture handles from GpuImage if needed
    // For now, DXGI handles are already stored globally in create_shared_textures
}
