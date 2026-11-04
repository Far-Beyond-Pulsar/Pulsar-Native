//! Transparent geometry pass with SrcAlpha / OneMinusSrcAlpha blending,
//! read-only depth, and Radiant template support.
//!
//! Templates are composed with `transparent_base.wgsl` (shared in the `helio`
//! crate). The materials' transparent registry
//! (`helio_mats::MaterialTemplates::transparent`) does not reach this pass
//! yet: it draws with its built-in templates.
//! The default template (class 0) reads SceneDB material tint, alpha and PBR properties.

pub mod feature;
pub use feature::TransparentDrawers;

use std::collections::HashMap;
use std::sync::OnceLock;

use bytemuck::{Pod, Zeroable};
use helio_core::graph::ResourceBuilder;
use helio_core::{PassContext, PrepareContext, RenderPass, Result as HelioResult};
use helio_mats::radiant::{RadiantShaderCache, RadiantShaderKey};
use pulsar_scenedb::gpu::{world_mirror::DEFAULT_AUTO_REGISTER_CAPACITY, BufferKey};

/// Fixed capacity for the `"scene_lights"` SceneDB buffer, kept equal to
/// `helio_light_api::MAX_LIGHTS`/`helio_light_api::MAX_LIGHTS`
/// by construction (all three are literally `DEFAULT_AUTO_REGISTER_CAPACITY`).
const MAX_LIGHTS: u32 = DEFAULT_AUTO_REGISTER_CAPACITY;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct TransparentGlobals {
    frame: u32,
    delta_time: f32,
    light_count: u32,
    ambient_intensity: f32,
    ambient_color: [f32; 4],
    csm_splits: [f32; 4],
    num_tiles_x: u32,
    num_tiles_y: u32,
    screen_width: f32,
    screen_height: f32,
    // 1 when `lights`/`transforms` share the same raw entity index
    // (SceneDB-direct); 0 when `lights` is `ctx.scene.lights`, a freshly
    // rebuilt dense array needing `light_entity_indices[light_idx]`. See
    // `transparent_base.wgsl`'s `light_entity_indices` binding doc.
    light_mode_direct_index: u32,
    /// The host-driven frame clock (`PrepareContext::time`): the
    /// shader-graph `time` node.
    time: f32,
    _pad1: u32,
    _pad2: u32,
}

pub struct TransparentPass {
    /// The frame's integrated medium (`helio_frame_api::media`), when a
    /// media feature published one.
    media: Option<helio_frame_api::media::MediaResources>,
    medium_layout: wgpu::BindGroupLayout,
    medium_group: Option<wgpu::BindGroup>,
    medium_key: Option<(wgpu::TextureView, wgpu::Buffer)>,
    medium_fallback: wgpu::TextureView,
    medium_fallback_parameters: wgpu::Buffer,
    medium_sampler: wgpu::Sampler,
    pipelines: HashMap<RadiantShaderKey, wgpu::RenderPipeline>,
    shader_cache: RadiantShaderCache,
    /// This pass's own class-0 override (the transparent base — never
    /// synced from the scene).
    local_class0: helio_mats::radiant::RadiantTemplate,
    /// User-registered custom templates (id >= 5), shared with the renderer
    /// and other passes — never deep-cloned (see `SharedTemplateRegistry`).
    shared_registry: Option<helio_mats::radiant::SharedTemplateRegistry>,
    /// Key set as of the last sync, to detect content changes cheaply.
    last_shared_keys: Vec<u32>,
    pipeline_layout: wgpu::PipelineLayout,
    bind_group_layout_0: wgpu::BindGroupLayout,
    /// Rebuilt per-frame in `execute()` — its `instances` source
    /// (`object_batch.instances`) is a `GrowableBuffer` that can reallocate
    /// across frames, so this can't be built once at construction time (see
    /// `bind_group_key`).
    bind_group: Option<wgpu::BindGroup>,
    bind_group_key: Option<[wgpu::Buffer; 4]>,
    bind_group_layout_1: wgpu::BindGroupLayout,
    bind_group_1: Option<wgpu::BindGroup>,
    bind_group_1_key: Option<(
        usize,
        usize,
        Option<wgpu::Buffer>,
        Option<wgpu::Buffer>,
        usize,
        usize,
    )>,
    globals_buf: wgpu::Buffer,
    surface_format: wgpu::TextureFormat,
    /// The scene colour image this pass draws over (`None`: the target the
    /// graph hands it), and the image that one may be a view of.
    color_target: Option<&'static str>,
    color_alias: Option<&'static str>,
    reactive_mask: bool,
    timing_query: Option<wgpu::QuerySet>,
    draw_slots: helio_mesh_api::DrawSlots,
}

