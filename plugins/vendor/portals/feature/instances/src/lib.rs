//! Draws the portal-duplicated instances `helio-pass-portal-cull` selected,
//! into the G-buffer, clipped to every portal along each survivor's chain.
//!
//! Fused into the same physical render pass `helio-pass-gbuffer` opened
//! (`LoadOp::Load` on all 8 attachments), following `helio-pass-foliage-gbuffer`'s
//! precedent exactly: real depth buffer, real materials, no separate camera,
//! no compositing. One `multi_draw_indexed_indirect` call, same shape as the
//! plain non-portal G-buffer pass — one draw per mesh/material draw group,
//! not one per chain; each instance's chain is looked up per-instance from a
//! buffer `helio-pass-portal-cull` wrote, not chosen by a per-draw uniform.
//!
//! # Integration
//!
//! ```ignore
//! let cull_pass = PortalCullPass::new(device);
//! let indirect_buf = Arc::clone(&cull_pass.portal_indirect_buf);
//! let compacted_buf = Arc::clone(&cull_pass.portal_compacted_indices_buf);
//! let compacted_chains_buf = Arc::clone(&cull_pass.portal_compacted_chains_buf);
//! graph.add_pass(Box::new(cull_pass));           // before GBufferPass
//! // ... GBufferPass added here ...
//! graph.add_pass(Box::new(PortalMaskPass::new(device))); // after GBufferPass, before PortalInstancePass
//! graph.add_pass(Box::new(
//!     PortalInstancePass::new(device, indirect_buf, compacted_buf, compacted_chains_buf),
//! )); // immediately after PortalMaskPass, before FoliageGBufferPass/VirtualGeometryPass
//! ```

pub mod feature;
pub use feature::PortalFeature;

use std::{borrow::Cow, collections::HashMap, sync::Arc};

use helio_mats::radiant::{RadiantShaderCache, RadiantShaderKey, RadiantTemplateRegistry};
use helio_pass_portal_cull::SharedPortalSegments;

use helio_core::graph::ResourceBuilder;
use helio_core::{PassContext, PrepareContext, RenderPass, Result as HelioResult};
use helio_pass_portal_cull::PORTAL_DRAW_CAPACITY;
use pulsar_scenedb::gpu::BufferKey;

mod mask;
pub use mask::PortalMaskPass;

mod editor_overlay;
pub use editor_overlay::{
    PortalDebugText, PortalEditorOverlayPass, PORTAL_DEBUG_GEOMETRY, PORTAL_DEBUG_TEXTS,
};

