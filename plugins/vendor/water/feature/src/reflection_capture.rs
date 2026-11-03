//! Secondary reflected-camera render used by the water surface.
//!
//! The parent RenderGraph owns the retained child graph so its scene passes and
//! pipelines survive frames. This pass owns the capture target and mip chain.

use bytemuck::Zeroable;
use glam::{Mat4, Vec3, Vec4};
use helio_core::{
    graph::RenderGraph, GpuCameraUniforms, PassContext, SceneBufferProjection, SceneInput,
};
use std::sync::Arc;

pub(super) const REFLECTION_RESOURCE: helio_core::ResourceKey<&'static wgpu::TextureView> =
    helio_core::ResourceKey::new("water_planar_reflection");

pub(super) struct PlanarReflectionCapture {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    camera_buffer: wgpu::Buffer,
    hdr: wgpu::Texture,
    hdr_full_view: wgpu::TextureView,
    mip_views: Vec<wgpu::TextureView>,
    depth: wgpu::Texture,
    depth_view: wgpu::TextureView,
    mip_sampler: wgpu::Sampler,
    mip_pipeline: wgpu::RenderPipeline,
    mip_layout: wgpu::BindGroupLayout,
    mip_bind_groups: Vec<wgpu::BindGroup>,
    width: u32,
    height: u32,
    reflected_camera: GpuCameraUniforms,
}