impl TransparentPass {
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Transparent Globals"),
            size: std::mem::size_of::<TransparentGlobals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bgl_0 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Transparent BGL 0"),
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
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
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

        let bgl_1 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Transparent BGL 1"),
            entries: &[
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
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 3: light_entity_indices storage read -- parallel to `lights`
                // only in CPU-resolved mode, see `transparent_base.wgsl`'s
                // binding doc.
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 4: SceneDB `Transform` storage read -- entity-indexed the same
                // way `lights` is, see `transparent_base.wgsl`'s binding doc.
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
            ],
        });

        let medium_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Transparent medium BGL"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D3,
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
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let medium_fallback = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("Transparent neutral medium"),
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
        let medium_fallback_parameters = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Transparent neutral medium parameters"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM,
            mapped_at_creation: false,
        });
        let medium_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Transparent medium sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Transparent PL"),
            bind_group_layouts: &[Some(&bgl_0), Some(&bgl_1), Some(&medium_layout)],
            immediate_size: 0,
        });

        // Just the transparent base shader at class 0 — NOT
        // RadiantTemplateRegistry::new(), which populates classes 0-4 with
        // gbuffer templates that have incompatible bind group layouts.
        let base_src = helio_mats::templates::TRANSPARENT_BASE;
        let resolved_src: &'static str = if base_src.contains("//!use pbr_eval") {
            static RESOLVED: OnceLock<String> = OnceLock::new();
            RESOLVED
                .get_or_init(|| {
                    let mut resolved =
                        String::with_capacity(base_src.len() + helio_mats::PBR_EVAL.len());
                    resolved.push_str(helio_mats::PBR_EVAL);
                    resolved.push('\n');
                    // The lights' row and forward light function.
                    resolved.push_str(helio_light_api::LIGHT_LIST_SNIPPET.source);
                    resolved.push_str(helio_light_api::LIGHT_FORWARD_SNIPPET.source);
                    resolved.push_str(&base_src.replace(
                        "// HELIO_VIEW_SURFACE_EFFECT",
                        "surface = apply_medium(surface, input.world_position);",
                    ));
                    resolved.push_str(include_str!("../shaders/medium.wgsl"));
                    resolved
                })
                .as_str()
        } else {
            base_src
        };
        let local_class0 = helio_mats::radiant::RadiantTemplate {
            name: std::sync::Arc::from("transparent_base"),
            wgsl_source: std::sync::Arc::from(resolved_src),
        };

        Self {
            media: None,
            medium_layout,
            medium_group: None,
            medium_key: None,
            medium_fallback,
            medium_fallback_parameters,
            medium_sampler,
            pipelines: HashMap::new(),
            shader_cache: RadiantShaderCache::new(),
            local_class0,
            shared_registry: None,
            last_shared_keys: Vec::new(),
            pipeline_layout,
            bind_group_layout_0: bgl_0,
            bind_group: None,
            bind_group_key: None,
            bind_group_layout_1: bgl_1,
            bind_group_1: None,
            bind_group_1_key: None,
            globals_buf,
            surface_format,
            color_target: None,
            color_alias: None,
            reactive_mask: false,
            timing_query: None,
            draw_slots: Default::default(),
        }
    }

    /// Blend into the graph's linear lighting target before tonemapping.
    /// `surface_format` supplied to `new` must match that lighting target.
    pub fn with_pre_aa_target(self) -> Self {
        self.with_color_target(helio_frame_api::color::SCENE_COLOR, None)
    }

    /// Blend into the scene colour image `target` (the colour chain's
    /// current), which may be a view of `alias`
    /// (`helio_frame_api::color::alias`).
    pub fn with_color_target(mut self, target: &'static str, alias: Option<&'static str>) -> Self {
        self.color_target = Some(target);
        self.color_alias = alias;
        self
    }

    /// Composite each fragment through `media` at its own depth (over a
    /// scene colour the media were already composited into).
    pub fn with_media(mut self, media: Option<helio_frame_api::media::MediaResources>) -> Self {
        self.media = media;
        self
    }

    /// Publish R8 transparency coverage for temporal AA. Custom templates can
    /// opt in with HELIO_TRANSPARENT_REACTIVITY and a vec4 alpha at location 1.
    pub fn with_reactive_mask(mut self) -> Self {
        self.reactive_mask = true;
        self
    }

    /// Optional GPU timestamps around the transparent render pass.
    pub fn enable_timing(&mut self, device: &wgpu::Device) -> bool {
        if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            return false;
        }
        self.timing_query = Some(device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("Transparent pass timings"),
            ty: wgpu::QueryType::Timestamp,
            count: 2,
        }));
        true
    }

    pub fn timing_query(&self) -> Option<&wgpu::QuerySet> {
        self.timing_query.as_ref()
    }
}

