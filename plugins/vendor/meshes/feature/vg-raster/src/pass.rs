//! Pipelines, targets and the per-frame encode of the virtual-geometry raster.

use crate::abi::{self, ClassifyParams, VgRasterParams, TILE_SIZE};
use crate::shaders;
use helio_core::CommandRecorder;
use wgpu::util::DeviceExt;

/// Format of the visibility target: `vis_pack(visible_cluster, tri)` per pixel.
pub const VIS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Uint;
/// Format of the Tier 1 depth target (reversed-Z, see `helio_core::depth`).
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// Device features Tier 2 uses when available. Request them as OPTIONAL
/// (intersect with the adapter's features). Both are needed: wgpu requires
/// `SHADER_INT64` for 64-bit atomics and naga validates 64-bit atomics against
/// both capabilities.
pub fn optional_features() -> wgpu::Features {
    wgpu::Features::SHADER_INT64 | wgpu::Features::SHADER_INT64_ATOMIC_MIN_MAX
}

/// True when `features` allow the 64-bit atomic software kernel.
pub fn supports_int64_atomics(features: wgpu::Features) -> bool {
    features.contains(optional_features())
}

#[derive(Clone, Debug)]
pub struct VgRasterConfig {
    /// Cull back faces in Tier 1 and Tier 2. Double-sided materials would need
    /// a per-material flag instead (open issue in the crate docs).
    pub cull_back: bool,
    /// Capacity of each 16x16 tile's triangle list in the no-int64 fallback.
    pub tile_bin_cap: u32,
    /// A tile with at least this many overlapping triangles is dense.
    pub dense_tri_threshold: u32,
    /// A tile with at least this many depth-test-passing pixel hits is dense
    /// (256 pixels per tile, so `256 * K` = average depth complexity K).
    pub dense_hit_threshold: u32,
    /// A previously dense tile stays dense above this fraction of the thresholds.
    pub dense_hysteresis: f32,
    /// Frames a tile stays dense after its counters last qualified (latch with
    /// decay; 1 = no latch).
    pub dense_latch_frames: u32,
    /// Most dense tiles per frame; the excess stays on Tier 2.
    pub dense_max_tiles: u32,
    /// Use the tiled fallback even when 64-bit atomics are available.
    pub force_tile_fallback: bool,
}

/// Calibration override (measurement runs only): a `u32` from the environment.
fn env_u32(name: &str) -> Option<u32> {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok())
}

/// Higher sensitivity lowers the overlap required before a tile is classified
/// as dense. Kept as a startup setting because the classifier pipelines are
/// built with the raster pass.
fn dense_sensitivity() -> f32 {
    std::env::var("HELIO_VG_DENSE_SENSITIVITY")
        .ok()
        .and_then(|value| value.trim().parse::<f32>().ok())
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or(1.0)
        .clamp(0.125, 8.0)
}

fn sensitivity_threshold(base: u32, sensitivity: f32) -> u32 {
    (f64::from(base.max(1)) / f64::from(sensitivity))
        .round()
        .clamp(1.0, f64::from(u32::MAX)) as u32
}

impl Default for VgRasterConfig {
    fn default() -> Self {
        let sensitivity = dense_sensitivity();
        Self {
            cull_back: true,
            tile_bin_cap: 1024,
            dense_tri_threshold: sensitivity_threshold(
                env_u32("HELIO_VG_DENSE_TRI").unwrap_or(64),
                sensitivity,
            ),
            dense_hit_threshold: sensitivity_threshold(
                env_u32("HELIO_VG_DENSE_HIT").unwrap_or(64),
                sensitivity,
            ),
            dense_hysteresis: 0.5,
            dense_latch_frames: env_u32("HELIO_VG_DENSE_LATCH").unwrap_or(8),
            dense_max_tiles: env_u32("HELIO_VG_DENSE_MAX_TILES").unwrap_or(512),
            force_tile_fallback: false,
        }
    }
}

