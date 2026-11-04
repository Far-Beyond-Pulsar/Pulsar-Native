pub mod feature;
pub use feature::ForwardDrawers;

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use helio_core::graph::{ResourceBuilder, ResourceSize};
use helio_core::{PassContext, PrepareContext, RenderPass, Result as HelioResult};
use helio_mats::radiant::{RadiantShaderCache, RadiantShaderKey};

use helio_light_api::{GpuLight, MAX_LIGHTS, SCENE_LIGHTS};
use pulsar_scenedb::gpu::BufferKey;

/// The meshes' forward program (`shaders/forward_lit.wgsl`): the vertex
/// layout and instance rows, lit by the lights' forward function; a
/// template's surface function is composed after it.
pub const FORWARD_PROGRAM: &str = include_str!("../shaders/forward_lit.wgsl");

const TILE_SIZE: u32 = 16;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ForwardLitGlobals {
    frame: u32,
    delta_time: f32,
    light_count: u32,
    ambient_intensity: f32,
    ambient_color: [f32; 4],
    num_tiles_x: u32,
    num_tiles_y: u32,
    screen_width: f32,
    screen_height: f32,
    _pad0: u32,
    /// The host-driven frame clock (`PrepareContext::time`): the
    /// shader-graph `time` node.
    time: f32,
    _pad1: u32,
    _pad2: u32,
}

pub struct ForwardLitPass {
    /// The draw slots the vertex shader reads (see `helio_mesh_api::DrawSlots`).
    draw_slots: helio_mesh_api::DrawSlots,
    material_binding: helio_mats::MaterialBindingConfig,
    pipelines: HashMap<RadiantShaderKey, wgpu::RenderPipeline>,
    shader_cache: RadiantShaderCache,
    /// The built-in material classes, when the shared registry is
    /// absent or lacks a class.
    local_templates: helio_mats::RadiantTemplateRegistry,
    /// The material bindings without the materials plugin (default grey
    /// PBR), made on first use.
    stand_in: Option<helio_mats::MaterialStandIn>,
    /// User-registered custom templates (from `FIRST_CUSTOM_MATERIAL_CLASS` up), shared with the renderer
    /// and other passes — never deep-cloned (see `SharedTemplateRegistry`).
    shared_registry: Option<helio_mats::radiant::SharedTemplateRegistry>,
    /// Key set as of the last sync, to detect content changes cheaply.
    last_shared_keys: Vec<u32>,
    pipeline_layout: wgpu::PipelineLayout,
    bind_group_layout_0: wgpu::BindGroupLayout,
    bind_group_layout_1: wgpu::BindGroupLayout,
    bind_group_0: Option<wgpu::BindGroup>,
    bind_group_0_key: Option<(
        wgpu::Buffer,
        wgpu::Buffer,
        wgpu::Buffer,
        usize,
        Option<wgpu::Buffer>,
        Option<wgpu::Buffer>,
        wgpu::Buffer,
        wgpu::Buffer,
    )>,
    /// Zero light counts, bound while the lights publish no binning.
    no_tiles: wgpu::Buffer,
    bind_group_1: Option<wgpu::BindGroup>,
    bind_group_1_version: Option<(u64, u64)>,
    globals_buf: wgpu::Buffer,
    surface_format: wgpu::TextureFormat,
    /// When true, renders the opaque draw segments (all opaque draws)
    /// instead of the forward ones (only FLAG_FORWARD_SHADING).
    pub render_all_opaque: bool,
}