impl PlanarReflectionCapture {
    pub(super) fn new(device: &wgpu::Device, queue: &wgpu::Queue, width: u32, height: u32) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Water Reflected Camera"),
            size: 2 * std::mem::size_of::<GpuCameraUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM
                | wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mip_count = mip_count(width, height);
        let hdr = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Water Planar Reflection HDR Mips"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: mip_count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let hdr_full_view = hdr.create_view(&wgpu::TextureViewDescriptor::default());
        let mip_views: Vec<wgpu::TextureView> = (0..mip_count)
            .map(|level| {
                hdr.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("Water Planar Reflection Mip"),
                    format: Some(wgpu::TextureFormat::Rgba16Float),
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_mip_level: level,
                    mip_level_count: Some(1),
                    base_array_layer: 0,
                    array_layer_count: Some(1),
                    aspect: wgpu::TextureAspect::All,
                    usage: None,
                })
            })
            .collect();
        let depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Water Planar Reflection Depth"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
        let mip_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Water Planar Reflection Mip Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let mip_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Water Reflection Mip BGL"),
            entries: &[
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
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let mip_bind_groups = make_mip_bind_groups(device, &mip_layout, &mip_views, &mip_sampler);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Water Reflection Mip Shader"),
            source: wgpu::ShaderSource::Wgsl(
                r#"
@group(0) @binding(0) var source_tex: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;
struct VertexOut { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32> }
@vertex fn vs(@builtin(vertex_index) index: u32) -> VertexOut {
    let p = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var out: VertexOut;
    out.position = vec4<f32>(p * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
    out.uv = p;
    return out;
}
@fragment fn fs(in: VertexOut) -> @location(0) vec4<f32> {
    return textureSample(source_tex, source_sampler, in.uv);
}
"#
                .into(),
            ),
        });
        let mip_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Water Reflection Mip Pipeline Layout"),
            bind_group_layouts: &[Some(&mip_layout)],
            immediate_size: 0,
        });
        let mip_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Water Reflection Mip Pipeline"),
            layout: Some(&mip_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba16Float,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });

        Self {
            device: Arc::new(device.clone()),
            queue: Arc::new(queue.clone()),
            camera_buffer,
            hdr,
            hdr_full_view,
            mip_views,
            depth,
            depth_view,
            mip_sampler,
            mip_pipeline,
            mip_layout,
            mip_bind_groups,
            width,
            height,
            reflected_camera: GpuCameraUniforms::zeroed(),
        }
    }

    pub(super) fn reflection_view(&self) -> &wgpu::TextureView {
        &self.hdr_full_view
    }

    pub(super) fn camera_buffer(&self) -> &wgpu::Buffer {
        &self.camera_buffer
    }

    pub(super) fn resize(&mut self, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        if (width, height) == (self.width, self.height) {
            return;
        }
        self.width = width;
        self.height = height;
        self.hdr = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Water Planar Reflection HDR Mips"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: mip_count(width, height),
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        self.hdr_full_view = self
            .hdr
            .create_view(&wgpu::TextureViewDescriptor::default());
        self.mip_views = (0..mip_count(width, height))
            .map(|level| {
                self.hdr.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("Water Planar Reflection Mip"),
                    format: Some(wgpu::TextureFormat::Rgba16Float),
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_mip_level: level,
                    mip_level_count: Some(1),
                    base_array_layer: 0,
                    array_layer_count: Some(1),
                    aspect: wgpu::TextureAspect::All,
                    usage: None,
                })
            })
            .collect();
        self.mip_bind_groups = make_mip_bind_groups(
            &self.device,
            &self.mip_layout,
            &self.mip_views,
            &self.mip_sampler,
        );
        self.depth = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Water Planar Reflection Depth"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        self.depth_view = self
            .depth
            .create_view(&wgpu::TextureViewDescriptor::default());
    }

    pub(super) fn render(
        &mut self,
        source: &dyn SceneInput,
        frame: u64,
        rest_y: f64,
        ctx: &mut PassContext<'_>,
    ) -> helio_core::Result<()> {
        if f64::from(source.camera_data().position_near[1]) + source.world_origin().y <= rest_y {
            // When the eye is below the authored rest plane,
            // the above-water mirror capture is not a meaningful view.
            self.reflected_camera.jitter_frame[3] = 0.0;
            self.queue.write_buffer(
                &self.camera_buffer,
                0,
                bytemuck::cast_slice(&[self.reflected_camera, self.reflected_camera]),
            );
            return Ok(());
        }
        let source_camera = source.camera_data();
        let source_view = Mat4::from_cols_array(&source_camera.view);
        let source_proj = Mat4::from_cols_array(&source_camera.proj);
        let plane_y = (rest_y - source.world_origin().y) as f32;
        let reflection = Mat4::from_translation(Vec3::new(0.0, 2.0 * plane_y, 0.0))
            * Mat4::from_scale(Vec3::new(1.0, -1.0, 1.0));
        let view = source_view * reflection;
        let plane_world = Vec4::new(0.0, 1.0, 0.0, -plane_y);
        let plane_view = view.inverse().transpose() * plane_world;
        let projection = oblique_reverse_z(source_proj, plane_view);
        let source_position = Vec3::new(
            source_camera.position_near[0],
            source_camera.position_near[1],
            source_camera.position_near[2],
        );
        let reflected_position = reflection.transform_point3(source_position);
        let view_proj = projection * view;
        self.reflected_camera = GpuCameraUniforms::new(
            view,
            projection,
            reflected_position,
            source_camera.position_near[3],
            source_camera.forward_far[3],
            frame as u32,
            [source_camera.jitter_frame[0], source_camera.jitter_frame[1]],
            view_proj,
        )
        .with_view_id(0x5741_5445);
        self.queue.write_buffer(
            &self.camera_buffer,
            0,
            bytemuck::cast_slice(&[self.reflected_camera, self.reflected_camera]),
        );
        let input = ReflectedScene {
            device: Arc::clone(&self.device),
            queue: Arc::clone(&self.queue),
            camera: &self.camera_buffer,
            camera_data: &self.reflected_camera,
            camera_generation: frame.wrapping_add(1),
            frame,
            scene_buffers: source.scene_buffers(),
            world_origin: source.world_origin(),
        };
        let width = self.width;
        let height = self.height;
        let camera_buffer = self.camera_buffer.clone();
        // The reflected render owns a separate ResourceRegistry. Propagate
        // the renderer-owned values its GBuffer and deferred-light passes
        // consume so the child view has the same material/environment state.
        let material_textures = ctx
            .registry
            .read::<helio_mats::MaterialTextureBindings<'_>>(
                helio_core::ResourceKey::new("material_textures"),
                "WaterPlanarReflection",
            )
            .expect("renderer material-texture bindings for water reflection");
        let render_environment = ctx
            .registry
            .read::<helio_core::RenderEnvironment<'_>>(
                helio_core::resource_keys::render_environment(),
                "WaterPlanarReflection",
            )
            .expect("renderer environment for water reflection");
        // SAFETY: the mip-zero view is owned by this graph-owned pass and
        // remains alive through the synchronous child graph execution.
        let target_view: &'static wgpu::TextureView =
            unsafe { std::mem::transmute(&self.mip_views[0]) };
        ctx.execute_secondary_graph(
            "water_planar_reflection",
            width,
            height,
            &input,
            &self.mip_views[0],
            &self.depth_view,
            move |device, queue| {
                build_secondary_graph(device, queue, &camera_buffer, width, height)
            },
            |registry| {
                registry.write(
                    helio_core::ResourceKey::new("pre_aa"),
                    target_view,
                    "WaterPlanarReflection",
                );
                registry.write(
                    helio_core::ResourceKey::new("material_textures"),
                    material_textures,
                    "Renderer",
                );
                registry.write(
                    helio_core::resource_keys::render_environment(),
                    render_environment,
                    "Renderer",
                );
            },
        )?;
        self.generate_mips();
        Ok(())
    }

    pub(super) fn render_from_context(
        &mut self,
        ctx: &mut PassContext<'_>,
        rest_y: f64,
    ) -> helio_core::Result<()> {
        let source = PassScene {
            device: Arc::new(ctx.device.clone()),
            queue: Arc::new(ctx.queue.clone()),
            camera: ctx.camera,
            camera_data: ctx.camera_data,
            camera_generation: ctx.camera_generation,
            frame: ctx.frame_num,
            scene_buffers: ctx.scene_buffers,
            world_origin: ctx.world_origin,
        };
        self.render(&source, ctx.frame_num, rest_y, ctx)
    }

    fn generate_mips(&self) {
        if self.mip_views.len() <= 1 {
            return;
        }
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Water Reflection Mip Generation"),
            });
        for (index, group) in self.mip_bind_groups.iter().enumerate() {
            let level = index + 1;
            let attachments = [Some(wgpu::RenderPassColorAttachment {
                view: &self.mip_views[level],
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })];
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Water Reflection Mip"),
                color_attachments: &attachments,
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.mip_pipeline);
            pass.set_bind_group(0, group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([encoder.finish()]);
    }
}