use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ScreenSize {
    width: f32,
    height: f32,
    _pad0: f32,
    _pad1: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PortalGlobals {
    time: f32,
    _pad: [f32; 3],
}

type PortalBindGroupKey = ([wgpu::Buffer; 10], wgpu::TextureView);

pub struct PortalInstancePass {
    /// Live portal chains in the host's projection (`PORTAL_COUNTS`); the
    /// pass is a graph member only while there are some.
    active_chain_count: Option<u32>,
    /// The draw slots the vertex shader reads (see `helio_mesh_api::DrawSlots`).
    draw_slots: helio_mesh_api::DrawSlots,
    material_binding: helio_mats::MaterialBindingConfig,
    /// Draws materials without a graph (and every group until the segments exist).
    pipeline: wgpu::RenderPipeline,
    /// One pipeline per material graph, built when its segment first draws: the
    /// same surface evaluation as the G-buffer's pipeline for that graph.
    graph_pipelines: HashMap<u64, wgpu::RenderPipeline>,
    shader_cache: RadiantShaderCache,
    pipeline_layout: wgpu::PipelineLayout,
    /// The portal shader before any material graph is filled in.
    base_shader: Cow<'static, str>,
    /// Where the portal draw reads its per-material records (`PortalCullPass`).
    segments: SharedPortalSegments,
    bind_group_layout_0: wgpu::BindGroupLayout,
    bind_group_layout_1: wgpu::BindGroupLayout,

    screen_buf: wgpu::Buffer,
    globals_buf: wgpu::Buffer,

    /// Shared with `PortalCullPass` — this pass only reads them.
    portal_indirect_buf: Arc<wgpu::Buffer>,
    portal_compacted_indices_buf: Arc<wgpu::Buffer>,
    portal_compacted_chains_buf: Arc<wgpu::Buffer>,

    bind_group_0: Option<wgpu::BindGroup>,
    bind_group_0_key: Option<PortalBindGroupKey>,
    bind_group_1: Option<wgpu::BindGroup>,
    /// `(material texture version, materials buffer epoch)` bind group 1 was built for.
    bind_group_1_version: Option<(u64, u64)>,

    draw_count: u32,
}

impl PortalInstancePass {
    pub fn new(
        device: &wgpu::Device,
        portal_indirect_buf: Arc<wgpu::Buffer>,
        portal_compacted_indices_buf: Arc<wgpu::Buffer>,
        portal_compacted_chains_buf: Arc<wgpu::Buffer>,
        segments: SharedPortalSegments,
    ) -> Self {
        let material_binding = helio_mats::MaterialBindingConfig::for_device(device);

        let shader_source = portal_shader_source(material_binding);

        let shader = helio_core::shader::module(device, "PortalInstance Shader", &*shader_source);
        // The raw text, for filling in material graphs (the registered, hot-reloadable
        // module above is the default pipeline's).
        let base_shader = base_shader_text();

        let screen_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("PortalInstance/ScreenSize"),
            size: std::mem::size_of::<ScreenSize>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("PortalInstance/Globals"),
            size: std::mem::size_of::<PortalGlobals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group_layout_0 =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("PortalInstance BGL 0"),
                entries: &[
                    storage_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT, true), // cameras
                    uniform_entry(1, wgpu::ShaderStages::FRAGMENT, false),       // screen size
                    storage_entry(2, wgpu::ShaderStages::VERTEX, true),          // instance_data
                    storage_entry(3, wgpu::ShaderStages::VERTEX_FRAGMENT, true), // coordinate_spaces
                    storage_entry(4, wgpu::ShaderStages::VERTEX, true), // coordinate_spaces_prev
                    storage_entry(5, wgpu::ShaderStages::VERTEX, true), // portal_compacted_indices
                    storage_entry(6, wgpu::ShaderStages::VERTEX_FRAGMENT, true), // portal_views
                    storage_entry(7, wgpu::ShaderStages::VERTEX_FRAGMENT, true), // chain handles
                    storage_entry(8, wgpu::ShaderStages::VERTEX_FRAGMENT, true), // portal_compacted_chains
                    wgpu::BindGroupLayoutEntry {
                        binding: 9,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Uint,
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    }, // portal_mask — written by helio-pass-portal-mask
                    storage_entry(10, wgpu::ShaderStages::VERTEX_FRAGMENT, true), // chain portal IDs
                    storage_entry(11, wgpu::ShaderStages::VERTEX, true), // skeletal skin stream
                    storage_entry(12, wgpu::ShaderStages::VERTEX, true), // current + previous palettes
                    uniform_entry(13, wgpu::ShaderStages::FRAGMENT, false), // frame globals
                ],
            });
        let bind_group_layout_1 = helio_pass_gbuffer::create_material_bgl(device, material_binding);

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("PortalInstance PL"),
            bind_group_layouts: &[Some(&bind_group_layout_0), Some(&bind_group_layout_1)],
            immediate_size: 0,
        });

        let pipeline = Self::create_pipeline(device, &pipeline_layout, &shader);

        Self {
            active_chain_count: None,
            draw_slots: Default::default(),
            material_binding,
            pipeline,
            graph_pipelines: HashMap::new(),
            shader_cache: RadiantShaderCache::new(),
            pipeline_layout,
            base_shader,
            segments,
            bind_group_layout_0,
            bind_group_layout_1,
            screen_buf,
            globals_buf,
            portal_indirect_buf,
            portal_compacted_indices_buf,
            portal_compacted_chains_buf,
            bind_group_0: None,
            bind_group_0_key: None,
            bind_group_1: None,
            bind_group_1_version: None,
            draw_count: 0,
        }
    }
}

