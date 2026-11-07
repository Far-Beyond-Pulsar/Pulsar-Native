//! Virtual geometry deferred shading (plan §5.0, §5.5).
//!
//! Turns the visibility results of the raster tiers into the classic G-buffer:
//!
//! 1. `vis_resolve` (compute): merges the hardware R32Uint vis target + depth
//!    with the software / dense 64-bit-key buffers (nearest wins, reversed-Z)
//!    and the optional ray-traced payload, depth-tests the winner against the
//!    classic scene depth, recomputes **exact perspective-correct
//!    barycentrics** from the decoded triangle and the pixel, and writes the
//!    unified payload `RG32Uint` (`R` = vis id, `G` = `bary_pack(u, v)`) plus
//!    the resolved depth `R32Float`.
//! 2. Material binning (compute): per-class pixel counts, prefix sum,
//!    per-class pixel lists and indirect dispatch arguments.
//! 3. `vg_shade` (compute, one indirect dispatch per class): interpolates the
//!    attribute stream with analytic UV derivatives and evaluates the material
//!    through **the classic G-buffer's own evaluator** (its WGSL is spliced
//!    out of `helio-pass-gbuffer/shaders/gbuffer.wgsl` at build time, template
//!    overrides included), writing all eight G-buffer targets by storage
//!    texture, velocity included.
//! 4. Final depth (render pass): a fullscreen triangle writes `Depth32Float`
//!    with `frag_depth`, depth-tested `>=` against the classic depth.
//!
//! The pass is driven through [`VgShade::encode`] with explicit resources
//! rather than the graph registry: graph wiring is `helio-presets`'
//! job (T10). See [`VgShadeInputs`] for the contract with the cull/raster
//! crates and `helio_vg_format::DECODE_WGSL` for the vertex decode.

#![allow(clippy::disallowed_methods)]

pub mod cpu_ref;
mod debug;
pub mod graph;
pub use debug::{debug_source, VgShadeDebug, DEBUG_WGSL};
pub use graph::VgShadePass;
pub use helio_vg_format::debug_view as debug_views;

use bytemuck::{Pod, Zeroable};
use std::collections::HashMap;

/// Material classes are binned into this many bins; the last bin takes every
/// class `>= BIN_COUNT - 1`. Each bin runs its own pipeline (the template for
/// that class).
pub const BIN_COUNT: u32 = 16;
/// Words per `static_objects` row (252 bytes).
pub const OBJECT_ROW_WORDS: u32 = 63;
/// Workgroups per dispatch row of the shade dispatches (`vg_bin.wgsl`).
pub const SHADE_ROW_GROUPS: u32 = 32768;
/// Marks "no VG pixel" in the vis payload and per-pixel material rows.
pub const INVALID: u32 = u32::MAX;

pub const RESOLVE_WGSL: &str = include_str!("../shaders/vg_resolve.wgsl");
pub const BIN_WGSL: &str = include_str!("../shaders/vg_bin.wgsl");
pub const SHADE_WGSL: &str = include_str!("../shaders/vg_shade.wgsl");
pub const DEPTH_WGSL: &str = include_str!("../shaders/vg_depth.wgsl");
pub const COMMON_WGSL: &str = include_str!("../shaders/vg_common.wgsl");
/// The shared vertex decode (positions, indices, attributes): `helio_vg_format::DECODE_WGSL`
/// followed by `DECODE_ATTR_WGSL`. The shade shaders declare `vg_page_pool` and
/// `vg_asset_quant` themselves.
pub fn shared_decode() -> String {
    format!(
        "{}
{}",
        helio_vg_format::DECODE_WGSL,
        helio_vg_format::DECODE_ATTR_WGSL
    )
}

/// The meshes' G-buffer program, whose `Globals` and motion vectors
/// (`compute_velocity`) the shade pass shares (the camera is the prelude's).
const GBUFFER_WGSL: &str = helio_pass_gbuffer::GBUFFER_PROGRAM;

/// One visible cluster, produced by the cull pass: `{instance, cluster_slot}`.
/// A vis id's cluster index (`helio_vg_format::vis_cluster`) indexes the array
/// of these (the cull's unified `bin * bin_capacity + k` index).
pub type VgVisibleCluster = helio_vg_format::GpuVisibleCluster;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct VgShadeParams {
    width: u32,
    height: u32,
    flags: u32,
    bin_count: u32,
    pixel_count: u32,
    _pad: [u32; 3],
}

const FLAG_SW: u32 = 1;
const FLAG_DENSE: u32 = 2;
const FLAG_DENSE_MASK: u32 = 8;
const FLAG_RT: u32 = 4;
const FLAG_DENSE_MERGE: u32 = 16;

/// The per-frame `Globals` uniform the classic G-buffer shader (and so every
/// material template) reads. Same layout as `helio_pass_gbuffer::GBufferGlobals`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct VgShadeGlobals {
    pub frame: u32,
    pub delta_time: f32,
    pub light_count: u32,
    pub ambient_intensity: f32,
    pub ambient_color: [f32; 4],
    pub csm_splits: [f32; 4],
    pub debug_mode: u32,
    pub screen_width: f32,
    pub screen_height: f32,
    pub time: f32,
}

// ── Shader sources ──────────────────────────────────────────────────────────

/// Shader assembly. `decode` is the vertex-decode include (the stub, or T6's
/// `vg_decode.wgsl` once it exists).
pub mod shader_source {
    use super::*;

