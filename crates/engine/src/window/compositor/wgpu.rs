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
use anyhow::{Context, Result};
use engine_backend::subsystems::render::NativeTextureHandle;
use gpui::SharedTextureHandle;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

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
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
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

        // Request device and queue
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("Compositor Device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
            },
            None,
        ))?;

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
                entry_point: "vs_main",
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
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
        // Import GPUI texture if needed
        // TODO: Implement platform-specific texture import
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
