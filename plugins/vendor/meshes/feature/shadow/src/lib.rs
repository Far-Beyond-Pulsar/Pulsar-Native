//! Packed, budgeted shadow atlases with persistent static and dynamic caches.
//! CPU scheduling scans a fixed resident pool. GPU dirty bits gate cached draws;
//! each scheduled tile is cleared through its scissor and publishes its matrix
//! only after the matching depth commands. Pending dirty bits survive deferral.

pub mod feature;
pub use feature::ShadowDrawers;

use helio_core::graph::ResourceBuilder;
use helio_core::{BufferKey, PassContext, PrepareContext, RenderPass, Result as HelioResult};
use std::sync::Arc;

mod transmittance;
pub use transmittance::{TRANSMITTANCE_FORMAT, TRANSMITTANCE_KEY};

// ── Constants ─────────────────────────────────────────────────────────────────

/// Maximum logical faces in the bounded residency pool.
const MAX_SHADOW_FACES: usize = helio_pass_shadow_matrix::MAX_SHADOW_FACES;

/// Byte stride between consecutive face-index entries in `face_idx_buf`.
///
/// Must satisfy `device.limits().min_uniform_buffer_offset_alignment`, which is
/// guaranteed to be ≤ 256 on every wgpu backend (Metal, Vulkan, DX12, WebGPU).
const FACE_BUF_STRIDE: u64 = 256;
/// Faces serviced each frame outside the shadow update budget: those whose
/// cached static layer is current, so only the GPU's dirty flags decide
/// whether they draw anything.
const MAX_UNMETERED_FACES_PER_FRAME: usize = 192;

/// Regions of the face-index buffer, each `MAX_SHADOW_FACES` slots. The slot's
/// second word is the `ShadowDirty` bit its layer draws only when set (see
/// `FACE_REGION_GATES`); `0` draws unconditionally.
const FACE_REGIONS: usize = 3;
/// Pages per face (an 8x8 grid): the instances of the depth-clear draw.
const PAGES_PER_FACE: u32 = 64;
/// Region 0: ungated (the face's cached static layer is being rebuilt).
/// Region 1: gated on the dynamic bit. Region 2: gated on the static bit.
const FACE_REGION_GATES: [u32; FACE_REGIONS] = [0, 1, 2];

/// Dynamic offset into the face-index buffer for one layer of `face`. A face
/// whose static layer is being rebuilt draws both layers unconditionally;
/// otherwise each layer draws only if `ShadowDirty` marked that layer, so a
/// moving movable caster redraws the dynamic layer and leaves the cached
/// static layer (and glass, which follows it) alone.
fn face_idx_offset(face: usize, static_dirty: bool, static_layer: bool) -> u32 {
    let region = if static_dirty {
        0
    } else if static_layer {
        2
    } else {
        1
    };
    ((region * MAX_SHADOW_FACES + face) as u64 * FACE_BUF_STRIDE) as u32
}

/// Regions of `ShadowDirty`'s per-face draw counts (`face_geom_count_buf`),
/// each `MAX_SHADOW_FACES` `u32`s.
const MOVABLE_COUNTS: usize = 0;
const STATIC_COUNTS: usize = 1;
const TRANSMISSIVE_COUNTS: usize = 2;

/// Byte offset of `face`'s count in a region of `face_geom_count_buf`.
fn face_count_offset(region: usize, face: usize) -> u64 {
    (region * MAX_SHADOW_FACES + face) as u64 * 4
}

// ── Pass struct ───────────────────────────────────────────────────────────────

