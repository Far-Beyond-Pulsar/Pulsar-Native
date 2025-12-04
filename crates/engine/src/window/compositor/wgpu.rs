//! wgpu-Based Cross-Platform Compositor
//!
//! A unified compositor implementation using wgpu for all platforms (Windows, Linux, macOS).
//! Handles zero-copy GPU composition by importing platform-specific shared textures into wgpu.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────┐
//! │         wgpu Compositor Pipeline            │
//! ├─────────────────────────────────────────────┤
//! │ 1. Import Bevy texture (platform-specific)  │
//! │ 2. Import GPUI texture (platform-specific)  │
//! │ 3. Render fullscreen quad with Bevy         │
//! │ 4. Render fullscreen quad with GPUI (alpha) │
//! │ 5. Present to surface                       │
//! └─────────────────────────────────────────────┘
//! ```
//!
//! ## Platform-Specific Texture Importing
//!
//! - **Windows**: Import D3D11/D3D12 textures via HAL
//! - **Linux**: Import DMA-BUF/Vulkan textures via HAL
//! - **macOS**: Import Metal IOSurface textures via HAL

use super::{Compositor, CompositorState};
use anyhow::{anyhow, Context, Result};
use engine_backend::subsystems::render::NativeTextureHandle;
use gpui::SharedTextureHandle;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use std::sync::Arc;

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use ash::vk;

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use std::os::unix::io::RawFd;

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use wgpu_hal::vulkan;

#[cfg(target_os = "windows")]
use wgpu_hal::dx12;

#[cfg(target_os = "macos")]
use wgpu_hal::metal;

/// wgpu-based cross-platform compositor
pub struct WgpuCompositor {
    /// wgpu device
    device: wgpu::Device,

    /// wgpu command queue
    queue: wgpu::Queue,

    /// Surface for presenting to the window
    surface: wgpu::Surface<'static>,

    /// Surface configuration
    surface_config: wgpu::SurfaceConfiguration,

    /// Render pipeline for fullscreen quad composition
    pipeline: wgpu::RenderPipeline,

    /// Bind group layout for textures
    bind_group_layout: wgpu::BindGroupLayout,

    /// Sampler for texture sampling
    sampler: wgpu::Sampler,

    /// Imported Bevy texture (if any)
    bevy_texture: Option<wgpu::Texture>,
    bevy_bind_group: Option<wgpu::BindGroup>,

    /// Imported GPUI texture
    gpui_texture: Option<wgpu::Texture>,
    gpui_bind_group: Option<wgpu::BindGroup>,

    /// Window dimensions
    width: u32,
    height: u32,

    /// Compositor state
    state: CompositorState,
}

