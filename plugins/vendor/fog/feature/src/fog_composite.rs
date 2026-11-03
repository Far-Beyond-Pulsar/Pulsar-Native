//! Scene-linear participating-medium composite.
//!
//! Reads opaque HDR (`pre_aa` by default), scene depth, and the
//! `fog_accum`/`fog_parameters` pair published by `VolumetricFogPass`, and
//! writes `fogged_hdr`: `color * T + S` at each pixel's depth.
//!
//! Placement contract (see `helio-pass-volumetric-fog/README.md`):
//!
//! ```text
//! opaque lighting -> VolumetricFogPass -> FogCompositePass -> Transparent
//!   (through the published media: each fragment fogged at its own depth)
//!   -> TSR/FXAA -> LensFlarePass -> PostProcessPass (meter, bloom, lens, tonemap)
//! ```
//!
//! Compositing here rather than in the uber shader is what lets exposure
//! metering, bloom and lens extraction see scattered light: bright shafts
//! bloom and flare because they are real radiance in the image they read.
//! Missing producers bind a zero-range fallback and the pass is a copy.
//!
//! When no source can put a medium in the grid (checked on the CPU, below),
//! the composite would be exactly that copy, so the pass draws nothing and
//! publishes its input's view as `fogged_hdr` instead. The feature advances
//! the colour chain with `color::advance_aliasing`, so readers of
//! `fogged_hdr` also declare a read of the input and the pool never reuses
//! its memory while they may still see it.

use helio_core::graph::{ResourceBuilder, ResourceSize};
use helio_core::{PassContext, PrepareContext, RenderPass, ResourceKey, Result as HelioResult};
use pulsar_scenedb::gpu::BufferKey;

/// Graph key of the fogged scene-linear image.
pub const FOGGED_HDR: &str = "fogged_hdr";
/// Default format. Graphs whose lighting target is packed HDR (e.g. HLFS's
/// Rg11b10Ufloat) pass that format so transparency can blend into it.
pub const FOGGED_HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

pub struct FogCompositePass {
    input: &'static str,
    format: wgpu::TextureFormat,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    fallback_medium: wgpu::TextureView,
    fallback_parameters: wgpu::Buffer,
    bind_group: Option<wgpu::BindGroup>,
    bind_group_key: Option<(
        wgpu::TextureView,
        wgpu::TextureView,
        wgpu::TextureView,
        wgpu::Buffer,
        wgpu::Buffer,
    )>,
    /// SceneDB media rows VolumetricFogPass classifies; any non-zero row
    /// counts as a possible medium.
    global_media: helio_core::SceneBufferLiveness,
    local_media: helio_core::SceneBufferLiveness,
    legacy_media: helio_core::SceneBufferLiveness,
    /// Some source may have held a medium last frame.
    medium_last_frame: bool,
    /// No source can have held a medium this frame or last, so the grid is
    /// neutral and compositing is the identity. VolumetricFogPass still
    /// integrates the frame after a medium disappears, hence both frames.
    fog_quiet: bool,
}

impl FogCompositePass {
    pub fn new(device: &wgpu::Device) -> Self {
        Self::with_format(device, FOGGED_HDR_FORMAT)
    }

    pub fn with_format(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = helio_core::shader::module(
            device,
            "Fog Composite",
            helio_core::include_wgsl!("../shaders/fog_composite.wgsl"),
        );
        let fragment = wgpu::ShaderStages::FRAGMENT;
        let texture = |binding, sample_type, view_dimension| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: fragment,
            ty: wgpu::BindingType::Texture {
                sample_type,
                view_dimension,
                multisampled: false,
            },
            count: None,
        };
        let buffer = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: fragment,
            ty: wgpu::BindingType::Buffer {
                ty,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Fog Composite BGL"),
            entries: &[
                buffer(0, wgpu::BufferBindingType::Storage { read_only: true }),
                buffer(1, wgpu::BufferBindingType::Uniform),
                texture(
                    2,
                    wgpu::TextureSampleType::Float { filterable: false },
                    wgpu::TextureViewDimension::D2,
                ),
                texture(
                    3,
                    wgpu::TextureSampleType::Depth,
                    wgpu::TextureViewDimension::D2,
                ),
                texture(
                    4,
                    wgpu::TextureSampleType::Float { filterable: true },
                    wgpu::TextureViewDimension::D3,
                ),
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: fragment,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Fog Composite PL"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = helio_core::shader::create_render_pipeline(
            &device,
            wgpu::RenderPipelineDescriptor {
                label: Some("Fog Composite"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_fullscreen"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_composite"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            },
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Fog Composite Sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        // Zero-initialised: max_distance = 0 makes the shader a pass-through,
        // so the medium texture's contents are never read.
        let fallback_parameters = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Fog Composite Neutral Parameters"),
            size: std::mem::size_of::<helio_pass_postprocess::GpuFogUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM,
            mapped_at_creation: false,
        });
        let fallback_medium = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("Fog Composite Neutral Medium"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D3,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        Self {
            input: "pre_aa",
            format,
            pipeline,
            layout,
            sampler,
            fallback_medium,
            fallback_parameters,
            bind_group: None,
            bind_group_key: None,
            global_media: helio_core::SceneBufferLiveness::default(),
            local_media: helio_core::SceneBufferLiveness::default(),
            legacy_media: helio_core::SceneBufferLiveness::default(),
            medium_last_frame: true,
            fog_quiet: false,
        }
    }

    /// Select the opaque scene-linear image to fog. Defaults to `pre_aa`.
    pub fn with_color_input(mut self, key: &'static str) -> Self {
        self.input = key;
        self
    }

    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    /// The input view to publish as `fogged_hdr` instead of compositing, when
    /// the grid is certainly neutral and the input can stand in for the pooled
    /// target (same format, size and sample count, and renderable, since
    /// TransparentPass draws into `fogged_hdr`).
    fn pass_through_view<'a>(
        &self,
        resources: &helio_core::ResourceRegistry<'a>,
    ) -> Option<&'a wgpu::TextureView> {
        // Readers of fogged_hdr extend the input's lifetime (the chain alias).
        if !self.fog_quiet {
            return None;
        }
        let input = resources.get::<&wgpu::TextureView>(ResourceKey::new(self.input))?;
        let target = resources.get::<&wgpu::TextureView>(ResourceKey::new(FOGGED_HDR))?;
        let (i, t) = (input.texture(), target.texture());
        let compatible = i.format() == t.format()
            && i.size() == t.size()
            && i.sample_count() == t.sample_count()
            && i.usage().contains(wgpu::TextureUsages::RENDER_ATTACHMENT);
        compatible.then_some(input)
    }
}

