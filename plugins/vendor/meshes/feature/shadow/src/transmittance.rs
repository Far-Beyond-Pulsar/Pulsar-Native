//! Coloured shadow transmittance for translucent casters (stained glass).
//!
//! Opaque casters write the depth atlases; transparent-only static casters
//! are drawn here instead, into an `Rgba16Float` array with one layer per
//! atlas face: rgb is 1 minus the product of every pane's transmittance in
//! front of the nearest opaque occluder, alpha 1 minus the nearest pane's
//! depth. Zero therefore means unfiltered, so fresh layers need no clear. Lighting
//! and volumetric fog multiply a light by rgb for receivers behind that
//! depth, so sunlight through a window arrives coloured.

use helio_core::{CommandRecorder, RenderCmds};

/// Graph key under which the transmittance array view is published.
pub const TRANSMITTANCE_KEY: &str = "shadow_transmittance";
pub const TRANSMITTANCE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

pub(crate) struct Transmittance {
    pipeline: wgpu::RenderPipeline,
    clear_pipeline: wgpu::RenderPipeline,
    bgl_1: wgpu::BindGroupLayout,
    params: wgpu::Buffer,
    pub(crate) view: wgpu::TextureView,
    face_views: Box<[wgpu::TextureView]>,
    /// Whether each face may hold a pane from its last render. A face that
    /// does not is all zero (textures start zeroed, and emptied faces are
    /// cleared once), so re-rendering it with no translucent caster is skipped.
    bg_1: Option<wgpu::BindGroup>,
    bg_1_key: Option<(wgpu::Buffer, wgpu::TextureView)>,
}