impl ForwardLitPass {
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let material_binding = helio_mats::MaterialBindingConfig::for_device(device);
        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ForwardLitGlobals"),
            size: std::mem::size_of::<ForwardLitGlobals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group_layout_0 =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("ForwardLit BGL 0"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
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
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 4,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 5,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 6,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 9,
                        visibility: wgpu::ShaderStages::VERTEX,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 10,
                        visibility: wgpu::ShaderStages::VERTEX,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });

        let bind_group_layout_1 = create_material_bgl(device, material_binding);

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ForwardLit PL"),
            bind_group_layouts: &[Some(&bind_group_layout_0), Some(&bind_group_layout_1)],
            immediate_size: 0,
        });

        Self {
            draw_slots: Default::default(),
            material_binding,
            pipelines: HashMap::new(),
            shader_cache: RadiantShaderCache::new(),
            local_templates: helio_mats::RadiantTemplateRegistry::new(),
            stand_in: None,
            shared_registry: None,
            last_shared_keys: Vec::new(),
            pipeline_layout,
            bind_group_layout_0,
            bind_group_layout_1,
            bind_group_0: None,
            bind_group_0_key: None,
            bind_group_1: None,
            bind_group_1_version: None,
            globals_buf,
            no_tiles: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ForwardLit No Light Tiles"),
                size: 16,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            }),
            surface_format,
            render_all_opaque: false,
        }
    }

    fn get_or_create_pipeline(
        &mut self,
        device: &wgpu::Device,
        key: RadiantShaderKey,
        graph_wgsl: &str,
        write_depth: bool,
    ) -> &wgpu::RenderPipeline {
        if !self.pipelines.contains_key(&key) {
            // Ids from `FIRST_CUSTOM_MATERIAL_CLASS` up are user-registered custom templates that live in the
            // shared scene-wide registry; anything else (including a miss)
            // falls back to this pass's own class-0 override. `shared_arc`
            // is a fresh local Arc clone so `guard`'s lifetime doesn't tie
            // up `self`.
            let shared_arc = if key.template_id >= helio_mats::FIRST_CUSTOM_MATERIAL_CLASS {
                self.shared_registry.clone()
            } else {
                None
            };
            let guard = shared_arc.as_ref().map(|a| a.read().unwrap());
            let template = guard
                .as_ref()
                .and_then(|g| g.get(key.template_id))
                .or_else(|| self.local_templates.get(key.template_id))
                .unwrap_or_else(|| {
                    log::debug!(
                        "[ForwardLit] template class {} not found, falling back to class 0",
                        key.template_id
                    );
                    self.local_templates
                        .get(0)
                        .expect("the default template (class 0)")
                });
            let module = self.shader_cache.get_or_compile(
                device,
                key,
                template,
                FORWARD_PROGRAM,
                graph_wgsl,
                &[
                    helio_mesh_api::DRAW_ROWS_SNIPPET,
                    helio_light_api::LIGHT_LIST_SNIPPET,
                    helio_light_api::LIGHT_FORWARD_SNIPPET,
                ],
                self.material_binding,
                "ForwardLit Shader",
            );
            let pipeline = helio_core::shader::create_render_pipeline(
                &device,
                wgpu::RenderPipelineDescriptor {
                    label: Some("ForwardLit Pipeline"),
                    layout: Some(&self.pipeline_layout),
                    vertex: wgpu::VertexState {
                        module,
                        entry_point: Some("vs_main"),
                        compilation_options: Default::default(),
                        buffers: &[
                            Some(wgpu::VertexBufferLayout {
                                array_stride: 40,
                                step_mode: wgpu::VertexStepMode::Vertex,
                                attributes: &[
                                    wgpu::VertexAttribute {
                                        format: wgpu::VertexFormat::Float32x3,
                                        offset: 0,
                                        shader_location: 0,
                                    },
                                    wgpu::VertexAttribute {
                                        format: wgpu::VertexFormat::Float32,
                                        offset: 12,
                                        shader_location: 1,
                                    },
                                    wgpu::VertexAttribute {
                                        format: wgpu::VertexFormat::Float32x2,
                                        offset: 16,
                                        shader_location: 2,
                                    },
                                    wgpu::VertexAttribute {
                                        format: wgpu::VertexFormat::Float32x2,
                                        offset: 24,
                                        shader_location: 5,
                                    },
                                    wgpu::VertexAttribute {
                                        format: wgpu::VertexFormat::Uint32,
                                        offset: 32,
                                        shader_location: 3,
                                    },
                                    wgpu::VertexAttribute {
                                        format: wgpu::VertexFormat::Uint32,
                                        offset: 36,
                                        shader_location: 4,
                                    },
                                ],
                            }),
                            Some(helio_mesh_api::DRAW_SLOT_LAYOUT),
                        ],
                    },
                    fragment: Some(wgpu::FragmentState {
                        module,
                        entry_point: Some("fs_main"),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: self.surface_format,
                            blend: if write_depth {
                                None
                            } else {
                                Some(wgpu::BlendState {
                                    color: wgpu::BlendComponent {
                                        src_factor: wgpu::BlendFactor::One,
                                        dst_factor: wgpu::BlendFactor::Zero,
                                        operation: wgpu::BlendOperation::Add,
                                    },
                                    alpha: wgpu::BlendComponent::OVER,
                                })
                            },
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    primitive: wgpu::PrimitiveState {
                        topology: wgpu::PrimitiveTopology::TriangleList,
                        cull_mode: if key.feature_flags & helio_mats::FLAG_DOUBLE_SIDED != 0 {
                            None
                        } else {
                            Some(wgpu::Face::Back)
                        },
                        ..Default::default()
                    },
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: wgpu::TextureFormat::Depth32Float,
                        depth_write_enabled: Some(write_depth),
                        depth_compare: Some(helio_core::depth::NEARER_EQUAL),
                        stencil: wgpu::StencilState::default(),
                        bias: wgpu::DepthBiasState::default(),
                    }),
                    multisample: wgpu::MultisampleState::default(),
                    multiview_mask: None,
                    cache: None,
                },
            );
            self.pipelines.insert(key, pipeline);
        }
        self.pipelines.get(&key).unwrap()
    }
}