/// Scene-side inputs of one raster encode. These are the (documented)
/// placeholder bindings for T5's outputs and the scene join; see the crate docs.
pub struct VgRasterInputs<'a> {
    /// Camera storage buffer (`GpuCameraUniforms` layout; `view_index` selects).
    pub camera: &'a wgpu::Buffer,
    pub view_index: u32,
    /// `helio-vg-stream` page pool (`array<u32>`, `PAGE_BYTES` per slot).
    pub pages: &'a wgpu::Buffer,
    /// `vg_asset_quant` (`array<vec4<f32>>`: lattice origin xyz, step w) by asset slot.
    pub asset_quant: &'a wgpu::Buffer,
    /// The cull's `visible_clusters` (`GpuVisibleCluster {instance, cluster_slot}`);
    /// the HW bins and the SW range index into it by unified index.
    pub visible: &'a wgpu::Buffer,
    /// `GpuVgInstance` rows.
    pub vg_instances: &'a wgpu::Buffer,
    /// The scene join's `static_objects` rows (`abi::OBJECT_STRIDE` = 252 B stride).
    pub objects: &'a wgpu::Buffer,
    /// 4 x `DrawIndirectArgs` (one per `abi::HW_BIN_CAPS`), at this byte offset.
    pub hw_indirect: &'a wgpu::Buffer,
    pub hw_indirect_offset: u64,
    /// `{first, count}` of the software bin at word `sw_meta_offset_words`: a
    /// standalone [`abi::SwMeta`] (offset 0) or the cull's `state` buffer
    /// (offset `helio_pass_vg_cull::state::RANGES + phase * 12 + 4 * 2`).
    pub sw_meta: &'a wgpu::Buffer,
    pub sw_meta_offset_words: u32,
    /// `DispatchIndirect` args for the software clusters `{min(n, 32768), ceil(n / 32768), 1}` (`abi::sw_dispatch_args`).
    pub sw_dispatch: &'a wgpu::Buffer,
    pub sw_dispatch_offset: u64,
    /// Alpha-masked clusters (bins 6 and 7 of the cull). `None` records nothing
    /// for them; the opaque tiers are identical either way.
    pub masked: Option<VgMaskedInputs<'a>>,
    /// Dense-bin clusters (cull bin 5) of this phase: their overlap is estimated
    /// into the classifier so a tile does not stop being dense just because its
    /// clusters were diverted away from Tier 2. `None` = no estimate.
    pub dense: Option<VgDenseInputs<'a>>,
}

/// The cull's dense bin for one phase (see [`VgRasterInputs::dense`]).
#[derive(Clone, Copy)]
pub struct VgDenseInputs<'a> {
    /// Buffer holding the dense bin `{first, count}` (the cull `state`).
    pub meta: &'a wgpu::Buffer,
    /// Word offset of the dense bin's `{first, count}` in `meta`.
    pub meta_offset_words: u32,
    /// `DispatchIndirect` args of the dense bin `{min(n, 32768), ceil(n / 32768), 1}`.
    pub dispatch: &'a wgpu::Buffer,
    pub dispatch_offset: u64,
}

/// Inputs of the alpha-masked tiers (see `helio_mats::vg_alpha`).
#[derive(Clone, Copy)]
pub struct VgMaskedInputs<'a> {
    /// The scene's `materials` table (`GpuMaterial` rows).
    pub materials: &'a wgpu::Buffer,
    /// Material texture metadata and the texture table.
    pub material_textures: helio_mats::MaterialTextureBindings<'a>,
    /// Byte offset in `hw_indirect` of the masked hardware `DrawIndirectArgs`
    /// (vertex count `3 * 128`).
    pub hw_indirect_offset: u64,
    /// Word offset in `sw_meta` of the masked software bin's `{first, count}`.
    pub sw_meta_offset_words: u32,
    /// Byte offset in `sw_dispatch` of the masked software `DispatchIndirect`.
    pub sw_dispatch_offset: u64,
}

