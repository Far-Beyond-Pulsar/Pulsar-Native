//! G-Buffer pass.
//!
//! Renders opaque geometry to 8 render targets (albedo, normal, ORM, emissive,
//! lightmap_uv, sss, extra, velocity) + depth.
//! O(1) CPU: single `multi_draw_indexed_indirect` call regardless of scene size.
//!
//! # Render Targets (owned by this pass)
//!
//! | Slot | Name           | Format        | Contents                          |
//! |------|----------------|---------------|-----------------------------------|
//! | 0    | albedo         | Rgba8Unorm    | albedo.rgb + alpha                |
//! | 1    | normal         | Rgba16Float   | world normal.xyz + F0.r           |
//! | 2    | orm            | Rgba8Unorm    | AO, roughness, metallic, F0.g     |
//! | 3    | emissive       | Rgba16Float   | emissive.rgb + F0.b               |
//! | 7    | gbuffer_velocity | Rgba16Float  | motion.xy + depth residual.z / validity.w  |
//!
//! # Material Bind Group
//!
//! Group 1 is the materials' table (`helio_mats`): the material rows, the
//! texture metadata and the texture table the materials' feature publishes
//! (`MATERIAL_TEXTURES`), or the stand-in without the materials plugin
//! (`MaterialStandIn`: default grey PBR):
//!  - binding 0: materials storage buffer
//!  - binding 1: material_textures storage buffer (MaterialTextureData array)
//!  - native: binding arrays for scene textures and samplers
//!  - WebGPU: 16 fixed texture bindings followed by 16 fixed sampler bindings
//!
//! # Program
//!
//! [`GBUFFER_PROGRAM`] is the meshes' vertex layout, instance rows and
//! G-buffer packing; a material class's template (its surface function)
//! and graph are composed into it (`RadiantTemplate::compose`).
//!
//! # Vertex / Index Buffers
//!
//! This pass owns no mesh data.  The caller must bind the shared mesh vertex
//! buffer (slot 0) and index buffer before this pass executes.

pub mod feature;
pub use feature::GBufferDrawers;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

pub mod depth_material;
use helio_mats::radiant::{RadiantShaderCache, RadiantShaderKey};
use helio_mesh_api::{
    multi_draw_indexed_indirect, CulledBatchFrameData, DrawSlots, ObjectBatchFrameData,
    DRAW_SLOT_LAYOUT,
};

/// Debug mode id of the wireframe view (needs `POLYGON_MODE_LINE`).
const WIREFRAME_DEBUG_MODE: u32 = 40;
use helio_core::graph::{ResourceBuilder, ResourceFormat, ResourceSize};
use helio_core::{
    DebugViewDescriptor, PassContext, PrepareContext, RenderPass, Result as HelioResult,
};
use pulsar_scenedb::gpu::BufferKey;
use std::collections::HashMap;

// ── Uniform types ─────────────────────────────────────────────────────────────

/// Per-frame globals uploaded to the GPU each frame (matches `Globals` in gbuffer.wgsl).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct GBufferGlobals {
    pub frame: u32,
    pub delta_time: f32,
    pub light_count: u32,
    pub ambient_intensity: f32,
    pub ambient_color: [f32; 4],
    pub csm_splits: [f32; 4],
    pub debug_mode: u32,
    pub screen_width: f32,
    pub screen_height: f32,
    /// The host-driven frame clock (`PrepareContext::time`): the
    /// shader-graph `time` node.
    pub time: f32,
}

// ── Pass struct ───────────────────────────────────────────────────────────────