fn make_mip_bind_groups(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    views: &[wgpu::TextureView],
    sampler: &wgpu::Sampler,
) -> Vec<wgpu::BindGroup> {
    (1..views.len())
        .map(|level| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Water Reflection Mip BG"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&views[level - 1]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                ],
            })
        })
        .collect()
}

fn build_secondary_graph(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    camera_buffer: &wgpu::Buffer,
    width: u32,
    height: u32,
) -> RenderGraph {
    let stats = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Water Reflection Cull Stats"),
        size: 4096,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut graph = RenderGraph::new_with_external_device_ref(device, queue);
    graph.declare_external_input("pre_aa");
    graph.declare_external_input("material_textures");
    graph.declare_external_input("render_environment");
    graph.add_pass(Box::new(helio_pass_object_batch::ObjectBatchPass::new(
        device,
    )));
    graph.add_pass(Box::new(
        helio_pass_indirect_dispatch::IndirectDispatchPass::new(device, stats.clone()),
    ));
    let cull_sampler = Arc::new(device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("Water Reflection Cull Sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    }));
    graph.add_pass(Box::new(helio_pass_occlusion_cull::OcclusionCullPass::new(
        device,
        cull_sampler,
        width,
        height,
        stats,
    )));
    let mut gbuffer = helio_pass_gbuffer::GBufferPass::new(device);
    gbuffer.set_mirrored_view(true);
    graph.add_pass(Box::new(gbuffer));
    graph.add_pass(Box::new(helio_pass_deferred_light::DeferredLightPass::new(
        device,
        queue,
        camera_buffer,
        wgpu::TextureFormat::Rgba16Float,
    )));
    graph
}

struct ReflectedScene<'a> {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    camera: &'a wgpu::Buffer,
    camera_data: &'a GpuCameraUniforms,
    camera_generation: u64,
    frame: u64,
    scene_buffers: &'a SceneBufferProjection,
    world_origin: glam::DVec3,
}

struct PassScene<'a> {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    camera: &'a wgpu::Buffer,
    camera_data: &'a GpuCameraUniforms,
    camera_generation: u64,
    frame: u64,
    scene_buffers: &'a SceneBufferProjection,
    world_origin: glam::DVec3,
}

impl SceneInput for PassScene<'_> {
    fn device(&self) -> &Arc<wgpu::Device> {
        &self.device
    }
    fn queue(&self) -> &Arc<wgpu::Queue> {
        &self.queue
    }
    fn frame_count(&self) -> u64 {
        self.frame
    }
    fn camera(&self) -> &wgpu::Buffer {
        self.camera
    }
    fn camera_data(&self) -> &GpuCameraUniforms {
        self.camera_data
    }
    fn camera_generation(&self) -> u64 {
        self.camera_generation
    }
    fn scene_buffers(&self) -> &SceneBufferProjection {
        self.scene_buffers
    }
    fn world_origin(&self) -> glam::DVec3 {
        self.world_origin
    }
}

impl SceneInput for ReflectedScene<'_> {
    fn device(&self) -> &Arc<wgpu::Device> {
        &self.device
    }
    fn queue(&self) -> &Arc<wgpu::Queue> {
        &self.queue
    }
    fn frame_count(&self) -> u64 {
        self.frame
    }
    fn camera(&self) -> &wgpu::Buffer {
        self.camera
    }
    fn camera_data(&self) -> &GpuCameraUniforms {
        self.camera_data
    }
    fn camera_generation(&self) -> u64 {
        self.camera_generation
    }
    fn scene_buffers(&self) -> &SceneBufferProjection {
        self.scene_buffers
    }
    fn world_origin(&self) -> glam::DVec3 {
        self.world_origin
    }
}

fn mip_count(width: u32, height: u32) -> u32 {
    u32::BITS - width.max(height).leading_zeros()
}

/// Replace the reversed-Z projection's near clip plane (`w-z >= 0`) with the
/// supplied camera-space plane while preserving its infinite far corner.
fn oblique_reverse_z(projection: Mat4, plane: Vec4) -> Mat4 {
    let inverse = projection.inverse();
    let corner = inverse * Vec4::new(plane.x.signum(), plane.y.signum(), 0.0, 1.0);
    let denominator = plane.dot(corner);
    if denominator.abs() < 1.0e-5 {
        return projection;
    }
    let clip = plane / denominator;
    let mut columns = projection.to_cols_array_2d();
    for (index, component) in [clip.x, clip.y, clip.z, clip.w].into_iter().enumerate() {
        columns[index][2] = columns[index][3] - component;
    }
    Mat4::from_cols_array_2d(&columns)
}