/// Targets and per-frame buffers owned by the raster. Recreate on resize.
pub struct VgTargets {
    pub width: u32,
    pub height: u32,
    pub tiles_x: u32,
    pub tiles_y: u32,
    /// `R32Uint`, cleared to [`abi::VIS_EMPTY`]; Tier 1 `vis_pack` ids.
    pub vis: wgpu::Texture,
    pub vis_view: wgpu::TextureView,
    /// `Depth32Float`, cleared to 0 (reversed-Z); Tier 1 depth.
    pub depth: wgpu::Texture,
    pub depth_view: wgpu::TextureView,
    /// Tier 2 result: `width * height` 64-bit keys `depth_bits << 32 | id`
    /// (row-major, 0 = empty). Read it as `array<vec2<u32>>` (x = id, y =
    /// depth bits) in either kernel mode.
    pub sw_keys: wgpu::Buffer,
    /// `[tile*2]` triangles, `[tile*2+1]` hits, per 16x16 tile (`u32`).
    pub tile_counters: wgpu::Buffer,
    /// `abi::stat` counters (`u32`).
    pub stats: wgpu::Buffer,
    /// Dense masks (one `u32` per tile), double-buffered.
    pub dense_masks: [wgpu::Buffer; 2],
    /// Compact list of dense tile indices (`u32`, `tile_count` capacity).
    pub dense_list: wgpu::Buffer,
    /// `[x, y, z, count]`: DispatchIndirect args for one workgroup per dense tile + count.
    pub dense_args: wgpu::Buffer,
    /// Per-tile triangle estimate of this frame's dense-bin clusters (`u32`).
    pub dense_est: wgpu::Buffer,
    /// Persistent per-tile latch age of the dense classification.
    pub dense_age: wgpu::Buffer,
    pub(crate) tile_bin_counts: wgpu::Buffer,
    pub(crate) tile_bin_entries: wgpu::Buffer,
    pub(crate) params: [wgpu::Buffer; 2],
    pub(crate) params_masked: [wgpu::Buffer; 2],
    pub(crate) params_dense: [wgpu::Buffer; 2],
    pub(crate) classify_params: wgpu::Buffer,
    parity: usize,
}

impl VgTargets {
    pub fn new(device: &wgpu::Device, width: u32, height: u32, raster: &VgRaster) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        let tiles_x = width.div_ceil(TILE_SIZE);
        let tiles_y = height.div_ceil(TILE_SIZE);
        let tiles = (tiles_x * tiles_y) as u64;
        let tex = |label, format, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let vis = tex(
            "VG Vis",
            VIS_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
        );
        let depth = tex(
            "VG Depth",
            DEPTH_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
        );
        let buf = |label, size: u64, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size.max(16),
                usage,
                mapped_at_creation: false,
            })
        };
        let st = wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC;
        let fallback = raster.uses_tile_fallback();
        let cap = raster.config.tile_bin_cap as u64;
        let make_params = || {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("VG Raster Params"),
                size: std::mem::size_of::<VgRasterParams>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let params = [make_params(), make_params()];
        let params_masked = [make_params(), make_params()];
        let params_dense = [make_params(), make_params()];
        let classify_params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("VG Classify Params"),
            contents: bytemuck::bytes_of(&ClassifyParams {
                tile_count: tiles as u32,
                tri_thresh: raster.config.dense_tri_threshold,
                hit_thresh: raster.config.dense_hit_threshold,
                hysteresis: raster.config.dense_hysteresis,
                max_dense: raster.config.dense_max_tiles,
                latch: raster.config.dense_latch_frames,
                pad: [0; 2],
            }),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        Self {
            width,
            height,
            tiles_x,
            tiles_y,
            vis_view: vis.create_view(&Default::default()),
            vis,
            depth_view: depth.create_view(&Default::default()),
            depth,
            sw_keys: buf("VG SW Keys", width as u64 * height as u64 * 8, st),
            tile_counters: buf("VG Tile Counters", tiles * 8, st),
            stats: buf("VG Raster Stats", abi::stat::COUNT as u64 * 4, st),
            dense_masks: [
                buf("VG Dense Mask A", tiles * 4, st),
                buf("VG Dense Mask B", tiles * 4, st),
            ],
            dense_list: buf("VG Dense List", tiles * 4, st),
            dense_args: buf("VG Dense Args", 16, st | wgpu::BufferUsages::INDIRECT),
            dense_est: buf("VG Dense Estimate", tiles * 4, st),
            dense_age: buf("VG Dense Age", tiles * 4, st),
            tile_bin_counts: buf(
                "VG Tile Bin Counts",
                if fallback { tiles * 4 } else { 16 },
                st,
            ),
            tile_bin_entries: buf(
                "VG Tile Bin Entries",
                if fallback { tiles * cap * 4 } else { 16 },
                st,
            ),
            params,
            params_masked,
            params_dense,
            classify_params,
            parity: 0,
        }
    }

    /// The dense mask classified in the previous `encode`: use it to route
    /// *this* frame's clusters (one-frame-late detection, plan 5.3). All zero
    /// before the first encode.
    pub fn dense_mask_last_frame(&self) -> &wgpu::Buffer {
        &self.dense_masks[self.parity ^ 1]
    }
}