impl Transmittance {
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bgl_0: &wgpu::BindGroupLayout,
        atlas_size: u32,
        layers: u32,
    ) -> Self {
        // Half the depth resolution: the tint is low-frequency next to the
        // shadow edge itself, and the opaque edge still comes from the depth.
        let size = (atlas_size / 2).max(1);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Shadow/Transmittance"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: layers,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TRANSMITTANCE_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("Shadow/Transmittance Array"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let face_views = (0..layers)
            .map(|layer| {
                texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("Shadow/TransmittanceFace"),
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: layer,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();

        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Shadow/TransmittanceParams"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let depth_scale = atlas_size as f32 / size as f32;
        queue.write_buffer(
            &params,
            0,
            bytemuck::cast_slice(&[depth_scale, 0.0, 0.0, 0.0]),
        );

        let fs = wgpu::ShaderStages::FRAGMENT;
        let bgl_1 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Shadow/Transmittance BGL 1"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: fs,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: fs,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: fs,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let shader = helio_core::shader::module(
            device,
            "Shadow/Transmittance",
            helio_core::include_wgsl!("../shaders/shadow_transmittance.wgsl"),
        );
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Shadow/Transmittance PL"),
            bind_group_layouts: &[Some(bgl_0), Some(&bgl_1)],
            immediate_size: 0,
        });
        let pipeline = helio_core::shader::create_render_pipeline(
            &device,
            wgpu::RenderPipelineDescriptor {
                label: Some("Shadow/Transmittance Pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[
                        Some(wgpu::VertexBufferLayout {
                            array_stride: 40,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &[wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x3,
                                offset: 0,
                                shader_location: 0,
                            }],
                        }),
                        Some(helio_pass_gbuffer::DRAW_SLOT_LAYOUT),
                    ],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: TRANSMITTANCE_FORMAT,
                        blend: Some(wgpu::BlendState {
                            // rgb = 1 - T: src (1 - dst) + dst = 1 - T_src T_dst,
                            // so panes in series multiply.
                            color: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::OneMinusDst,
                                dst_factor: wgpu::BlendFactor::One,
                                operation: wgpu::BlendOperation::Add,
                            },
                            // a = 1 - nearest pane depth.
                            alpha: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::One,
                                dst_factor: wgpu::BlendFactor::One,
                                operation: wgpu::BlendOperation::Max,
                            },
                        }),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    // Panes are single sheets: both sides filter the light.
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            },
        );

        let clear_source = format!(
            "{}\n@fragment fn fs_clear() -> @location(0) vec4f {{ return vec4f(0); }}",
            include_str!("../shaders/depth_clear.wgsl")
        );
        let clear_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Clear transmission tile"),
            source: wgpu::ShaderSource::Wgsl(clear_source.into()),
        });
        let clear_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(bgl_0)],
            immediate_size: 0,
        });
        let clear_pipeline = helio_core::shader::create_render_pipeline(
            &device,
            wgpu::RenderPipelineDescriptor {
                label: Some("Clear transmission tile"),
                layout: Some(&clear_layout),
                vertex: wgpu::VertexState {
                    module: &clear_shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &clear_shader,
                    entry_point: Some("fs_clear"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: TRANSMITTANCE_FORMAT,
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

        Self {
            pipeline,
            clear_pipeline,
            bgl_1,
            params,
            view,
            face_views,
            bg_1: None,
            bg_1_key: None,
        }
    }

    fn clear_face<'e>(
        face_view: &wgpu::TextureView,
        cmds: &'e mut CommandRecorder<'_>,
    ) -> RenderCmds<'e> {
        cmds.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Shadow/Transmittance"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: face_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        })
    }

    /// Re-renders one face: clear to white, then every translucent static
    /// caster the static depth does not hide.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_face(
        &mut self,
        device: &wgpu::Device,
        cmds: &mut CommandRecorder<'_>,
        materials: Option<&wgpu::Buffer>,
        bg_0: &wgpu::BindGroup,
        face: usize,
        dyn_offset: u32,
        static_depth: &wgpu::TextureView,
        indirect: &wgpu::Buffer,
        draw_count: u32,
        gpu_count: Option<helio_pass_gbuffer::GpuDrawCount<'_>>,
        vertices: &wgpu::Buffer,
        slots: &wgpu::Buffer,
        indices: &wgpu::Buffer,
        tile: Option<[u32; 3]>,
    ) {
        if face >= self.face_views.len() {
            return;
        }
        // Clear only this tile; other cached transmissions share the layer.
        {
            let mut pass = Self::clear_face(&self.face_views[face], cmds);
            if let Some([x, y, size]) = tile {
                pass.set_viewport(
                    (x / 2) as f32,
                    (y / 2) as f32,
                    (size / 2) as f32,
                    (size / 2) as f32,
                    0.0,
                    1.0,
                );
                pass.set_scissor_rect(x / 2, y / 2, (size / 2).max(1), (size / 2).max(1));
            }
            pass.set_pipeline(&self.clear_pipeline);
            pass.set_bind_group(0, bg_0, &[dyn_offset]);
            pass.draw(0..3, 0..1);
        }
        let (Some(materials), true) = (materials, draw_count > 0) else {
            return;
        };
        let key = (materials.clone(), static_depth.clone());
        if self.bg_1_key.as_ref() != Some(&key) {
            self.bg_1 = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Shadow/Transmittance BG 1"),
                layout: &self.bgl_1,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: materials.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(static_depth),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.params.as_entire_binding(),
                    },
                ],
            }));
            self.bg_1_key = Some(key);
        }
        let mut pass = Self::clear_face(&self.face_views[face], cmds);
        if let Some([x, y, size]) = tile {
            pass.set_viewport(
                (x / 2) as f32,
                (y / 2) as f32,
                (size / 2) as f32,
                (size / 2) as f32,
                0.0,
                1.0,
            );
            pass.set_scissor_rect(x / 2, y / 2, (size / 2).max(1), (size / 2).max(1));
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, bg_0, &[dyn_offset]);
        pass.set_bind_group(1, self.bg_1.as_ref().unwrap(), &[]);
        pass.set_vertex_buffer(0, vertices.slice(..));
        pass.set_vertex_buffer(1, slots.slice(..));
        pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
        helio_pass_gbuffer::multi_draw_indexed_indirect(
            &mut pass, indirect, 0, draw_count, gpu_count,
        );
    }
}