pub struct GBufferPass {
    /// The draw slots the vertex shader reads (see [`DrawSlots`]).
    draw_slots: DrawSlots,
    timing_query: Option<wgpu::QuerySet>,
    material_binding: helio_mats::MaterialBindingConfig,
    pipelines: HashMap<RadiantShaderKey, wgpu::RenderPipeline>,
    /// Device supports `POLYGON_MODE_LINE`, so the wireframe debug view is offered.
    wireframe_supported: bool,
    /// The reflected camera reverses handedness; flip front-face winding while
    /// preserving ordinary back-face culling for the capture.
    mirrored_view: bool,
    shader_cache: RadiantShaderCache,
    /// The materials' opaque templates (`helio_mats::MaterialTemplates`),
    /// shared, never deep-cloned (see `SharedTemplateRegistry`). `None`: the
    /// built-in templates.
    template_registry: Option<helio_mats::radiant::SharedTemplateRegistry>,
    /// Key set as of the last sync, to detect content changes cheaply
    /// (registry identity is stable, so `Arc::ptr_eq` can't detect a
    /// template being re-registered under an existing id).
    last_template_keys: Vec<u32>,
    pipeline_layout: wgpu::PipelineLayout,
    bind_group_layout_0: wgpu::BindGroupLayout,
    bind_group_layout_1: wgpu::BindGroupLayout,
    /// Shared UV reference image used by the UV visualization mode
    /// ([`UV_DEBUG_MODE`]); a white texel while that mode is off.
    uv_grid_texture: wgpu::Texture,
    /// Pixels waiting for their upload in `prepare` (empty once uploaded).
    uv_grid_pixels: Vec<u8>,
    uv_grid_extent: (u32, u32),
    uv_grid_bind_group_layout: wgpu::BindGroupLayout,
    uv_grid_bind_group: wgpu::BindGroup,
    uv_grid_uploaded: bool,
    /// Whether `uv_grid_texture` holds the reference image (not the texel).
    uv_grid_full: bool,
    /// Group 0: camera + globals + instance_data. Rebuilt when buffer pointers change.
    bind_group_0: Option<wgpu::BindGroup>,
    bind_group_0_key: Option<[wgpu::Buffer; 7]>,
    /// Group 1: materials + material_textures + bindless texture arrays.
    bind_group_1: Option<wgpu::BindGroup>,
    bind_group_1_version: Option<(u64, u64)>,
    /// Per-frame globals uploaded in `prepare()`.
    globals_buf: wgpu::Buffer,
    /// CSM cascade split distances. Must match the values used in shadow_matrices.wgsl
    /// so that cascade selection in any shader that reads `globals.csm_splits` is
    /// consistent with the shadow maps that were actually generated.
    pub csm_splits: [f32; 4],
    /// Debug visualisation mode forwarded to the GBuffer shader (0 = off).
    pub debug_mode: u32,
    /// Lightmap atlas regions buffer (empty until bake data is loaded)
    lightmap_atlas_regions_buf: wgpu::Buffer,
    /// Portal/sublevel-local transforms, indexed by an instance's `space_id`.
    /// Pass-owned (not `Renderer`-owned): every consumer of `"coordinate_
    /// spaces"` (`OcclusionCullPass`, `IndirectDispatchPass`, `ShadowPass`,
    /// both portal passes) needs `space_id 0` to always
    /// resolve to identity even when nothing authors portal/sublevel content,
    /// and `GBufferPass` is the one pass guaranteed present in every graph --
    /// see `helio_core::CoordinateSpacesFrameData`'s doc for the
    /// still-pending real fix (a SceneDB-driven portal/sublevel component).
    coordinate_spaces: wgpu::Buffer,
    coordinate_spaces_prev: wgpu::Buffer,
    /// Zeroed sentinel storage for scenes without skeletal assets.
    fallback_skeletal_storage: wgpu::Buffer,
    /// What the material bind group holds without the materials plugin's
    /// texture table or material rows (default grey PBR), made on first use.
    stand_in: Option<helio_mats::MaterialStandIn>,
    /// CPU-side projection input staged by the generic graph interface and
    /// uploaded during `prepare()` with the current frame queue.
    pending_coordinate_spaces: Option<Vec<glam::Mat4>>,
    /// The table last uploaded (`COORDINATE_SPACE_SLOTS * 16` words), which becomes
    /// the previous-frame table when the next one arrives.
    last_coordinate_words: Option<Vec<f32>>,
    /// `coordinate_spaces_prev` differs from `coordinate_spaces` and must be
    /// brought level next frame (a table that stops changing has no motion).
    coordinate_prev_stale: bool,
}

/// The meshes' G-buffer program (`shaders/gbuffer.wgsl`): the vertex
/// layout, the instance rows (`//!use mesh_draw_rows`) and the G-buffer
/// packing, over the materials' snippets; a template's surface function is
/// composed after it.
pub const GBUFFER_PROGRAM: &str = include_str!("../shaders/gbuffer.wgsl");

/// Coordinate-space slots backed by GPU storage (slot 0 is the identity). An
/// instance carries its space id in 8 bits of its flags, so 256 is the most.
pub const COORDINATE_SPACE_SLOTS: usize = 256;

/// The table as GPU words: slot 0 identity, then `spaces` (the non-identity
/// tail) from slot 1.
fn coordinate_space_words(spaces: &[glam::Mat4]) -> Vec<f32> {
    let mut words = vec![0.0f32; COORDINATE_SPACE_SLOTS * 16];
    words[..16].copy_from_slice(&glam::Mat4::IDENTITY.to_cols_array());
    for (slot, space) in spaces.iter().take(COORDINATE_SPACE_SLOTS - 1).enumerate() {
        words[(slot + 1) * 16..(slot + 2) * 16].copy_from_slice(&space.to_cols_array());
    }
    words
}

impl GBufferPass {
    /// Configure front-face winding for a reflected camera before the first
    /// frame builds material pipelines.
    pub fn set_mirrored_view(&mut self, mirrored: bool) {
        self.mirrored_view = mirrored;
    }

    /// Create the GBuffer pass.
    /// Compiles surfaces from `templates` (custom template classes
    /// included) instead of the built-in templates alone.
    pub fn with_template_registry(
        mut self,
        templates: helio_mats::radiant::SharedTemplateRegistry,
    ) -> Self {
        self.template_registry = Some(templates);
        self
    }