impl PortalInstancePass {
    fn create_pipeline(
        device: &wgpu::Device,
        pipeline_layout: &wgpu::PipelineLayout,
        shader: &wgpu::ShaderModule,
    ) -> wgpu::RenderPipeline {
        helio_core::shader::create_render_pipeline(
            &device,
            wgpu::RenderPipelineDescriptor {
                label: Some("PortalInstance Pipeline"),
                layout: Some(pipeline_layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    // Shared mesh vertex buffer layout — identical to GBufferPass
                    // (stride = 40 bytes; see there for the per-field breakdown).
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
                    module: shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &helio_frame_api::gbuffer::color_targets(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: Some(wgpu::Face::Back),
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    // `helio-pass-portal-mask` (run immediately before this pass)
                    // already reset depth to the clear value everywhere a portal's
                    // opening is actually visible on screen, and left real depth
                    // untouched everywhere else. So a normal nearer-or-equal+write test
                    // here does the right thing on both sides of that boundary:
                    // duplicate content behind a visible opening self-occludes
                    // correctly (nearer copies win), while any pixel the mask
                    // pass *didn't* reset (portal not visible / occluded from
                    // here) keeps real depth and would fail this test too — belt
                    // and suspenders alongside the mask discard in the shader.
                    // Writing depth lets downstream passes in the same fused
                    // pass (foliage, virtual geometry) occlude correctly against
                    // the illusion content instead of the stale real depth.
                    depth_write_enabled: Some(true),
                    depth_compare: Some(helio_core::depth::NEARER_EQUAL),
                    stencil: wgpu::StencilState::default(),
                    // Small nudge so adjacent duplicate copies' coplanar seams
                    // (e.g. two segments' floors meeting end to end) don't
                    // z-fight.
                    bias: helio_core::depth::bias_toward_camera(1, 0.0, 0.0),
                }),
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            },
        )
    }

    /// The pipeline drawing materials with `graph_hash` (0: none).
    fn pipeline_for(&mut self, device: &wgpu::Device, graph_hash: u64) -> &wgpu::RenderPipeline {
        if graph_hash == 0 {
            return &self.pipeline;
        }
        if !self.graph_pipelines.contains_key(&graph_hash) {
            let graph = helio_mats::graph_source(graph_hash);
            let registry = RadiantTemplateRegistry::new();
            let template = registry
                .get(0)
                .expect("Default PBR template (class 0) missing");
            let key = RadiantShaderKey {
                template_id: 0,
                graph_hash,
                feature_flags: 0,
            };
            let module = self.shader_cache.get_or_compile(
                device,
                key,
                template,
                &self.base_shader,
                graph.as_deref().unwrap_or(""),
                &[helio_mesh_api::DRAW_ROWS_SNIPPET],
                self.material_binding,
                "PortalInstance Graph Shader",
            );
            let pipeline = Self::create_pipeline(device, &self.pipeline_layout, module);
            self.graph_pipelines.insert(graph_hash, pipeline);
        }
        &self.graph_pipelines[&graph_hash]
    }
}

/// The portal G-buffer program (`gbuffer_portal.wgsl`): the meshes' rows
/// (`mesh_draw_rows`) and the materials' snippets are resolved when a
/// material template is composed into it (`RadiantTemplate::compose`).
fn base_shader_text() -> Cow<'static, str> {
    helio_core::shader::source_text(
        "PortalInstance Shader",
        helio_core::include_wgsl!("../shaders/gbuffer_portal.wgsl"),
    )
}