fn storage_entry(
    binding: u32,
    vis: wgpu::ShaderStages,
    read_only: bool,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: vis,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}
fn uniform_entry(binding: u32, vis: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: vis,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}
fn depth_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Depth,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}
fn bgl(
    device: &wgpu::Device,
    label: &str,
    entries: &[wgpu::BindGroupLayoutEntry],
) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries,
    })
}
fn pipeline_layout(
    device: &wgpu::Device,
    label: &str,
    layouts: &[&wgpu::BindGroupLayout],
) -> wgpu::PipelineLayout {
    let layouts: Vec<Option<&wgpu::BindGroupLayout>> = layouts.iter().map(|l| Some(*l)).collect();
    device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &layouts,
        immediate_size: 0,
    })
}
fn module(device: &wgpu::Device, label: &str, src: String) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(src.into()),
    })
}
fn compute(
    device: &wgpu::Device,
    label: &str,
    layout: &wgpu::PipelineLayout,
    module: &wgpu::ShaderModule,
    entry: &str,
) -> wgpu::ComputePipeline {
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        module,
        entry_point: Some(entry),
        compilation_options: Default::default(),
        cache: None,
    })
}

struct Sw64 {
    pipeline: wgpu::ComputePipeline,
    bgl1: wgpu::BindGroupLayout,
}
struct TileFallback {
    bin: wgpu::ComputePipeline,
    bin_bgl1: wgpu::BindGroupLayout,
    raster: wgpu::ComputePipeline,
    raster_bgl1: wgpu::BindGroupLayout,
}

/// The raster pass: Tier 1 pipeline, Tier 2 kernels (64-bit atomic or tiled
/// fallback, chosen at construction), and tile classification.
pub struct VgRaster {
    pub config: VgRasterConfig,
    hw: wgpu::RenderPipeline,
    bgl0_vertex: wgpu::BindGroupLayout,
    bgl0_compute: wgpu::BindGroupLayout,
    sw64: Option<Sw64>,
    tile: Option<TileFallback>,
    classify: wgpu::ComputePipeline,
    classify_args: wgpu::ComputePipeline,
    classify_bgl: wgpu::BindGroupLayout,
    dense_count: wgpu::ComputePipeline,
    dense_count_bgl1: wgpu::BindGroupLayout,
    binding: helio_mats::MaterialBindingConfig,
    /// Built on first use: devices and scenes without alpha-tested materials
    /// never compile (or pay for) the masked pipelines.
    masked: std::sync::OnceLock<MaskedPipelines>,
}

struct MaskedPipelines {
    hw: wgpu::RenderPipeline,
    hw_bgl1: wgpu::BindGroupLayout,
    /// Software masked kernel (64-bit atomics only) and its alpha group (2).
    sw: Option<(wgpu::ComputePipeline, wgpu::BindGroupLayout)>,
}