impl RenderPass for FogCompositePass {
    fn name(&self) -> &'static str {
        "FogComposite"
    }

    fn reads(&self) -> &'static [&'static str] {
        &["depth", crate::FOG_ACCUM, crate::FOG_PARAMETERS]
    }

    fn writes(&self) -> &'static [&'static str] {
        &[FOGGED_HDR]
    }

    fn prepare(&mut self, ctx: &PrepareContext) -> HelioResult<()> {
        // Mirrors VolumetricFogPass's classification (`cs_classify`): a medium
        // comes from the resolved post-process fog block, weighted volume rows,
        // or global/local media rows. Unknown row contents count as a medium.
        let mut medium = ctx
            .registry
            .get::<bool>(ResourceKey::new(helio_pass_postprocess::FOG_SETTINGS_MAYBE_ACTIVE))
            .unwrap_or(true);
        for (liveness, key) in [
            (&mut self.global_media, "global_fog_media"),
            (&mut self.local_media, "local_fog_media"),
            (&mut self.legacy_media, "fog_components"),
        ] {
            let rows = ctx.scene_buffers.get(BufferKey::of(key));
            liveness.update(ctx.device, ctx.queue, rows);
            medium |= rows.is_some_and(|handle| liveness.maybe_live(handle));
        }
        self.fog_quiet = !medium && !self.medium_last_frame;
        self.medium_last_frame = medium;
        Ok(())
    }

    fn declare_resources(&self, builder: &mut ResourceBuilder) {
        builder.read(self.input);
        builder.read(crate::FOG_ACCUM);
        builder.read(crate::FOG_PARAMETERS);
        builder.write_color_raw(FOGGED_HDR, self.format, ResourceSize::MatchSurface);
    }

    fn render_pass_descriptor_with_storage<'a>(
        &'a self,
        _target: &'a wgpu::TextureView,
        _depth: &'a wgpu::TextureView,
        resources: &'a helio_core::ResourceRegistry<'a>,
        storage: &'a mut helio_core::RenderFrameStorage,
    ) -> Option<wgpu::RenderPassDescriptor<'a>> {
        if self.pass_through_view(resources).is_some() {
            return None;
        }
        let target = resources.get(ResourceKey::new(FOGGED_HDR))?;
        let color_attachments: &'a [Option<wgpu::RenderPassColorAttachment<'a>>] = storage
            .retain_boxed_slice(Box::new([Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                depth_slice: None,
                // Every pixel is written by the full-screen triangle.
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })]));
        Some(wgpu::RenderPassDescriptor {
            label: Some("Fog Composite"),
            color_attachments,
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        })
    }

    fn execute(&mut self, ctx: &mut PassContext) -> HelioResult<()> {
        if self.pass_through_view(ctx.registry).is_some() {
            return Ok(());
        }
        let input: &wgpu::TextureView = ctx
            .registry
            .read(ResourceKey::new(self.input), "FogComposite")
            .ok_or_else(|| {
                helio_core::Error::InvalidPassConfig(format!(
                    "FogCompositePass requires published {} input",
                    self.input
                ))
            })?;
        let medium = ctx
            .registry
            .get::<&wgpu::TextureView>(ResourceKey::new(crate::FOG_ACCUM))
            .unwrap_or(&self.fallback_medium);
        let parameters = ctx
            .registry
            .get::<&wgpu::Buffer>(ResourceKey::new(crate::FOG_PARAMETERS))
            .unwrap_or(&self.fallback_parameters);
        let key = (
            input.clone(),
            ctx.depth.clone(),
            medium.clone(),
            parameters.clone(),
            ctx.camera.clone(),
        );
        if self.bind_group_key.as_ref() != Some(&key) {
            self.bind_group = Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Fog Composite BG"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: ctx.camera.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: parameters.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(input),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(ctx.depth),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(medium),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            }));
            self.bind_group_key = Some(key);
        }
        let mut pass = ctx.render_cmds().unwrap();
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, self.bind_group.as_ref().unwrap(), &[]);
        pass.draw(0..3, 0..1);
        Ok(())
    }

    fn publish<'a>(&self, frame: &mut helio_core::ResourceRegistry<'a>) {
        if let Some(input) = self.pass_through_view(frame) {
            frame.route_named_texture(FOGGED_HDR, input, "FogComposite");
        }
    }

    fn on_resize(&mut self, _device: &wgpu::Device, _size: helio_core::RenderSize) {
        // The graph may reuse a pool slot on resize; view identity is not a
        // reliable generation, so rebuild on the next frame.
        self.bind_group = None;
        self.bind_group_key = None;
    }
}