/// The program with the default material class (class 0) composed in: the
/// pipeline materials without a graph draw with.
fn portal_shader_source(material_binding: helio_mats::MaterialBindingConfig) -> Cow<'static, str> {
    let registry = RadiantTemplateRegistry::new();
    let template = registry
        .get(0)
        .expect("Default PBR template (class 0) missing");
    let source = template.compose(
        &base_shader_text(),
        "",
        material_binding.max_textures,
        &[helio_mesh_api::DRAW_ROWS_SNIPPET],
    );
    if material_binding.uses_binding_arrays() {
        Cow::Owned(source)
    } else {
        Cow::Owned(helio_mats::apply_webgpu_material_bindings(
            &source,
            material_binding.max_textures,
        ))
    }
}

fn storage_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
    read_only: bool,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
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
    visibility: wgpu::ShaderStages,
    dynamic: bool,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: dynamic,
            min_binding_size: None,
        },
        count: None,
    }
}

impl RenderPass for PortalInstancePass {
    fn scene_buffers(&self) -> &'static [&'static str] {
        const LIST: &[&str] = &[
            helio_mesh_api::MESH_INDICES.name(),
            helio_mesh_api::MESH_VERTICES.name(),
            helio_mats::MATERIALS.name(),
            "portal_views",
            helio_mesh_api::SKELETAL_BONE_PALETTE,
            helio_mesh_api::SKELETAL_SKIN_VERTICES,
        ];
        LIST
    }

    fn name(&self) -> &'static str {
        "PortalInstance"
    }

    /// A member while the host's portal projection has live chains
    /// (`helio_pass_portal_cull::PORTAL_COUNTS`).
    fn is_member(&mut self, scene: &mut helio_core::SceneContent<'_>) -> bool {
        self.active_chain_count = scene
            .scene_buffers()
            .map(|buffers| helio_pass_portal_cull::portal_counts(buffers).chains);
        self.active_chain_count.unwrap_or(0) > 0
    }

    fn declare_resources(&self, builder: &mut ResourceBuilder) {
        // Read-only, exactly like FoliageGBufferPass — the eight G-buffer
        // targets are declared and owned by GBufferPass; this pass loads and
        // draws into them, which is what makes fusion into the same physical
        // pass possible.
        builder.read(helio_frame_api::gbuffer::GROUP);
        // Written by helio-pass-portal-mask, which must run before this pass.
        builder.read("portal_mask");
        builder.read("object_batch");
        // The G-buffer's draw segments, whose layout the portal segments follow.
        builder.read("culled_batch");
    }

    fn reads(&self) -> &'static [&'static str] {
        &[
            helio_frame_api::gbuffer::GROUP,
            "portal_mask",
            "object_batch",
            "culled_batch",
            helio_mats::MATERIAL_TEXTURES,
        ]
    }

    fn render_pass_descriptor_with_storage<'a>(
        &'a self,
        _target: &'a wgpu::TextureView,
        depth: &'a wgpu::TextureView,
        resources: &'a helio_core::ResourceRegistry<'a>,
        storage: &'a mut helio_core::RenderFrameStorage,
    ) -> Option<wgpu::RenderPassDescriptor<'a>> {
        // Always `Some` when the G-buffer exists (chain fusion is decided by
        // attachment identity at lock time, not per-frame content — see
        // helio-pass-foliage-gbuffer's docs for why returning `None` here
        // conditionally would break fusion).
        let gbuffer = resources.read::<helio_core::ViewGroup<'_, 4>>(
            helio_core::ResourceKey::new(helio_frame_api::gbuffer::GROUP),
            "PortalInstance",
        )?;
        let lightmap_uv = resources.read(
            helio_core::ResourceKey::new(helio_frame_api::gbuffer::LIGHTMAP_UV),
            "PortalInstance",
        )?;
        let sss_target = resources.read(
            helio_core::ResourceKey::new(helio_frame_api::gbuffer::SSS),
            "PortalInstance",
        )?;
        let extra_target = resources.read(
            helio_core::ResourceKey::new(helio_frame_api::gbuffer::EXTRA),
            "PortalInstance",
        )?;
        let velocity_target = resources.read(
            helio_core::ResourceKey::new(helio_frame_api::gbuffer::VELOCITY),
            "PortalInstance",
        )?;

        const LOAD: wgpu::Operations<wgpu::Color> = wgpu::Operations {
            load: wgpu::LoadOp::Load,
            store: wgpu::StoreOp::Store,
        };
        let color_attachments: &'a [Option<wgpu::RenderPassColorAttachment<'a>>] = storage
            .retain_boxed_slice(Box::new([
                Some(wgpu::RenderPassColorAttachment {
                    view: gbuffer.views[0],
                    resolve_target: None,
                    depth_slice: None,
                    ops: LOAD,
                }),
                Some(wgpu::RenderPassColorAttachment {
                    view: gbuffer.views[1],
                    resolve_target: None,
                    depth_slice: None,
                    ops: LOAD,
                }),
                Some(wgpu::RenderPassColorAttachment {
                    view: gbuffer.views[2],
                    resolve_target: None,
                    depth_slice: None,
                    ops: LOAD,
                }),
                Some(wgpu::RenderPassColorAttachment {
                    view: gbuffer.views[3],
                    resolve_target: None,
                    depth_slice: None,
                    ops: LOAD,
                }),
                Some(wgpu::RenderPassColorAttachment {
                    view: lightmap_uv,
                    resolve_target: None,
                    depth_slice: None,
                    ops: LOAD,
                }),
                Some(wgpu::RenderPassColorAttachment {
                    view: sss_target,
                    resolve_target: None,
                    depth_slice: None,
                    ops: LOAD,
                }),
                Some(wgpu::RenderPassColorAttachment {
                    view: extra_target,
                    resolve_target: None,
                    depth_slice: None,
                    ops: LOAD,
                }),
                Some(wgpu::RenderPassColorAttachment {
                    view: velocity_target,
                    resolve_target: None,
                    depth_slice: None,
                    ops: LOAD,
                }),
            ]));

        Some(wgpu::RenderPassDescriptor {
            label: Some("PortalInstance"),
            color_attachments,
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
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
        // The most groups to draw; with GPU counts the draw reads this
        // frame's live count (see `execute`).
        self.draw_count = ctx
            .registry
            .get::<helio_mesh_api::ObjectBatchFrameData<'_>>(helio_core::ResourceKey::new(
                "object_batch",
            ))
            .map(|b| b.all_draws().0)
            .unwrap_or(0);
        let screen = ScreenSize {
            width: ctx.width as f32,
            height: ctx.height as f32,
            _pad0: 0.0,
            _pad1: 0.0,
        };
        ctx.write_buffer(&self.screen_buf, 0, bytemuck::bytes_of(&screen));
        ctx.write_buffer(
            &self.globals_buf,
            0,
            bytemuck::bytes_of(&PortalGlobals {
                time: ctx.time,
                _pad: [0.0; 3],
            }),
        );
        Ok(())
    }

    fn execute(&mut self, ctx: &mut PassContext) -> HelioResult<()> {
        if ctx.frame_num < 3 || ctx.frame_num.is_multiple_of(120) {
            log::trace!(
                "[PortalInstance] frame={} draw_count={} render_pass_open={}",
                ctx.frame_num,
                self.draw_count,
                ctx.render_cmds().is_some(),
            );
        }
        if self.draw_count == 0 {
            return Ok(());
        }
        let Some(mut pass) = ctx.render_cmds() else {
            log::warn!(
                "[PortalInstance] frame={} no active render pass — G-buffer chain not fused/opened",
                ctx.frame_num
            );
            return Ok(());
        };
        let material_textures = ctx
            .registry
            .get::<helio_mats::MaterialTextureBindings<'_>>(helio_mats::material_textures());
        if ctx.frame_num < 3 {
            log::info!(
                "[PortalInstance] frame={} material_textures_available={}",
                ctx.frame_num,
                material_textures.is_some()
            );
        }

        // helio-pass-portal-mask must run before this pass every frame (see
        // helio-presets' add_pass order) — its output gates every
        // fragment below on actual on-screen portal visibility.
        let Some(portal_mask_view) = ctx.resource_pool.get_view("portal_mask") else {
            log::warn!("[PortalInstance] frame={} portal_mask not available — PortalMaskPass not wired before this pass?", ctx.frame_num);
            return Ok(());
        };

        let Some(batch) = ctx
            .registry
            .get::<helio_mesh_api::ObjectBatchFrameData<'_>>(helio_core::ResourceKey::new(
                "object_batch",
            ))
        else {
            return Ok(());
        };
        let Some(coord_data) = ctx
            .registry
            .get::<helio_core::CoordinateSpacesFrameData<'_>>(
                helio_core::resource_keys::coordinate_spaces(),
            )
        else {
            return Ok(());
        };
        let Some(portal_views) = ctx.scene_buffers.get(BufferKey::of("portal_views")) else {
            return Ok(());
        };
        let Some(portal_chain_handles) = ctx.scene_buffers.get(BufferKey::of(
            helio_pass_portal_cull::PORTAL_CHAIN_HANDLE_BUFFER,
        )) else {
            return Ok(());
        };
        let Some(portal_chain_portals) = ctx.scene_buffers.get(BufferKey::of(
            helio_pass_portal_cull::PORTAL_CHAIN_PORTAL_POOL_BUFFER,
        )) else {
            return Ok(());
        };

        // ── Bind group 0 ──────────────────────────────────────────────────
        let key = (
            [
                ctx.camera.clone(),
                batch.instances.clone(),
                coord_data.coordinate_spaces.clone(),
                portal_views.buffer.clone(),
                portal_chain_handles.buffer.clone(),
                portal_chain_portals.buffer.clone(),
                (*self.portal_compacted_indices_buf).clone(),
                (*self.portal_compacted_chains_buf).clone(),
                ctx.scene_buffers
                    .get(BufferKey::of(helio_mesh_api::SKELETAL_SKIN_VERTICES))
                    .map_or_else(|| batch.instances.clone(), |h| h.buffer.clone()),
                ctx.scene_buffers
                    .get(BufferKey::of(helio_mesh_api::SKELETAL_BONE_PALETTE))
                    .map_or_else(|| batch.instances.clone(), |h| h.buffer.clone()),
            ],
            portal_mask_view.clone(),
        );
        if self.bind_group_0_key.as_ref() != Some(&key) {
            self.bind_group_0 = Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("PortalInstance BG 0"),
                layout: &self.bind_group_layout_0,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: ctx.camera.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: self.screen_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: batch.instances.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: coord_data.coordinate_spaces.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: coord_data.coordinate_spaces_prev.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: self.portal_compacted_indices_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: portal_views.buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: portal_chain_handles.buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 8,
                        resource: self.portal_compacted_chains_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 9,
                        resource: wgpu::BindingResource::TextureView(portal_mask_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 10,
                        resource: portal_chain_portals.buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 11,
                        resource: key.0[8].as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 12,
                        resource: key.0[9].as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 13,
                        resource: self.globals_buf.as_entire_binding(),
                    },
                ],
            }));
            self.bind_group_0_key = Some(key);
        }

        // ── Bind group 1 (materials) — rebuilt when texture set changes ────
        let Some(material_textures) = material_textures else {
            return Ok(());
        };
        let Some(vertices_handle) = ctx.scene_buffers.get(helio_mesh_api::MESH_VERTICES.key())
        else {
            return Ok(());
        };
        let Some(indices_handle) = ctx.scene_buffers.get(helio_mesh_api::MESH_INDICES.key()) else {
            return Ok(());
        };
        // The material rows are the scene join's output, which reallocates as meshes
        // load: bind group 1 must follow the buffer, not only the texture set (the
        // main G-buffer pass keys on the same pair).
        let materials_handle = ctx.scene_buffers.get(helio_mats::MATERIALS.key());
        let materials_buf = materials_handle
            .map(|handle| &handle.buffer)
            .unwrap_or(batch.instances);
        let materials_epoch = materials_handle.map_or(0, |handle| handle.epoch);
        let needs_rebuild = self.bind_group_1_version
            != Some((material_textures.version, materials_epoch))
            || self.bind_group_1.is_none();
        if needs_rebuild {
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
                label: Some("PortalInstance BG 1"),
                layout: &self.bind_group_layout_1,
                entries: &entries,
            }));
            self.bind_group_1_version = Some((material_textures.version, materials_epoch));
        }

        let vertices = &vertices_handle.buffer;
        let indices = &indices_handle.buffer;

        // The slot indexes `portal_compacted_indices`.
        let slots = self
            .draw_slots
            .buffer(
                &ctx.device,
                helio_mesh_api::DrawSlots::len_of(self.portal_compacted_indices_buf.size(), 4),
            )
            .clone();
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, vertices.slice(..));
        pass.set_vertex_buffer(1, slots.slice(..));
        pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.set_bind_group(0, self.bind_group_0.as_ref().unwrap(), &[]);
        pass.set_bind_group(1, self.bind_group_1.as_ref().unwrap(), &[]);

        let culled: Option<helio_mesh_api::CulledBatchFrameData<'_>> = ctx
            .registry
            .get(helio_core::ResourceKey::new("culled_batch"));
        // The packed records exist once the cull has laid them out like the
        // G-buffer's (it sizes the portal segment buffers to follow its table).
        let packed_fit = {
            let shared = self.segments.lock().unwrap_or_else(|p| p.into_inner());
            culled.as_ref().is_some_and(|culled| {
                shared.indirect.size() >= culled.segments.indirect.size()
                    && culled
                        .segments
                        .counts
                        .is_none_or(|c| shared.counts.size() >= c.size())
            })
        };
        let packed = culled.as_ref().filter(|culled| {
            packed_fit && !culled.segments.is_empty() && batch.range_slot_capacity != 0
        });
        if let Some(culled) = packed {
            // One draw per material key, with that key's pipeline: a duplicate is
            // shaded by its own material (`PortalCullPass` packed the records per
            // segment, laid out by the G-buffer's own segment table).
            let segments = Arc::clone(&self.segments);
            let shared = segments.lock().unwrap_or_else(|p| p.into_inner());
            for (index, segment) in culled
                .segments
                .in_bucket(helio_mesh_api::ShadingBucket::Opaque)
            {
                pass.set_pipeline(self.pipeline_for(ctx.device, segment.graph_hash));
                let count = culled
                    .segments
                    .counts
                    .map(|_| helio_mesh_api::GpuDrawCount {
                        buffer: &shared.counts,
                        offset: index as u64 * 4,
                    });
                helio_mesh_api::multi_draw_indexed_indirect(
                    &mut pass,
                    &shared.indirect,
                    segment.first,
                    segment.capacity,
                    count,
                );
            }
            return Ok(());
        }

        // Before the segments exist (the first frames), draw every group with the
        // default pipeline. One indirect command per draw group (mesh+material):
        // every chain's surviving instances for a group are already merged into
        // that group's single indirect command by PortalCullPass's `finalize`
        // step. The GPU count is this frame's live group count; the `_count` draw
        // clamps it to this maximum, matching the capacity clamp here.
        let (_, draw_count) = batch.all_draws();
        let indirect_draw_count = self.draw_count.min(PORTAL_DRAW_CAPACITY);
        helio_mesh_api::multi_draw_indexed_indirect(
            &mut pass,
            &self.portal_indirect_buf,
            0,
            indirect_draw_count,
            draw_count,
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::portal_shader_source;
    use helio_mats::{MaterialBindingConfig, MaterialBindingMode};

    #[test]
    fn expanded_tier_rewrites_the_portal_material_bindings() {
        let source = portal_shader_source(MaterialBindingConfig {
            mode: MaterialBindingMode::Expanded,
            max_textures: 2,
        });

        assert!(!source.contains("wgpu_binding_array"));
        assert!(!source.contains("binding_array<"));
        assert!(source.contains("@group(1) @binding(2) var scene_texture_0"));
        assert!(source.contains("@group(1) @binding(5) var scene_sampler_1"));
        assert!(source.contains("case 1u: { return textureSampleLevel"));
    }

    #[test]
    fn binding_array_tier_keeps_the_native_portal_shader() {
        let source = portal_shader_source(MaterialBindingConfig {
            mode: MaterialBindingMode::BindingArray,
            max_textures: 256,
        });

        assert!(source.contains("enable wgpu_binding_array;"));
        assert!(
            source.contains("fn specular_aa_roughness"),
            "specular anti-aliasing spliced in"
        );
        assert!(source.contains("binding_array<texture_2d<f32>, 256>"));
    }
}