    pub fn new(device: &wgpu::Device) -> Self {
        let material_binding = helio_mats::MaterialBindingConfig::for_device(device);
        // ── Globals buffer ────────────────────────────────────────────────────
        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("GBufferGlobals"),
            size: std::mem::size_of::<GBufferGlobals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // ── Bind Group Layout 0 ───────────────────────────────────────────────
        let bind_group_layout_0 =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("GBuffer BGL 0"),
                entries: &[
                    // binding 0: camera (storage, VERTEX | FRAGMENT)
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
                    // binding 1: globals (uniform, FRAGMENT)
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
                    // binding 2: instance_data (storage read, VERTEX)
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
                    // binding 3: lightmap_atlas_regions (storage read, VERTEX)
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
                    // binding 4: compacted_indices_2 (storage read, VERTEX) — per-group
                    // surviving instance slots after frustum + Hi-Z occlusion culling
                    // (IndirectDispatchPass, then OcclusionCullPass).
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
                    // binding 5: coordinate_spaces (storage read, VERTEX) — current-frame
                    // per-space transforms (sublevels/portals), slot 0 = identity.
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
                    // binding 6: coordinate_spaces_prev (storage read, VERTEX) — same
                    // indexing, previous frame, for correct per-space motion vectors.
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
                    // binding 7: four joint indices + weights per vertex.
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
                    // binding 8: shared skinning palette as column-major mat4 rows.
                    wgpu::BindGroupLayoutEntry {
                        binding: 8,
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

        // ── Bind Group Layout 1: material + textures ──────────────────────────
        let bind_group_layout_1 = create_material_bgl(device, material_binding);
        let uv_grid_bind_group_layout = uv_grid_layout(device);
        let (uv_grid_texture, uv_grid_pixels, uv_grid_extent, uv_grid_bind_group) =
            create_uv_grid(device, &uv_grid_bind_group_layout, false);

        // ── Pipeline layout (shared by all pipeline variants) ─────────────────
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("GBuffer PL"),
            bind_group_layouts: &[
                Some(&bind_group_layout_0),
                Some(&bind_group_layout_1),
                Some(&uv_grid_bind_group_layout),
            ],
            immediate_size: 0,
        });

        // Create empty lightmap atlas regions buffer (populated when bake data is loaded).
        // Each region is 32 bytes: uv_offset(8) + uv_scale(8) + uv_clamp_min(8) + uv_clamp_max(8).
        let lightmap_atlas_regions_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Lightmap Atlas Regions"),
            size: 32, // One empty region sentinel; recreated when real data loads.
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let fallback_skeletal_storage = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Empty skeletal storage"),
            size: 64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut coordinate_space_words = vec![0.0f32; COORDINATE_SPACE_SLOTS * 16];
        coordinate_space_words[..16].copy_from_slice(&glam::Mat4::IDENTITY.to_cols_array());
        let coordinate_spaces = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Coordinate Spaces (space_id 0 = identity)"),
            contents: bytemuck::cast_slice(&coordinate_space_words),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        let coordinate_spaces_prev = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Coordinate Spaces Prev (space_id 0 = identity)"),
            contents: bytemuck::cast_slice(&coordinate_space_words),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        Self {
            draw_slots: DrawSlots::default(),
            timing_query: None,
            material_binding,
            pipelines: HashMap::new(),
            wireframe_supported: device
                .features()
                .contains(wgpu::Features::POLYGON_MODE_LINE),
            mirrored_view: false,
            shader_cache: RadiantShaderCache::new(),
            template_registry: None,
            last_template_keys: Vec::new(),
            pipeline_layout,
            bind_group_layout_0,
            bind_group_layout_1,
            uv_grid_texture,
            uv_grid_pixels,
            uv_grid_extent,
            uv_grid_bind_group_layout,
            uv_grid_bind_group,
            uv_grid_uploaded: false,
            uv_grid_full: false,
            bind_group_0: None,
            bind_group_0_key: None,
            bind_group_1: None,
            bind_group_1_version: None,
            globals_buf,
            // Default CSM splits — single source of truth is helio_light_api::CSM_SPLITS.
            csm_splits: helio_light_api::CSM_SPLITS,
            debug_mode: 0,
            lightmap_atlas_regions_buf,
            coordinate_spaces,
            coordinate_spaces_prev,
            fallback_skeletal_storage,
            stand_in: None,
            pending_coordinate_spaces: None,
            last_coordinate_words: None,
            coordinate_prev_stale: false,
        }
    }

    /// Portal/sublevel-local transform buffer (`space_id 0` = identity).
    /// Exposed so `Renderer::render` can publish `"coordinate_spaces"` into
    /// the frame's `ResourceRegistry` before the graph executes -- the same
    /// `find_pass`-based reach-in `Renderer` already uses for `PostProcessPass`.
    pub fn coordinate_spaces_buffer(&self) -> &wgpu::Buffer {
        &self.coordinate_spaces
    }

    /// Optional GPU timestamps around rasterizing the GBuffer.
    pub fn enable_timing(&mut self, device: &wgpu::Device) -> bool {
        if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            return false;
        }
        self.timing_query = Some(device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("GBuffer timings"),
            ty: wgpu::QueryType::Timestamp,
            count: 2,
        }));
        true
    }

    pub fn timing_query(&self) -> Option<&wgpu::QuerySet> {
        self.timing_query.as_ref()
    }

    /// See [`Self::coordinate_spaces_buffer`].
    pub fn coordinate_spaces_prev_buffer(&self) -> &wgpu::Buffer {
        &self.coordinate_spaces_prev
    }

    /// Upload the current portal/sublevel coordinate-space table. Slot zero is
    /// always world space; additional slots map authored target geometry into
    /// the source portal that displays it.
    pub fn set_coordinate_spaces(&self, queue: &wgpu::Queue, spaces: &[glam::Mat4]) {
        let words = coordinate_space_words(spaces);
        queue.write_buffer(&self.coordinate_spaces, 0, bytemuck::cast_slice(&words));
        queue.write_buffer(
            &self.coordinate_spaces_prev,
            0,
            bytemuck::cast_slice(&words),
        );
    }
}

