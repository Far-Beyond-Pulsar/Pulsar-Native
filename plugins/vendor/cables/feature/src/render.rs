use cables_api::{CABLE_CONNECTORS, CABLE_DEFINITIONS};
use helio_core::{PassContext, PrepareContext, RenderPass, Result as HelioResult};

const MAX_CONNECTORS: u32 = 128;
const MAX_BUNDLE_STRANDS: u32 = 64;
const SEGMENTS: u32 = 32;
const TUBE_SIDES: u32 = 12;
/// Instances each list (ribbon, tube) can hold. A pair with a bundle adds one
/// instance per lane; instances past this are dropped by the build pass and
/// skipped by the vertex stage.
const INSTANCE_CAPACITY: u32 = 32_768;
const RIBBON_VERTICES: u32 = SEGMENTS * 6;
const TUBE_VERTICES: u32 = SEGMENTS * TUBE_SIDES * 6;
const UNIFORMS_WGSL: &str =
    "struct CableUniforms { connector_count: u32, definition_count: u32, frame: u32, _pad: u32, };";

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CableUniforms {
    connector_count: u32,
    definition_count: u32,
    frame: u32,
    _pad: u32,
}

/// Two `DrawIndirect` records (ribbon, tube). The build pass bumps each
/// `instance_count` on the GPU; the vertex counts are constant.
fn indirect_args() -> [u32; 8] {
    [RIBBON_VERTICES, 0, 0, 0, TUBE_VERTICES, 0, 0, 0]
}

/// The WGSL both shaders share, generated from the Rust constants so the two
/// languages cannot drift.
fn common_wgsl() -> String {
    format!(
        "const CONNECTOR_WORDS: u32 = {}u;\n\
         const DEFINITION_WORDS: u32 = {}u;\n\
         const BUNDLE_LANES: u32 = {MAX_BUNDLE_STRANDS}u;\n\
         const SEGMENTS: u32 = {SEGMENTS}u;\n\
         const TUBE_SIDES: u32 = {TUBE_SIDES}u;\n\
         const INSTANCE_CAPACITY: u32 = {INSTANCE_CAPACITY}u;\n\
         const TAU: f32 = 6.283185307179586;\n\
         fn fword(word: u32) -> f32 {{ return bitcast<f32>(word); }}\n",
        std::mem::size_of::<cables_api::CableConnectorRow>() / 4,
        std::mem::size_of::<cables_api::CableDefinitionRow>() / 4,
    )
}

/// Buffers the build and draw passes hand each other.
#[derive(Clone)]
struct CableBuffers {
    uniforms: wgpu::Buffer,
    args: wgpu::Buffer,
    instances: wgpu::Buffer,
    fallback_connectors: wgpu::Buffer,
    fallback_definitions: wgpu::Buffer,
}

impl CableBuffers {
    fn new(device: &wgpu::Device) -> Self {
        let buffer = |label, size: u64, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        Self {
            uniforms: buffer(
                "Cable view uniforms",
                std::mem::size_of::<CableUniforms>() as u64,
                wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            ),
            args: buffer(
                "Cable indirect args",
                std::mem::size_of::<[u32; 8]>() as u64,
                wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::INDIRECT
                    | wgpu::BufferUsages::COPY_DST,
            ),
            instances: buffer(
                "Cable pair instances",
                u64::from(INSTANCE_CAPACITY) * 2 * 16,
                wgpu::BufferUsages::STORAGE,
            ),
            fallback_connectors: buffer(
                "Empty cable connector rows",
                std::mem::size_of::<cables_api::CableConnectorRow>() as u64,
                wgpu::BufferUsages::STORAGE,
            ),
            fallback_definitions: buffer(
                "Empty cable definition rows",
                std::mem::size_of::<cables_api::CableDefinitionRow>() as u64,
                wgpu::BufferUsages::STORAGE,
            ),
        }
    }

    /// The scene's connector and definition rows, or empty stand-ins.
    fn rows<'a>(
        &'a self,
        scene: &'a helio_core::SceneBufferProjection,
    ) -> (&'a wgpu::Buffer, &'a wgpu::Buffer) {
        (
            scene
                .get(CABLE_CONNECTORS.key())
                .map(|handle| &handle.buffer)
                .unwrap_or(&self.fallback_connectors),
            scene
                .get(CABLE_DEFINITIONS.key())
                .map(|handle| &handle.buffer)
                .unwrap_or(&self.fallback_definitions),
        )
    }
}

