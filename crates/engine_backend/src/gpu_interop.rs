//! Cross-platform GPU texture sharing for zero-copy rendering with GPUI.
//!
//! This module provides utilities to create shared GPU textures that can be
//! rendered by both the engine (DX12/Vulkan/Metal) and GPUI (DX11/OpenGL/Metal)
//! without any CPU copies.

#[cfg(all(target_os = "windows", feature = "gpui-interop"))]
use gpui::{GpuCanvasSource, GpuTextureHandle};
use std::sync::Arc;

#[cfg(target_os = "windows")]
mod windows_impl {
    use windows::Win32::Graphics::Direct3D12::*;
    use windows::Win32::Graphics::Dxgi::Common::*;
    use windows::Win32::Foundation::*;
    use windows::core::Interface;
    use anyhow::{Result, Context};

    /// Shared texture handle that can be passed to GPUI
    #[derive(Clone, Debug)]
    pub struct SharedTextureHandle {
        pub nt_handle: isize,
        pub width: u32,
        pub height: u32,
    }

    /// Creates a pair of shared DX12 textures that can be accessed by DX11.
    ///
    /// Returns NT handles that can be used to open the textures in DX11.
    pub fn create_shared_textures(
        device: &ID3D12Device,
        width: u32,
        height: u32,
    ) -> Result<(SharedTextureHandle, SharedTextureHandle)> {
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Alignment: 0,
            Width: width as u64,
            Height: height,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
            Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
        };

        let heap_props = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_DEFAULT,
            CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_UNKNOWN,
            MemoryPoolPreference: D3D12_MEMORY_POOL_UNKNOWN,
            CreationNodeMask: 1,
            VisibleNodeMask: 1,
        };

        let mut handles = Vec::with_capacity(2);

        for _ in 0..2 {
            let mut texture: Option<ID3D12Resource> = None;
            unsafe {
                device.CreateCommittedResource(
                    &heap_props,
                    D3D12_HEAP_FLAG_SHARED,
                    &desc,
                    D3D12_RESOURCE_STATE_COMMON,
                    None,
                    &mut texture,
                )?
            };

            let texture = texture.context("Failed to create texture")?;

            let nt_handle = unsafe {
                device.CreateSharedHandle(
                    &texture,
                    None,
                    GENERIC_ALL.0,
                    None,
                )?
            };

            let handle_value = nt_handle.0 as isize;
            handles.push(SharedTextureHandle {
                nt_handle: handle_value,
                width,
                height,
            });
        }

        Ok((handles[0].clone(), handles[1].clone()))
    }

    /// Helper to get the DX12 device from wgpu for creating shared textures.
    #[cfg(feature = "wgpu")]
    pub fn get_dx12_device_from_wgpu(device: &wgpu::Device) -> Option<ID3D12Device> {
        use wgpu_hal::Api;
        
        unsafe {
            device.as_hal::<wgpu_hal::api::Dx12, _, _>(|hal_device| {
                hal_device.map(|d| d.raw_device().clone())
            })
        }
    }
}

#[cfg(target_os = "macos")]
mod macos_impl {
    use anyhow::{Result, bail};

    #[derive(Clone, Debug)]
    pub struct SharedTextureHandle {
        pub surface_id: u64,
        pub width: u32,
        pub height: u32,
    }

    /// Creates a pair of shared IOSurfaces that can be accessed by Metal and OpenGL.
    pub fn create_shared_textures(
        width: u32,
        height: u32,
    ) -> Result<(SharedTextureHandle, SharedTextureHandle)> {
        bail!("macOS IOSurface sharing not yet fully implemented");
    }
}

#[cfg(target_os = "linux")]
mod linux_impl {
    use anyhow::{Result, bail};

    #[derive(Clone, Debug)]
    pub struct SharedTextureHandle {
        pub dma_buf_fd: i32,
        pub width: u32,
        pub height: u32,
    }

    /// Creates a pair of shared DMA-BUF textures for Vulkan/OpenGL sharing.
    pub fn create_shared_textures(
        width: u32,
        height: u32,
    ) -> Result<(SharedTextureHandle, SharedTextureHandle)> {
        bail!("Linux DMA-BUF sharing not yet implemented");
    }
}

#[cfg(target_os = "windows")]
pub use windows_impl::*;

#[cfg(target_os = "macos")]
pub use macos_impl::*;

#[cfg(target_os = "linux")]
pub use linux_impl::*;

