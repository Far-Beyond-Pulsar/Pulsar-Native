use cables_api::{CABLE_CONNECTORS, CABLE_DEFINITIONS};
use helio_core::{PassContext, PrepareContext, RenderPass, Result as HelioResult};

const MAX_CONNECTORS: u32 = 128;
const MAX_BUNDLE_STRANDS: u32 = 64;
const SEGMENTS: u32 = 32;
const TUBE_SIDES: u32 = 12;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CableUniforms {
    connector_count: u32,
    definition_count: u32,
    frame: u32,
    _pad: u32,
}

/// GPU vertex generation for cable pairs. The shader searches the connector
/// and definition rows directly; no endpoint pairs or vertices are built on
/// the CPU.
pub struct CableRenderPass {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    texture_binding: helio_mats::MaterialBindingConfig,
    texture_bind_group_layout: wgpu::BindGroupLayout,
    texture_bind_group: Option<wgpu::BindGroup>,
    texture_version: Option<u64>,
    uniforms: wgpu::Buffer,
    fallback_connectors: wgpu::Buffer,
    fallback_definitions: wgpu::Buffer,
    connector_count: u32,
    definition_count: u32,
    bind_group: Option<wgpu::BindGroup>,
    bound_buffers: Option<[wgpu::Buffer; 2]>,
}

impl CableRenderPass {
    pub fn new(
        device: &wgpu::Device,
        texture_binding: helio_mats::MaterialBindingConfig,
        color_format: wgpu::TextureFormat,
    ) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Cable render bindings"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<CableUniforms>() as u64,
                        ),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        // The WGSL camera prefix is view, projection and
                        // view-projection: three 64-byte matrices.
                        min_binding_size: wgpu::BufferSize::new(192),
                    },
                    count: None,
                },
            ],
        });
        let mut texture_entries = Vec::new();
        texture_binding.append_layout_entries(
            &mut texture_entries,
            0,
            wgpu::ShaderStages::FRAGMENT,
        );
        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Cable scene texture table"),
                entries: &texture_entries,
            });
        let source = cable_shader_source(texture_binding);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("GPU cable vertex generation"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Cable render pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout), Some(&texture_bind_group_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("GPU cable ribbons and tubes"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Cable view uniforms"),
            size: std::mem::size_of::<CableUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let fallback_connectors = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Empty cable connector rows"),
            size: std::mem::size_of::<cables_api::CableConnectorRow>() as u64,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let fallback_definitions = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Empty cable definition rows"),
            size: std::mem::size_of::<cables_api::CableDefinitionRow>() as u64,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        Self {
            pipeline,
            bind_group_layout,
            texture_binding,
            texture_bind_group_layout,
            texture_bind_group: None,
            texture_version: None,
            uniforms,
            fallback_connectors,
            fallback_definitions,
            connector_count: 0,
            definition_count: 0,
            bind_group: None,
            bound_buffers: None,
        }
    }
}