    fn between<'a>(text: &'a str, from: &str, to: &str) -> &'a str {
        let start = text
            .find(from)
            .unwrap_or_else(|| panic!("gbuffer.wgsl no longer contains `{from}`"));
        let end = start
            + text[start..]
                .find(to)
                .unwrap_or_else(|| panic!("gbuffer.wgsl no longer contains `{to}` after `{from}`"));
        &text[start..end]
    }

    /// `vis_resolve` module.
    pub fn resolve(decode: &str) -> String {
        format!(
            "{}\n{COMMON_WGSL}\n{decode}\n{RESOLVE_WGSL}",
            helio_vg_format::WGSL
        )
    }

    /// Material binning module (`bin_count`, `bin_scan`, `bin_scatter`).
    pub fn bin() -> String {
        // The material row's words (the materials' snippet).
        format!("{}\n{BIN_WGSL}", helio_mats::MATERIAL_ROW_SNIPPET.source)
    }

    /// Final depth module (`vs_main`, `fs_main`).
    pub fn depth() -> String {
        DEPTH_WGSL.to_string()
    }

    /// The shade module as a drawer program: the G-buffer program's view
    /// declarations, `vg_shade.wgsl` and the G-buffer's `compute_velocity`,
    /// over the materials' snippets (the row, `material_surface`, specular
    /// anti-aliasing). A material template's surface function is composed
    /// after it (`RadiantTemplate::compose`), as into the G-buffer's;
    /// `textureSample` is still the materials' implicit call, see
    /// [`gradient_sampling`].
    pub fn shade_program(decode: &str) -> String {
        let gbuffer = GBUFFER_WGSL.replace("\r\n", "\n");
        // Globals (the camera is the prelude's).
        let declarations = between(&gbuffer, "struct Globals {", "struct SkinVertex {");
        let velocity = between(&gbuffer, "fn compute_velocity", "@fragment");
        format!(
            "enable wgpu_binding_array;\n//!use helio_prelude\n{}\n//!use pbr_eval\n//!use material_row\n//!use material_surface\n{}\n{COMMON_WGSL}\n{decode}\n{declarations}\n{SHADE_WGSL}\n{velocity}",
            helio_mats::SPECULAR_AA_MARKER,
            helio_vg_format::WGSL
        )
    }

    /// Compute shaders have no implicit derivatives: rewrite the evaluator's
    /// texture sampling to `textureSampleGrad` with the analytic UV
    /// derivatives (`vg_duv_dx/dy`), and specular anti-aliasing's normal
    /// derivatives to the analytic ones (`vg_dn_dx/dy`). Handles both
    /// material binding modes (the expanded one must already have had its
    /// WebGPU fixups applied).
    pub fn gradient_sampling(src: &str, max_textures: usize, bindless: bool) -> String {
        const GRAD: &str = "uv, vg_slot_grad(slot, vg_duv_dx), vg_slot_grad(slot, vg_duv_dy)";
        let src = &src
            .replace("dpdx(n)", "vg_dn_dx")
            .replace("dpdy(n)", "vg_dn_dy");
        if bindless {
            return src.replace(
                "textureSample(scene_textures[slot.texture_index], scene_samplers[slot.texture_index], uv)",
                &format!(
                    "textureSampleGrad(scene_textures[slot.texture_index], scene_samplers[slot.texture_index], {GRAD})"
                ),
            );
        }
        let mut out = src.to_string();
        for i in 0..max_textures {
            out = out.replace(
                &format!("textureSampleLevel(scene_texture_{i}, scene_sampler_{i}, uv, 0.0)"),
                &format!("textureSampleGrad(scene_texture_{i}, scene_sampler_{i}, {GRAD})"),
            );
        }
        out
    }

    /// The final shade module for one material template.
    pub fn shade(
        template: &helio_mats::RadiantTemplate,
        binding: helio_mats::MaterialBindingConfig,
        decode: &str,
    ) -> String {
        let composed = template.compose(&shade_program(decode), "", binding.max_textures, &[]);
        let bindless = binding.uses_binding_arrays();
        let composed = if bindless {
            composed
        } else {
            helio_mats::RadiantTemplate::apply_webgpu_fixups(&composed, binding.max_textures)
        };
        gradient_sampling(&composed, binding.max_textures, bindless)
    }
}

// ── Pass ────────────────────────────────────────────────────────────────────

/// Frame inputs of the shade pass.
///
/// Contracts with the neighbouring passes (all reversed-Z, full resolution):
///
/// - `hw_vis`: `R32Uint`, `vis_pack(visible_cluster, tri)` per pixel,
///   [`INVALID`] where the HW raster drew nothing. `hw_depth`: the depth it
///   was tested against (`Depth32Float`). May be the same texture as
///   `scene_depth` (a shared depth buffer): equal depth keeps VG.
/// - `sw_keys` / `dense_keys`: one 64-bit key per pixel,
///   `raster_key(depth, id)` = `depth_bits << 32 | id`, i.e. little-endian
///   words `[id, depth_bits]`; a zero depth word is "empty".
/// - `rt`: ray-traced payload (`Rg32Uint`: id, `bary_pack`) and its depth
///   (`R32Float`); optional.
/// - `visible`: [`VgVisibleCluster`] rows (the cull's `visible_clusters`); `asset_quant`: `vg_asset_quant`;
///   `instances`: `GpuVgInstance` rows; `pages`: the page pool as `u32`s.
/// - `static_objects`: the scene join's 252-byte rows. A cluster's material
///   section index (`GpuCluster::material_slot`) selects row
///   `GpuVgInstance::object_row + section`: the join emits an instance's
///   sections as consecutive rows sharing one transform.
pub struct VgShadeInputs<'a> {
    pub camera: &'a wgpu::Buffer,
    pub static_objects: &'a wgpu::Buffer,
    pub coordinate_spaces: &'a wgpu::Buffer,
    pub coordinate_spaces_prev: &'a wgpu::Buffer,
    pub materials: &'a wgpu::Buffer,
    pub material_textures: helio_mats::MaterialTextureBindings<'a>,
    pub visible: &'a wgpu::Buffer,
    /// `vg_asset_quant` (`array<vec4<f32>>`) by asset slot.
    pub asset_quant: &'a wgpu::Buffer,
    pub instances: &'a wgpu::Buffer,
    pub pages: &'a wgpu::Buffer,
    pub hw_vis: &'a wgpu::TextureView,
    pub hw_depth: &'a wgpu::TextureView,
    pub scene_depth: &'a wgpu::TextureView,
    pub sw_keys: Option<&'a wgpu::Buffer>,
    pub dense_keys: Option<&'a wgpu::Buffer>,
    pub rt: Option<(&'a wgpu::TextureView, &'a wgpu::TextureView)>,
    /// Visible rows of the ray-traced backend (ids `>= DENSE_RT_VISIBLE_BASE`
    /// index it, not the cull's list). `None` binds an empty buffer.
    pub rt_visible: Option<&'a wgpu::Buffer>,
    /// The dense-tile mask `dense_keys` is valid under (one `u32` per 16x16 tile,
    /// nonzero = resolved by Tier 3). With it, a dense tile takes only the dense
    /// result and other tiles ignore `dense_keys`; `None` merges per pixel.
    pub dense_mask: Option<&'a wgpu::Buffer>,
    /// `dense_keys` holds only the clusters Tier 3 was given: inside dense tiles take
    /// the nearest of it and the Tier 1/2 pixel (instead of replacing the latter).
    pub dense_merge: bool,
    /// A virtual-geometry debug view to overlay on the G-buffer albedo after
    /// shading (see [`debug_views`]); `None` records nothing extra.
    pub debug: Option<VgShadeDebug<'a>>,
}

