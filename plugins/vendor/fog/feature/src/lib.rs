//! Volumetric fog — froxel grid.
//!
//! A view-space 3D grid (Hillaire, "Physically Based and Unified Volumetric
//! Rendering in Frostbite", SIGGRAPH 2015), rather than a raymarch per pixel:
//!
//! 1. **Resolve/classify** — select SceneDB view settings and compact media/lights.
//! 2. **Cull** — conservatively assign lights to 8x8x4 froxel clusters.
//! 3. **Inject** — indirect dispatch over the selected quality tier, evaluating
//!    world media, geometric shadows, medium transmittance and rejected history.
//! 4. **Integrate** — one thread per (x,y) column: marches z once, producing
//!    accumulated in-scattering + transmittance.
//! 5. **Composite** (owned by the PP pass) — one trilinear 3D fetch at the
//!    pixel's depth.
//!
//! # Why a grid
//!
//! Cost is decoupled from screen resolution: ~2.65M froxels lit once each, against
//! ~59M samples for a 1280x720 per-pixel march at 64 steps. The trilinear fetch
//! filters in depth as well as x/y, so there is no reduced-resolution upsample to
//! hide — which is what made the earlier per-pixel version pixelate the geometry
//! seen through it.
//!
//! Temporal reprojection is what makes one shadow tap per froxel sufficient;
//! without it the grid is far too noisy to use.
//!
//! # Placement
//!
//! Runs after current-frame shadows on the graphics encoder. Optional scene
//! lights/shadow atlas illuminate pass-owned SceneDB world media. Legacy PP fog
//! remains an adapter. Publishes `fog_accum` and `fog_parameters` for compositing.
//!
//! # Owned resources
//!
//! The graph's texture pool is 2D-only, so the three 3D textures are owned here
//! and handed to later passes via [`RenderPass::publish`].

pub mod feature;
#[cfg(feature = "host-rows")]
pub mod host_rows;
pub mod scene_rows;
pub use feature::FogFeature;
pub use scene_rows::FogRowSources;
mod fog_composite;
pub use fog_composite::{FogCompositePass, FOGGED_HDR, FOGGED_HDR_FORMAT};

/// The integrated medium and its parameters, published as the frame's
/// participating media (`helio_frame_api::media`).
pub const FOG_ACCUM: &str = "fog_accum";
pub const FOG_PARAMETERS: &str = "fog_parameters";

use bytemuck::{Pod, Zeroable};
use helio_core::{PassContext, PrepareContext, RenderPass, Result as HelioResult};
use helio_frame_api::lighting::sun_transmittance::{self, SunTransmittance};

pub mod components;
pub use components::{
    GlobalFogRow, LocalFogVolumeRow, VolumetricFogSettingsRow, GLOBAL_FOG_MEDIA, LOCAL_FOG_MEDIA,
    VOLUMETRIC_FOG_SETTINGS,
};

/// Froxel grid dimensions.
///
/// Maximum screen footprint and 128 logarithmic slices, adjusted for aspect.
/// The economical tier lights half the extent on every axis (1/8 the froxels).
/// Both tiers share maximum-size allocations (~60.75 MiB at 16:9).
const FROXEL_W: u32 = 192;
const FROXEL_H: u32 = 108;
const FROXEL_D: u32 = 128;

const WG_X: u32 = 8;
const WG_Y: u32 = 8;

/// Weight of the current frame in the temporal blend.
///
/// Lighting edits and discontinuities override this weight and reject history.
const TEMPORAL_BLEND: f32 = 0.05;

const FMT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// `ActiveMedia` up to its trailing `lights` array: 32 header words plus the
/// volume, global and local lists of 64 each (volumetric_fog.wgsl).
const ACTIVE_MEDIA_HEADER_BYTES: u64 = (32 + 64 * 3) * 4;
/// Stride of a `"scene_lights"` row (the shader's full 128-byte `GpuLight`).
const LIGHT_ROW_BYTES: u64 = 128;
// FogVolume's WGSL opaque prefix/suffix must agree with the PP row ABI.
const _: () =
    assert!(helio_pass_postprocess::GpuPostProcessUniforms::FOG_BLOCK_OFFSET.is_multiple_of(16));

/// Assemble the legacy PP row layout from its Rust ABI: the opaque words
/// before and after the fog block follow wherever PP places it.
pub fn shader_source() -> String {
    use helio_pass_postprocess::GpuPostProcessUniforms as Uniforms;
    let head = Uniforms::FOG_BLOCK_OFFSET / 16;
    let tail = (std::mem::size_of::<Uniforms>() as u64
        - Uniforms::FOG_BLOCK_OFFSET
        - Uniforms::FOG_BLOCK_SIZE)
        / 16;
    let source = include_str!("../shaders/volumetric_fog.wgsl")
        .replace("__PP_HEAD_VEC4__", &head.to_string())
        .replace("__PP_TAIL_VEC4__", &tail.to_string());
    // The terrain's sun occlusion (`//!use sun_occlusion`), the frame's
    // sun transmittance (`//!use sun_transmittance`) and the lights' row and
    // shadow atlas and lookup (`//!use light_list`, `//!use shadow_atlas`,
    // `//!use shadow_lookup`) and the
    // frame's media grid mapping (`//!use frame_media`) ahead of it; the
    // prelude is the module's to resolve.
    format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{source}",
        helio_frame_api::media::SNIPPET.source,
        helio_light_api::SUN_OCCLUSION_SNIPPET.source,
        helio_frame_api::lighting::sun_transmittance::SNIPPET.source,
        helio_light_api::LIGHT_LIST_SNIPPET.source,
        helio_light_api::SHADOW_ATLAS_SNIPPET.source,
        helio_light_api::SHADOW_LOOKUP_SNIPPET.source,
    )
}