impl RenderPass for TransparentPass {
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
        "TransparentPass"
    }

    fn reads(&self) -> &'static [&'static str] {
        // The scene colour drawn over and the medium are named when the pass
        // is built, so declared once per distinct list.
        let mut reads = Vec::with_capacity(7);
        reads.extend(self.color_target);
        if let Some(media) = self.media {
            reads.extend([media.grid, media.parameters]);
        }
        reads.extend([
            "depth",
            helio_light_api::CLUSTER_LIGHT_GRID,
            "object_batch",
            "culled_batch",
        ]);
        helio_frame_api::static_names(&reads)
    }

    fn writes(&self) -> &'static [&'static str] {
        let mut writes = Vec::with_capacity(2);
        writes.extend(self.color_target);
        if self.reactive_mask {
            writes.push("transparency_reactivity");
        }
        helio_frame_api::static_names(&writes)
    }

    fn declare_resources(&self, builder: &mut ResourceBuilder) {
        if self.reactive_mask {
            builder.write_color_raw(
                "transparency_reactivity",
                wgpu::TextureFormat::R8Unorm,
                helio_core::graph::ResourceSize::MatchSurface,
            );
        }
        if let Some(target) = self.color_target {
            builder.read(target);
        }
        // The scene colour may be a view of an earlier image; keep it alive.
        if let (Some(_), Some(alias)) = (self.color_target, self.color_alias) {
            builder.read(alias);
        }
        if let Some(media) = self.media {
            builder.read(media.grid);
            builder.read(media.parameters);
        }
        builder.read("depth");
        builder.read(helio_light_api::CLUSTER_LIGHT_GRID);
        builder.read("object_batch");
        builder.read("culled_batch");
    }

    fn prepare(&mut self, ctx: &PrepareContext) -> HelioResult<()> {
        let num_tiles_x = ctx.width.div_ceil(16);
        let num_tiles_y = ctx.height.div_ceil(16);
        // Prefer the SceneDB-direct `"scene_lights"` buffer (fixed capacity
        // `MAX_LIGHTS`) when populated; else the `Renderer`-seeded
        // `LightsFrameData` bridge's `movable_light_count`, production's
        // actual light count today -- see `ForwardLitPass`'s identical
        // `light_mode_direct_index` doc for the full reasoning.
        let scene_lights = ctx
            .scene_buffers
            .get(BufferKey::of(helio_light_api::LIGHT_LIST));
        let use_direct_index = scene_lights.is_some();
        let light_count = scene_lights.map_or(0, |lights| lights.row_capacity());
        ctx.write_buffer(
            &self.globals_buf,
            0,
            bytemuck::bytes_of(&TransparentGlobals {
                frame: ctx.frame_num as u32,
                delta_time: 0.0,
                light_count,
                ambient_intensity: 0.6,
                ambient_color: [0.3, 0.35, 0.4, 1.0],
                csm_splits: [0.0; 4],
                num_tiles_x,
                num_tiles_y,
                screen_width: ctx.width as f32,
                screen_height: ctx.height as f32,
                light_mode_direct_index: use_direct_index as u32,
                time: ctx.time,
                _pad1: 0,
                _pad2: 0,
            }),
        );
        Ok(())
    }

    fn render_pass_descriptor_with_storage<'a>(
        &'a self,
        target: &'a wgpu::TextureView,
        depth: &'a wgpu::TextureView,
        resources: &'a helio_core::ResourceRegistry<'a>,
        storage: &'a mut helio_core::RenderFrameStorage,
    ) -> Option<wgpu::RenderPassDescriptor<'a>> {
        let pre_aa = self
            .color_target
            .and_then(|target| resources.get(helio_core::ResourceKey::new(target)));
        let target = pre_aa.unwrap_or(target);
        let mut attachments = vec![Some(wgpu::RenderPassColorAttachment {
            view: target,
            resolve_target: None,
            depth_slice: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Load,
                store: wgpu::StoreOp::Store,
            },
        })];
        if self.reactive_mask {
            attachments.push(Some(wgpu::RenderPassColorAttachment {
                view: resources
                    .texture_view(helio_core::ResourceKey::new("transparency_reactivity"))?,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            }));
        }
        let color_attachments = storage.retain_boxed_slice(attachments.into_boxed_slice());
        let depth_view = if pre_aa.is_some() {
            depth
        } else {
            resources
                .get(helio_core::ResourceKey::new("full_res_depth"))
                .unwrap_or(depth)
        };
        Some(wgpu::RenderPassDescriptor {
            label: Some("Transparent"),
            color_attachments,
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: self.timing_query.as_ref().map(|query| {
                wgpu::RenderPassTimestampWrites {
                    query_set: query,
                    beginning_of_pass_write_index: Some(0),
                    end_of_pass_write_index: Some(1),
                }
            }),
            occlusion_query_set: None,
            multiview_mask: None,
        })
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
        let segments = culled.segments;
        if segments
            .in_bucket(helio_mesh_api::ShadingBucket::Transparent)
            .next()
            .is_none()
        {
            return Ok(());
        }

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

        // Rebuild bind group 1 (lights + transforms + cluster data) when
        // buffer pointers change. Lights are always read from the SceneDB
        // component buffer; the camera buffer is a valid binding fallback
        // when no light component has been authored yet.
        let cluster = ctx.registry.get::<helio_light_api::ClusterLightGrid<'_>>(
            helio_core::ResourceKey::new(helio_light_api::CLUSTER_LIGHT_GRID),
        );
        let lights_handle = ctx
            .scene_buffers
            .get(BufferKey::of(helio_light_api::LIGHT_LIST));
        let lights_buf = lights_handle
            .map(|handle| &handle.buffer)
            .unwrap_or(batch.instances);
        let Some(materials_handle) = ctx.scene_buffers.get(BufferKey::of("materials")) else {
            return Ok(());
        };
        let materials_buf = &materials_handle.buffer;
        // SceneDB epoch, not this frame's handle address: a reallocated
        // lights buffer can land at the same address.
        let lights_ptr = lights_handle.map_or(usize::MAX, |handle| handle.epoch as usize);
        let light_entity_indices_ptr = 0;
        let transforms_ptr = 0;
        let bg1_key = (
            lights_ptr,
            light_entity_indices_ptr,
            cluster.map(|c| c.tile_light_lists.clone()),
            cluster.map(|c| c.tile_light_counts.clone()),
            transforms_ptr,
            materials_handle.epoch as usize,
        );
        if self.bind_group_1_key.as_ref() != Some(&bg1_key) {
            let fallback = batch.instances;
            let tile_lists = cluster.map(|c| c.tile_light_lists).unwrap_or(fallback);
            let tile_counts = cluster.map(|c| c.tile_light_counts).unwrap_or(fallback);
            // `light_count` is 0 whenever no real `Transform` buffer exists
            // yet, so this fallback is never actually dereferenced at a live
            // light's index in practice -- same reasoning as
            // `helio_pass_forward_lit`'s identical fallback.
            let transforms = fallback;
            let light_entity_indices_buf = fallback;
            self.bind_group_1 = Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Transparent BG 1"),
                layout: &self.bind_group_layout_1,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: lights_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: tile_lists.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: tile_counts.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: light_entity_indices_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: transforms.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: materials_buf.as_entire_binding(),
                    },
                ],
            }));
            self.bind_group_1_key = Some(bg1_key);
        }

        // Rebuild bind group 0 (camera + globals + instances) when buffer
        // handles change -- `batch.instances` is a `GrowableBuffer` that can
        // reallocate across frames as the scene grows, so this can't be
        // built once at construction time (mirrors `bind_group_1`'s pattern).
        let skin_buf = ctx
            .scene_buffers
            .get(BufferKey::of("skeletal_skin_vertices"))
            .map(|h| h.buffer.clone())
            .unwrap_or_else(|| batch.instances.clone());
        let palette_buf = ctx
            .scene_buffers
            .get(BufferKey::of("skeletal_bone_palette"))
            .map(|h| h.buffer.clone())
            .unwrap_or_else(|| batch.instances.clone());
        let bg0_key = [
            ctx.camera.clone(),
            batch.instances.clone(),
            skin_buf,
            palette_buf,
        ];
        if self.bind_group_key.as_ref() != Some(&bg0_key) {
            self.bind_group = Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Transparent BG 0"),
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
                        resource: bg0_key[2].as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: bg0_key[3].as_entire_binding(),
                    },
                ],
            }));
            self.bind_group_key = Some(bg0_key);
        }

        // The medium belongs to its producer; retain only borrowed GPU handles
        // in this derived binding cache. Without one, a zero-range bypass.
        let medium_view = self
            .media
            .and_then(|media| {
                ctx.registry
                    .get::<&wgpu::TextureView>(helio_core::ResourceKey::new(media.grid))
            })
            .unwrap_or(&self.medium_fallback);
        let medium_parameters = self
            .media
            .and_then(|media| {
                ctx.registry
                    .get::<&wgpu::Buffer>(helio_core::ResourceKey::new(media.parameters))
            })
            .unwrap_or(&self.medium_fallback_parameters);
        let medium_key = (medium_view.clone(), medium_parameters.clone());
        if self.medium_key.as_ref() != Some(&medium_key) {
            self.medium_group = Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Transparent medium"),
                layout: &self.medium_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(medium_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.medium_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: medium_parameters.as_entire_binding(),
                    },
                ],
            }));
            self.medium_key = Some(medium_key);
        }
        let slots = self
            .draw_slots
            .buffer(
                &ctx.device,
                helio_mesh_api::DrawSlots::len_of(
                    batch.instances.size(),
                    helio_mesh_api::draw_slots::INSTANCE_BYTES,
                ),
            )
            .clone();
        let mut rp = ctx.render_cmds().unwrap();
        rp.set_bind_group(0, self.bind_group.as_ref().unwrap(), &[]);
        rp.set_bind_group(1, self.bind_group_1.as_ref().unwrap(), &[]);
        rp.set_bind_group(2, self.medium_group.as_ref().unwrap(), &[]);
        rp.set_vertex_buffer(0, vertices.slice(..));
        rp.set_vertex_buffer(1, slots.slice(..));
        rp.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);

        // One draw per transparent material segment (see `DrawSegments`).
        for (index, segment) in segments.in_bucket(helio_mesh_api::ShadingBucket::Transparent) {
            let key = RadiantShaderKey {
                template_id: segment.template_class(),
                graph_hash: segment.graph_hash,
                feature_flags: segment.raster_flags(),
            };
            let pipeline = self.get_or_create_pipeline(&ctx.device, key, "");
            rp.set_pipeline(pipeline);
            segments.draw(&mut rp, index);
        }
        Ok(())
    }
}