/// The G-buffer targets written, plus the depth buffer. All eight colour
/// targets need `STORAGE_BINDING` (see the crate report for what the classic
/// pass declares today).
pub struct VgShadeTargets<'a> {
    pub albedo: &'a wgpu::TextureView,      // Rgba8Unorm
    pub normal: &'a wgpu::TextureView,      // Rgba16Float
    pub orm: &'a wgpu::TextureView,         // Rgba8Unorm
    pub emissive: &'a wgpu::TextureView,    // Rgba16Float
    pub lightmap_uv: &'a wgpu::TextureView, // Rg16Float, RENDER_ATTACHMENT only (written by the depth pass)
    pub sss: &'a wgpu::TextureView,         // Rgba16Float
    pub extra: &'a wgpu::TextureView,       // Rgba16Float
    pub velocity: &'a wgpu::TextureView,    // Rgba16Float
    /// Depth32Float attachment the final depth pass writes (the classic depth).
    pub depth: &'a wgpu::TextureView,
}

struct Size {
    width: u32,
    height: u32,
}

/// The pass. Create once per device; call [`Self::resize`] when the target
/// size changes (the first `encode` does it too).
pub struct VgShade {
    binding: helio_mats::MaterialBindingConfig,
    decode: String,
    templates: Option<helio_mats::SharedTemplateRegistry>,
    bin_mask: Option<u32>,

    resolve_bgl0: wgpu::BindGroupLayout,
    resolve_bgl1: wgpu::BindGroupLayout,
    resolve_pipeline: wgpu::ComputePipeline,
    bin_bgl: wgpu::BindGroupLayout,
    bin_count_pipeline: wgpu::ComputePipeline,
    bin_scan_pipeline: wgpu::ComputePipeline,
    bin_scatter_pipeline: wgpu::ComputePipeline,
    shade_bgl0: wgpu::BindGroupLayout,
    shade_bgl1: wgpu::BindGroupLayout,
    shade_bgl2: wgpu::BindGroupLayout,
    shade_layout: wgpu::PipelineLayout,
    shade_modules: HashMap<u32, wgpu::ShaderModule>,
    shade_pipelines: Vec<Option<wgpu::ComputePipeline>>,
    depth_bgl: wgpu::BindGroupLayout,
    depth_pipeline: wgpu::RenderPipeline,
    /// Created on the first frame a debug view is selected.
    debug: Option<debug::DebugPipeline>,

    params: wgpu::Buffer,
    globals: wgpu::Buffer,
    bins: wgpu::Buffer,
    dispatch_args: wgpu::Buffer,
    dummy_keys: wgpu::Buffer,
    dummy_rt_payload: wgpu::TextureView,
    dummy_rt_depth: wgpu::TextureView,

    size: Option<Size>,
    pixel_material: Option<wgpu::Buffer>,
    pixel_list: Option<wgpu::Buffer>,
    payload_view: Option<wgpu::TextureView>,
    depth_view: Option<wgpu::TextureView>,
    payload_tex: Option<wgpu::Texture>,
    depth_tex: Option<wgpu::Texture>,
    /// What produced each pixel (`R32Uint`, `VGS_SRC_*`): written by `vis_resolve`
    /// from the candidate that won, read by the debug views.
    source_view: Option<wgpu::TextureView>,
    source_tex: Option<wgpu::Texture>,
}

fn buf(b: &wgpu::Buffer) -> wgpu::BindingResource<'_> {
    b.as_entire_binding()
}

fn storage_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn texture_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
    sample_type: wgpu::TextureSampleType,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Texture {
            sample_type,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn storage_texture_entry(binding: u32, format: wgpu::TextureFormat) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format,
            view_dimension: wgpu::TextureViewDimension::D2,
        },
        count: None,
    }
}

const UINT: wgpu::TextureSampleType = wgpu::TextureSampleType::Uint;
const UNFILTERABLE: wgpu::TextureSampleType = wgpu::TextureSampleType::Float { filterable: false };
const DEPTH: wgpu::TextureSampleType = wgpu::TextureSampleType::Depth;
const CS: wgpu::ShaderStages = wgpu::ShaderStages::COMPUTE;

impl VgShade {
    pub fn new(device: &wgpu::Device) -> Self {
        Self::with_decode(device, &shared_decode())
    }