pub struct ShadowPass {
    depth_materials: helio_pass_gbuffer::depth_material::DepthMaterialBindings,
    /// Shadow geometry pipeline (depth-only, front-face culled, depth-bias = 2.0).
    pipeline: wgpu::RenderPipeline,
    face_static_gen: Vec<u64>,
    face_light_gen: Vec<u64>,
    face_last_update: Vec<u64>,
    face_strength: Vec<f32>,
    /// The strength last written to the GPU for each face, and the caster
    /// generation it was written under: a fade is the only reason to write
    /// it again, but a residency change rewrites it behind our back.
    face_strength_written: Vec<(f32, u64)>,
    face_ownership: Vec<(u32, helio_pass_shadow_matrix::ShadowTile)>,
    schedule_frame: u64,
    last_work: (u32, u32),
    /// Faces serviced last frame: (metered by the budget, unmetered GPU-gated).
    last_visits: (usize, usize),

    /// Depth-clear pipeline — renders a full-screen triangle at z=1.0 with
    /// `DepthCompare::Always` to GPU-clear individual atlas faces before geometry.
    depth_clear_pipeline: wgpu::RenderPipeline,

    #[allow(dead_code)]
    bgl_0: wgpu::BindGroupLayout,

    /// Per-face face-index values, written once at construction and never touched again.
    face_idx_buf: wgpu::Buffer,

    // ── Dynamic shadow atlas (Movable objects only) ───────────────────────────
    face_views: Box<[wgpu::TextureView]>,
    bg_0: Option<wgpu::BindGroup>,
    bg_0_key: Option<[wgpu::Buffer; 5]>,

    // ── Static shadow atlas (Static/Stationary objects only) ─────────────────
    static_face_views: Box<[wgpu::TextureView]>,
    /// Whole-array view of the static atlas, for the transmittance depth test.
    static_array_view: Option<wgpu::TextureView>,
    dynamic_array_view: wgpu::TextureView,
    /// Coloured transmittance of translucent static casters.
    transmittance: transmittance::Transmittance,
    /// Translucent casters were drawn: the layer is published only then, so
    /// receivers skip RGB visibility entirely in scenes without glass.
    has_glass: bool,

    pub compare_sampler: wgpu::Sampler,

    // ── GPU dirty buffers (shared with ShadowDirtyPass) ───────────────────────
    /// `array<u32, 256>` — 0 = clean, 1 = dirty (written by ShadowDirtyPass).
    /// Used as indirect draw count for the depth-clear triangle (0 = no clear, 1 = clear).
    face_dirty_buf: Arc<wgpu::Buffer>,
    /// Per-face draw counts written by ShadowDirtyPass, in movable, static and
    /// transmissive regions: 0 = clean, the list's live count = dirty. Used as
    /// the indirect draw count (`multi_draw_indexed_indirect_count`) for faces
    /// not known to be dirty on the CPU.
    #[allow(dead_code)]
    face_geom_count_buf: Arc<wgpu::Buffer>,
    /// Per-face 8x8 page masks of the dynamic layer written by ShadowDirtyPass
    /// (two words per face); a serviced face's words are cleared.
    face_pages_buf: Arc<wgpu::Buffer>,

    /// True when the device supports MULTI_DRAW_INDIRECT_COUNT (Vulkan 1.2+, DX12 tier2).
    /// False on macOS Metal, WASM, and older Vulkan/DX12.  When false the ObjectDirty path
    /// falls back to a full LoadOp::Clear + multi_draw_indexed_indirect (no per-face GPU culling).
    supports_multi_draw_count: bool,
    draw_slots: helio_mesh_api::DrawSlots,
}

impl ShadowPass {
    /// Reserved atlas operations and texels in the last frame; GPU dirty gates may reduce actual work.
    pub fn last_update_work(&self) -> (u32, u32) {
        self.last_work
    }

    /// Faces serviced in the last frame: (metered by the update budget,
    /// unmetered because only the GPU's dirty flags decide their work).
    pub fn last_face_visits(&self) -> (usize, usize) {
        self.last_visits
    }