/// Preserve approximately square screen tiles, bounded by the tier's budget.
pub fn froxel_dimensions(width: u32, height: u32, quality: u32) -> [u32; 3] {
    let aspect = width.max(1) as f64 / height.max(1) as f64;
    let (w, h, d) = (192u32, 108u32, 128u32);
    let high = if aspect >= 16.0 / 9.0 {
        [w, ((w as f64 / aspect).round() as u32).clamp(2, h) & !1, d]
    } else {
        [((h as f64 * aspect).round() as u32).clamp(2, w) & !1, h, d]
    };
    high.map(|value| value / if quality == 0 { 2 } else { 1 })
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct FogGlobals {
    csm_splits: [f32; 4],
    _reserved: u32,
    frame: u32,
    history_valid: u32,
    temporal_blend: f32,
    time: f32,
    /// Last frame's camera jitter (NDC): history lives in its unjittered grid.
    prev_jitter: [f32; 2],
    /// Seconds since the previous frame: smoke history is advected by it.
    delta_time: f32,
    grid: [u32; 3],
    enabled: u32,
    /// The frame's world origin as an f32 pair (hi, lo): samples and volume
    /// bounds are in the frame's space; planet centres (world anchors) are
    /// rebased by it and heights along world Y add it back.
    origin_hi: [f32; 4],
    origin_lo: [f32; 4],
    /// The world origin modulo [`SMOKE_PERIOD`], which anchors smoke noise to
    /// the world.
    noise_phase: [f32; 4],
}

/// Smoke noise repeats every this many metres of world position, so the frame
/// origin's phase in it is exact in f32 however far the origin is.
const SMOKE_PERIOD: f64 = 4096.0;

/// Bytes of the pass's globals uniform (binding 2), for callers that bind
/// the fog shader directly.
pub const FOG_GLOBALS_BYTES: usize = std::mem::size_of::<FogGlobals>();

pub struct VolumetricFogPass {
    /// Whether this frame can contribute fog. The graph owns the pass even when
    /// the scene has no fog, so avoid dispatching the full froxel grid in that
    /// common case.
    active: bool,
    timing_query: Option<wgpu::QuerySet>,
    classify_pipeline: wgpu::ComputePipeline,
    resolve_pipeline: wgpu::ComputePipeline,
    cull_pipeline: wgpu::ComputePipeline,
    resolve_bgl: wgpu::BindGroupLayout,
    classify_bgl: wgpu::BindGroupLayout,
    resolve_bg: Option<wgpu::BindGroup>,
    classify_bg: Option<wgpu::BindGroup>,
    inject_pipeline: wgpu::ComputePipeline,
    integrate_pipeline: wgpu::ComputePipeline,
    inject_bgl: wgpu::BindGroupLayout,
    /// Integration camera, resolved range, active-grid metadata and sampler.
    ///
    /// Deliberately *not* inject_bgl. That one binds the scattering grid as a
    /// write-only storage texture, and cs_integrate samples the same grid from
    /// group 1 — binding both in one dispatch is a usage conflict wgpu rejects
    /// outright (STORAGE_WRITE_ONLY is exclusive).
    integrate_g0_bgl: wgpu::BindGroupLayout,
    integrate_bgl: wgpu::BindGroupLayout,

    /// GPU-resolved view range plus the optional legacy global fog adapter.
    fog_uniform_buf: wgpu::Buffer,
    globals_buf: wgpu::Buffer,
    shadow_sampler: wgpu::Sampler,
    linear_sampler: wgpu::Sampler,
    active_media_buf: wgpu::Buffer,
    resolved_fog_buf: wgpu::Buffer,
    indirect_buf: wgpu::Buffer,
    clusters_buf: wgpu::Buffer,
    fallback_global: wgpu::Buffer,
    fallback_local: wgpu::Buffer,
    fallback_settings: wgpu::Buffer,
    fallback_volumes: wgpu::Buffer,
    fallback_lights: wgpu::Buffer,
    fallback_shadow: wgpu::TextureView,
    /// Cleared to the far plane on first use: a zeroed depth reads as fully
    /// shadowed, which blacked out every fog light in graphs without atlases.
    fallback_shadow_cleared: bool,
    /// Zeroed (the layer stores 1 - T): nothing translucent filters the light.
    fallback_transmittance: wgpu::TextureView,
    /// No terrain's sun occlusion: an empty atlas, a frame of none and an
    /// empty page table.
    fallback_sun_occlusion: (wgpu::TextureView, wgpu::Buffer, wgpu::Buffer),
    /// The sun's transmittance a medium published when this pass was built
    /// (the clouds' shadow), if any; and what stands in for none.
    sun_transmittance: Option<SunTransmittance>,
    fallback_sun_transmittance: (wgpu::TextureView, wgpu::Buffer),

    /// Ping-ponged scattering grids: one is read as history while the other is
    /// written. Sampling and storing to one texture in a single dispatch is a
    /// data race, hence two.
    scatter_view: [wgpu::TextureView; 2],
    _scatter: [wgpu::Texture; 2],
    integrated_view: wgpu::TextureView,
    _integrated: wgpu::Texture,

    /// Index of the scatter grid written this frame; `1 - write_idx` is history.
    write_idx: usize,

    inject_bg: [Option<wgpu::BindGroup>; 2],
    inject_bg_key: Option<[wgpu::Buffer; 8]>,
    inject_shadow: Option<(
        wgpu::TextureView,
        wgpu::TextureView,
        wgpu::TextureView,
        (wgpu::TextureView, wgpu::Buffer, wgpu::Buffer),
        wgpu::TextureView,
        wgpu::Buffer,
    )>,
    integrate_g0_bg: Option<wgpu::BindGroup>,
    integrate_bg: [Option<wgpu::BindGroup>; 2],

    frame: u32,
    history_valid: bool,
    /// A freshly allocated grid holds zeros (transmittance 0 = black), so the
    /// first integration after allocation is recorded directly, not GPU-gated.
    integrate_initialized: bool,
    temporal_blend: f32,
    time: f32,
    grid: [u32; 3],
    previous_camera: Option<helio_core::GpuCameraUniforms>,
}

fn make_grid(
    device: &wgpu::Device,
    label: &str,
    grid: [u32; 3],
) -> (wgpu::Texture, wgpu::TextureView) {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: grid[0],
            height: grid[1],
            depth_or_array_layers: grid[2],
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format: FMT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
        view_formats: &[],
    });
    let view = tex.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D3),
        ..Default::default()
    });
    (tex, view)
}