impl RenderPass for ForwardLitPass {
    fn scene_buffers(&self) -> &'static [&'static str] {
        &[
            "builtin_mesh_index",
            "builtin_mesh_vertex",
            "materials",
            helio_light_api::LIGHT_LIST,
            "skeletal_bone_palette",
            "skeletal_skin_vertices",
        ]
    }

    fn name(&self) -> &'static str {
        "ForwardLit"
    }

    fn reads(&self) -> &'static [&'static str] {
        &[
            helio_mats::MATERIAL_TEXTURES,
            "render_environment",
            "depth",
            "pre_aa",
            helio_light_api::CLUSTER_LIGHT_GRID,
            "object_batch",
            "culled_batch",
        ]
    }

    fn declare_resources(&self, builder: &mut ResourceBuilder) {
        builder.read("depth");
        builder.read(helio_light_api::CLUSTER_LIGHT_GRID);
        builder.read("object_batch");
        builder.read("culled_batch");
        builder.write_color_raw("pre_aa", self.surface_format, ResourceSize::MatchSurface);
    }

    fn publish<'a>(&self, _frame: &mut helio_core::ResourceRegistry<'a>) {}

    fn render_pass_descriptor_with_storage<'a>(
        &'a self,
        _target: &'a wgpu::TextureView,
        depth: &'a wgpu::TextureView,
        resources: &'a helio_core::ResourceRegistry<'a>,
        storage: &'a mut helio_core::RenderFrameStorage,
    ) -> Option<wgpu::RenderPassDescriptor<'a>> {
        let pre_aa_view = resources.read(helio_core::ResourceKey::new("pre_aa"), "ForwardLit")?;
        let color_attachments: &'a [Option<wgpu::RenderPassColorAttachment<'a>>] = storage
            .retain_boxed_slice(Box::new([Some(wgpu::RenderPassColorAttachment {
                view: pre_aa_view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })]));
        Some(wgpu::RenderPassDescriptor {
            label: Some("ForwardLit"),
            color_attachments,
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: if self.render_all_opaque {
                        wgpu::LoadOp::Clear(helio_core::depth::CLEAR)
                    } else {
                        wgpu::LoadOp::Load
                    },
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        })
    }

    fn prepare(&mut self, ctx: &PrepareContext) -> HelioResult<()> {
        let (ambient_color, ambient_intensity) = if let Some(ref environment) = ctx
            .registry
            .get::<helio_core::RenderEnvironment>(helio_core::resource_keys::render_environment())
            .as_ref()
        {
            (environment.ambient_color, environment.ambient_intensity)
        } else {
            ([0.1, 0.1, 0.15], 0.1)
        };

        let num_tiles_x = ctx.width.div_ceil(TILE_SIZE);
        let num_tiles_y = ctx.height.div_ceil(TILE_SIZE);

        // The lights' light list (its live rows).
        let scene_lights = ctx
            .scene_buffers
            .get(BufferKey::of(helio_light_api::LIGHT_LIST));
        let light_count = scene_lights.map_or(0, |lights| lights.row_capacity());

        let globals = ForwardLitGlobals {
            frame: ctx.frame_num as u32,
            delta_time: ctx.delta_time,
            light_count,
            ambient_intensity,
            ambient_color: [ambient_color[0], ambient_color[1], ambient_color[2], 1.0],
            num_tiles_x,
            num_tiles_y,
            screen_width: ctx.width as f32,
            screen_height: ctx.height as f32,
            _pad0: 0,
            time: ctx.time,
            _pad1: 0,
            _pad2: 0,
        };
        ctx.write_buffer(&self.globals_buf, 0, bytemuck::bytes_of(&globals));
        Ok(())
    }

    fn execute(&mut self, ctx: &mut PassContext) -> HelioResult<()> {
        let Some(batch) = ctx
            .registry
            .get::<helio_mesh_api::ObjectBatchFrameData<'_>>(helio_core::ResourceKey::new(
                "object_batch",
            ))
        else {
            return Ok(());
        };
        let Some(culled) = ctx
            .registry
            .get::<helio_mesh_api::CulledBatchFrameData<'_>>(helio_core::ResourceKey::new(
                "culled_batch",
            ))
        else {
            return Ok(());
        };
        let (max_draws, draw_count) = batch.all_draws();
        if max_draws == 0 {
            return Ok(());
        }
        // The materials' texture table, or the stand-in without them.
        let stand_in = self
            .stand_in
            .get_or_insert_with(|| helio_mats::MaterialStandIn::new(&ctx.device, ctx.queue));
        let material_textures = ctx
            .registry
            .get::<helio_mats::MaterialTextureBindings<'_>>(helio_mats::material_textures())
            .unwrap_or_else(|| stand_in.textures());
        let Some(vertices_handle) = ctx.scene_buffers.get(BufferKey::of("builtin_mesh_vertex"))
        else {
            return Ok(());
        };
        let Some(indices_handle) = ctx.scene_buffers.get(BufferKey::of("builtin_mesh_index"))
        else {
            return Ok(());
        };
        let vertices = &vertices_handle.buffer;
        let indices = &indices_handle.buffer;

        // Material rows are SceneDB component data.  Resolve the column by
        // key so the renderer never becomes the material authority again.
        let materials_handle = ctx.scene_buffers.get(BufferKey::of("materials"));
        let materials_buf = materials_handle
            .map(|handle| &handle.buffer)
            .unwrap_or(stand_in.rows());
        let materials_epoch = materials_handle.map(|handle| handle.epoch).unwrap_or(0);

        let lights_handle = ctx
            .scene_buffers
            .get(BufferKey::of(helio_light_api::LIGHT_LIST));
        let lights_buf = lights_handle
            .map(|handle| &handle.buffer)
            .unwrap_or(batch.instances);

        // SceneDB epoch, not this frame's handle address: a reallocated
        // lights buffer can land at the same address.
        let lights_ptr = lights_handle.map_or(usize::MAX, |handle| handle.epoch as usize);
        let skin_buffer = ctx
            .scene_buffers
            .get(BufferKey::of("skeletal_skin_vertices"))
            .map(|h| h.buffer.clone())
            .unwrap_or_else(|| batch.instances.clone());
        let palette_buffer = ctx
            .scene_buffers
            .get(BufferKey::of("skeletal_bone_palette"))
            .map(|h| h.buffer.clone())
            .unwrap_or_else(|| batch.instances.clone());

        let cluster = ctx.registry.get::<helio_light_api::ClusterLightGrid<'_>>(
            helio_core::ResourceKey::new(helio_light_api::CLUSTER_LIGHT_GRID),
        );

        let bg0_key = (
            ctx.camera.clone(),
            batch.instances.clone(),
            culled.compacted_indices.clone(),
            lights_ptr,
            cluster.map(|c| c.tile_light_lists.clone()),
            cluster.map(|c| c.tile_light_counts.clone()),
            skin_buffer.clone(),
            palette_buffer.clone(),
        );
        if self.bind_group_0_key.as_ref() != Some(&bg0_key) {
            let cluster_ref = ctx.registry.get::<helio_light_api::ClusterLightGrid<'_>>(
                helio_core::ResourceKey::new(helio_light_api::CLUSTER_LIGHT_GRID),
            );
            // Without the lights' binning (no lights installed) every tile
            // is empty: zero counts, never another buffer's words.
            let tile_lists = cluster_ref
                .map(|c| c.tile_light_lists)
                .unwrap_or(&self.no_tiles);
            let tile_counts = cluster_ref
                .map(|c| c.tile_light_counts)
                .unwrap_or(&self.no_tiles);

            log::debug!("ForwardLit: rebuilding bind group 0 (buffers changed)");
            self.bind_group_0 = Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ForwardLit BG 0"),
                layout: &self.bind_group_layout_0,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: ctx.camera.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: self.globals_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: batch.instances.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: culled.compacted_indices.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: lights_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: tile_lists.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: tile_counts.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 9,
                        resource: skin_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 10,
                        resource: palette_buffer.as_entire_binding(),
                    },
                ],
            }));
            self.bind_group_0_key = Some(bg0_key);
        }

        let needs_rebuild = self.bind_group_1_version
            != Some((material_textures.version, materials_epoch))
            || self.bind_group_1.is_none();
        if needs_rebuild {
            log::debug!("ForwardLit: rebuilding bind group 1 (material textures version changed)");
            let mut entries = vec![
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: materials_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: material_textures.material_textures.as_entire_binding(),
                },
            ];
            self.material_binding.append_bind_group_entries(
                &mut entries,
                2,
                material_textures.texture_views,
                material_textures.samplers,
            );
            self.bind_group_1 = Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ForwardLit BG 1"),
                layout: &self.bind_group_layout_1,
                entries: &entries,
            }));
            self.bind_group_1_version = Some((material_textures.version, materials_epoch));
        }

        let indirect = culled.indirect;
        // The slot indexes `compacted_indices`.
        let slots = self
            .draw_slots
            .buffer(
                &ctx.device,
                helio_mesh_api::DrawSlots::len_of(culled.compacted_indices.size(), 4),
            )
            .clone();
        let mut pass = ctx.render_cmds().unwrap();
        pass.set_bind_group(0, self.bind_group_0.as_ref().unwrap(), &[]);
        pass.set_bind_group(1, self.bind_group_1.as_ref().unwrap(), &[]);
        pass.set_vertex_buffer(0, vertices.slice(..));
        pass.set_vertex_buffer(1, slots.slice(..));
        pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);

        // One draw per material segment of the bucket this pass renders (see
        // `DrawSegments`).
        let segments = culled.segments;
        let bucket = if self.render_all_opaque {
            helio_mesh_api::ShadingBucket::Opaque
        } else {
            helio_mesh_api::ShadingBucket::Forward
        };
        if segments.is_empty() {
            // No material key read back yet (the first frames): draw every
            // group with the default pipeline.
            let key = RadiantShaderKey {
                template_id: 0,
                graph_hash: 0,
                feature_flags: if self.render_all_opaque { 1 << 31 } else { 0 },
            };
            let pipeline =
                self.get_or_create_pipeline(&ctx.device, key, "", self.render_all_opaque);
            pass.set_pipeline(pipeline);
            helio_mesh_api::multi_draw_indexed_indirect(
                &mut pass, indirect, 0, max_draws, draw_count,
            );
        } else {
            for (index, segment) in segments.in_bucket(bucket) {
                let mut key = RadiantShaderKey {
                    template_id: segment.template_class(),
                    graph_hash: segment.graph_hash,
                    feature_flags: segment.raster_flags(),
                };
                if self.render_all_opaque {
                    key.feature_flags |= 1 << 31;
                }
                let pipeline =
                    self.get_or_create_pipeline(&ctx.device, key, "", self.render_all_opaque);
                pass.set_pipeline(pipeline);
                segments.draw(&mut pass, index);
            }
        }
        Ok(())
    }
}

fn create_material_bgl(
    device: &wgpu::Device,
    material_binding: helio_mats::MaterialBindingConfig,
) -> wgpu::BindGroupLayout {
    let mut entries = vec![
        wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
    ];
    material_binding.append_layout_entries(&mut entries, 2, wgpu::ShaderStages::FRAGMENT);

    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("ForwardLit BGL 1"),
        entries: &entries,
    })
}