/// Rows the passes consider: buffer capacity capped at `MAX_CONNECTORS`. The
/// placed rows are packed and dead ones are zero, so the GPU build pass skips
/// them; the CPU never learns a live count.
fn row_counts(scene: &helio_core::SceneBufferProjection) -> (u32, u32) {
    let rows = |size: u64, stride: usize| {
        (size / stride as u64).min(u64::from(MAX_CONNECTORS)) as u32
    };
    let connectors = scene.get(CABLE_CONNECTORS.key()).map_or(0, |h| {
        rows(h.buffer.size(), std::mem::size_of::<cables_api::CableConnectorRow>())
    });
    let definitions = scene.get(CABLE_DEFINITIONS.key()).map_or(0, |h| {
        rows(h.buffer.size(), std::mem::size_of::<cables_api::CableDefinitionRow>())
    });
    (connectors, definitions)
}

fn storage_entry(
    binding: u32,
    read_only: bool,
    stage: wgpu::ShaderStages,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: stage,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn uniform_entry(
    binding: u32,
    size: u64,
    stage: wgpu::ShaderStages,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: stage,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: wgpu::BufferSize::new(size),
        },
        count: None,
    }
}

fn entry(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: buffer.as_entire_binding(),
    }
}

/// The pair expansion and the draw are built together: the build pass fills
/// the instance lists and indirect args the draw pass consumes.
pub fn build_passes(
    device: &wgpu::Device,
    texture_binding: helio_mats::MaterialBindingConfig,
    color_format: wgpu::TextureFormat,
) -> (CableBuildPass, CableRenderPass) {
    let buffers = CableBuffers::new(device);
    (
        CableBuildPass::new(device, buffers.clone()),
        CableRenderPass::new(device, buffers, texture_binding, color_format),
    )
}

/// Expands live connector pairs into ribbon and tube instance lists and
/// indirect draw arguments on the GPU. Compute-only; runs just before
/// [`CableRenderPass`] on the graphics stream.
pub struct CableBuildPass {
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    buffers: CableBuffers,
    connector_count: u32,
    definition_count: u32,
    bound: Option<([wgpu::Buffer; 2], wgpu::BindGroup)>,
}

impl CableBuildPass {
    fn new(device: &wgpu::Device, buffers: CableBuffers) -> Self {
        let c = wgpu::ShaderStages::COMPUTE;
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Cable build bindings"),
            entries: &[
                uniform_entry(0, std::mem::size_of::<CableUniforms>() as u64, c),
                storage_entry(1, true, c),
                storage_entry(2, true, c),
                storage_entry(3, false, c),
                storage_entry(4, false, c),
            ],
        });
        let source = format!(
            "{}\n{UNIFORMS_WGSL}\n{}",
            common_wgsl(),
            include_str!("cables_build.wgsl"),
        );
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("GPU cable pair expansion"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Cable build pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("GPU cable pair expansion"),
            layout: Some(&layout),
            module: &shader,
            entry_point: Some("build"),
            compilation_options: Default::default(),
            cache: None,
        });
        Self {
            pipeline,
            bind_group_layout,
            buffers,
            connector_count: 0,
            definition_count: 0,
            bound: None,
        }
    }
}

impl RenderPass for CableBuildPass {
    fn name(&self) -> &'static str {
        "CableBuildPass"
    }

    fn scene_buffers(&self) -> &'static [&'static str] {
        &["cable_connectors", "cable_definitions"]
    }

    fn is_member(&mut self, scene: &mut helio_core::SceneContent<'_>) -> bool {
        scene.holds_rows("cable_definitions") && scene.holds_rows("cable_connector_sources")
    }

    fn render_pass_descriptor<'a>(
        &'a self,
        _target: &'a wgpu::TextureView,
        _depth: &'a wgpu::TextureView,
        _resources: &'a helio_core::ResourceRegistry<'a>,
    ) -> Option<wgpu::RenderPassDescriptor<'a>> {
        None // compute-only pass
    }

    fn prepare(&mut self, ctx: &PrepareContext) -> HelioResult<()> {
        (self.connector_count, self.definition_count) = row_counts(ctx.scene_buffers);
        ctx.write_buffer(
            &self.buffers.uniforms,
            0,
            bytemuck::bytes_of(&CableUniforms {
                connector_count: self.connector_count,
                definition_count: self.definition_count,
                frame: ctx.frame_num as u32,
                _pad: 0,
            }),
        );
        ctx.write_buffer(&self.buffers.args, 0, bytemuck::bytes_of(&indirect_args()));
        Ok(())
    }

    fn execute(&mut self, ctx: &mut PassContext) -> HelioResult<()> {
        if self.connector_count < 2 || self.definition_count == 0 {
            return Ok(());
        }
        let (connectors, definitions) = self.buffers.rows(ctx.scene_buffers);
        let key = [connectors.clone(), definitions.clone()];
        if self.bound.as_ref().map(|(k, _)| k) != Some(&key) {
            let group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Cable build rows"),
                layout: &self.bind_group_layout,
                entries: &[
                    entry(0, &self.buffers.uniforms),
                    entry(1, connectors),
                    entry(2, definitions),
                    entry(3, &self.buffers.args),
                    entry(4, &self.buffers.instances),
                ],
            });
            self.bound = Some((key, group));
        }
        let Some((_, group)) = self.bound.as_ref() else {
            return Ok(());
        };
        let mut encoder = ctx.graphics_cmds();
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Cable pair expansion"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, group, &[]);
        let threads = self.connector_count * self.connector_count;
        pass.dispatch_workgroups(threads.div_ceil(64), 1, 1);
        Ok(())
    }
}