impl VolumetricFogPass {
    /// Lights the medium through the sun's transmittance `sun` published
    /// (none: the sun reaches every froxel).
    pub fn with_sun_transmittance(mut self, sun: Option<SunTransmittance>) -> Self {
        self.sun_transmittance = sun;
        self
    }

    pub fn new(device: &wgpu::Device) -> Self {
        let shader = helio_core::shader::module(device, "Volumetric Fog Shader", &shader_source());

        let cv = wgpu::ShaderStages::COMPUTE;
        let uniform = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: cv,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let storage_ro = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: cv,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let storage_rw = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: cv,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: false },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let tex3d = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: cv,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D3,
                multisampled: false,
            },
            count: None,
        };
        let storage3d = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: cv,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format: FMT,
                view_dimension: wgpu::TextureViewDimension::D3,
            },
            count: None,
        };

        let inject_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Volumetric Fog Inject BGL"),
            entries: &[
                // camera
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: cv,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                uniform(1),    // fog
                uniform(2),    // globals
                storage_ro(3), // lights
                storage_ro(4), // shadow matrices
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: cv,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: cv,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
                // Static casters are cached in their own atlas; lighting takes the
                // minimum of both, and so must the medium or shafts ignore walls.
                wgpu::BindGroupLayoutEntry {
                    binding: 20,
                    visibility: cv,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                // Coloured transmittance of translucent casters (stained glass).
                wgpu::BindGroupLayoutEntry {
                    binding: 21,
                    visibility: cv,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                // The sun's occlusion by terrain outside the shadow maps.
                wgpu::BindGroupLayoutEntry {
                    binding: 22,
                    visibility: cv,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                uniform(23),
                storage_ro(26),
                // The clouds' shadow along the sun.
                wgpu::BindGroupLayoutEntry {
                    binding: 24,
                    visibility: cv,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                uniform(25),
                tex3d(7), // scatter history
                wgpu::BindGroupLayoutEntry {
                    binding: 8,
                    visibility: cv,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                storage3d(9),   // scatter out
                storage_ro(11), // SceneDB post-process volumes
                storage_ro(14), // global world media
                storage_ro(15), // local world media
                storage_rw(19), // per-cluster light lists
                wgpu::BindGroupLayoutEntry {
                    binding: 12,
                    visibility: cv,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let resolve_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Fog SceneDB Resolve BGL"),
            entries: &[
                storage_ro(0),
                uniform(1),
                uniform(2),
                storage_ro(11), // bounded media extents grow the grid range
                storage_rw(12),
                storage_ro(15),
                storage_ro(16),
                storage_ro(17),
                storage_rw(18),
            ],
        });
        let classify_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Fog Classify BGL"),
            entries: &[
                uniform(1),
                uniform(2),
                storage_ro(3),
                storage_ro(11),
                storage_rw(12),
                storage_rw(13),
                storage_ro(14),
                storage_ro(15),
            ],
        });

        // Only what cs_integrate actually reads — see the field's doc comment.
        let integrate_g0_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Volumetric Fog Integrate Group0 BGL"),
            entries: &[
                storage_ro(0),
                uniform(1),
                storage_rw(12),
                wgpu::BindGroupLayoutEntry {
                    binding: 8,
                    visibility: cv,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let integrate_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Volumetric Fog Integrate BGL"),
            entries: &[tex3d(0), storage3d(1)],
        });

        let inject_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Volumetric Fog Inject PL"),
            bind_group_layouts: &[Some(&inject_bgl)],
            immediate_size: 0,
        });
        let integrate_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Volumetric Fog Integrate PL"),
            bind_group_layouts: &[Some(&integrate_g0_bgl), Some(&integrate_bgl)],
            immediate_size: 0,
        });

        let classify_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Fog Classify PL"),
            bind_group_layouts: &[Some(&classify_bgl)],
            immediate_size: 0,
        });
        let resolve_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Fog Resolve PL"),
            bind_group_layouts: &[Some(&resolve_bgl)],
            immediate_size: 0,
        });
        let resolve_pipeline = helio_core::shader::create_compute_pipeline(
            &device,
            wgpu::ComputePipelineDescriptor {
                label: Some("Fog SceneDB Resolve"),
                layout: Some(&resolve_pl),
                module: &shader,
                entry_point: Some("cs_resolve"),
                compilation_options: Default::default(),
                cache: None,
            },
        );
        let cull_pipeline = helio_core::shader::create_compute_pipeline(
            &device,
            wgpu::ComputePipelineDescriptor {
                label: Some("Fog Cluster Lights"),
                layout: Some(&inject_pl),
                module: &shader,
                entry_point: Some("cs_cull"),
                compilation_options: Default::default(),
                cache: None,
            },
        );
        let classify_pipeline = helio_core::shader::create_compute_pipeline(
            &device,
            wgpu::ComputePipelineDescriptor {
                label: Some("Volumetric Fog Classify"),
                layout: Some(&classify_pl),
                module: &shader,
                entry_point: Some("cs_classify"),
                compilation_options: Default::default(),
                cache: None,
            },
        );
        let inject_pipeline = helio_core::shader::create_compute_pipeline(
            &device,
            wgpu::ComputePipelineDescriptor {
                label: Some("Volumetric Fog Inject"),
                layout: Some(&inject_pl),
                module: &shader,
                entry_point: Some("cs_inject"),
                compilation_options: Default::default(),
                cache: None,
            },
        );
        let integrate_pipeline = helio_core::shader::create_compute_pipeline(
            &device,
            wgpu::ComputePipelineDescriptor {
                label: Some("Volumetric Fog Integrate"),
                layout: Some(&integrate_pl),
                module: &shader,
                entry_point: Some("cs_integrate"),
                compilation_options: Default::default(),
                cache: None,
            },
        );

        let fog_uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Volumetric Fog Uniforms"),
            size: helio_pass_postprocess::GpuPostProcessUniforms::FOG_BLOCK_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Volumetric Fog Globals"),
            size: std::mem::size_of::<FogGlobals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Volumetric Fog Shadow Sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        // Clamped + trilinear: history reprojection and the composite both rely on
        // filtering across all three axes.
        let linear_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Volumetric Fog Linear Sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });

        let grid = [FROXEL_W, FROXEL_H, FROXEL_D];
        let (s0, v0) = make_grid(device, "Fog Scatter 0", grid);
        let (s1, v1) = make_grid(device, "Fog Scatter 1", grid);
        let (integrated, integrated_view) = make_grid(device, "Fog Integrated", grid);

        let active_media_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Fog Active Volume and Light Indices"),
            size: ACTIVE_MEDIA_HEADER_BYTES + 256 * 4,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let empty_storage = |label, size| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            })
        };
        let fallback_global = empty_storage(
            "Fog Empty Global Media",
            std::mem::size_of::<GlobalFogRow>() as u64,
        );
        let fallback_local = empty_storage(
            "Fog Empty Local Media",
            std::mem::size_of::<LocalFogVolumeRow>() as u64,
        );
        let fallback_settings = empty_storage("Fog Empty View Settings", 48);
        let clusters_buf = empty_storage("Fog Cluster Light Lists", (24 * 14 * 32 * 65 * 4) as u64);
        let resolved_fog_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Fog Resolved Parameters"),
            size: 64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let indirect_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Fog Active Dispatch"),
            size: 40,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::INDIRECT
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let fallback_volumes = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Fog Empty SceneDB Volumes"),
            size: std::mem::size_of::<helio_pass_postprocess::GpuPostProcessVolume>() as u64,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let fallback_lights = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Fog Empty Lights"),
            size: 128,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let fallback_shadow = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("Fog Empty Shadow Atlas"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            });
        Self {
            active: true,
            timing_query: None,
            classify_pipeline,
            resolve_pipeline,
            cull_pipeline,
            resolve_bgl,
            classify_bgl,
            resolve_bg: None,
            classify_bg: None,
            resolved_fog_buf,
            indirect_buf,
            clusters_buf,
            fallback_global,
            fallback_local,
            fallback_settings,
            active_media_buf,
            fallback_volumes,
            fallback_lights,
            fallback_shadow,
            fallback_shadow_cleared: false,
            fallback_transmittance: clear_transmittance(device),
            fallback_sun_occlusion: (
                device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some("Fog No Sun Occlusion"),
                        size: wgpu::Extent3d {
                            width: 1,
                            height: 1,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: helio_light_api::SUN_OCCLUSION_FORMAT,
                        usage: wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    })
                    .create_view(&Default::default()),
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("Fog No Sun Occlusion Frame"),
                    size: helio_light_api::SUN_OCCLUSION_FRAME_BYTES,
                    usage: wgpu::BufferUsages::UNIFORM,
                    mapped_at_creation: false,
                }),
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("Fog No Sun Occlusion Pages"),
                    size: 16,
                    usage: wgpu::BufferUsages::STORAGE,
                    mapped_at_creation: false,
                }),
            ),
            sun_transmittance: None,
            fallback_sun_transmittance: sun_transmittance::stand_in(device),
            inject_pipeline,
            integrate_pipeline,
            inject_bgl,
            integrate_g0_bgl,
            integrate_bgl,
            fog_uniform_buf,
            globals_buf,
            shadow_sampler,
            linear_sampler,
            scatter_view: [v0, v1],
            _scatter: [s0, s1],
            integrated_view,
            _integrated: integrated,
            write_idx: 0,
            inject_bg: [None, None],
            inject_bg_key: None,
            inject_shadow: None,
            integrate_g0_bg: None,
            integrate_bg: [None, None],
            frame: 0,
            history_valid: false,
            integrate_initialized: false,
            temporal_blend: TEMPORAL_BLEND,
            time: 0.0,
            grid,
            previous_camera: None,
        }
    }

    /// Optional explicit pass override retained for API compatibility. Normal
    /// activity comes entirely from SceneDB and GPU classification. Disabling
    /// still writes neutral output, so consumers cannot retain stale fog.
    pub fn set_active(&mut self, active: bool) {
        self.active = active;
    }

    /// Weight of the current frame in the temporal blend, 0..1.
    ///
    /// Lower is steadier but slower to react to lighting changes; higher reacts
    /// faster but lets the single-sample noise through.
    pub fn set_temporal_blend(&mut self, blend: f32) {
        self.temporal_blend = blend.clamp(0.01, 1.0);
    }

    /// Drop the temporal history — call after a camera cut, or reprojected fog
    /// from the previous shot smears across the first frames of the new one.
    pub fn reset_history(&mut self) {
        self.history_valid = false;
    }

    /// Optional GPU timestamps around classification, injection and integration.
    pub fn enable_timing(&mut self, device: &wgpu::Device) -> bool {
        if !device.features().contains(
            wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS,
        ) {
            return false;
        }
        self.timing_query = Some(device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("Volumetric fog timings"),
            ty: wgpu::QueryType::Timestamp,
            count: 4,
        }));
        true
    }

    pub fn timing_query(&self) -> Option<&wgpu::QuerySet> {
        self.timing_query.as_ref()
    }
}