impl RenderPass for GBufferPass {
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
        "GBuffer"
    }

    fn set_frame_inputs(&mut self, inputs: &helio_core::RenderFrameInputs<'_>) {
        self.pending_coordinate_spaces = Some(inputs.coordinate_spaces.to_vec());
    }

    fn publish_frame_inputs<'a>(
        &self,
        _camera: &helio_core::GpuCameraUniforms,
        frame: &mut helio_core::ResourceRegistry<'a>,
    ) {
        // SAFETY: these buffers are owned by the GBuffer pass, which is owned
        // by the RenderGraph for longer than the current frame registry.
        let coordinate_spaces: &'a wgpu::Buffer =
            unsafe { std::mem::transmute(&self.coordinate_spaces) };
        let coordinate_spaces_prev: &'a wgpu::Buffer =
            unsafe { std::mem::transmute(&self.coordinate_spaces_prev) };
        frame.write(
            helio_core::resource_keys::coordinate_spaces(),
            helio_core::CoordinateSpacesFrameData {
                coordinate_spaces,
                coordinate_spaces_prev,
            },
            "GBuffer",
        );
    }

    fn declare_resources(&self, builder: &mut ResourceBuilder) {
        builder.read("object_batch");
        builder.read("culled_batch");
        // The 4 bundled G-buffer targets consumed together downstream as one
        // `Tracked<GBufferViews>` (see `publish_group` below) — declared as a
        // single named group so the generic allocator combines them itself,
        // rather than as 4 independent `write_color_raw` calls. See
        // `docs/helio_3_0_spec.md` §5.
        builder.write_group(
            "gbuffer",
            [
                ("gbuffer_albedo", ResourceFormat::Rgba8Unorm),
                ("gbuffer_normal", ResourceFormat::Rgba16Float),
                ("gbuffer_orm", ResourceFormat::Rgba8Unorm),
                ("gbuffer_emissive", ResourceFormat::Rgba16Float),
            ],
            ResourceSize::MatchSurface,
        );
        builder.with_group_extra_usage("gbuffer", wgpu::TextureUsages::STORAGE_BINDING);
        // The remaining 4 G-buffer targets are each read independently
        // downstream (no bundling) — plain single-view declarations.
        builder.write_color_raw(
            "gbuffer_lightmap_uv",
            wgpu::TextureFormat::Rg16Float,
            ResourceSize::MatchSurface,
        );
        builder.write_color_raw(
            "gbuffer_sss",
            wgpu::TextureFormat::Rgba16Float,
            ResourceSize::MatchSurface,
        );
        // `gbuffer_sss`, `gbuffer_extra` and `gbuffer_velocity` are also written
        // by storage texture when virtual geometry shades its pixels
        // (`helio-pass-vg-shade`). Not `gbuffer_lightmap_uv`: Rg16Float is not a
        // storage format (the VG depth pass writes it as an attachment).
        builder.with_extra_usage(wgpu::TextureUsages::STORAGE_BINDING);
        builder.write_color_raw(
            "gbuffer_extra",
            wgpu::TextureFormat::Rgba16Float,
            ResourceSize::MatchSurface,
        );
        builder.with_extra_usage(wgpu::TextureUsages::STORAGE_BINDING);
        builder.write_color_raw(
            "gbuffer_velocity",
            // XY remain pixel motion. Z is the FP16 view-depth residual and W=2
            // marks a valid correction for precise ray-query receiver origins.
            wgpu::TextureFormat::Rgba16Float,
            ResourceSize::MatchSurface,
        );
        builder.with_extra_usage(wgpu::TextureUsages::STORAGE_BINDING);
    }

    fn publish<'a>(&self, _frame: &mut helio_core::ResourceRegistry<'a>) {}

    fn publish_group<'a>(
        &self,
        group_name: &'static str,
        views: &[&'a wgpu::TextureView],
        frame: &mut helio_core::ResourceRegistry<'a>,
    ) {
        // Turns the generically-resolved "gbuffer" write_group into the
        // stable bundled contract downstream passes (DeferredLight, SSAO,
        // SSR, VirtualGeometry, …) read as `frame.gbuffer`. Order matches
        // the `write_group` call in `declare_resources` above.
        if group_name == "gbuffer" {
            if let [albedo, normal, orm, emissive] = *views {
                frame.write(
                    helio_core::ResourceKey::new("gbuffer"),
                    helio_core::ViewGroup::<4> {
                        views: [albedo, normal, orm, emissive],
                    },
                    "GBufferPass",
                );
            }
        }
    }

    fn render_pass_descriptor_with_storage<'a>(
        &'a self,
        _target: &'a wgpu::TextureView,
        depth: &'a wgpu::TextureView,
        resources: &'a helio_core::ResourceRegistry<'a>,
        storage: &'a mut helio_core::RenderFrameStorage,
    ) -> Option<wgpu::RenderPassDescriptor<'a>> {
        let gbuffer = resources.read::<helio_core::ViewGroup<'_, 4>>(
            helio_core::ResourceKey::new("gbuffer"),
            "GBuffer",
        )?;
        let lightmap_uv = resources.read(
            helio_core::ResourceKey::new("gbuffer_lightmap_uv"),
            "GBuffer",
        )?;
        let sss_target = resources.read(helio_core::ResourceKey::new("gbuffer_sss"), "GBuffer")?;
        let extra_target =
            resources.read(helio_core::ResourceKey::new("gbuffer_extra"), "GBuffer")?;
        let velocity_target =
            resources.read(helio_core::ResourceKey::new("gbuffer_velocity"), "GBuffer")?;
        let color_attachments: &'a [Option<wgpu::RenderPassColorAttachment<'a>>] = storage
            .retain_boxed_slice(Box::new([
                Some(wgpu::RenderPassColorAttachment {
                    view: gbuffer.views[0],
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                }),
                Some(wgpu::RenderPassColorAttachment {
                    view: gbuffer.views[1],
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                }),
                Some(wgpu::RenderPassColorAttachment {
                    view: gbuffer.views[2],
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                }),
                Some(wgpu::RenderPassColorAttachment {
                    view: gbuffer.views[3],
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                }),
                Some(wgpu::RenderPassColorAttachment {
                    view: lightmap_uv,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                }),
                Some(wgpu::RenderPassColorAttachment {
                    view: sss_target,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                }),
                Some(wgpu::RenderPassColorAttachment {
                    view: extra_target,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                }),
                Some(wgpu::RenderPassColorAttachment {
                    view: velocity_target,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                }),
            ]));
        Some(wgpu::RenderPassDescriptor {
            label: Some("GBuffer"),
            color_attachments,
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(helio_core::depth::CLEAR),
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

    fn prepare(&mut self, ctx: &PrepareContext) -> HelioResult<()> {
        // The reference image exists only while its debug view is selected.
        let uv_view = self.debug_mode == UV_DEBUG_MODE;
        if uv_view != self.uv_grid_full {
            (
                self.uv_grid_texture,
                self.uv_grid_pixels,
                self.uv_grid_extent,
                self.uv_grid_bind_group,
            ) = create_uv_grid(ctx.device, &self.uv_grid_bind_group_layout, uv_view);
            self.uv_grid_full = uv_view;
            self.uv_grid_uploaded = false;
        }
        if !self.uv_grid_uploaded {
            let (width, height) = self.uv_grid_extent;
            ctx.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.uv_grid_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &self.uv_grid_pixels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(width * 4),
                    rows_per_image: Some(height),
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
            self.uv_grid_uploaded = true;
            self.uv_grid_pixels = Vec::new();
        }
        if let Some(pending_coordinate_spaces) = self.pending_coordinate_spaces.take() {
            let words = coordinate_space_words(&pending_coordinate_spaces);
            // The previous frame's table is what was uploaded before this one
            // (the first upload has no history: still frames).
            let previous = self.last_coordinate_words.as_deref().unwrap_or(&words);
            ctx.queue
                .write_buffer(&self.coordinate_spaces, 0, bytemuck::cast_slice(&words));
            ctx.write_buffer(
                &self.coordinate_spaces_prev,
                0,
                bytemuck::cast_slice(previous),
            );
            self.coordinate_prev_stale = previous != words.as_slice();
            self.last_coordinate_words = Some(words);
        } else if self.coordinate_prev_stale {
            // The table held still since the last upload: a space that stopped
            // moving has no motion, so the history catches up.
            if let Some(words) = self.last_coordinate_words.as_ref() {
                ctx.write_buffer(&self.coordinate_spaces_prev, 0, bytemuck::cast_slice(words));
            }
            self.coordinate_prev_stale = false;
        }
        // Read per-scene values from the frame registry so the GBuffer globals match
        // what the renderer configured (ambient light).
        let (ambient_color, ambient_intensity) = if let Some(ref environment) = ctx
            .registry
            .get::<helio_core::RenderEnvironment>(helio_core::resource_keys::render_environment())
            .as_ref()
        {
            (
                [
                    environment.ambient_color[0],
                    environment.ambient_color[1],
                    environment.ambient_color[2],
                    1.0,
                ],
                environment.ambient_intensity,
            )
        } else {
            // Fallback for headless / test usage without a full renderer.
            ([0.1, 0.1, 0.15, 1.0], 0.1)
        };

        // Upload per-frame globals (O(1) — fixed-size struct).
        let globals = GBufferGlobals {
            frame: ctx.frame_num as u32,
            delta_time: ctx.delta_time,
            light_count: ctx
                .scene_buffers
                .get(pulsar_scenedb::gpu::BufferKey::of(
                    helio_light_api::LIGHT_LIST,
                ))
                .map_or(0, |lights| lights.row_capacity()),
            ambient_intensity,
            ambient_color,
            csm_splits: self.csm_splits,
            debug_mode: self.debug_mode,
            screen_width: ctx.width as f32,
            screen_height: ctx.height as f32,
            time: ctx.time,
        };
        ctx.write_buffer(&self.globals_buf, 0, bytemuck::bytes_of(&globals));
        Ok(())
    }

    fn execute(&mut self, ctx: &mut PassContext) -> HelioResult<()> {
        let Some(batch) = ctx
            .registry
            .get::<crate::ObjectBatchFrameData<'_>>(helio_core::ResourceKey::new("object_batch"))
        else {
            return Ok(());
        };
        let Some(culled) = ctx
            .registry
            .get::<crate::CulledBatchFrameData<'_>>(helio_core::ResourceKey::new("culled_batch"))
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

        // Rebuild bind group 0 when camera or instances buffers change (GrowableBuffer realloc).
        let coord_spaces = ctx
            .registry
            .get::<helio_core::CoordinateSpacesFrameData<'_>>(
                helio_core::resource_keys::coordinate_spaces(),
            );
        let coordinate_spaces_buf = coord_spaces
            .map(|c| c.coordinate_spaces)
            .unwrap_or(ctx.camera);
        let coordinate_spaces_prev_buf = coord_spaces
            .map(|c| c.coordinate_spaces_prev)
            .unwrap_or(ctx.camera);
        let skin_handle = ctx
            .scene_buffers
            .get(BufferKey::of("skeletal_skin_vertices"));
        let palette_handle = ctx
            .scene_buffers
            .get(BufferKey::of("skeletal_bone_palette"));
        let skin_buf = skin_handle
            .map(|handle| &handle.buffer)
            .unwrap_or(&self.fallback_skeletal_storage);
        let palette_buf = palette_handle
            .map(|handle| &handle.buffer)
            .unwrap_or(&self.fallback_skeletal_storage);
        let key = [
            ctx.camera.clone(),
            batch.instances.clone(),
            culled.compacted_indices.clone(),
            coordinate_spaces_buf.clone(),
            coordinate_spaces_prev_buf.clone(),
            skin_buf.clone(),
            palette_buf.clone(),
        ];
        if self.bind_group_0_key.as_ref() != Some(&key) {
            log::debug!("GBuffer: rebuilding bind group 0 (buffers changed)");
            self.bind_group_0 = Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("GBuffer BG 0"),
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
                        resource: self.lightmap_atlas_regions_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: culled.compacted_indices.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: coordinate_spaces_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: coordinate_spaces_prev_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: skin_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 8,
                        resource: palette_buf.as_entire_binding(),
                    },
                ],
            }));
            self.bind_group_0_key = Some(key);
        }

        // Material rows are SceneDB component data.  The pass resolves the
        // component column by key; it must not consume a renderer-owned
        // material table hidden inside ResourceRegistry.
        let materials_handle = ctx.scene_buffers.get(BufferKey::of("materials"));
        let materials_buf = materials_handle
            .map(|handle| &handle.buffer)
            .unwrap_or(stand_in.rows());
        let materials_epoch = materials_handle.map(|handle| handle.epoch).unwrap_or(0);

        // Rebuild bind group 1 when material textures version changes.
        let needs_rebuild = self.bind_group_1_version
            != Some((material_textures.version, materials_epoch))
            || self.bind_group_1.is_none();
        if needs_rebuild {
            log::debug!("GBuffer: rebuilding bind group 1 (material textures version changed)");
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
                label: Some("GBuffer BG 1"),
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
                DrawSlots::len_of(culled.compacted_indices.size(), 4),
            )
            .clone();

        let mut pass = ctx.render_cmds().unwrap();
        pass.set_bind_group(0, self.bind_group_0.as_ref().unwrap(), &[]);
        pass.set_bind_group(1, self.bind_group_1.as_ref().unwrap(), &[]);
        pass.set_bind_group(2, &self.uv_grid_bind_group, &[]);
        pass.set_vertex_buffer(0, vertices.slice(..));
        pass.set_vertex_buffer(1, slots.slice(..));
        pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);

        // One draw per material segment, at its fixed offset with this
        // frame's GPU count (see `DrawSegments`).
        let segments = culled.segments;
        if segments.is_empty() {
            // No material key read back yet (the first frames): draw every
            // group with the default PBR pipeline.
            let key = RadiantShaderKey {
                template_id: 0,
                graph_hash: 0,
                feature_flags: 0,
            };
            let pipeline = self.get_or_create_pipeline(&ctx.device, key, "");
            pass.set_pipeline(pipeline);
            multi_draw_indexed_indirect(&mut pass, indirect, 0, max_draws, draw_count);
        } else {
            for (index, segment) in segments.in_bucket(helio_mesh_api::ShadingBucket::Opaque) {
                let key = RadiantShaderKey {
                    template_id: segment.template_class(),
                    graph_hash: segment.graph_hash,
                    feature_flags: segment.raster_flags(),
                };
                let pipeline = self.get_or_create_pipeline(&ctx.device, key, "");
                pass.set_pipeline(pipeline);
                segments.draw(&mut pass, index);
            }
        }
        Ok(())
    }

    fn set_debug_mode(&mut self, mode: u32) {
        // Wireframe changes the pipeline's polygon mode, so cached pipelines
        // must be rebuilt when entering or leaving it.
        if (mode == WIREFRAME_DEBUG_MODE) != (self.debug_mode == WIREFRAME_DEBUG_MODE) {
            self.pipelines.clear();
        }
        self.debug_mode = mode;
    }

    fn debug_views(&self) -> &'static [DebugViewDescriptor] {
        static WIREFRAME_VIEW: DebugViewDescriptor = DebugViewDescriptor {
            name: "Wireframe",
            debug_mode: WIREFRAME_DEBUG_MODE,
            description: "Rasterise geometry as unculled triangle edges",
        };
        static VIEWS_WITH_WIREFRAME: &[DebugViewDescriptor] = &[
            DebugViewDescriptor {
                name: "UV Visualisation",
                debug_mode: 1,
                description: "Sample the shared UV reference grid with UV0",
            },
            DebugViewDescriptor {
                name: "Raw Texture",
                debug_mode: 2,
                description: "Raw texture sample without material multiply",
            },
            DebugViewDescriptor {
                name: "Geometry Normals",
                debug_mode: 3,
                description: "Geometry normals only (skip normal mapping)",
            },
            WIREFRAME_VIEW,
        ];
        if self.wireframe_supported {
            return VIEWS_WITH_WIREFRAME;
        }
        static VIEWS: &[DebugViewDescriptor] = &[
            DebugViewDescriptor {
                name: "UV Visualisation",
                debug_mode: 1,
                description: "Sample the shared UV reference grid with UV0",
            },
            DebugViewDescriptor {
                name: "Raw Texture",
                debug_mode: 2,
                description: "Raw texture sample without material multiply",
            },
            DebugViewDescriptor {
                name: "Geometry Normals",
                debug_mode: 3,
                description: "Geometry normals only (skip normal mapping)",
            },
        ];
        VIEWS
    }

    fn reads(&self) -> &'static [&'static str] {
        &[
            helio_mats::MATERIAL_TEXTURES,
            "render_environment",
            "object_batch",
            "culled_batch",
        ]
    }
}