impl Compositor for WgpuCompositor {
    fn init(
        window: &(impl HasWindowHandle + HasDisplayHandle),
        width: u32,
        height: u32,
        scale_factor: f32,
    ) -> Result<Self>
    where
        Self: Sized,
    {
        // Create wgpu instance
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });

        // Create surface from window handle
        // Safety: The window must outlive the surface. Since the compositor is stored in WindowState
        // and WindowState contains the winit window, the window will outlive the surface.
        let surface = unsafe { instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::from_window(window)?) }?;

        // Request adapter
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .context("Failed to find suitable GPU adapter")?;

        // Request device and queue with external memory features
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor::default(),
        ))?;

        // Extract HAL device for zero-copy texture import
        // Note: wgpu 26 as_hal() returns Option<impl Deref> which can't be stored directly
        // We'll access HAL on-demand in composite_gpui instead

        // Configure surface
        let surface_caps = surface.get_capabilities(&adapter);
        let surface_format = surface_caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(surface_caps.formats[0]);

        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width,
            height,
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };

        surface.configure(&device, &surface_config);

        // Create bind group layout for textures
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Texture Bind Group Layout"),
            entries: &[
                // Texture
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // Sampler
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        // Create sampler
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Texture Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        // Create shader module
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Compositor Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("compositor.wgsl").into()),
        });

        // Create pipeline layout
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Compositor Pipeline Layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        // Create render pipeline
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Compositor Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview: None,
            cache: None,
        });

        println!("✅ wgpu compositor initialized successfully!");

        Ok(Self {
            device,
            queue,
            surface,
            surface_config,
            pipeline,
            bind_group_layout,
            sampler,
            bevy_texture: None,
            bevy_bind_group: None,
            gpui_texture: None,
            gpui_bind_group: None,
            width,
            height,
            state: CompositorState {
                width,
                height,
                scale_factor,
                needs_render: true,
            },
        })
    }

    fn begin_frame(&mut self) -> Result<()> {
        // Nothing to do here for wgpu
        Ok(())
    }

    fn composite_bevy(&mut self, handle: &NativeTextureHandle) -> Result<Option<()>> {
        // Import Bevy texture if not already imported or if handle changed
        // For now, return None to indicate Bevy texture import not yet implemented
        // TODO: Implement platform-specific texture import
        Ok(None)
    }

    fn composite_gpui(&mut self, handle: &SharedTextureHandle, should_render: bool) -> Result<()> {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        {
            match handle {
                SharedTextureHandle::DmaBuf { fd, size, format, stride, modifier } => {
                    // Only import texture once
                    if self.gpui_texture.is_none() {
                        log::info!("🔥 ZERO-COPY DMA-BUF import: fd={}, {}x{}, format={}, stride={}, modifier={}",
                            fd, size.width.0, size.height.0, format, stride, modifier);

                        unsafe {
                            // Access HAL device on-demand
                            let hal_device_guard = self.device.as_hal::<vulkan::Api>()
                                .ok_or_else(|| anyhow!("Failed to get Vulkan HAL device"))?;

                            let vk_device = hal_device_guard.raw_device();
                            let vk_physical = hal_device_guard.raw_physical_device();

                            // Duplicate the FD so we own it
                            let owned_fd = libc::dup(*fd);
                            if owned_fd < 0 {
                                return Err(anyhow!("Failed to duplicate DMA-BUF FD"));
                            }

                            // Map GPUI Vulkan format to wgpu format
                            let (vk_format, wgpu_format) = match *format {
                                44 => (vk::Format::B8G8R8A8_UNORM, wgpu::TextureFormat::Bgra8Unorm),
                                50 => (vk::Format::B8G8R8A8_SRGB, wgpu::TextureFormat::Bgra8UnormSrgb),
                                _ => (vk::Format::B8G8R8A8_UNORM, wgpu::TextureFormat::Bgra8Unorm),
                            };

                            // Create Vulkan image from DMA-BUF FD
                            let mut external_memory_image_create_info = vk::ExternalMemoryImageCreateInfo::default()
                                .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);

                            let image_create_info = vk::ImageCreateInfo::default()
                                .push_next(&mut external_memory_image_create_info)
                                .image_type(vk::ImageType::TYPE_2D)
                                .format(vk_format)
                                .extent(vk::Extent3D {
                                    width: size.width.0 as u32,
                                    height: size.height.0 as u32,
                                    depth: 1,
                                })
                                .mip_levels(1)
                                .array_layers(1)
                                .samples(vk::SampleCountFlags::TYPE_1)
                                .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
                                .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST)
                                .sharing_mode(vk::SharingMode::EXCLUSIVE)
                                .initial_layout(vk::ImageLayout::UNDEFINED);

                            let vk_image = vk_device.create_image(&image_create_info, None)
                                .map_err(|e| anyhow!("Failed to create Vulkan image: {:?}", e))?;

                            // Get memory requirements
                            let mem_reqs = vk_device.get_image_memory_requirements(vk_image);

                            // Import DMA-BUF memory
                            let mut import_fd_info = vk::ImportMemoryFdInfoKHR::default()
                                .fd(owned_fd)
                                .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);

                            let alloc_info = vk::MemoryAllocateInfo::default()
                                .push_next(&mut import_fd_info)
                                .allocation_size(mem_reqs.size)
                                .memory_type_index(
                                    // Find suitable memory type - just use first available
                                    mem_reqs.memory_type_bits.trailing_zeros()
                                );

                            let vk_memory = vk_device.allocate_memory(&alloc_info, None)
                                .map_err(|e| anyhow!("Failed to import DMA-BUF memory: {:?}", e))?;

                            // Bind image to memory
                            vk_device.bind_image_memory(vk_image, vk_memory, 0)
                                .map_err(|e| anyhow!("Failed to bind image memory: {:?}", e))?;

                            // Import the Vulkan image as HAL texture via wgpu-hal
                            use wgpu_hal::Device as _;
                            let hal_texture = hal_device_guard.texture_from_raw(
                                vk_image,
                                &wgpu_hal::TextureDescriptor {
                                    label: Some("GPUI DMA-BUF"),
                                    size: wgpu::Extent3d {
                                        width: size.width.0 as u32,
                                        height: size.height.0 as u32,
                                        depth_or_array_layers: 1,
                                    },
                                    mip_level_count: 1,
                                    sample_count: 1,
                                    dimension: wgpu::TextureDimension::D2,
                                    format: wgpu_format,
                                    usage: wgpu::TextureUses::from_bits_retain(1 << 2), // TEXTURE_BINDING
                                    memory_flags: wgpu_hal::MemoryFlags::empty(),
                                    view_formats: vec![],
                                },
                                None, // No custom drop callback
                            );

                            // Wrap HAL texture as wgpu texture
                            let texture = self.device.create_texture_from_hal::<vulkan::Api>(
                                hal_texture,
                                &wgpu::TextureDescriptor {
                                    label: Some("GPUI DMA-BUF Texture"),
                                    size: wgpu::Extent3d {
                                        width: size.width.0 as u32,
                                        height: size.height.0 as u32,
                                        depth_or_array_layers: 1,
                                    },
                                    mip_level_count: 1,
                                    sample_count: 1,
                                    dimension: wgpu::TextureDimension::D2,
                                    format: wgpu_format,
                                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                                    view_formats: &[],
                                },
                            );

                            // Store cleanup data
                            std::mem::forget(VulkanExternalMemory { vk_memory, vk_image, vk_device: vk_device.clone() });

                            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

                            let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                                label: Some("GPUI Bind Group"),
                                layout: &self.bind_group_layout,
                                entries: &[
                                    wgpu::BindGroupEntry {
                                        binding: 0,
                                        resource: wgpu::BindingResource::TextureView(&view),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 1,
                                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                                    },
                                ],
                            });

                            self.gpui_texture = Some(texture);
                            self.gpui_bind_group = Some(bind_group);

                            log::info!("✅ ZERO-COPY DMA-BUF import successful!");
                        }
                    }
                }
            }
        }

        #[cfg(target_os = "windows")]
        {
            match handle {
                SharedTextureHandle::D3D11NTHandle { handle, size, format } => {
                    if self.gpui_texture.is_none() {
                        log::info!("🔥 ZERO-COPY D3D11 import: handle={:?}, {}x{}, format={}",
                            handle, size.width.0, size.height.0, format);

                        unsafe {
                            // Import D3D11 shared handle via D3D12
                            let hal_device = &self.hal_device;

                            // Create D3D12 resource from NT handle
                            let d3d12_resource = hal_device.open_shared_handle(*handle)
                                .map_err(|e| anyhow!("Failed to open D3D11 shared handle: {:?}", e))?;

                            let wgpu_format = match *format {
                                87 => wgpu::TextureFormat::Bgra8Unorm,
                                91 => wgpu::TextureFormat::Bgra8UnormSrgb,
                                _ => wgpu::TextureFormat::Bgra8Unorm,
                            };

                            // Wrap as HAL texture
                            let hal_texture = hal_device.texture_from_raw(
                                d3d12_resource,
                                wgpu_format,
                                wgpu::TextureDimension::D2,
                                wgpu::Extent3d {
                                    width: size.width.0 as u32,
                                    height: size.height.0 as u32,
                                    depth_or_array_layers: 1,
                                },
                                1,
                                1,
                            );

                            let texture = self.device.create_texture_from_hal::<Dx12>(
                                hal_texture,
                                &wgpu::TextureDescriptor {
                                    label: Some("GPUI D3D11 Texture"),
                                    size: wgpu::Extent3d {
                                        width: size.width.0 as u32,
                                        height: size.height.0 as u32,
                                        depth_or_array_layers: 1,
                                    },
                                    mip_level_count: 1,
                                    sample_count: 1,
                                    dimension: wgpu::TextureDimension::D2,
                                    format: wgpu_format,
                                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                                    view_formats: &[],
                                },
                            );

                            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                            let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                                label: Some("GPUI Bind Group"),
                                layout: &self.bind_group_layout,
                                entries: &[
                                    wgpu::BindGroupEntry {
                                        binding: 0,
                                        resource: wgpu::BindingResource::TextureView(&view),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 1,
                                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                                    },
                                ],
                            });

                            self.gpui_texture = Some(texture);
                            self.gpui_bind_group = Some(bind_group);

                            log::info!("✅ ZERO-COPY D3D11 import successful!");
                        }
                    }
                }
            }
        }

        #[cfg(target_os = "macos")]
        {
            match handle {
                SharedTextureHandle::IOSurface { surface_id, size, format } => {
                    if self.gpui_texture.is_none() {
                        log::info!("🔥 ZERO-COPY IOSurface import: id={}, {}x{}, format={}",
                            surface_id, size.width.0, size.height.0, format);

                        unsafe {
                            let hal_device = &self.hal_device;

                            // Get IOSurface from ID
                            let io_surface = metal::IOSurfaceRef::from_id(*surface_id as u64);

                            let wgpu_format = match *format {
                                80 => wgpu::TextureFormat::Bgra8Unorm,
                                _ => wgpu::TextureFormat::Bgra8Unorm,
                            };

                            // Create Metal texture from IOSurface
                            let hal_texture = hal_device.texture_from_io_surface(
                                io_surface,
                                &wgpu_hal::TextureDescriptor {
                                    label: Some("GPUI IOSurface Texture"),
                                    size: wgpu::Extent3d {
                                        width: size.width.0 as u32,
                                        height: size.height.0 as u32,
                                        depth_or_array_layers: 1,
                                    },
                                    mip_level_count: 1,
                                    sample_count: 1,
                                    dimension: wgpu::TextureDimension::D2,
                                    format: wgpu_format,
                                    usage: wgpu_hal::TextureUses::RESOURCE,
                                    memory_flags: wgpu_hal::MemoryFlags::empty(),
                                    view_formats: vec![],
                                },
                            )?;

                            let texture = self.device.create_texture_from_hal::<Metal>(
                                hal_texture,
                                &wgpu::TextureDescriptor {
                                    label: Some("GPUI IOSurface Texture"),
                                    size: wgpu::Extent3d {
                                        width: size.width.0 as u32,
                                        height: size.height.0 as u32,
                                        depth_or_array_layers: 1,
                                    },
                                    mip_level_count: 1,
                                    sample_count: 1,
                                    dimension: wgpu::TextureDimension::D2,
                                    format: wgpu_format,
                                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                                    view_formats: &[],
                                },
                            );

                            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                            let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                                label: Some("GPUI Bind Group"),
                                layout: &self.bind_group_layout,
                                entries: &[
                                    wgpu::BindGroupEntry {
                                        binding: 0,
                                        resource: wgpu::BindingResource::TextureView(&view),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 1,
                                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                                    },
                                ],
                            });

                            self.gpui_texture = Some(texture);
                            self.gpui_bind_group = Some(bind_group);

                            log::info!("✅ ZERO-COPY IOSurface import successful!");
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn present(&mut self) -> Result<()> {
        // Get current surface texture
        let output = self.surface.get_current_texture()?;
        let view = output.texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Create command encoder
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Compositor Encoder"),
            });

        // Begin render pass
        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Compositor Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            render_pass.set_pipeline(&self.pipeline);

            // Draw Bevy layer (if available)
            if let Some(ref bind_group) = self.bevy_bind_group {
                render_pass.set_bind_group(0, bind_group, &[]);
                render_pass.draw(0..4, 0..1);
            }

            // Draw GPUI layer (if available)
            if let Some(ref bind_group) = self.gpui_bind_group {
                render_pass.set_bind_group(0, bind_group, &[]);
                render_pass.draw(0..4, 0..1);
            }
        }

        // Submit and present
        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();

        Ok(())
    }

    fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        self.width = width;
        self.height = height;
        self.state.width = width;
        self.state.height = height;
        self.surface_config.width = width;
        self.surface_config.height = height;
        self.surface.configure(&self.device, &self.surface_config);
        Ok(())
    }

    fn state(&self) -> &CompositorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut CompositorState {
        &mut self.state
    }
}


#[cfg(any(target_os = "linux", target_os = "freebsd"))]
struct VulkanExternalMemory {
    vk_memory: vk::DeviceMemory,
    vk_image: vk::Image,
    vk_device: ash::Device,
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
impl Drop for VulkanExternalMemory {
    fn drop(&mut self) {
        unsafe {
            self.vk_device.destroy_image(self.vk_image, None);
            self.vk_device.free_memory(self.vk_memory, None);
        }
    }
}