/// Whether `camera` projects in parallel (`proj`'s `w` row is `0 0 0 1`).
fn orthographic(camera: &helio_core::GpuCameraUniforms) -> bool {
    camera.proj[15] == 1.0
}

impl RenderPass for VolumetricFogPass {
    fn scene_buffers(&self) -> &'static [&'static str] {
        &["post_process_volumes", helio_light_api::LIGHT_LIST]
    }

    fn name(&self) -> &'static str {
        "VolumetricFogPass"
    }

    fn writes(&self) -> &'static [&'static str] {
        &[FOG_ACCUM, FOG_PARAMETERS]
    }

    fn reads(&self) -> &'static [&'static str] {
        let mut reads = vec![
            helio_light_api::SHADOW_ATLAS,
            helio_light_api::STATIC_SHADOW_ATLAS,
            helio_light_api::SHADOW_TRANSMITTANCE,
            helio_light_api::SHADOW_MATRICES,
            helio_pass_postprocess::POST_SETTINGS,
            helio_light_api::SUN_OCCLUSION_MAPS,
            helio_light_api::SUN_OCCLUSION_PAGES,
            helio_light_api::SUN_OCCLUSION_FRAME,
        ];
        if let Some(sun) = self.sun_transmittance {
            reads.extend([sun.map, sun.frame]);
        }
        helio_frame_api::static_names(&reads)
    }

    fn publish<'a>(&self, frame: &mut helio_core::ResourceRegistry<'a>) {
        // The graph's pool is 2D-only, so this texture is pass-owned and handed
        // over here rather than routed by name.
        let view: &'a wgpu::TextureView = unsafe { std::mem::transmute(&self.integrated_view) };
        frame.write_texture_view(
            helio_core::ResourceKey::new(FOG_ACCUM),
            view,
            "VolumetricFogPass",
        );
        let parameters: &'a wgpu::Buffer = unsafe { std::mem::transmute(&self.fog_uniform_buf) };
        frame.write_buffer(
            helio_core::ResourceKey::new(FOG_PARAMETERS),
            parameters,
            "VolumetricFogPass",
        );
    }

    fn render_pass_descriptor<'a>(
        &'a self,
        _target: &'a wgpu::TextureView,
        _depth: &'a wgpu::TextureView,
        _resources: &'a helio_core::ResourceRegistry<'a>,
    ) -> Option<wgpu::RenderPassDescriptor<'a>> {
        None
    }

    fn chain_transparent(&self) -> bool {
        // Shadows are produced on the graphics encoder. Fog must execute after
        // those writes, and before its graphics composite on the same encoder.
        false
    }

    fn prepare(&mut self, ctx: &PrepareContext) -> HelioResult<()> {
        self.frame = ctx.frame_num as u32;
        self.time += ctx.delta_time.max(0.0);
        let grid = froxel_dimensions(ctx.width, ctx.height, 1);
        if grid != self.grid {
            self.grid = grid;
            let (s0, v0) = make_grid(ctx.device, "Fog Scatter 0", grid);
            let (s1, v1) = make_grid(ctx.device, "Fog Scatter 1", grid);
            let (integrated, view) = make_grid(ctx.device, "Fog Integrated", grid);
            self._scatter = [s0, s1];
            self.scatter_view = [v0, v1];
            self._integrated = integrated;
            self.integrated_view = view;
            self.inject_bg = [None, None];
            self.integrate_bg = [None, None];
            self.integrate_initialized = false;
            self.history_valid = false;
        }
        if let Some(previous) = &self.previous_camera {
            // The previous matrix must describe the frame actually rendered by
            // this pass. This detects camera switches/cuts and skipped frames.
            let discontinuity = previous
                .view_proj
                .iter()
                .zip(ctx.camera_data.prev_view_proj)
                .any(|(a, b)| !b.is_finite() || (a - b).abs() > 0.001);
            if discontinuity
                || previous.jitter_frame[3].to_bits() != ctx.camera_data.jitter_frame[3].to_bits()
            {
                self.history_valid = false;
            }
        }
        // One light-list slot per SceneDB light row (Pulsar-Native#838): the
        // classify pass lists every active row, so per-froxel loops never
        // fall back to sweeping all rows, dead ones included.
        let light_rows = ctx
            .scene_buffers
            .get(helio_core::BufferKey::of(helio_light_api::LIGHT_LIST))
            .map_or(0, |lights| lights.buffer.size() / LIGHT_ROW_BYTES);
        let required = ACTIVE_MEDIA_HEADER_BYTES + light_rows.max(256) * 4;
        if self.active_media_buf.size() < required {
            self.active_media_buf = ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Fog Active Volume and Light Indices"),
                size: required.next_power_of_two(),
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            });
            // Every group binding the old buffer, and its persistent history
            // state, starts over.
            self.inject_bg = [None, None];
            self.integrate_g0_bg = None;
            self.resolve_bg = None;
            self.classify_bg = None;
            self.history_valid = false;
        }
        let prev_jitter = self.previous_camera.map_or(
            [
                ctx.camera_data.jitter_frame[0],
                ctx.camera_data.jitter_frame[1],
            ],
            |c| [c.jitter_frame[0], c.jitter_frame[1]],
        );
        self.previous_camera = Some(*ctx.camera_data);

        let origin_hi = ctx.world_origin.as_vec3();
        let globals = FogGlobals {
            csm_splits: helio_light_api::CSM_SPLITS,
            _reserved: 0,
            frame: self.frame,
            history_valid: self.history_valid as u32,
            temporal_blend: self.temporal_blend,
            time: self.time,
            prev_jitter,
            delta_time: ctx.delta_time.clamp(0.0, 0.25),
            grid: self.grid,
            // Orthographic views never fog: fog is a function of the distance
            // from the eye, which means nothing in a parallel projection, and
            // a haze over a drawing meant for measuring defeats it.
            enabled: (self.active && !orthographic(ctx.camera_data)) as u32,
            origin_hi: origin_hi.extend(0.0).to_array(),
            origin_lo: (ctx.world_origin - origin_hi.as_dvec3())
                .as_vec3()
                .extend(0.0)
                .to_array(),
            noise_phase: ctx
                .world_origin
                .map(|x| x.rem_euclid(SMOKE_PERIOD))
                .as_vec3()
                .extend(0.0)
                .to_array(),
        };
        ctx.queue
            .write_buffer(&self.globals_buf, 0, bytemuck::bytes_of(&globals));
        Ok(())
    }

    fn execute(&mut self, ctx: &mut PassContext) -> HelioResult<()> {
        if !self.fallback_shadow_cleared {
            let mut cmds = ctx.graphics_cmds();
            let _clear = cmds.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Fog Empty Shadow Atlas clear"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.fallback_shadow,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.fallback_shadow_cleared = true;
        }
        let postprocess_buf = ctx.registry.get(helio_core::ResourceKey::new(
            helio_pass_postprocess::POST_SETTINGS,
        ));
        let shadow_atlas = ctx
            .registry
            .get(helio_core::ResourceKey::new(helio_light_api::SHADOW_ATLAS))
            .unwrap_or(&self.fallback_shadow);
        let static_shadow_atlas = ctx
            .registry
            .get(helio_core::ResourceKey::new(
                helio_light_api::STATIC_SHADOW_ATLAS,
            ))
            .unwrap_or(&self.fallback_shadow);
        let transmittance = ctx
            .registry
            .get(helio_core::ResourceKey::new(
                helio_light_api::SHADOW_TRANSMITTANCE,
            ))
            .unwrap_or(&self.fallback_transmittance);
        let sun_occlusion = match (
            ctx.registry
                .texture_binding(helio_light_api::SUN_OCCLUSION_MAPS),
            ctx.registry
                .buffer_binding(helio_light_api::SUN_OCCLUSION_FRAME),
            ctx.registry
                .buffer_binding(helio_light_api::SUN_OCCLUSION_PAGES),
        ) {
            (Some(maps), Some(frame), Some(pages)) => (maps, frame, pages),
            _ => (
                &self.fallback_sun_occlusion.0,
                &self.fallback_sun_occlusion.1,
                &self.fallback_sun_occlusion.2,
            ),
        };
        let cloud_shadow = self
            .sun_transmittance
            .and_then(|sun| {
                ctx.registry
                    .texture_binding(sun.map)
                    .zip(ctx.registry.buffer_binding(sun.frame))
            })
            .unwrap_or((
                &self.fallback_sun_transmittance.0,
                &self.fallback_sun_transmittance.1,
            ));

        let camera_buf = ctx.camera;
        let lights_buf = ctx
            .scene_buffers
            .get(helio_core::BufferKey::of(helio_light_api::LIGHT_LIST))
            .map(|handle| &handle.buffer)
            .unwrap_or(&self.fallback_lights);
        let volumes_buf = ctx
            .scene_buffers
            .get(helio_core::BufferKey::of("post_process_volumes"))
            .map(|handle| &handle.buffer)
            .unwrap_or(&self.fallback_volumes);
        let resolve_scene = |name, fallback| {
            ctx.scene_buffers
                .get(helio_core::BufferKey::of(name))
                .map(|handle| &handle.buffer)
                .unwrap_or(fallback)
        };
        let global_buf = resolve_scene("global_fog_media", &self.fallback_global);
        let local_buf = resolve_scene("local_fog_media", &self.fallback_local);
        let settings_buf = resolve_scene("volumetric_fog_settings", &self.fallback_settings);
        let legacy_buf = resolve_scene("fog_components", &self.fallback_global);
        let shadow_matrices = ctx
            .registry
            .get::<helio_light_api::ShadowMatricesFrameData<'_>>(helio_light_api::shadow_matrices())
            .map(|s| s.shadow_matrices)
            .unwrap_or(ctx.camera);

        // Swap the ping-pong: last frame's write target is this frame's history.
        self.write_idx ^= 1;
        let write_idx = self.write_idx;
        let history_idx = 1 - write_idx;

        let key = [
            camera_buf.clone(),
            lights_buf.clone(),
            shadow_matrices.clone(),
            volumes_buf.clone(),
            global_buf.clone(),
            local_buf.clone(),
            settings_buf.clone(),
            legacy_buf.clone(),
        ];
        if self.inject_bg_key.as_ref() != Some(&key)
            || self
                .inject_shadow
                .as_ref()
                .map(|(d, s, t, (o, f, p), c, cf)| (d, s, t, o, f, p, c, cf))
                != Some((
                    shadow_atlas,
                    static_shadow_atlas,
                    transmittance,
                    sun_occlusion.0,
                    sun_occlusion.1,
                    sun_occlusion.2,
                    cloud_shadow.0,
                    cloud_shadow.1,
                ))
        {
            // Both sides are rebuilt together: each pins a fixed history/write
            // pair, so a stale one would read the grid it is also writing.
            self.inject_bg = [None, None];
            self.inject_bg_key = Some(key);
            self.inject_shadow = Some((
                shadow_atlas.clone(),
                static_shadow_atlas.clone(),
                transmittance.clone(),
                (
                    sun_occlusion.0.clone(),
                    sun_occlusion.1.clone(),
                    sun_occlusion.2.clone(),
                ),
                cloud_shadow.0.clone(),
                cloud_shadow.1.clone(),
            ));
            self.integrate_g0_bg = None;
            self.resolve_bg = None;
            self.classify_bg = None;
        }

        let buffer_group =
            |label, layout: &wgpu::BindGroupLayout, entries: &[(u32, &wgpu::Buffer)]| {
                ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(label),
                    layout,
                    entries: &entries
                        .iter()
                        .map(|(binding, buffer)| wgpu::BindGroupEntry {
                            binding: *binding,
                            resource: buffer.as_entire_binding(),
                        })
                        .collect::<Vec<_>>(),
                })
            };
        if self.resolve_bg.is_none() {
            self.resolve_bg = Some(buffer_group(
                "Fog SceneDB Resolve BG",
                &self.resolve_bgl,
                &[
                    (0, camera_buf),
                    (1, &self.fog_uniform_buf),
                    (2, &self.globals_buf),
                    (11, volumes_buf),
                    (12, &self.active_media_buf),
                    (15, local_buf),
                    (16, settings_buf),
                    (17, legacy_buf),
                    (18, &self.resolved_fog_buf),
                ],
            ));
            self.classify_bg = Some(buffer_group(
                "Fog Classify BG",
                &self.classify_bgl,
                &[
                    (1, &self.fog_uniform_buf),
                    (2, &self.globals_buf),
                    (3, lights_buf),
                    (11, volumes_buf),
                    (12, &self.active_media_buf),
                    (13, &self.indirect_buf),
                    (14, global_buf),
                    (15, local_buf),
                ],
            ));
        }

        if self.inject_bg[write_idx].is_none() {
            self.inject_bg[write_idx] =
                Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("Volumetric Fog Inject BG"),
                    layout: &self.inject_bgl,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 14,
                            resource: global_buf.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 15,
                            resource: local_buf.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 19,
                            resource: self.clusters_buf.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 11,
                            resource: volumes_buf.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 12,
                            resource: self.active_media_buf.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: camera_buf.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: self.fog_uniform_buf.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: self.globals_buf.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: lights_buf.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: shadow_matrices.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 5,
                            resource: wgpu::BindingResource::TextureView(shadow_atlas),
                        },
                        wgpu::BindGroupEntry {
                            binding: 20,
                            resource: wgpu::BindingResource::TextureView(static_shadow_atlas),
                        },
                        wgpu::BindGroupEntry {
                            binding: 21,
                            resource: wgpu::BindingResource::TextureView(transmittance),
                        },
                        wgpu::BindGroupEntry {
                            binding: 22,
                            resource: wgpu::BindingResource::TextureView(sun_occlusion.0),
                        },
                        wgpu::BindGroupEntry {
                            binding: 23,
                            resource: sun_occlusion.1.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 26,
                            resource: sun_occlusion.2.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 24,
                            resource: wgpu::BindingResource::TextureView(cloud_shadow.0),
                        },
                        wgpu::BindGroupEntry {
                            binding: 25,
                            resource: cloud_shadow.1.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 6,
                            resource: wgpu::BindingResource::Sampler(&self.shadow_sampler),
                        },
                        wgpu::BindGroupEntry {
                            binding: 7,
                            resource: wgpu::BindingResource::TextureView(
                                &self.scatter_view[history_idx],
                            ),
                        },
                        wgpu::BindGroupEntry {
                            binding: 8,
                            resource: wgpu::BindingResource::Sampler(&self.linear_sampler),
                        },
                        wgpu::BindGroupEntry {
                            binding: 9,
                            resource: wgpu::BindingResource::TextureView(
                                &self.scatter_view[write_idx],
                            ),
                        },
                    ],
                }));
        }

        if self.integrate_g0_bg.is_none() {
            self.integrate_g0_bg = Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Volumetric Fog Integrate Group0 BG"),
                layout: &self.integrate_g0_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 12,
                        resource: self.active_media_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 8,
                        resource: wgpu::BindingResource::Sampler(&self.linear_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: camera_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: self.fog_uniform_buf.as_entire_binding(),
                    },
                ],
            }));
        }

        if self.integrate_bg[write_idx].is_none() {
            self.integrate_bg[write_idx] =
                Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("Volumetric Fog Integrate BG"),
                    layout: &self.integrate_bgl,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(
                                &self.scatter_view[write_idx],
                            ),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&self.integrated_view),
                        },
                    ],
                }));
        }

        let (Some(inject_bg), Some(integrate_g0_bg), Some(integrate_bg)) = (
            self.inject_bg[write_idx].as_ref(),
            self.integrate_g0_bg.as_ref(),
            self.integrate_bg[write_idx].as_ref(),
        ) else {
            return Ok(());
        };

        let mut cmds = ctx.graphics_cmds();

        // Optional legacy adapter. Native world media require no PP resources.
        if let Some(postprocess_buf) = postprocess_buf {
            cmds.copy_buffer_to_buffer(
                postprocess_buf,
                helio_pass_postprocess::GpuPostProcessUniforms::FOG_BLOCK_OFFSET,
                &self.fog_uniform_buf,
                0,
                64,
            );
        } else {
            cmds.clear_buffer(&self.fog_uniform_buf, 0, None);
        }

        if let Some(query) = &self.timing_query {
            cmds.write_timestamp(query, 0);
        }
        {
            let mut pass = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Fog SceneDB Resolve"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.resolve_pipeline);
            pass.set_bind_group(0, self.resolve_bg.as_ref().unwrap(), &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        cmds.copy_buffer_to_buffer(&self.resolved_fog_buf, 0, &self.fog_uniform_buf, 0, 64);
        {
            let mut cpass = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Volumetric Fog Classify"),
                timestamp_writes: None,
            });
            cpass.set_pipeline(&self.classify_pipeline);
            cpass.set_bind_group(0, self.classify_bg.as_ref().unwrap(), &[]);
            cpass.dispatch_workgroups(1, 1, 1);
        }
        // Classification zeroes the published range (max_distance, byte 20)
        // while the grid is neutral, so FogComposite and transparency pass
        // the image through rather than sampling an identity grid per pixel.
        // Inject and integrate are dispatched with zero groups in that case,
        // so nothing below reads the zeroed range.
        cmds.copy_buffer_to_buffer(&self.indirect_buf, 36, &self.fog_uniform_buf, 20, 4);
        {
            let mut pass = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Fog Cluster Lights"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.cull_pipeline);
            pass.set_bind_group(0, inject_bg, &[]);
            pass.dispatch_workgroups_indirect(&self.indirect_buf, 12);
        }
        if let Some(query) = &self.timing_query {
            cmds.write_timestamp(query, 1);
        }
        {
            let mut cpass = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Volumetric Fog Inject"),
                timestamp_writes: None,
            });
            cpass.set_pipeline(&self.inject_pipeline);
            cpass.set_bind_group(0, inject_bg, &[]);
            cpass.dispatch_workgroups_indirect(&self.indirect_buf, 0);
        }

        if let Some(query) = &self.timing_query {
            cmds.write_timestamp(query, 2);
        }
        {
            // One thread per (x,y) column — each marches all FROXEL_D slices, so
            // z is 1 here, not FROXEL_D.
            let mut cpass = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Volumetric Fog Integrate"),
                timestamp_writes: None,
            });
            cpass.set_pipeline(&self.integrate_pipeline);
            cpass.set_bind_group(0, integrate_g0_bg, &[]);
            cpass.set_bind_group(1, integrate_bg, &[]);
            if self.integrate_initialized {
                // Classification zeroes these args once the grid is neutral and
                // no medium exists, so an empty scene pays nothing here.
                cpass.dispatch_workgroups_indirect(&self.indirect_buf, 24);
            } else {
                cpass.dispatch_workgroups(
                    self.grid[0].div_ceil(WG_X),
                    self.grid[1].div_ceil(WG_Y),
                    1,
                );
                self.integrate_initialized = true;
            }
        }

        if let Some(query) = &self.timing_query {
            cmds.write_timestamp(query, 3);
        }
        // History is only meaningful once a grid has actually been written.
        self.history_valid = true;

        Ok(())
    }
}

/// 1×1 zeroed transmittance array. The shadow pass stores 1 - T, so zero
/// filters nothing when no shadow pass publishes a layer.
fn clear_transmittance(device: &wgpu::Device) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("Fog Fallback Transmittance"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        })
}