impl GBufferPass {
    /// Populate the lightmap atlas regions buffer from baked data.
    ///
    /// Called by the renderer after a successful bake to upload per-mesh UV atlas regions.
    /// Each region maps a mesh_id to its atlas location with precomputed clamp bounds that
    /// prevent bilinear filtering from bleeding across adjacent atlas region boundaries.
    ///
    /// # Arguments
    /// * `queue`   - GPU queue for buffer uploads
    /// * `regions` - `[uv_offset.x, uv_offset.y, uv_scale.x, uv_scale.y,
    ///                uv_clamp_min.x, uv_clamp_min.y, uv_clamp_max.x, uv_clamp_max.y]`
    ///              per mesh (32 bytes each, matching `LightmapAtlasRegion` in gbuffer.wgsl)
    pub fn upload_lightmap_atlas_regions(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        regions: &[[f32; 8]],
    ) {
        if regions.is_empty() {
            return;
        }

        // Each region is 32 bytes: uv_offset(8) + uv_scale(8) + uv_clamp_min(8) + uv_clamp_max(8).
        let buf_size = (regions.len() * 32) as u64;

        // Recreate buffer if size changed
        if self.lightmap_atlas_regions_buf.size() < buf_size {
            self.lightmap_atlas_regions_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Lightmap Atlas Regions"),
                size: buf_size.max(32),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });

            // Invalidate bind group since buffer changed
            self.bind_group_0_key = None;
        }

        queue.write_buffer(
            &self.lightmap_atlas_regions_buf,
            0,
            bytemuck::cast_slice(regions),
        );

        log::info!(
            "[GBufferPass] Uploaded {} lightmap atlas regions ({} bytes)",
            regions.len(),
            buf_size
        );
    }

    /// Get or create a render pipeline for the given key.
    /// Compiles the shader lazily on first access, injecting `graph_wgsl` when
    /// `graph_hash != 0`.
    fn get_or_create_pipeline(
        &mut self,
        device: &wgpu::Device,
        key: RadiantShaderKey,
        graph_wgsl: &str,
    ) -> &wgpu::RenderPipeline {
        if !self.pipelines.contains_key(&key) {
            let wireframe = self.wireframe_supported && self.debug_mode == WIREFRAME_DEBUG_MODE;
            let registered_graph;
            let graph_wgsl = if graph_wgsl.is_empty() && key.graph_hash != 0 {
                registered_graph = helio_mats::graph_source(key.graph_hash);
                registered_graph.as_deref().unwrap_or("")
            } else {
                graph_wgsl
            };
            // The renderer only ever hands us a shared registry when a
            // caller actually registers custom (from `FIRST_CUSTOM_MATERIAL_CLASS` up) templates; nothing
            // wires that up yet (see `template_registry`'s doc). Rather than
            // hard-require it for every draw -- panicking on the overwhelming
            // majority of scenes that only ever use the built-in classes
            // (0 = default_pbr .. 4 = skin) -- fall back to a local registry
            // that has those same built-ins, mirroring TransparentPass's
            // `local_class0` fallback for the identical situation.
            let local_fallback;
            let registry_guard;
            let template = if let Some(shared) = self.template_registry.as_ref() {
                registry_guard = shared.read().unwrap();
                match registry_guard.get(key.template_id) {
                    Some(t) => t,
                    None => {
                        log::warn!(
                            "Radiant: template class {} not found, falling back to class 0",
                            key.template_id,
                        );
                        registry_guard
                            .get(0)
                            .expect("Default PBR template (class 0) missing")
                    }
                }
            } else {
                local_fallback = helio_mats::radiant::RadiantTemplateRegistry::new();
                if key.template_id != 0 {
                    log::warn!(
                        "Radiant: no shared template registry synced yet; class {} unavailable, using class 0",
                        key.template_id,
                    );
                }
                local_fallback
                    .get(0)
                    .expect("Default PBR template (class 0) missing")
            };
            let module = self.shader_cache.get_or_compile(
                device,
                key,
                template,
                GBUFFER_PROGRAM,
                graph_wgsl,
                &[helio_mesh_api::DRAW_ROWS_SNIPPET],
                self.material_binding,
                "GBuffer Shader",
            );
            let pipeline = helio_core::shader::create_render_pipeline(
                &device,
                wgpu::RenderPipelineDescriptor {
                    label: Some("GBuffer Pipeline"),
                    layout: Some(&self.pipeline_layout),
                    vertex: wgpu::VertexState {
                        module,
                        entry_point: Some("vs_main"),
                        compilation_options: Default::default(),
                        // Full vertex layout (stride = 40 bytes, matching shared mesh buffer).
                        //   offset  0 — position       Float32x3  location 0
                        //   offset 12 — bitangent_sign Float32    location 1
                        //   offset 16 — tex_coords0   Float32x2  location 2  (UV0: material/albedo)
                        //   offset 24 — tex_coords1   Float32x2  location 5  (UV1: lightmap)
                        //   offset 32 — normal        Uint32     location 3
                        //   offset 36 — tangent       Uint32     location 4
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
                            Some(crate::DRAW_SLOT_LAYOUT),
                        ],
                    },
                    fragment: Some(wgpu::FragmentState {
                        module,
                        entry_point: Some("fs_main"),
                        compilation_options: Default::default(),
                        targets: &[
                            Some(wgpu::ColorTargetState {
                                format: wgpu::TextureFormat::Rgba8Unorm,
                                blend: None,
                                write_mask: wgpu::ColorWrites::ALL,
                            }),
                            Some(wgpu::ColorTargetState {
                                format: wgpu::TextureFormat::Rgba16Float,
                                blend: None,
                                write_mask: wgpu::ColorWrites::ALL,
                            }),
                            Some(wgpu::ColorTargetState {
                                format: wgpu::TextureFormat::Rgba8Unorm,
                                blend: None,
                                write_mask: wgpu::ColorWrites::ALL,
                            }),
                            Some(wgpu::ColorTargetState {
                                format: wgpu::TextureFormat::Rgba16Float,
                                blend: None,
                                write_mask: wgpu::ColorWrites::ALL,
                            }),
                            Some(wgpu::ColorTargetState {
                                format: wgpu::TextureFormat::Rg16Float,
                                blend: None,
                                write_mask: wgpu::ColorWrites::ALL,
                            }),
                            Some(wgpu::ColorTargetState {
                                format: wgpu::TextureFormat::Rgba16Float,
                                blend: None,
                                write_mask: wgpu::ColorWrites::ALL,
                            }),
                            Some(wgpu::ColorTargetState {
                                format: wgpu::TextureFormat::Rgba16Float,
                                blend: None,
                                write_mask: wgpu::ColorWrites::ALL,
                            }),
                            Some(wgpu::ColorTargetState {
                                format: wgpu::TextureFormat::Rgba16Float,
                                blend: None,
                                write_mask: wgpu::ColorWrites::ALL,
                            }),
                        ],
                    }),
                    primitive: wgpu::PrimitiveState {
                        topology: wgpu::PrimitiveTopology::TriangleList,
                        front_face: if self.mirrored_view {
                            wgpu::FrontFace::Cw
                        } else {
                            wgpu::FrontFace::Ccw
                        },
                        cull_mode: if wireframe
                            || key.feature_flags & helio_mats::FLAG_DOUBLE_SIDED != 0
                        {
                            None
                        } else {
                            Some(wgpu::Face::Back)
                        },
                        polygon_mode: if wireframe {
                            wgpu::PolygonMode::Line
                        } else {
                            wgpu::PolygonMode::Fill
                        },
                        ..Default::default()
                    },
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: wgpu::TextureFormat::Depth32Float,
                        depth_write_enabled: Some(true),
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

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Build the BGL for group 1 (bindless materials + textures).
// TODO: Generalize material BGL creation to avoid cross-pass deps.
// Both GBufferPass and TransparentPass need the same material bind group layout.
// This should be moved to a shared crate (helio-core or a shared pass crate) so neither
// pass depends on the other for basic infrastructure.
/// The debug mode that samples the UV reference image.
const UV_DEBUG_MODE: u32 = 1;

/// The UV reference image (`full`) or a white texel, its pixels (uploaded by
/// `prepare`), size and bind group.
fn create_uv_grid(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    full: bool,
) -> (wgpu::Texture, Vec<u8>, (u32, u32), wgpu::BindGroup) {
    const UV_GRID_BYTES: &[u8] = include_bytes!("../assets/UV_Grid_Sm.jpg");
    let (pixels, (width, height)) = if full {
        let decoded = image::load_from_memory(UV_GRID_BYTES)
            .expect("embedded UV reference image is valid")
            .to_rgba8();
        let extent = decoded.dimensions();
        (decoded.into_raw(), extent)
    } else {
        (vec![255; 4], (1, 1))
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("GBuffer UV reference image"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("GBuffer UV reference sampler"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("GBuffer UV reference bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
        ],
    });
    (texture, pixels, (width, height), bind_group)
}

fn uv_grid_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("GBuffer UV reference layout"),
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
    })
}

pub fn create_material_bgl(
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
        label: Some("GBuffer BGL 1"),
        entries: &entries,
    })
}
