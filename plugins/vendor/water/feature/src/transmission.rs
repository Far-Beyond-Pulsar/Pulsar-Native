//! Water's lighting input: the light it focuses onto what lies under it
//! (caustics), published as the frame's `transmission`
//! ([`helio_frame_api::lighting::TRANSMISSION`]). The lighting model adds it
//! without knowing water exists.
//!
//! Reads the caustics textures [`crate::WaterSimPass`] publishes (last
//! frame's, as the lighting always did: the simulation runs after lighting).

use helio_core::graph::{ResourceBuilder, ResourceSize};
use helio_core::{PassContext, PrepareContext, RenderPass, ResourceKey, Result as HelioResult};
use helio_frame_api::lighting::{
    TRANSMISSION, TRANSMISSION_FORMAT, VIEW_REMAPS, VIEW_REMAPS_BYTES, VIEW_REMAP_SNIPPET,
};
use pulsar_scenedb::gpu::BufferKey;
use wgpu::util::DeviceExt;

/// Writes `transmission` while the scene has water.
pub struct WaterTransmissionPass {
    pipeline: wgpu::RenderPipeline,
    frame_layout: wgpu::BindGroupLayout,
    texture_layout: wgpu::BindGroupLayout,
    phase: wgpu::Buffer,
    sampler: wgpu::Sampler,
    fallback_remaps: wgpu::Buffer,
    frame_group: Option<([wgpu::Buffer; 3], wgpu::BindGroup)>,
    texture_group: Option<([wgpu::TextureView; 4], wgpu::BindGroup)>,
}

fn entry(binding: u32, ty: wgpu::BindingType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty,
        count: None,
    }
}

fn view(binding: u32, view: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::TextureView(view),
    }
}

fn texture(sample_type: wgpu::TextureSampleType) -> wgpu::BindingType {
    wgpu::BindingType::Texture {
        sample_type,
        view_dimension: wgpu::TextureViewDimension::D2,
        multisampled: false,
    }
}

impl WaterTransmissionPass {
    pub fn new(device: &wgpu::Device) -> Self {
        let buffer = |read_only: bool| wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        let uniform = |size: u64| wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: wgpu::BufferSize::new(size),
        };
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Water transmission frame"),
            entries: &[
                entry(0, buffer(true)),
                entry(1, buffer(true)),
                entry(2, uniform(16)),
                entry(3, uniform(VIEW_REMAPS_BYTES)),
            ],
        });
        let filterable = wgpu::TextureSampleType::Float { filterable: true };
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Water transmission textures"),
            entries: &[
                entry(0, texture(wgpu::TextureSampleType::Depth)),
                entry(
                    1,
                    texture(wgpu::TextureSampleType::Float { filterable: false }),
                ),
                entry(2, texture(filterable)),
                entry(3, texture(filterable)),
                entry(
                    4,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                ),
            ],
        });
        let shader = helio_core::shader::module_with(
            device,
            "Water transmission",
            helio_core::include_wgsl!("../shaders/transmission.wgsl"),
            &[VIEW_REMAP_SNIPPET],
        );
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Water transmission"),
            bind_group_layouts: &[Some(&frame_layout), Some(&texture_layout)],
            immediate_size: 0,
        });
        let pipeline = helio_core::shader::create_render_pipeline(
            device,
            wgpu::RenderPipelineDescriptor {
                label: Some("Water transmission"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: TRANSMISSION_FORMAT,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            },
        );
        let phase = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Water transmission phase"),
            contents: &[0u8; 16],
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        // Bound until a view remap is published; only remapped pixels index it.
        let fallback_remaps = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Water transmission: no view remaps"),
            size: VIEW_REMAPS_BYTES,
            usage: wgpu::BufferUsages::UNIFORM,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Water transmission caustics"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        Self {
            pipeline,
            frame_layout,
            texture_layout,
            phase,
            sampler,
            fallback_remaps,
            frame_group: None,
            texture_group: None,
        }
    }
}