impl RenderPass for CableRenderPass {
    fn name(&self) -> &'static str {
        "CableRenderPass"
    }

    fn scene_buffers(&self) -> &'static [&'static str] {
        &["cable_connectors", "cable_definitions"]
    }

    fn reads(&self) -> &'static [&'static str] {
        &[helio_frame_api::color::SCENE_COLOR]
    }

    fn writes(&self) -> &'static [&'static str] {
        &[helio_frame_api::color::SCENE_COLOR]
    }

    fn declare_resources(&self, builder: &mut helio_core::graph::ResourceBuilder) {
        builder.read(helio_frame_api::color::SCENE_COLOR);
        builder.read(helio_mats::MATERIAL_TEXTURES);
        builder.read("cable_connectors");
        builder.read("cable_definitions");
    }

    fn render_pass_descriptor_with_storage<'a>(
        &'a self,
        target: &'a wgpu::TextureView,
        _depth: &'a wgpu::TextureView,
        resources: &'a helio_core::ResourceRegistry<'a>,
        storage: &'a mut helio_core::RenderFrameStorage,
    ) -> Option<wgpu::RenderPassDescriptor<'a>> {
        let scene_color = resources
            .get(helio_core::ResourceKey::new(helio_frame_api::color::SCENE_COLOR))
            .unwrap_or(target);
        let attachments = storage.retain_boxed_slice(Box::new([Some(
            wgpu::RenderPassColorAttachment {
                view: scene_color,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            },
        )]));
        Some(wgpu::RenderPassDescriptor {
            label: Some("Cable overlay"),
            color_attachments: attachments,
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        })
    }

    fn prepare(&mut self, ctx: &PrepareContext) -> HelioResult<()> {
        self.connector_count = CABLE_CONNECTORS
            .resolve(ctx.scene_buffers)
            .expect("cable connector row ABI")
            .map_or(0, |buffer| {
                (buffer.buffer.size() / std::mem::size_of::<cables_api::CableConnectorRow>() as u64)
                    .min(u64::from(MAX_CONNECTORS)) as u32
            });
        self.definition_count = CABLE_DEFINITIONS
            .resolve(ctx.scene_buffers)
            .expect("cable definition row ABI")
            .map_or(0, |buffer| {
                (buffer.buffer.size() / std::mem::size_of::<cables_api::CableDefinitionRow>() as u64)
                    .min(u64::from(MAX_CONNECTORS)) as u32
            });
        ctx.write_buffer(
            &self.uniforms,
            0,
            bytemuck::bytes_of(&CableUniforms {
                connector_count: self.connector_count,
                definition_count: self.definition_count,
                frame: ctx.frame_num as u32,
                _pad: 0,
            }),
        );
        Ok(())
    }

    fn execute(&mut self, ctx: &mut PassContext) -> HelioResult<()> {
        let connector = ctx
            .scene_buffers
            .get(CABLE_CONNECTORS.key())
            .map(|handle| &handle.buffer)
            .unwrap_or(&self.fallback_connectors);
        let definitions = ctx
            .scene_buffers
            .get(CABLE_DEFINITIONS.key())
            .map(|handle| &handle.buffer)
            .unwrap_or(&self.fallback_definitions);
        let key = [connector.clone(), definitions.clone()];
        if self.bound_buffers.as_ref() != Some(&key) {
            self.bind_group = Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Cable scene rows"),
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.uniforms.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: connector.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: definitions.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: ctx.camera.as_entire_binding(),
                    },
                ],
            }));
            self.bound_buffers = Some(key);
        }
        if self.connector_count < 2 || self.definition_count == 0 {
            return Ok(());
        }
        let textures = ctx
            .registry
            .get::<helio_mats::MaterialTextureBindings<'_>>(helio_mats::material_textures());
        let Some(textures) = textures else { return Ok(()); };
        let mut pass = ctx.render_cmds().ok_or_else(|| {
            helio_core::Error::ResourceNotFound("cable overlay render pass".into())
        })?;
        if self.texture_version != Some(textures.version) || self.texture_bind_group.is_none() {
            let mut entries = Vec::new();
            self.texture_binding.append_bind_group_entries(
                &mut entries,
                0,
                textures.texture_views,
                textures.sampler,
            );
            self.texture_bind_group = Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Cable material texture bindings"),
                layout: &self.texture_bind_group_layout,
                entries: &entries,
            }));
            self.texture_version = Some(textures.version);
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, self.bind_group.as_ref().expect("bind group built"), &[]);
        pass.set_bind_group(1, self.texture_bind_group.as_ref().expect("texture group built"), &[]);
        // Each candidate endpoint pair and bundle lane is an instance. The
        // vertex shader rejects nonmatching IDs and emits only valid spans.
        let instances = self
            .connector_count
            .saturating_mul(self.connector_count.saturating_sub(1))
            .saturating_mul(MAX_BUNDLE_STRANDS);
        pass.draw(0..SEGMENTS * TUBE_SIDES * 6, 0..instances);
        Ok(())
    }
}

fn cable_shader_source(binding: helio_mats::MaterialBindingConfig) -> String {
    let src = include_str!("cables.wgsl");
    if binding.uses_binding_arrays() {
        src.replace(
            "binding_array<texture_2d<f32>, 256>",
            &format!("binding_array<texture_2d<f32>, {}>", binding.max_textures),
        )
    } else {
        helio_mats::apply_webgpu_decal_bindings(src, binding.max_textures)
    }
}