    /// As [`Self::new`] with an explicit vertex-decode include (default:
    /// [`shared_decode`]).
    pub fn with_decode(device: &wgpu::Device, decode: &str) -> Self {
        let binding = helio_mats::MaterialBindingConfig::for_device(device);
        let module = |label: &str, source: String| {
            helio_core::shader::module(device, label, source.as_str())
        };

        // ── resolve ──
        let mut e0 = vec![storage_entry(0, true), uniform_entry(1)];
        e0.extend((2..=9).map(|b| storage_entry(b, true)));
        e0.push(storage_entry(10, false));
        e0.push(storage_entry(11, true));
        e0.push(storage_entry(12, true));
        let resolve_bgl0 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("VgShade resolve 0"),
            entries: &e0,
        });
        let resolve_bgl1 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("VgShade resolve 1"),
            entries: &[
                texture_entry(0, CS, UINT),
                texture_entry(1, CS, DEPTH),
                texture_entry(2, CS, DEPTH),
                texture_entry(3, CS, UINT),
                texture_entry(4, CS, UNFILTERABLE),
                storage_texture_entry(5, wgpu::TextureFormat::Rg32Uint),
                storage_texture_entry(6, wgpu::TextureFormat::R32Float),
                storage_texture_entry(7, wgpu::TextureFormat::R32Uint),
            ],
        });
        let resolve_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("VgShade resolve"),
            bind_group_layouts: &[Some(&resolve_bgl0), Some(&resolve_bgl1)],
            immediate_size: 0,
        });
        let resolve_module = module("VgShade resolve", shader_source::resolve(decode));
        let compute = |label: &str,
                       layout: &wgpu::PipelineLayout,
                       module: &wgpu::ShaderModule,
                       entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let resolve_pipeline = compute(
            "VgShade vis_resolve",
            &resolve_layout,
            &resolve_module,
            "vis_resolve",
        );

        // ── binning ──
        let bin_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("VgShade bin"),
            entries: &[
                uniform_entry(0),
                storage_entry(1, true),
                storage_entry(2, true),
                storage_entry(3, false),
                storage_entry(4, false),
                storage_entry(5, false),
            ],
        });
        let bin_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("VgShade bin"),
            bind_group_layouts: &[Some(&bin_bgl)],
            immediate_size: 0,
        });
        let bin_module = module("VgShade bin", shader_source::bin());
        let bin_count_pipeline =
            compute("VgShade bin_count", &bin_layout, &bin_module, "bin_count");
        let bin_scan_pipeline = compute("VgShade bin_scan", &bin_layout, &bin_module, "bin_scan");
        let bin_scatter_pipeline = compute(
            "VgShade bin_scatter",
            &bin_layout,
            &bin_module,
            "bin_scatter",
        );

        // ── shade ──
        let mut s0 = vec![storage_entry(0, true), uniform_entry(1)];
        s0.extend((2..=8).map(|b| storage_entry(b, true)));
        s0.push(uniform_entry(9));
        s0.push(storage_entry(10, true));
        s0.push(storage_entry(11, true));
        s0.push(storage_entry(12, true));
        let shade_bgl0 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("VgShade shade 0"),
            entries: &s0,
        });
        let mut s1 = vec![storage_entry(0, true), storage_entry(1, true)];
        binding.append_layout_entries(&mut s1, 2, CS);
        let shade_bgl1 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("VgShade shade 1 (materials)"),
            entries: &s1,
        });
        use helio_frame_api::gbuffer as G;
        let shade_bgl2 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("VgShade shade 2 (payload, targets)"),
            entries: &[
                texture_entry(0, CS, UINT),
                texture_entry(1, CS, UNFILTERABLE),
                // The frame's G-buffer targets, written as storage.
                storage_texture_entry(2, G::ALBEDO_FORMAT.to_wgpu()),
                storage_texture_entry(3, G::NORMAL_FORMAT.to_wgpu()),
                storage_texture_entry(4, G::ORM_FORMAT.to_wgpu()),
                storage_texture_entry(5, G::EMISSIVE_FORMAT.to_wgpu()),
                storage_texture_entry(6, G::SSS_FORMAT.to_wgpu()),
                storage_texture_entry(7, G::EXTRA_FORMAT.to_wgpu()),
                storage_texture_entry(8, G::VELOCITY_FORMAT.to_wgpu()),
            ],
        });
        let shade_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("VgShade shade"),
            bind_group_layouts: &[Some(&shade_bgl0), Some(&shade_bgl1), Some(&shade_bgl2)],
            immediate_size: 0,
        });

        // ── final depth ──
        let depth_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("VgShade depth"),
            entries: &[texture_entry(0, wgpu::ShaderStages::FRAGMENT, UNFILTERABLE)],
        });
        let depth_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("VgShade depth"),
            bind_group_layouts: &[Some(&depth_bgl)],
            immediate_size: 0,
        });
        let depth_module = module("VgShade depth", shader_source::depth());
        let depth_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("VgShade depth"),
            layout: Some(&depth_layout),
            vertex: wgpu::VertexState {
                module: &depth_module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &depth_module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: helio_frame_api::gbuffer::LIGHTMAP_UV_FORMAT.to_wgpu(),
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
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
        });

        // ── small fixed buffers / dummies ──
        let buffer = |label: &str, size: u64, usage: wgpu::BufferUsages| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let storage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;
        let params = buffer(
            "VgShade params",
            std::mem::size_of::<VgShadeParams>() as u64,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let globals = buffer(
            "VgShade globals",
            std::mem::size_of::<VgShadeGlobals>() as u64,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let bins = buffer(
            "VgShade bins",
            u64::from(BIN_COUNT) * 16,
            storage | wgpu::BufferUsages::COPY_SRC,
        );
        let dispatch_args = buffer(
            "VgShade dispatch args",
            u64::from(BIN_COUNT) * 16,
            storage | wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_SRC,
        );
        let dummy_keys = buffer("VgShade empty keys", 8, storage);
        let dummy = |label: &str, format: wgpu::TextureFormat| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };

        Self {
            binding,
            decode: decode.to_string(),
            templates: None,
            bin_mask: None,
            resolve_bgl0,
            resolve_bgl1,
            resolve_pipeline,
            bin_bgl,
            bin_count_pipeline,
            bin_scan_pipeline,
            bin_scatter_pipeline,
            shade_bgl0,
            shade_bgl1,
            shade_bgl2,
            shade_layout,
            shade_modules: HashMap::new(),
            shade_pipelines: (0..BIN_COUNT).map(|_| None).collect(),
            depth_bgl,
            depth_pipeline,
            params,
            globals,
            bins,
            dispatch_args,
            dummy_keys,
            dummy_rt_payload: dummy("VgShade no rt payload", wgpu::TextureFormat::Rg32Uint),
            dummy_rt_depth: dummy("VgShade no rt depth", wgpu::TextureFormat::R32Float),
            debug: None,
            size: None,
            pixel_material: None,
            pixel_list: None,
            payload_view: None,
            depth_view: None,
            source_view: None,
            source_tex: None,
            payload_tex: None,
            depth_tex: None,
        }
    }

    /// Share the renderer's material template registry (custom classes).
    /// Without one, the built-in templates are used.
    pub fn set_template_registry(&mut self, registry: helio_mats::SharedTemplateRegistry) {
        self.templates = Some(registry);
        self.shade_modules.clear();
        self.shade_pipelines.iter_mut().for_each(|p| *p = None);
    }

    /// Restrict shading to the material-class bins in `mask` (bit `b` = bin
    /// `b`), so only classes that exist in the scene compile a pipeline. The
    /// default is every bin with a registered template.
    pub fn set_bin_mask(&mut self, mask: u32) {
        self.bin_mask = Some(mask);
    }

    /// The unified payload (`Rg32Uint`: id, `bary_pack`) of the last encode.
    pub fn payload_view(&self) -> Option<&wgpu::TextureView> {
        self.payload_view.as_ref()
    }
    /// What produced each pixel of the last encode (`R32Uint`, `VGS_SRC_*`; see
    /// `helio_vg_format::debug_view::pixel_source`).
    pub fn source_texture(&self) -> Option<&wgpu::Texture> {
        self.source_tex.as_ref()
    }

    /// The unified payload (`Rg32Uint`: id, `bary_pack`) of the last encode.
    pub fn payload_texture(&self) -> Option<&wgpu::Texture> {
        self.payload_tex.as_ref()
    }
    /// The resolved depth (`R32Float`, reversed-Z, 0 = no VG pixel).
    pub fn resolved_depth_view(&self) -> Option<&wgpu::TextureView> {
        self.depth_view.as_ref()
    }
    pub fn resolved_depth_texture(&self) -> Option<&wgpu::Texture> {
        self.depth_tex.as_ref()
    }
    /// Per-bin `[offset, count, cursor, _]`, for tools and tests.
    pub fn bins_buffer(&self) -> &wgpu::Buffer {
        &self.bins
    }
    /// Per-bin indirect dispatch arguments.
    pub fn dispatch_args_buffer(&self) -> &wgpu::Buffer {
        &self.dispatch_args
    }
    pub fn pixel_list_buffer(&self) -> Option<&wgpu::Buffer> {
        self.pixel_list.as_ref()
    }

    /// (Re)create the size-dependent resources.
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if self
            .size
            .as_ref()
            .is_some_and(|s| s.width == width && s.height == height)
        {
            return;
        }
        let pixels = u64::from(width.max(1)) * u64::from(height.max(1));
        let buf = |label: &str| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: pixels * 4,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        };
        let tex = |label: &str, format: wgpu::TextureFormat| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: width.max(1),
                    height: height.max(1),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::STORAGE_BINDING
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        };
        self.pixel_material = Some(buf("VgShade pixel material"));
        self.pixel_list = Some(buf("VgShade pixel list"));
        let payload = tex("VgShade payload", wgpu::TextureFormat::Rg32Uint);
        let depth = tex("VgShade resolved depth", wgpu::TextureFormat::R32Float);
        let source = tex("VgShade pixel source", wgpu::TextureFormat::R32Uint);
        self.payload_view = Some(payload.create_view(&Default::default()));
        self.depth_view = Some(depth.create_view(&Default::default()));
        self.source_view = Some(source.create_view(&Default::default()));
        self.payload_tex = Some(payload);
        self.depth_tex = Some(depth);
        self.source_tex = Some(source);
        self.size = Some(Size { width, height });
    }

    fn registry_class_bins(&self) -> u32 {
        match &self.templates {
            Some(shared) => {
                let reg = shared.read().unwrap();
                reg.keys()
                    .into_iter()
                    .filter(|k| *k < BIN_COUNT)
                    .fold(0u32, |m, k| m | 1 << k)
            }
            None => helio_mats::RadiantTemplateRegistry::new()
                .keys()
                .into_iter()
                .filter(|k| *k < BIN_COUNT)
                .fold(0u32, |m, k| m | 1 << k),
        }
    }

    fn ensure_shade_pipelines(&mut self, device: &wgpu::Device) {
        let mask = self.bin_mask.unwrap_or_else(|| self.registry_class_bins()) | 1;
        for bin in 0..BIN_COUNT {
            if mask & (1 << bin) == 0 || self.shade_pipelines[bin as usize].is_some() {
                continue;
            }
            // A bin without a registered template shades as class 0.
            let class = match &self.templates {
                Some(shared) if shared.read().unwrap().get(bin).is_some() => bin,
                None if helio_mats::RadiantTemplateRegistry::new()
                    .get(bin)
                    .is_some() =>
                {
                    bin
                }
                _ => 0,
            };
            if !self.shade_modules.contains_key(&class) {
                let source = match &self.templates {
                    Some(shared) => {
                        let reg = shared.read().unwrap();
                        let template = reg.get(class).expect("class checked above");
                        shader_source::shade(template, self.binding, &self.decode)
                    }
                    None => {
                        let reg = helio_mats::RadiantTemplateRegistry::new();
                        let template = reg.get(class).expect("built-in class");
                        shader_source::shade(template, self.binding, &self.decode)
                    }
                };
                let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("VgShade shade"),
                    source: wgpu::ShaderSource::Wgsl(source.into()),
                });
                self.shade_modules.insert(class, module);
            }
            let module = &self.shade_modules[&class];
            self.shade_pipelines[bin as usize] = Some(device.create_compute_pipeline(
                &wgpu::ComputePipelineDescriptor {
                    label: Some("VgShade vg_shade"),
                    layout: Some(&self.shade_layout),
                    module,
                    entry_point: Some("vg_shade"),
                    compilation_options: wgpu::PipelineCompilationOptions {
                        constants: &[("BIN", f64::from(bin))],
                        ..Default::default()
                    },
                    cache: None,
                },
            ));
        }
    }

    /// Record resolve, binning, shading and the final depth pass.
    ///
    /// Order in the frame: the classic G-buffer pass first (it clears its
    /// targets), then this. Shading overwrites G-buffer pixels only where VG
    /// wins the depth test, so the pass composes with classic geometry.
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        width: u32,
        height: u32,
        inputs: &VgShadeInputs<'_>,
        targets: &VgShadeTargets<'_>,
        globals: &VgShadeGlobals,
    ) {
        let mut rec = helio_core::CommandRecorder::from_encoder(encoder);
        self.encode_rec(
            device, queue, &mut rec, width, height, inputs, targets, globals,
        );
    }

    /// As [`Self::encode`], into a render-graph command stream.
    pub fn encode_rec(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut helio_core::CommandRecorder<'_>,
        width: u32,
        height: u32,
        inputs: &VgShadeInputs<'_>,
        targets: &VgShadeTargets<'_>,
        globals: &VgShadeGlobals,
    ) {
        self.resize(device, width, height);
        self.ensure_shade_pipelines(device);
        if inputs.debug.is_some() {
            self.ensure_debug(device);
        }
        let pixels = width * height;

        let mut flags = 0;
        if inputs.sw_keys.is_some() {
            flags |= FLAG_SW;
        }
        if inputs.dense_keys.is_some() {
            flags |= FLAG_DENSE;
        }
        if inputs.rt.is_some() {
            flags |= FLAG_RT;
        }
        if inputs.dense_keys.is_some() && inputs.dense_mask.is_some() {
            flags |= FLAG_DENSE_MASK;
            if inputs.dense_merge {
                flags |= FLAG_DENSE_MERGE;
            }
        }
        queue.write_buffer(
            &self.params,
            0,
            bytemuck::bytes_of(&VgShadeParams {
                width,
                height,
                flags,
                bin_count: BIN_COUNT,
                pixel_count: pixels,
                _pad: [0; 3],
            }),
        );
        queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(globals));
        encoder.clear_buffer(&self.bins, 0, None);

        let pixel_material = self.pixel_material.as_ref().unwrap();
        let pixel_list = self.pixel_list.as_ref().unwrap();
        let payload_view = self.payload_view.as_ref().unwrap();
        let depth_view = self.depth_view.as_ref().unwrap();
        let source_view = self.source_view.as_ref().unwrap();
        let res = wgpu::BindingResource::TextureView;

        // 1. vis_resolve
        let resolve0 = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("VgShade resolve 0"),
            layout: &self.resolve_bgl0,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buf(inputs.camera),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: buf(&self.params),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: buf(inputs.static_objects),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: buf(inputs.coordinate_spaces),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: buf(inputs.visible),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: buf(inputs.asset_quant),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: buf(inputs.instances),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: buf(inputs.pages),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: buf(inputs.sw_keys.unwrap_or(&self.dummy_keys)),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: buf(inputs.dense_keys.unwrap_or(&self.dummy_keys)),
                },
                wgpu::BindGroupEntry {
                    binding: 10,
                    resource: buf(pixel_material),
                },
                wgpu::BindGroupEntry {
                    binding: 11,
                    resource: buf(inputs.rt_visible.unwrap_or(&self.dummy_keys)),
                },
                wgpu::BindGroupEntry {
                    binding: 12,
                    resource: buf(inputs.dense_mask.unwrap_or(&self.dummy_keys)),
                },
            ],
        });
        let (rt_payload, rt_depth) = inputs
            .rt
            .unwrap_or((&self.dummy_rt_payload, &self.dummy_rt_depth));
        let resolve1 = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("VgShade resolve 1"),
            layout: &self.resolve_bgl1,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: res(inputs.hw_vis),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: res(inputs.hw_depth),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: res(inputs.scene_depth),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: res(rt_payload),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: res(rt_depth),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: res(payload_view),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: res(depth_view),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: res(source_view),
                },
            ],
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("VgShade vis_resolve"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.resolve_pipeline);
            pass.set_bind_group(0, &resolve0, &[]);
            pass.set_bind_group(1, &resolve1, &[]);
            pass.dispatch_workgroups(width.div_ceil(8), height.div_ceil(8), 1);
        }

        // 2. material binning
        let bin_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("VgShade bin"),
            layout: &self.bin_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buf(&self.params),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: buf(pixel_material),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: buf(inputs.materials),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: buf(&self.bins),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: buf(pixel_list),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: buf(&self.dispatch_args),
                },
            ],
        });
        let groups = pixels.div_ceil(64);
        let gx = groups.min(4096).max(1);
        let gy = groups.div_ceil(gx).max(1);
        for (label, pipeline, dispatch) in [
            (
                "VgShade bin_count",
                &self.bin_count_pipeline,
                Some((gx, gy)),
            ),
            ("VgShade bin_scan", &self.bin_scan_pipeline, None),
            (
                "VgShade bin_scatter",
                &self.bin_scatter_pipeline,
                Some((gx, gy)),
            ),
        ] {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some(label),
                timestamp_writes: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bin_group, &[]);
            match dispatch {
                Some((x, y)) => pass.dispatch_workgroups(x, y, 1),
                None => pass.dispatch_workgroups(1, 1, 1),
            }
        }

        // 3. shade, one indirect dispatch per material bin
        let mt = inputs.material_textures;
        let shade0 = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("VgShade shade 0"),
            layout: &self.shade_bgl0,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buf(inputs.camera),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: buf(&self.globals),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: buf(inputs.static_objects),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: buf(inputs.coordinate_spaces),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: buf(inputs.coordinate_spaces_prev),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: buf(inputs.visible),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: buf(inputs.asset_quant),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: buf(inputs.instances),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: buf(inputs.pages),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: buf(&self.params),
                },
                wgpu::BindGroupEntry {
                    binding: 10,
                    resource: buf(pixel_list),
                },
                wgpu::BindGroupEntry {
                    binding: 11,
                    resource: buf(&self.bins),
                },
                wgpu::BindGroupEntry {
                    binding: 12,
                    resource: buf(inputs.rt_visible.unwrap_or(&self.dummy_keys)),
                },
            ],
        });
        let mut material_entries = vec![
            wgpu::BindGroupEntry {
                binding: 0,
                resource: buf(inputs.materials),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: buf(mt.material_textures),
            },
        ];
        self.binding.append_bind_group_entries(
            &mut material_entries,
            2,
            mt.texture_views,
            mt.samplers,
        );
        let shade1 = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("VgShade shade 1"),
            layout: &self.shade_bgl1,
            entries: &material_entries,
        });
        let shade2 = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("VgShade shade 2"),
            layout: &self.shade_bgl2,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: res(payload_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: res(depth_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: res(targets.albedo),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: res(targets.normal),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: res(targets.orm),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: res(targets.emissive),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: res(targets.sss),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: res(targets.extra),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: res(targets.velocity),
                },
            ],
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("VgShade vg_shade"),
                timestamp_writes: None,
            });
            pass.set_bind_group(0, &shade0, &[]);
            pass.set_bind_group(1, &shade1, &[]);
            pass.set_bind_group(2, &shade2, &[]);
            for bin in 0..BIN_COUNT {
                if let Some(pipeline) = &self.shade_pipelines[bin as usize] {
                    pass.set_pipeline(pipeline);
                    pass.dispatch_workgroups_indirect(&self.dispatch_args, u64::from(bin) * 16);
                }
            }
        }

        // 3b. the selected debug view, over the albedo just written
        if let Some(debug) = &inputs.debug {
            self.encode_debug(
                device, queue, encoder, width, height, inputs, targets, debug,
            );
        }

        // 4. final depth
        let depth_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("VgShade depth"),
            layout: &self.depth_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: res(depth_view),
            }],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("VgShade depth"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: targets.lightmap_uv,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: targets.depth,
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
        pass.set_pipeline(&self.depth_pipeline);
        pass.set_bind_group(0, &depth_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{Mat4, Vec2, Vec3};

    fn validate(name: &str, source: &str) -> naga::Module {
        // What the device compiles: the prelude expanded.
        let source = &*helio_core::shader::resolve(source);
        let module = naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(source)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        module
    }

    fn struct_size(module: &naga::Module, name: &str) -> usize {
        let mut layouter = naga::proc::Layouter::default();
        layouter.update(module.to_ctx()).unwrap();
        let (handle, _) = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("missing {name}"));
        layouter[handle].size as usize
    }

    #[test]
    fn resolve_bin_depth_validate() {
        let m = validate("vg_resolve", &shader_source::resolve(&shared_decode()));
        assert_eq!(
            struct_size(&m, "VgVisibleCluster"),
            std::mem::size_of::<VgVisibleCluster>()
        );
        assert_eq!(
            struct_size(&m, "Params"),
            std::mem::size_of::<VgShadeParams>()
        );
        validate("vg_bin", &shader_source::bin());
        validate("vg_depth", &shader_source::depth());
    }

    #[test]
    fn debug_views_validate_and_every_view_has_a_shader_constant() {
        let source = debug_source();
        let m = validate("vg_debug", &source);
        assert_eq!(
            struct_size(&m, "DebugParams"),
            std::mem::size_of::<debug::DebugParams>()
        );
        // Every registered id is a `VGDBG_*` constant the shader switches on.
        for view in helio_vg_format::debug_view::ALL {
            let constant = format!(
                "const VGDBG_{}: u32 = {}u;",
                view.key().to_ascii_uppercase(),
                view.id()
            );
            assert!(source.contains(&constant), "{constant}");
            let case = format!("case VGDBG_{}:", view.key().to_ascii_uppercase());
            let tile = matches!(
                view,
                helio_vg_format::debug_view::VgDebugView::SwOverdraw
                    | helio_vg_format::debug_view::VgDebugView::DenseOverlap
                    | helio_vg_format::debug_view::VgDebugView::DenseMask
                    | helio_vg_format::debug_view::VgDebugView::VgMask
            );
            assert!(tile || source.contains(&case), "{case}");
        }
    }

    #[test]
    fn shade_validates_for_every_builtin_template_and_binding_mode() {
        let registry = helio_mats::RadiantTemplateRegistry::new();
        let mut checked = 0;
        for class in registry.keys() {
            let template = registry.get(class).unwrap();
            for (bindless, max_textures) in [(true, 256), (false, 8)] {
                let mode = if bindless {
                    helio_mats::MaterialBindingMode::BindingArray
                } else {
                    helio_mats::MaterialBindingMode::Expanded
                };
                let binding = material_binding(mode, max_textures);
                let src = shader_source::shade(template, binding, &shared_decode());
                assert!(
                    src.contains("textureSampleGrad"),
                    "class {class}: no gradient sampling"
                );
                assert!(
                    !src.contains("textureSample(") && !src.contains("textureSampleLevel("),
                    "class {class}: implicit-derivative sampling left in a compute shader"
                );
                let m = validate(&format!("vg_shade class {class} bindless={bindless}"), &src);
                if class == 0 && bindless {
                    assert_eq!(
                        struct_size(&m, "Globals"),
                        std::mem::size_of::<VgShadeGlobals>()
                    );
                    assert_eq!(
                        struct_size(&m, "Params"),
                        std::mem::size_of::<VgShadeParams>()
                    );
                }
                checked += 1;
            }
        }
        assert!(checked >= 10);
    }

    fn material_binding(
        mode: helio_mats::MaterialBindingMode,
        max_textures: usize,
    ) -> helio_mats::MaterialBindingConfig {
        // `MaterialBindingConfig` has no public constructor besides device
        // capabilities; synthesize the capability set that yields `mode`.
        let features = match mode {
            helio_mats::MaterialBindingMode::BindingArray => helio_mats::BINDLESS_MATERIAL_FEATURES,
            helio_mats::MaterialBindingMode::Expanded => wgpu::Features::empty(),
        };
        let limits = wgpu::Limits {
            max_sampled_textures_per_shader_stage: if mode
                == helio_mats::MaterialBindingMode::Expanded
            {
                (max_textures + helio_mats::EXPANDED_MATERIAL_TEXTURE_RESERVE) as u32
            } else {
                256
            },
            max_samplers_per_shader_stage: 256,
            max_bindings_per_bind_group: 1000,
            ..wgpu::Limits::default()
        };
        let config = helio_mats::MaterialBindingConfig::from_capabilities(features, limits);
        assert_eq!(config.mode, mode);
        config
    }

    #[test]
    fn the_shade_program_takes_a_template_and_the_material_table() {
        let program = shader_source::shade_program(&shared_decode());
        assert!(!program.contains("fn radiant_eval_surface"));
        assert!(program.contains("binding_array<texture_2d<f32>, 256>"));
        let registry = helio_mats::RadiantTemplateRegistry::new();
        let composed = registry.get(0).unwrap().compose(&program, "", 256, &[]);
        assert!(composed.contains("fn radiant_eval_surface"));
        assert!(composed.contains("fn default_pbr_surface"));
    }

    #[test]
    fn rust_layouts() {
        assert_eq!(std::mem::size_of::<VgVisibleCluster>(), 8);
        assert_eq!(std::mem::size_of::<VgShadeParams>(), 32);
        assert_eq!(std::mem::size_of::<VgShadeGlobals>(), 64);
        assert_eq!(OBJECT_ROW_WORDS * 4, 252);
    }

    // ── CPU reference of the barycentric reconstruction ──────────────────

    #[allow(deprecated)]
    fn camera(width: f32, height: f32) -> (Mat4, Mat4) {
        let view = Mat4::look_at_rh(Vec3::new(0.3, 0.2, 2.0), Vec3::new(0.1, 0.0, -3.0), Vec3::Y);
        (
            view,
            helio_core::depth::perspective(60f32.to_radians(), width / height, 0.1),
        )
    }

    /// Exact barycentrics from the clip-space solve match the ray / plane
    /// intersection over a grid of pixels, for oblique and extreme triangles.
    #[test]
    fn barycentrics_match_ray_plane_intersection() {
        let (w, h) = (640.0, 360.0);
        let (view, proj) = camera(w, h);
        let inv = (proj * view).inverse();
        let cam_pos = view.inverse().transform_point3(Vec3::ZERO);
        let triangles = [
            [
                Vec3::new(-1.0, -1.0, -3.0),
                Vec3::new(1.2, -0.8, -3.5),
                Vec3::new(0.1, 1.3, -2.5),
            ],
            // strongly foreshortened: a long ground-plane sliver receding away
            [
                Vec3::new(-2.0, -1.0, 0.0),
                Vec3::new(2.0, -1.0, 0.0),
                Vec3::new(0.0, -1.0, -60.0),
            ],
            // large depth range across a small screen area
            [
                Vec3::new(0.0, 0.0, -0.5),
                Vec3::new(0.2, 0.1, -40.0),
                Vec3::new(-0.2, 0.15, -39.0),
            ],
        ];
        let mut tested = 0;
        for tri in triangles {
            let c = tri.map(|p| cpu_ref::clip(view, proj, p.into()));
            for y in (0..360).step_by(7) {
                for x in (0..640).step_by(7) {
                    let ndc = cpu_ref::pixel_ndc(
                        Vec2::new(x as f32 + 0.5, y as f32 + 0.5),
                        Vec2::new(w, h),
                    );
                    let b = cpu_ref::bary_from_clip(c, ndc);
                    let far = inv * glam::Vec4::new(ndc.x, ndc.y, 1e-6, 1.0);
                    let dir = (far.truncate() / far.w - cam_pos).normalize();
                    let Some(r) = cpu_ref::bary_from_ray(tri, cam_pos, dir) else {
                        continue;
                    };
                    let inside = r.iter().all(|v| *v >= 0.0);
                    if !inside {
                        continue;
                    }
                    for i in 0..3 {
                        assert!(
                            (b[i] - r[i]).abs() < 2e-3,
                            "tri {tri:?} pixel ({x},{y}) bary {b:?} vs ray {r:?}"
                        );
                    }
                    // Depth interpolated with those weights is the clip depth
                    // of the intersection point.
                    let p = tri[0] * r[0] + tri[1] * r[1] + tri[2] * r[2];
                    let pc = cpu_ref::clip(view, proj, p);
                    let d = cpu_ref::depth_at(c, b);
                    assert!((d - pc.z / pc.w).abs() <= 2e-3 * (pc.z / pc.w).abs().max(1e-4));
                    tested += 1;
                }
            }
        }
        assert!(tested > 300, "too few covered samples: {tested}");
    }

    /// A triangle straddling the near plane (one vertex behind the camera)
    /// still resolves: the homogeneous solve needs no clipping.
    #[test]
    fn barycentrics_survive_near_plane_straddle() {
        let (w, h) = (320.0, 240.0);
        let view = Mat4::IDENTITY;
        let proj = helio_core::depth::perspective(70f32.to_radians(), w / h, 0.05);
        let tri = [
            Vec3::new(-1.0, -1.0, -2.0),
            Vec3::new(1.0, -1.0, -2.0),
            Vec3::new(0.0, 0.5, 1.0),
        ];
        let c = tri.map(|p| cpu_ref::clip(view, proj, p));
        assert!(
            c[2].w < 0.0 && c[0].w > 0.0,
            "vertex 2 must be behind the eye"
        );
        // A pixel low in the frame sees only the visible part: weights must be
        // a valid combination reproducing the NDC point.
        let ndc = cpu_ref::pixel_ndc(Vec2::new(160.0, 200.0), Vec2::new(w, h));
        let b = cpu_ref::bary_from_clip(c, ndc);
        assert!((b[0] + b[1] + b[2] - 1.0).abs() < 1e-5);
        let p = c[0] * b[0] + c[1] * b[1] + c[2] * b[2];
        // Compare without the divide: P.w may be tiny near the eye plane.
        let scale = c.iter().map(|v| v.abs().max_element()).fold(0.0, f32::max);
        assert!(
            (p.x - ndc.x * p.w).abs() < 1e-5 * scale && (p.y - ndc.y * p.w).abs() < 1e-5 * scale
        );
    }

    /// Payload precision: unorm16 packing round-trips within half a step, and
    /// the world-space error that implies is far below a texel at any
    /// realistic triangle size.
    #[test]
    fn bary_pack_precision() {
        let half_step = 0.5 / 65535.0 + 1e-7;
        let mut worst = 0.0f32;
        for i in 0..=400u32 {
            for j in 0..=(400 - i) {
                let (u, v) = (i as f32 / 400.0, j as f32 / 400.0);
                let b = cpu_ref::payload_unbary(cpu_ref::payload_bary([1.0 - u - v, u, v]));
                worst = worst.max((b[1] - u).abs()).max((b[2] - v).abs());
            }
        }
        assert!(worst <= half_step, "worst error {worst} > {half_step}");
        // Edge cases the packing must keep exactly.
        assert_eq!(cpu_ref::payload_bary([1.0, 0.0, 0.0]), 0);
        assert_eq!(
            cpu_ref::payload_unbary(helio_vg_format::bary_pack(1.0, 0.0))[1],
            1.0
        );
        // 128-pixel-wide triangle, UV spanning 1.0: error in texels.
        let texel_error = half_step * 128.0;
        assert!(texel_error < 0.002, "{texel_error}");
    }
}