    /// Allocate persistent atlas resources.
    ///
    /// `face_dirty_buf` and `face_geom_count_buf` are shared with `ShadowDirtyPass`
    /// which writes them each frame; they arrive via `Arc`.
    /// Compatibility constructor. Physical storage is now one packed layer.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        face_dirty_buf: Arc<wgpu::Buffer>,
        face_geom_count_buf: Arc<wgpu::Buffer>,
        face_pages_buf: Arc<wgpu::Buffer>,
        _face_cull_indirect: Arc<wgpu::Buffer>,
        _face_cull_counts: Arc<wgpu::Buffer>,
        atlas_size: u32,
        _atlas_layers: u32,
    ) -> Self {
        Self::new_tiled(
            device,
            queue,
            face_dirty_buf,
            face_geom_count_buf,
            face_pages_buf,
            atlas_size,
        )
    }
    pub fn new_tiled(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        face_dirty_buf: Arc<wgpu::Buffer>,
        face_geom_count_buf: Arc<wgpu::Buffer>,
        face_pages_buf: Arc<wgpu::Buffer>,
        atlas_size: u32,
    ) -> Self {
        let atlas_layers = 1;
        // ── Shader ────────────────────────────────────────────────────────────
        let depth_materials =
            helio_pass_gbuffer::depth_material::DepthMaterialBindings::new(device);
        let source = depth_materials.shader_source(include_str!("../shaders/shadow.wgsl"));
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Shadow cutout"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });

        let clear_shader = helio_core::shader::module(
            device,
            "Shadow/DepthClear",
            helio_core::include_wgsl!("../shaders/depth_clear.wgsl"),
        );

        // ── Bind Group Layout 0 ───────────────────────────────────────────────
        let bgl_0 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Shadow BGL 0"),
            entries: &[
                // binding 0: shadow_matrices — array of mat4x4 light-space transforms
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // binding 1: instances — per-instance world transforms
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
                // binding 2: face index — 16-byte uniform, dynamic offset selects face
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // binding 3: coordinate_spaces — current-frame per-space transforms
                // (sublevels/portals), slot 0 = identity. See gbuffer.wgsl for the
                // full mechanism; shadows only need the current-frame copy.
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
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // binding 7: per-face dirty page masks of the dynamic layer
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
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

        // ── Pipeline ──────────────────────────────────────────────────────────
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Shadow PL"),
            bind_group_layouts: &[Some(&bgl_0), Some(&depth_materials.layout)],
            immediate_size: 0,
        });

        let pipeline = helio_core::shader::create_render_pipeline(
            &device,
            wgpu::RenderPipelineDescriptor {
                label: Some("Shadow Pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    // Shared mesh vertex buffer layout (stride = 40 bytes, matches GBuffer pass).
                    // Only position (Float32x3 at offset 0) is needed for depth projection.
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
                                    format: wgpu::VertexFormat::Float32x2,
                                    offset: 16,
                                    shader_location: 1,
                                },
                            ],
                        }),
                        Some(helio_mesh_api::DRAW_SLOT_LAYOUT),
                    ],
                },
                // Depth-only: no colour outputs, no fragment shader.
                // The GPU writes depth from the vertex clip position automatically.
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    // Front-face culling: light "looks into" the scene; culling the faces
                    // visible to the light prevents writing depth for lit-surface geometry
                    // directly, eliminating shadow acne.  Identical convention to UE4/Unity.
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: wgpu::StencilState::default(),
                    // slope_scale compensates for FP depth precision on surfaces at
                    // grazing angles to the light.  Without it the shadow map depth for
                    // a surface can be equal-to or less-than the depth reconstructed in
                    // the lighting shader for that same surface, causing self-shadowing
                    // on every light independently (making each light appear to inherit
                    // every other light's shadow geometry).
                    // constant is left at 0 — that was the source of the visible offset.
                    bias: wgpu::DepthBiasState {
                        constant: 0,
                        slope_scale: 2.0,
                        clamp: 0.0,
                    },
                }),
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            },
        );

        // ── Depth-clear pipeline ───────────────────────────────────────────────
        // GPU-clear individual shadow atlas faces: renders a full-screen triangle
        // at depth=1.0 (far plane) using DepthCompare::Always to overwrite existing
        // depth values.  No vertex buffer, no fragment shader, no depth bias.
        let depth_clear_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Shadow/DepthClear PL"),
                bind_group_layouts: &[Some(&bgl_0)],
                immediate_size: 0,
            });

        let depth_clear_pipeline = helio_core::shader::create_render_pipeline(
            &device,
            wgpu::RenderPipelineDescriptor {
                label: Some("Shadow/DepthClear Pipeline"),
                layout: Some(&depth_clear_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &clear_shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: None,
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Always),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            },
        );

        // One u32 per face at FACE_BUF_STRIDE byte intervals.
        // The CPU never touches this buffer after construction.
        let mut face_idx_data =
            vec![0u8; FACE_REGIONS * MAX_SHADOW_FACES * FACE_BUF_STRIDE as usize];
        for i in 0..FACE_REGIONS * MAX_SHADOW_FACES {
            let offset = i * FACE_BUF_STRIDE as usize;
            face_idx_data[offset..offset + 4]
                .copy_from_slice(&((i % MAX_SHADOW_FACES) as u32).to_ne_bytes());
            face_idx_data[offset + 4..offset + 8]
                .copy_from_slice(&FACE_REGION_GATES[i / MAX_SHADOW_FACES].to_ne_bytes());
        }
        let face_idx_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Shadow/FaceIdx"),
            size: (FACE_REGIONS * MAX_SHADOW_FACES) as u64 * FACE_BUF_STRIDE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&face_idx_buf, 0, &face_idx_data);

        // ── Face views (lazily initialized from graph-owned textures) ──────────

        // Comparison sampler for PCF shadow lookups in the lighting pass.
        let compare_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Shadow/Compare"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });

        let transmittance =
            transmittance::Transmittance::new(device, queue, &bgl_0, atlas_size, atlas_layers);

        let make_atlas = |label| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: atlas_size,
                    height: atlas_size,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        };
        let dynamic_atlas = make_atlas("Dynamic shadow atlas");
        let static_atlas = make_atlas("Static shadow atlas");
        let array_view = |tex: &wgpu::Texture| {
            tex.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            })
        };
        let face_views = Self::create_face_views(&dynamic_atlas, "Dynamic tiles", 1);
        let static_face_views = Self::create_face_views(&static_atlas, "Static tiles", 1);
        Self {
            depth_materials,
            last_work: (0, 0),
            last_visits: (0, 0),
            dynamic_array_view: array_view(&dynamic_atlas),
            face_ownership: vec![(0, Default::default()); MAX_SHADOW_FACES],
            pipeline,
            face_static_gen: vec![u64::MAX; MAX_SHADOW_FACES],
            face_light_gen: vec![u64::MAX; MAX_SHADOW_FACES],
            face_last_update: vec![0; MAX_SHADOW_FACES],
            face_strength: vec![0.0; MAX_SHADOW_FACES],
            face_strength_written: vec![(f32::NAN, u64::MAX); MAX_SHADOW_FACES],
            schedule_frame: 0,
            depth_clear_pipeline,
            static_array_view: Some(array_view(&static_atlas)),
            transmittance,
            has_glass: false,
            bgl_0,
            bg_0: None,
            bg_0_key: None,
            face_idx_buf,
            face_views,
            static_face_views,
            compare_sampler,
            face_dirty_buf,
            face_geom_count_buf,
            face_pages_buf,
            supports_multi_draw_count: device
                .features()
                .contains(wgpu::Features::MULTI_DRAW_INDIRECT_COUNT),
            draw_slots: Default::default(),
        }
    }

    fn create_face_views(
        texture: &wgpu::Texture,
        label: &str,
        layer_count: u32,
    ) -> Box<[wgpu::TextureView]> {
        (0..layer_count)
            .map(|i| {
                texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some(label),
                    format: Some(wgpu::TextureFormat::Depth32Float),
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: i,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect()
    }
}

// ── RenderPass impl ───────────────────────────────────────────────────────────

impl RenderPass for ShadowPass {
    fn scene_buffers(&self) -> &'static [&'static str] {
        &[
            "builtin_mesh_index",
            "builtin_mesh_vertex",
            "materials",
            "skeletal_bone_palette",
            "skeletal_skin_vertices",
        ]
    }

    fn render_pass_descriptor<'a>(
        &'a self,
        _target: &'a wgpu::TextureView,
        _depth: &'a wgpu::TextureView,
        _resources: &'a helio_core::ResourceRegistry<'a>,
    ) -> Option<wgpu::RenderPassDescriptor<'a>> {
        None
    }

    fn declare_resources(&self, builder: &mut ResourceBuilder) {
        // Pass-owned one-layer array views, preserving the sampling ABI at one physical layer.
        builder.write_buffer("shadow_atlas");
        builder.write_buffer("static_shadow_atlas");
        builder.read("object_batch");
        builder.read("shadow_matrices");
        builder.read("shadow_dirty");
        builder.read("material_textures");
        // Pass-owned (it caches with the static atlas): declared for ordering,
        // routed in `publish`.
        builder.write_buffer(TRANSMITTANCE_KEY);
    }

    fn name(&self) -> &'static str {
        "Shadow"
    }

    fn writes(&self) -> &'static [&'static str] {
        &[
            "shadow_atlas",
            "shadow_sampler",
            "static_shadow_atlas",
            TRANSMITTANCE_KEY,
        ]
    }

    fn publish<'a>(&self, frame: &mut helio_core::ResourceRegistry<'a>) {
        frame.route_named_texture("shadow_atlas", &self.dynamic_array_view, self.name());
        if let Some(view) = self.static_array_view.as_ref() {
            frame.route_named_texture("static_shadow_atlas", view, self.name());
        }
        if self.has_glass {
            frame.route_named_texture(TRANSMITTANCE_KEY, &self.transmittance.view, self.name());
        }
    }

    fn prepare(&mut self, _ctx: &PrepareContext) -> HelioResult<()> {
        helio_core::cpu_scope!("Shadow: prepare");
        Ok(())
    }

    fn execute(&mut self, ctx: &mut PassContext) -> HelioResult<()> {
        helio_core::cpu_scope!("Shadow: execute");
        self.last_work = (0, 0);
        self.last_visits = (0, 0);
        if self.depth_materials.update(ctx) {
            self.face_static_gen.fill(u64::MAX);
            self.face_light_gen.fill(u64::MAX);
        }
        if self.depth_materials.group.is_none() {
            return Ok(());
        }
        let Some(batch) = ctx
            .registry
            .get::<helio_mesh_api::ObjectBatchFrameData<'_>>(helio_core::ResourceKey::new(
                "object_batch",
            ))
        else {
            return Ok(());
        };
        let Some(data) = ctx
            .registry
            .get::<helio_pass_shadow_matrix::ShadowMatricesFrameData<'_>>(
                helio_pass_shadow_matrix::shadow_matrices(),
            )
        else {
            return Ok(());
        };
        let Some(residency) = data.residency else {
            return Ok(());
        };
        let Some(coords) = ctx
            .registry
            .get::<helio_core::CoordinateSpacesFrameData<'_>>(
                helio_core::resource_keys::coordinate_spaces(),
            )
        else {
            return Ok(());
        };
        let Some(vertices) = ctx.scene_buffers.get(BufferKey::of("builtin_mesh_vertex")) else {
            return Ok(());
        };
        let Some(indices) = ctx.scene_buffers.get(BufferKey::of("builtin_mesh_index")) else {
            return Ok(());
        };
        let desired = data.desired_matrices.unwrap_or(data.shadow_matrices);
        let skin = ctx
            .scene_buffers
            .get(BufferKey::of("skeletal_skin_vertices"));
        let palette = ctx
            .scene_buffers
            .get(BufferKey::of("skeletal_bone_palette"));
        let key = [
            desired.clone(),
            batch.instances.clone(),
            coords.coordinate_spaces.clone(),
            skin.map_or_else(|| batch.instances.clone(), |h| h.buffer.clone()),
            palette.map_or_else(|| batch.instances.clone(), |h| h.buffer.clone()),
        ];
        if self.bg_0_key.as_ref() != Some(&key) {
            helio_core::cpu_scope!("Shadow: rebuild bind group");
            self.bg_0 = Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Shadow tiles"),
                layout: &self.bgl_0,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: desired.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: batch.instances.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &self.face_idx_buf,
                            offset: 0,
                            size: std::num::NonZeroU64::new(16),
                        }),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: coords.coordinate_spaces.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: self.face_dirty_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: key[3].as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: key[4].as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: self.face_pages_buf.as_entire_binding(),
                    },
                ],
            }));
            self.bg_0_key = Some(key);
        }
        helio_core::cpu_scope!("Shadow: gather candidates");
        self.schedule_frame += 1;
        self.has_glass = batch.readback_shadow_transmissive_draw_count > 0;
        let mut candidates = Vec::with_capacity(MAX_SHADOW_FACES);
        let fade_step = 1.0 / data.budget.fade_frames.max(1) as f32;
        for (slot, r) in residency.residents.iter().enumerate() {
            for (f, t) in r.tiles.iter().enumerate() {
                let face = slot * 6 + f;
                if r.owner == 0 || t.size == 0 {
                    self.face_strength[face] = 0.0;
                    continue;
                }
                if self.face_ownership[face] != (r.owner, *t) {
                    self.face_strength[face] = 0.0;
                    self.face_last_update[face] = 0;
                    self.face_ownership[face] = (r.owner, *t);
                }
                let changed = self.face_light_gen[face] != data.per_caster_dirty_gen[slot];

                let target = if self.face_last_update[face] == 0 {
                    0.0
                } else {
                    r.strength
                };
                self.face_strength[face] +=
                    (target - self.face_strength[face]).clamp(-fade_step, fade_step);
                let written = (self.face_strength[face], data.per_caster_dirty_gen[slot]);
                if self.face_strength_written[face] != written {
                    ctx.write_buffer(
                        data.shadow_matrices,
                        face as u64 * 96 + 76,
                        bytemuck::bytes_of(&self.face_strength[face]),
                    );
                    self.face_strength_written[face] = written;
                }
                let static_dirty =
                    changed || self.face_static_gen[face] != batch.shadow_static_generation;
                // Aging guarantees low-priority pending faces are eventually serviced.
                let age = self
                    .schedule_frame
                    .saturating_sub(self.face_last_update[face]) as f32;
                let urgency = if changed { 1.0e6 } else { r.score + age * 0.01 };
                candidates.push((face, static_dirty, urgency));
            }
        }
        {
            helio_core::cpu_scope!("Shadow: rank candidates");
            candidates.sort_by(|a, b| b.2.total_cmp(&a.2).then(a.0.cmp(&b.0)));
        }
        let mut updates = 0u32;
        let mut texels = 0u32;
        let mut free_visits = 0usize;
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
        let bg = self.bg_0.as_ref().unwrap();

        // ── Select the faces to service this frame ───────────────────────────
        helio_core::cpu_scope!("Shadow: select faces");
        let mut selected: Vec<(usize, bool)> = Vec::with_capacity(candidates.len());
        for (face, static_dirty, _) in candidates {
            let r = &residency.residents[face / 6];
            let tile = r.tiles[face % 6];
            let layers = 3;
            let cost = tile.size * tile.size * layers;
            // Without GPU-side draw counts a face that is not rebuilding its
            // static layer still draws the whole caster lists (the vertex
            // shader discards them), so it is real work to the budget.
            if static_dirty || !self.supports_multi_draw_count {
                // A rebuild of the cached static layer: real work known to the
                // CPU, so it is metered.
                if updates + layers > data.budget.updates_per_frame
                    || texels + cost > data.budget.update_texels_per_frame
                {
                    continue;
                }
                updates += layers;
                texels += cost;
            } else {
                // Anything else is driven by the GPU's own dirty flags
                // (`ShadowDirty`: a movable caster moved, a light changed): a
                // clean face runs no vertices, a dirty one redraws only its
                // dynamic layer. The CPU cannot tell which is which, so
                // charging these to the budget let clean faces starve the
                // dirty ones and a moving object's shadow updated in steps.
                // They are visited every frame, up to a safety cap.
                if free_visits >= MAX_UNMETERED_FACES_PER_FRAME {
                    continue;
                }
                free_visits += 1;
            }
            selected.push((face, static_dirty));
        }
        self.last_visits = (selected.len() - free_visits, free_visits);

        // ── Both depth layers: one render pass each, every selected face's
        // tile drawn in turn (a viewport and scissor per face), instead of two
        // passes per face. A clean face costs a few state changes and an empty
        // multi-draw, no pass boundary. The layers are separate textures, so
        // they are timed apart.
        let mut cmds = ctx.graphics_cmds();
        for is_static in [true, false] {
            let scope = if is_static {
                "Shadow: static layer"
            } else {
                "Shadow: dynamic layer"
            };
            helio_core::cpu_scope!(scope);
            ctx.begin_gpu_scope(&mut cmds, scope);
            {
                let view = if is_static {
                    &self.static_face_views[0]
                } else {
                    &self.face_views[0]
                };
                let mut pass = cmds.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some(scope),
                    color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_vertex_buffer(0, vertices.buffer.slice(..));
                pass.set_vertex_buffer(1, slots.slice(..));
                pass.set_index_buffer(indices.buffer.slice(..), wgpu::IndexFormat::Uint32);
                for &(face, static_dirty) in &selected {
                    let r = &residency.residents[face / 6];
                    let tile = r.tiles[face % 6];
                    let dynamic_offset = face_idx_offset(face, static_dirty, is_static);
                    let face_count = |region| helio_mesh_api::GpuDrawCount {
                        buffer: &self.face_geom_count_buf,
                        offset: face_count_offset(region, face),
                    };
                    pass.set_viewport(
                        tile.x as f32,
                        tile.y as f32,
                        tile.size as f32,
                        tile.size as f32,
                        0.0,
                        1.0,
                    );
                    pass.set_scissor_rect(tile.x, tile.y, tile.size, tile.size);
                    pass.set_pipeline(&self.depth_clear_pipeline);
                    pass.set_bind_group(0, bg, &[dynamic_offset]);
                    pass.draw(0..6, 0..PAGES_PER_FACE);
                    let disabled = r.flags & if is_static { 16 } else { 32 } != 0;
                    if disabled {
                        continue;
                    }
                    pass.set_pipeline(&self.pipeline);
                    pass.set_bind_group(1, self.depth_materials.group.as_ref().unwrap(), &[]);
                    pass.set_bind_group(0, bg, &[dynamic_offset]);
                    // With GPU counts, every list is bounded by its capacity
                    // and drawn to this frame's live count, so a caster
                    // spawned or despawned this frame is already (or no
                    // longer) in it.
                    if is_static {
                        let (max_draws, count) = batch.shadow_static_draws();
                        // A face not known to be dirty is redrawn only if
                        // `ShadowDirty` dirtied it on the GPU (a light's
                        // matrix changed): its per-face count is the live
                        // count then, else 0. Drawing the full list and
                        // discarding it in the vertex shader ran every static
                        // caster's vertices for every scheduled face.
                        let count = if self.supports_multi_draw_count && !static_dirty {
                            Some(face_count(STATIC_COUNTS))
                        } else {
                            count
                        };
                        helio_mesh_api::multi_draw_indexed_indirect(
                            &mut pass,
                            batch.shadow_static_indirect,
                            0,
                            max_draws,
                            count,
                        );
                    } else {
                        let (max_draws, count) = batch.shadow_movable_draws();
                        let count = if self.supports_multi_draw_count && !static_dirty {
                            // `ShadowDirty`'s per-face count: the live count
                            // when the face's movable casters changed, else 0.
                            Some(face_count(MOVABLE_COUNTS))
                        } else {
                            count
                        };
                        helio_mesh_api::multi_draw_indexed_indirect(
                            &mut pass,
                            batch.shadow_movable_indirect,
                            0,
                            max_draws,
                            count,
                        );
                    }
                }
            }
            ctx.end_gpu_scope(&mut cmds, scope);
        }

        // ── Glass transmittance (follows the static layer) ───────────────────
        if let Some(depth) = self.static_array_view.as_ref() {
            helio_core::cpu_scope!("Shadow: transmittance");
            ctx.begin_gpu_scope(&mut cmds, "Shadow: transmittance");
            let materials = ctx.scene_buffers.get(BufferKey::of("materials"));
            for &(face, static_dirty) in &selected {
                // Glass is cached with the static layer: with none in the
                // scene, only a rebuild has a stale tile to clear.
                if !static_dirty && !self.has_glass {
                    continue;
                }
                let r = &residency.residents[face / 6];
                let tile = r.tiles[face % 6];
                let face_count = |region| helio_mesh_api::GpuDrawCount {
                    buffer: &self.face_geom_count_buf,
                    offset: face_count_offset(region, face),
                };
                self.transmittance.render_face(
                    ctx.device,
                    &mut cmds,
                    materials.map(|m| &m.buffer),
                    bg,
                    0,
                    face_idx_offset(face, static_dirty, true),
                    depth,
                    batch.shadow_transmissive_indirect,
                    // Static layer: whether there is glass at all
                    // follows the readback, like the rest of it.
                    if r.flags & 16 == 0 && self.has_glass {
                        batch.shadow_transmissive_draws().0
                    } else {
                        0
                    },
                    // As the static layer: a clean face draws no glass.
                    if self.supports_multi_draw_count && !static_dirty {
                        Some(face_count(TRANSMISSIVE_COUNTS))
                    } else {
                        batch.shadow_transmissive_draws().1
                    },
                    &vertices.buffer,
                    &slots,
                    &indices.buffer,
                    Some([tile.x, tile.y, tile.size]),
                );
            }
            ctx.end_gpu_scope(&mut cmds, "Shadow: transmittance");
        }

        // ── Activate each serviced face's matrix and retire its dirty state ──
        // Only after its matching depth has been rendered.
        helio_core::cpu_scope!("Shadow: activate faces");
        ctx.begin_gpu_scope(&mut cmds, "Shadow: activate faces");
        for &(face, static_dirty) in &selected {
            cmds.copy_buffer_to_buffer(
                desired,
                face as u64 * 96,
                data.shadow_matrices,
                face as u64 * 96,
                64,
            );
            // The "ready" marker only changes when the face was (re)assigned:
            // a rebuild of its static layer, or its first service.
            if static_dirty || self.face_last_update[face] == 0 {
                ctx.write_buffer(
                    data.shadow_matrices,
                    face as u64 * 96 + 92,
                    bytemuck::bytes_of(&2u32),
                );
            }
            cmds.clear_buffer(&self.face_dirty_buf, face as u64 * 4, Some(4));
            cmds.clear_buffer(&self.face_pages_buf, face as u64 * 8, Some(8));
            for region in [MOVABLE_COUNTS, STATIC_COUNTS, TRANSMISSIVE_COUNTS] {
                cmds.clear_buffer(
                    &self.face_geom_count_buf,
                    face_count_offset(region, face),
                    Some(4),
                );
            }
            self.face_static_gen[face] = batch.shadow_static_generation;
            self.face_light_gen[face] = data.per_caster_dirty_gen[face / 6];
            self.face_last_update[face] = self.schedule_frame;
        }
        ctx.end_gpu_scope(&mut cmds, "Shadow: activate faces");
        self.last_work = (updates, texels);
        Ok(())
    }
}