impl TransparentPass {
    fn get_or_create_pipeline(
        &mut self,
        device: &wgpu::Device,
        key: RadiantShaderKey,
        graph_wgsl: &str,
    ) -> &wgpu::RenderPipeline {
        if !self.pipelines.contains_key(&key) {
            // Ids >= 5 are user-registered custom templates that live in the
            // shared scene-wide registry; class 0 is this pass's own
            // transparent base. `shared_arc` is a fresh local Arc clone so
            // `guard`'s lifetime doesn't tie up `self`.
            let shared_arc = if key.template_id >= 5 {
                self.shared_registry.clone()
            } else {
                None
            };
            let guard = shared_arc.as_ref().map(|a| a.read().unwrap());
            let template = guard
                .as_ref()
                .and_then(|g| g.get(key.template_id))
                .unwrap_or_else(|| {
                    if key.template_id >= 5 {
                        log::debug!(
                            "[Transparent] template class {} not found, falling back to class 0",
                            key.template_id
                        );
                    }
                    &self.local_class0
                });
            // A custom template is the transparent base with its own
            // surface: it takes the same snippets the base does.
            let composed;
            let template = if std::ptr::eq(template, &self.local_class0) {
                template
            } else {
                composed = helio_mats::radiant::RadiantTemplate {
                    name: template.name.clone(),
                    wgsl_source: std::sync::Arc::from(
                        helio_core::shader::resolve_with(
                            &template.wgsl_source,
                            &[
                                helio_mats::PBR_EVAL_SNIPPET,
                                helio_light_api::LIGHT_LIST_SNIPPET,
                                helio_light_api::LIGHT_FORWARD_SNIPPET,
                            ],
                        )
                        .as_ref(),
                    ),
                };
                &composed
            };
            let supports_reactivity = template
                .wgsl_source
                .contains("HELIO_TRANSPARENT_REACTIVITY");
            let module = self.shader_cache.get_or_compile(
                device,
                key,
                template,
                graph_wgsl,
                helio_mats::MaterialBindingConfig::for_device(device),
                "Transparent Shader",
            );
            let alpha_blend = wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::SrcAlpha,
                    dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent::OVER,
            };
            let mut targets = vec![Some(wgpu::ColorTargetState {
                format: self.surface_format,
                blend: Some(alpha_blend),
                write_mask: wgpu::ColorWrites::ALL,
            })];
            if self.reactive_mask {
                targets.push(Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::R8Unorm,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent::OVER,
                        alpha: wgpu::BlendComponent::OVER,
                    }),
                    write_mask: if supports_reactivity {
                        wgpu::ColorWrites::RED
                    } else {
                        wgpu::ColorWrites::empty()
                    },
                }));
            }
            let pipeline = helio_core::shader::create_render_pipeline(
                &device,
                wgpu::RenderPipelineDescriptor {
                    label: Some("Transparent Pipeline"),
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
                        targets: &targets,
                    }),
                    primitive: wgpu::PrimitiveState {
                        topology: wgpu::PrimitiveTopology::TriangleList,
                        cull_mode: None,
                        ..Default::default()
                    },
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: wgpu::TextureFormat::Depth32Float,
                        depth_write_enabled: Some(false),
                        depth_compare: Some(helio_core::depth::NEARER),
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

#[cfg(test)]
mod reactivity_tests;