impl VgRaster {
    /// Build every pipeline. 64-bit atomics are used iff the device was created
    /// with [`optional_features`] and `config.force_tile_fallback` is false.
    pub fn new(device: &wgpu::Device, config: VgRasterConfig) -> Self {
        let int64 = supports_int64_atomics(device.features()) && !config.force_tile_fallback;
        let v = wgpu::ShaderStages::VERTEX;
        let c = wgpu::ShaderStages::COMPUTE;
        let group0 = |vis| {
            let mut e: Vec<_> = (0..6).map(|b| storage_entry(b, vis, true)).collect();
            e.push(uniform_entry(6, vis));
            e
        };
        let bgl0_vertex = bgl(device, "VG Raster G0 (vertex)", &group0(v));
        let bgl0_compute = bgl(device, "VG Raster G0 (compute)", &group0(c));

        // Tier 1.
        let hw_mod = module(device, "VG HW Raster", shaders::hw_raster_source());
        let hw_layout = pipeline_layout(device, "VG HW Raster PL", &[&bgl0_vertex]);
        let hw = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("VG HW Raster"),
            layout: Some(&hw_layout),
            vertex: wgpu::VertexState {
                module: &hw_mod,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &hw_mod,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: VIS_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::RED,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: config.cull_back.then_some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(helio_core::depth::NEARER),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        // Tier 2.
        let (sw64, tile) = if int64 {
            let m = module(device, "VG SW Raster", shaders::sw_raster_source());
            let bgl1 = bgl(
                device,
                "VG SW Raster G1",
                &[
                    storage_entry(0, c, false),
                    depth_entry(1),
                    storage_entry(2, c, false),
                    storage_entry(3, c, false),
                    storage_entry(4, c, true),
                ],
            );
            let pl = pipeline_layout(device, "VG SW Raster PL", &[&bgl0_compute, &bgl1]);
            (
                Some(Sw64 {
                    pipeline: compute(device, "VG SW Raster", &pl, &m, "sw_raster_main"),
                    bgl1,
                }),
                None,
            )
        } else {
            let bm = module(device, "VG SW Bin", shaders::sw_bin_source());
            let bin_bgl1 = bgl(
                device,
                "VG SW Bin G1",
                &[
                    storage_entry(0, c, false),
                    storage_entry(1, c, false),
                    storage_entry(2, c, false),
                    storage_entry(3, c, true),
                ],
            );
            let bpl = pipeline_layout(device, "VG SW Bin PL", &[&bgl0_compute, &bin_bgl1]);
            let tm = module(device, "VG Tile Raster", shaders::tile_raster_source());
            let raster_bgl1 = bgl(
                device,
                "VG Tile Raster G1",
                &[
                    storage_entry(0, c, true),
                    storage_entry(1, c, true),
                    storage_entry(2, c, false),
                    depth_entry(3),
                    storage_entry(4, c, false),
                ],
            );
            let tpl = pipeline_layout(device, "VG Tile Raster PL", &[&bgl0_compute, &raster_bgl1]);
            (
                None,
                Some(TileFallback {
                    bin: compute(device, "VG SW Bin", &bpl, &bm, "sw_bin_main"),
                    bin_bgl1,
                    raster: compute(device, "VG Tile Raster", &tpl, &tm, "tile_raster_main"),
                    raster_bgl1,
                }),
            )
        };

        // Classification.
        let cm = module(device, "VG Tile Classify", shaders::tile_classify_source());
        let classify_bgl = bgl(
            device,
            "VG Classify G0",
            &[
                uniform_entry(0, c),
                storage_entry(1, c, true),
                storage_entry(2, c, true),
                storage_entry(3, c, false),
                storage_entry(4, c, false),
                storage_entry(5, c, false),
                storage_entry(6, c, true),
                storage_entry(7, c, false),
            ],
        );
        let cpl = pipeline_layout(device, "VG Classify PL", &[&classify_bgl]);
        let classify = compute(device, "VG Tile Classify", &cpl, &cm, "classify_main");
        let classify_args = compute(device, "VG Dense Args", &cpl, &cm, "dense_args_main");
        let dcm = module(device, "VG Dense Count", shaders::dense_count_source());
        let dense_count_bgl1 = bgl(
            device,
            "VG Dense Count G1",
            &[storage_entry(0, c, false), storage_entry(1, c, true)],
        );
        let dcpl = pipeline_layout(
            device,
            "VG Dense Count PL",
            &[&bgl0_compute, &dense_count_bgl1],
        );
        let dense_count = compute(device, "VG Dense Count", &dcpl, &dcm, "dense_count_main");

        Self {
            config,
            hw,
            bgl0_vertex,
            bgl0_compute,
            sw64,
            tile,
            classify,
            classify_args,
            classify_bgl,
            dense_count,
            dense_count_bgl1,
            binding: helio_mats::MaterialBindingConfig::for_device(device),
            masked: std::sync::OnceLock::new(),
        }
    }

    /// True when Tier 2 runs the tiled shared-memory fallback.
    pub fn uses_tile_fallback(&self) -> bool {
        self.tile.is_some()
    }

    /// True when the software masked kernel exists (64-bit atomics): the cull may
    /// then route masked software-class clusters to its software masked bin.
    pub fn supports_sw_masked(&self) -> bool {
        self.sw64.is_some()
    }

    fn masked_pipelines(&self, device: &wgpu::Device) -> &MaskedPipelines {
        self.masked.get_or_init(|| {
            let f = wgpu::ShaderStages::FRAGMENT;
            let c = wgpu::ShaderStages::COMPUTE;
            let hw_bgl1 = bgl(
                device,
                "VG HW Masked G1 (alpha)",
                &helio_mats::vg_alpha::layout_entries(self.binding, 0, f),
            );
            let hw_mod = module(
                device,
                "VG HW Raster Masked",
                shaders::hw_masked_source(self.binding),
            );
            let hw_layout = pipeline_layout(
                device,
                "VG HW Raster Masked PL",
                &[&self.bgl0_vertex, &hw_bgl1],
            );
            let hw = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("VG HW Raster Masked"),
                layout: Some(&hw_layout),
                vertex: wgpu::VertexState {
                    module: &hw_mod,
                    entry_point: Some("vs_masked"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &hw_mod,
                    entry_point: Some("fs_masked"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: VIS_FORMAT,
                        blend: None,
                        write_mask: wgpu::ColorWrites::RED,
                    })],
                }),
                // Two-sided foliage must keep its back faces: the fragment stage
                // discards them for single-sided materials instead.
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(helio_core::depth::NEARER),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            });
            let sw = self.sw64.as_ref().map(|sw64| {
                let alpha_bgl = bgl(
                    device,
                    "VG SW Masked G2 (alpha)",
                    &helio_mats::vg_alpha::layout_entries(self.binding, 0, c),
                );
                let m = module(
                    device,
                    "VG SW Raster Masked",
                    shaders::sw_masked_source(self.binding),
                );
                let pl = pipeline_layout(
                    device,
                    "VG SW Raster Masked PL",
                    &[&self.bgl0_compute, &sw64.bgl1, &alpha_bgl],
                );
                (
                    compute(
                        device,
                        "VG SW Raster Masked",
                        &pl,
                        &m,
                        "sw_raster_masked_main",
                    ),
                    alpha_bgl,
                )
            });
            MaskedPipelines { hw, hw_bgl1, sw }
        })
    }

    /// Record the whole raster for a single-phase frame: clears, Tier 1 draws,
    /// Tier 2, tile classify. Equivalent to [`Self::encode_phase`] for phase 0
    /// followed by [`Self::encode_classify`].
    ///
    /// `queue.write_buffer` is used for the small uniform block, so call this
    /// once per submission per `VgTargets`.
    pub fn encode(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        t: &mut VgTargets,
        i: &VgRasterInputs<'_>,
    ) {
        let mut rec = CommandRecorder::from_encoder(encoder);
        self.encode_phase(device, queue, &mut rec, t, i, 0);
        self.encode_classify(device, &mut rec, t);
    }

    /// Record one phase of a two-phase frame: Tier 1 draws then Tier 2.
    ///
    /// Phase 0 clears the targets and counters; phase 1 loads them and adds to
    /// them (the late delta of the cull). `i` carries that phase's indirect
    /// arguments and software `{first, count}` range. Call
    /// [`Self::encode_classify`] once after the last phase.
    pub fn encode_phase(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        rec: &mut CommandRecorder<'_>,
        t: &mut VgTargets,
        i: &VgRasterInputs<'_>,
        phase: u32,
    ) {
        let slot = (phase & 1) as usize;
        let params = VgRasterParams {
            width: t.width,
            height: t.height,
            tiles_x: t.tiles_x,
            tiles_y: t.tiles_y,
            flags: if self.config.cull_back {
                VgRasterParams::FLAG_CULL_BACK
            } else {
                0
            },
            tile_bin_cap: self.config.tile_bin_cap,
            view_index: i.view_index,
            sw_meta_word: i.sw_meta_offset_words,
        };
        queue.write_buffer(&t.params[slot], 0, bytemuck::bytes_of(&params));
        if let Some(mi) = &i.masked {
            let masked_params = VgRasterParams {
                sw_meta_word: mi.sw_meta_offset_words,
                ..params
            };
            queue.write_buffer(
                &t.params_masked[slot],
                0,
                bytemuck::bytes_of(&masked_params),
            );
        }

        if phase == 0 {
            for b in [
                &t.sw_keys,
                &t.tile_counters,
                &t.stats,
                &t.dense_args,
                &t.dense_est,
            ] {
                rec.clear_buffer(b, 0, None);
            }
        }
        // The fallback's per-tile lists are rebuilt by every phase.
        rec.clear_buffer(&t.tile_bin_counts, 0, None);

        let g0 = |layout: &wgpu::BindGroupLayout, params_buf: &wgpu::Buffer| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("VG Raster G0"),
                layout,
                entries: &[
                    bind(0, i.camera),
                    bind(1, i.pages),
                    bind(2, i.asset_quant),
                    bind(3, i.visible),
                    bind(4, i.vg_instances),
                    bind(5, i.objects),
                    bind(6, params_buf),
                ],
            })
        };

        // Tier 1.
        {
            let g0v = g0(&self.bgl0_vertex, &t.params[slot]);
            let masked_hw = i.masked.as_ref().map(|mi| {
                let m = self.masked_pipelines(device);
                let alpha = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("VG HW Masked alpha"),
                    layout: &m.hw_bgl1,
                    entries: &helio_mats::vg_alpha::bind_entries(
                        self.binding,
                        0,
                        mi.materials,
                        mi.material_textures,
                    ),
                });
                (m, alpha, mi.hw_indirect_offset)
            });
            let (color_load, depth_load) = if phase == 0 {
                (
                    wgpu::LoadOp::Clear(wgpu::Color {
                        r: abi::VIS_EMPTY as f64,
                        g: 0.0,
                        b: 0.0,
                        a: 0.0,
                    }),
                    wgpu::LoadOp::Clear(helio_core::depth::CLEAR),
                )
            } else {
                (wgpu::LoadOp::Load, wgpu::LoadOp::Load)
            };
            let mut pass = rec.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("VG HW Raster"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &t.vis_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: color_load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &t.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: depth_load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.hw);
            pass.set_bind_group(0, &g0v, &[]);
            for b in 0..abi::HW_BIN_CAPS.len() as u64 {
                pass.draw_indirect(i.hw_indirect, i.hw_indirect_offset + b * 16);
            }
            if let Some((m, alpha, offset)) = &masked_hw {
                pass.set_pipeline(&m.hw);
                pass.set_bind_group(0, &g0v, &[]);
                pass.set_bind_group(1, alpha, &[]);
                pass.draw_indirect(i.hw_indirect, *offset);
            }
        }

        // Tier 2.
        let g0c = g0(&self.bgl0_compute, &t.params[slot]);
        let depth_view = &t.depth_view;
        let mut cpass = rec.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("VG SW Raster"),
            timestamp_writes: None,
        });
        if let Some(sw) = &self.sw64 {
            let g1 = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("VG SW Raster G1"),
                layout: &sw.bgl1,
                entries: &[
                    bind(0, &t.sw_keys),
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(depth_view),
                    },
                    bind(2, &t.tile_counters),
                    bind(3, &t.stats),
                    bind(4, i.sw_meta),
                ],
            });
            cpass.set_pipeline(&sw.pipeline);
            cpass.set_bind_group(0, &g0c, &[]);
            cpass.set_bind_group(1, &g1, &[]);
            cpass.dispatch_workgroups_indirect(i.sw_dispatch, i.sw_dispatch_offset);
            if let Some(mi) = &i.masked {
                if let Some((pipeline, alpha_bgl)) = &self.masked_pipelines(device).sw {
                    let g0m = g0(&self.bgl0_compute, &t.params_masked[slot]);
                    let g1m = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("VG SW Masked G1"),
                        layout: &sw.bgl1,
                        entries: &[
                            bind(0, &t.sw_keys),
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::TextureView(depth_view),
                            },
                            bind(2, &t.tile_counters),
                            bind(3, &t.stats),
                            bind(4, i.sw_meta),
                        ],
                    });
                    let alpha = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("VG SW Masked alpha"),
                        layout: alpha_bgl,
                        entries: &helio_mats::vg_alpha::bind_entries(
                            self.binding,
                            0,
                            mi.materials,
                            mi.material_textures,
                        ),
                    });
                    cpass.set_pipeline(pipeline);
                    cpass.set_bind_group(0, &g0m, &[]);
                    cpass.set_bind_group(1, &g1m, &[]);
                    cpass.set_bind_group(2, &alpha, &[]);
                    cpass.dispatch_workgroups_indirect(i.sw_dispatch, mi.sw_dispatch_offset);
                }
            }
        }
        if let Some(d) = &i.dense {
            // Estimate the overlap of clusters diverted to Tier 3, so the tiles
            // they sit in keep counting as dense (see `vg_dense_count.wgsl`).
            let params_dense = VgRasterParams {
                sw_meta_word: d.meta_offset_words,
                ..params
            };
            queue.write_buffer(&t.params_dense[slot], 0, bytemuck::bytes_of(&params_dense));
            let g0d = g0(&self.bgl0_compute, &t.params_dense[slot]);
            let g1d = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("VG Dense Count G1"),
                layout: &self.dense_count_bgl1,
                entries: &[bind(0, &t.dense_est), bind(1, d.meta)],
            });
            cpass.set_pipeline(&self.dense_count);
            cpass.set_bind_group(0, &g0d, &[]);
            cpass.set_bind_group(1, &g1d, &[]);
            cpass.dispatch_workgroups_indirect(d.dispatch, d.dispatch_offset);
        }
        if let Some(tf) = &self.tile {
            let g1b = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("VG SW Bin G1"),
                layout: &tf.bin_bgl1,
                entries: &[
                    bind(0, &t.tile_bin_counts),
                    bind(1, &t.tile_bin_entries),
                    bind(2, &t.stats),
                    bind(3, i.sw_meta),
                ],
            });
            cpass.set_pipeline(&tf.bin);
            cpass.set_bind_group(0, &g0c, &[]);
            cpass.set_bind_group(1, &g1b, &[]);
            cpass.dispatch_workgroups_indirect(i.sw_dispatch, i.sw_dispatch_offset);

            let g1r = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("VG Tile Raster G1"),
                layout: &tf.raster_bgl1,
                entries: &[
                    bind(0, &t.tile_bin_counts),
                    bind(1, &t.tile_bin_entries),
                    bind(2, &t.sw_keys),
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(depth_view),
                    },
                    bind(4, &t.tile_counters),
                ],
            });
            cpass.set_pipeline(&tf.raster);
            cpass.set_bind_group(0, &g0c, &[]);
            cpass.set_bind_group(1, &g1r, &[]);
            cpass.dispatch_workgroups(t.tiles_x, t.tiles_y, 1);
        }
    }

    /// Record tile classification over the counters of every phase recorded so
    /// far, and flip the double-buffered dense mask. Afterwards
    /// [`VgTargets::dense_mask_last_frame`] is the mask just classified.
    pub fn encode_classify(
        &self,
        device: &wgpu::Device,
        rec: &mut CommandRecorder<'_>,
        t: &mut VgTargets,
    ) {
        let write = t.parity;
        let read = t.parity ^ 1;
        let gc = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("VG Classify G0"),
            layout: &self.classify_bgl,
            entries: &[
                bind(0, &t.classify_params),
                bind(1, &t.tile_counters),
                bind(2, &t.dense_masks[read]),
                bind(3, &t.dense_masks[write]),
                bind(4, &t.dense_list),
                bind(5, &t.dense_args),
                bind(6, &t.dense_est),
                bind(7, &t.dense_age),
            ],
        });
        let mut cpass = rec.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("VG Tile Classify"),
            timestamp_writes: None,
        });
        cpass.set_pipeline(&self.classify);
        cpass.set_bind_group(0, &gc, &[]);
        cpass.dispatch_workgroups((t.tiles_x * t.tiles_y).div_ceil(64), 1, 1);
        cpass.set_pipeline(&self.classify_args);
        cpass.dispatch_workgroups(1, 1, 1);
        drop(cpass);
        t.parity ^= 1;
    }
}

fn bind(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: buffer.as_entire_binding(),
    }
}