impl RenderPass for WaterTransmissionPass {
    fn name(&self) -> &'static str {
        "WaterTransmission"
    }

    fn scene_buffers(&self) -> &'static [&'static str] {
        &["water_volumes", VIEW_REMAPS]
    }

    /// A member while the scene has water, like the simulation.
    fn is_member(&mut self, scene: &mut helio_core::SceneContent<'_>) -> bool {
        scene.holds_rows("water_volumes")
    }

    fn reads(&self) -> &'static [&'static str] {
        &[
            "depth",
            helio_frame_api::gbuffer::LIGHTMAP_UV,
            "water_caustics",
            "water_caustics_deep",
        ]
    }

    fn declare_resources(&self, builder: &mut ResourceBuilder) {
        builder.write_color_raw(
            TRANSMISSION,
            TRANSMISSION_FORMAT,
            ResourceSize::MatchSurface,
        );
    }

    fn prepare(&mut self, ctx: &PrepareContext) -> HelioResult<()> {
        // Periodic over 12 m: reduced from the f64 origin, never through an
        // f32 world position.
        let phase = [
            ctx.world_origin.x.rem_euclid(12.0) as f32,
            ctx.world_origin.z.rem_euclid(12.0) as f32,
            0.0,
            0.0,
        ];
        ctx.write_buffer(&self.phase, 0, bytemuck::cast_slice(&phase));
        Ok(())
    }

    fn render_pass_descriptor_with_storage<'a>(
        &'a self,
        _target: &'a wgpu::TextureView,
        _depth: &'a wgpu::TextureView,
        resources: &'a helio_core::ResourceRegistry<'a>,
        storage: &'a mut helio_core::RenderFrameStorage,
    ) -> Option<wgpu::RenderPassDescriptor<'a>> {
        let target = resources.texture_view(ResourceKey::new(TRANSMISSION))?;
        let color_attachments: &'a [Option<wgpu::RenderPassColorAttachment<'a>>] = storage
            .retain_boxed_slice(Box::new([Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })]));
        Some(wgpu::RenderPassDescriptor {
            label: Some("WaterTransmission"),
            color_attachments,
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        })
    }

    fn execute(&mut self, ctx: &mut PassContext) -> HelioResult<()> {
        let Some(volumes) = ctx
            .scene_buffers
            .get(BufferKey::of("water_volumes"))
            .map(|handle| handle.buffer.clone())
        else {
            return Ok(());
        };
        let (Some(near), Some(deep), Some(lightmap_uv)) = (
            ctx.registry.get(ResourceKey::new("water_caustics")),
            ctx.registry.get(ResourceKey::new("water_caustics_deep")),
            ctx.registry
                .get(ResourceKey::new(helio_frame_api::gbuffer::LIGHTMAP_UV)),
        ) else {
            return Ok(());
        };
        let remaps = ctx
            .scene_buffers
            .get(BufferKey::of(VIEW_REMAPS))
            .map_or_else(
                || self.fallback_remaps.clone(),
                |handle| handle.buffer.clone(),
            );
        let frame_key = [ctx.camera.clone(), volumes, remaps];
        if self
            .frame_group
            .as_ref()
            .is_none_or(|(key, _)| key != &frame_key)
        {
            let group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Water transmission frame"),
                layout: &self.frame_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: frame_key[0].as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: frame_key[1].as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.phase.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: frame_key[2].as_entire_binding(),
                    },
                ],
            });
            self.frame_group = Some((frame_key, group));
        }
        let texture_key = [ctx.depth, lightmap_uv, near, deep].map(Clone::clone);
        if self
            .texture_group
            .as_ref()
            .is_none_or(|(key, _)| key != &texture_key)
        {
            let group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Water transmission textures"),
                layout: &self.texture_layout,
                entries: &[
                    view(0, &texture_key[0]),
                    view(1, &texture_key[1]),
                    view(2, &texture_key[2]),
                    view(3, &texture_key[3]),
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
            self.texture_group = Some((texture_key, group));
        }
        let (Some((_, frame)), Some((_, textures))) = (&self.frame_group, &self.texture_group)
        else {
            return Ok(());
        };
        let mut pass = ctx
            .render_cmds()
            .expect("the graph opened this pass's render pass");
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, frame, &[]);
        pass.set_bind_group(1, textures, &[]);
        pass.draw(0..3, 0..1);
        Ok(())
    }
}