/// Draws the build pass's ribbon and tube lists with two indirect draws.
pub struct CableRenderPass {
    ribbon_pipeline: wgpu::RenderPipeline,
    tube_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    texture_binding: helio_mats::MaterialBindingConfig,
    texture_bind_group_layout: wgpu::BindGroupLayout,
    texture_bind_group: Option<wgpu::BindGroup>,
    texture_version: Option<u64>,
    buffers: CableBuffers,
    connector_count: u32,
    definition_count: u32,
    bind_group: Option<wgpu::BindGroup>,
    bound_buffers: Option<[wgpu::Buffer; 3]>,
}

impl CableRenderPass {
    fn new(
        device: &wgpu::Device,
        buffers: CableBuffers,
        texture_binding: helio_mats::MaterialBindingConfig,
        color_format: wgpu::TextureFormat,
    ) -> Self {
        let v = wgpu::ShaderStages::VERTEX;
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Cable render bindings"),
            entries: &[
                uniform_entry(0, std::mem::size_of::<CableUniforms>() as u64, v),
                storage_entry(1, true, v),
                storage_entry(2, true, v),
                // The WGSL camera prefix is view, projection and
                // view-projection: three 64-byte matrices.
                uniform_entry(3, 192, v),
                storage_entry(4, true, v),
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
        let pipeline = |label, vertex_entry| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vertex_entry),
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
            })
        };
        Self {
            ribbon_pipeline: pipeline("GPU cable ribbons", "vs_ribbon"),
            tube_pipeline: pipeline("GPU cable tubes", "vs_tube"),
            bind_group_layout,
            texture_binding,
            texture_bind_group_layout,
            texture_bind_group: None,
            texture_version: None,
            buffers,
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

    fn is_member(&mut self, scene: &mut helio_core::SceneContent<'_>) -> bool {
        scene.holds_rows("cable_definitions") && scene.holds_rows("cable_connector_sources")
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
        (self.connector_count, self.definition_count) = row_counts(ctx.scene_buffers);
        Ok(())
    }

    fn execute(&mut self, ctx: &mut PassContext) -> HelioResult<()> {
        if self.connector_count < 2 || self.definition_count == 0 {
            return Ok(());
        }
        let (connectors, definitions) = self.buffers.rows(ctx.scene_buffers);
        let key = [connectors.clone(), definitions.clone(), ctx.camera.clone()];
        if self.bound_buffers.as_ref() != Some(&key) {
            self.bind_group = Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Cable scene rows"),
                layout: &self.bind_group_layout,
                entries: &[
                    entry(0, &self.buffers.uniforms),
                    entry(1, connectors),
                    entry(2, definitions),
                    entry(3, ctx.camera),
                    entry(4, &self.buffers.instances),
                ],
            }));
            self.bound_buffers = Some(key);
        }
        let textures = ctx
            .registry
            .get::<helio_mats::MaterialTextureBindings<'_>>(helio_mats::material_textures());
        let Some(textures) = textures else {
            return Ok(());
        };
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
        pass.set_bind_group(0, self.bind_group.as_ref().expect("bind group built"), &[]);
        pass.set_bind_group(1, self.texture_bind_group.as_ref().expect("texture group built"), &[]);
        // The build pass wrote each list's instance count; the GPU decides how
        // many pairs and lanes draw.
        pass.set_pipeline(&self.ribbon_pipeline);
        pass.draw_indirect(&self.buffers.args, 0);
        pass.set_pipeline(&self.tube_pipeline);
        pass.draw_indirect(&self.buffers.args, 16);
        Ok(())
    }
}

fn cable_shader_source(binding: helio_mats::MaterialBindingConfig) -> String {
    let src = include_str!("cables.wgsl").replace("// @common", &common_wgsl());
    if binding.uses_binding_arrays() {
        src.replace(
            "binding_array<texture_2d<f32>, 256>",
            &format!("binding_array<texture_2d<f32>, {}>", binding.max_textures),
        )
    } else {
        helio_mats::apply_webgpu_decal_bindings(&src, binding.max_textures)
    }
}