#[cfg(test)]
mod material_tests {
    use helio_mats::radiant::RadiantTemplateRegistry;

    /// What a Radiant material graph compiles to (the shape `material_graph.rs`
    /// produces): declarations, then the body that fills the surface.
    const GRAPH: &str = "/*RADIANT_GRAPH_DECLARATIONS*/\n\
        struct PulsarGraphOutput { base_color: vec4<f32>, roughness: f32, metallic: f32, ambient_occlusion: f32, emissive_color: vec4<f32>, normal: vec3<f32>, opacity: f32 }\n\
        fn pulsar_material_graph(material: GpuMaterial, input: MaterialInput) -> PulsarGraphOutput {\n\
            var o: PulsarGraphOutput;\n\
            o.base_color = vec4<f32>(input.world_normal * 0.5 + 0.5, 1.0);\n\
            o.roughness = 0.4; o.metallic = 0.0; o.ambient_occlusion = 1.0;\n\
            o.emissive_color = vec4<f32>(0.0); o.normal = vec3<f32>(0.0); o.opacity = 1.0;\n\
            return o;\n\
        }\n\
        /*RADIANT_GRAPH_BODY*/\n\
        let graph_surface = pulsar_material_graph(material, input);\n\
        albedo = graph_surface.base_color;\n\
        roughness = graph_surface.roughness;";

    fn validate(source: &str) {
        let module = naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|error| panic!("{}", error.emit_to_string(source)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|error| panic!("{error:?}"));
    }

    fn compose(template: &helio_mats::RadiantTemplate, graph: &str) -> String {
        template.compose(
            &super::base_shader_text(),
            graph,
            256,
            &[helio_mesh_api::DRAW_ROWS_SNIPPET],
        )
    }

    #[test]
    fn a_graph_material_compiles_into_the_portal_shader() {
        let registry = RadiantTemplateRegistry::new();
        let template = registry.get(0).expect("default template");
        let plain = compose(template, "");
        validate(&plain);
        let graph = compose(template, GRAPH);
        assert!(graph.contains("pulsar_material_graph(material, input)"));
        assert!(!graph.contains("// RADIANT_OVERRIDE_SURFACE"));
        validate(&graph);
    }

    #[test]
    fn the_built_in_material_classes_compile_into_the_portal_shader() {
        let registry = RadiantTemplateRegistry::new();
        for class in registry.keys() {
            let template = registry.get(class).unwrap();
            validate(&compose(template, ""));
        }
    }
}
