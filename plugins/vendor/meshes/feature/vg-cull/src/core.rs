//! GPU resources and command recording of the cull. Independent of the render
//! graph so tests and offline tools can drive it with a plain encoder; the
//! passes in `pass.rs` are thin wrappers.

use std::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use bytemuck::{Pod, Zeroable};
use helio_core::CommandRecorder;
use helio_vg_format::VgMode;

use crate::{
    mode_bits, state, stats, FLAG_DENSE_MASK, FLAG_MASKED, FLAG_NO_ROOT_SHORTCUT, FLAG_STATS,
    FLAG_SW_MASKED, FLAG_TWO_PHASE, MAX_BIN_CAPACITY,
};

const ROUND_STRIDE: u64 = 256;
/// Queue slots: A, B, deferred. (A fourth cursor, the sink, has no storage.)
const QUEUES: u64 = 3;

/// Tunables. Defaults match the plan (tau 1 px, SW at <= 2 px edges).
#[derive(Clone, Debug)]
pub struct VgCullConfig {
    /// Screen-space error threshold in pixels.
    pub tau_px: f32,
    /// Projected average triangle edge at or below which a cluster is SW.
    pub sw_edge_px: f32,
    /// Target selected cluster triangles per viewport pixel. Selection responds
    /// by changing its error threshold, so complete parent clusters replace
    /// finer cuts instead of visible geometry being dropped.
    pub selection_triangles_per_pixel: f32,
    /// Log-space response strength for the asynchronous selection feedback.
    /// 1 by default (0 disables the screen-wide density control); the multiplier
    /// stays within 1x to 8x of the base tau (`helio_vg_format::density`).
    pub selection_sensitivity: f32,
    /// Triangles the density solve may predict this frame (set by the pass each
    /// frame from the target and the observed visible fraction; 0 disables it).
    pub density_budget_tris: f32,
    pub mode: VgMode,
    /// Last-frame-visible-first occlusion with a Hi-Z hook between phases.
    pub two_phase_occlusion: bool,
    /// Traversal rounds per phase. Round `r+1` only runs work that round `r`
    /// redistributed after bounded local traversal, or spilled from a full
    /// workgroup stack. The final round drains its remaining work locally.
    pub rounds: u32,
    /// Cap on persistent workgroups per round.
    pub max_groups: u32,
    /// Initial entries per bin. The default is the ceiling [`MAX_BIN_CAPACITY`]:
    /// a bin that overflows drops clusters for that frame, and growing only after
    /// an asynchronous stats readback means several frames of silently missing
    /// geometry (which changes with the camera: it reads as flicker) plus a
    /// rebuild of every cull buffer that resets the visibility history. 8 bins of
    /// 1M entries cost 64 MiB, so the ceiling is allocated up front. Tests lower it
    /// to exercise the growth path.
    pub initial_bin_capacity: u32,
    /// Initial entries per job queue (3 queues, 8 bytes per entry). Jobs past a
    /// queue are lost work, so this also starts large (24 MiB); it still grows
    /// like the bins if a frame ever fills it.
    pub initial_queue_capacity: u32,
    /// Bits of the visible-last-frame hash set (power of two).
    pub history_bits: u32,
    /// Maintain the stats counters (small atomic cost).
    pub stats: bool,
    /// Upper bound on entries per bin (a ceiling, never an allocation): the
    /// visible-cluster ceiling of the settings divided over the bins.
    pub max_bin_capacity: u32,
    /// Alpha-tested clusters of the software class may be routed to the software
    /// masked bin (the raster must have the 64-bit atomic kernel). Default false:
    /// they use the hardware masked bin.
    pub sw_masked: bool,
    /// Draw the root clusters of an instance whose whole-asset root error projects
    /// under tau directly, without walking its hierarchy (exactly the same cut).
    /// `HELIO_VG_NO_ROOT_SHORTCUT=1` turns it off for A/B measurements.
    pub root_shortcut: bool,
}

impl Default for VgCullConfig {
    fn default() -> Self {
        let selection_triangles_per_pixel = std::env::var("HELIO_VG_SELECTION_TRIANGLES_PER_PIXEL")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|v| v.is_finite())
            .unwrap_or(2.0)
            .clamp(0.25, 32.0);
        let selection_sensitivity = std::env::var("HELIO_VG_SELECTION_SENSITIVITY")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|v| v.is_finite())
            .unwrap_or(1.0)
            .clamp(0.0, 1.0);
        Self {
            tau_px: 1.0,
            sw_edge_px: 2.0,
            selection_triangles_per_pixel,
            selection_sensitivity,
            density_budget_tris: 0.0,
            mode: VgMode::Auto,
            two_phase_occlusion: true,
            rounds: 4,
            max_groups: 256,
            initial_bin_capacity: MAX_BIN_CAPACITY,
            initial_queue_capacity: 1 << 20,
            history_bits: 1 << 22,
            stats: true,
            max_bin_capacity: MAX_BIN_CAPACITY,
            sw_masked: false,
            root_shortcut: std::env::var_os("HELIO_VG_NO_ROOT_SHORTCUT").is_none(),
        }
    }
}

/// Everything the cull reads that other crates own. Cheap to copy; buffers are
/// borrowed for the frame. Binding contract (all `read-only storage` except
/// the stream's feedback):
///
/// | field | element |
/// |---|---|
/// | `instances` (`"vg_instances"`) | [`helio_vg_format::GpuVgInstance`] |
/// | `objects` (`"static_objects"`) | 252-byte (63-word) scene-join row; `transform` is read via `GpuVgInstance::object_row` |
/// | `stream` | the eight `helio-vg-stream` buffers (pool, residency, nodes, group refs, headers, bases, quant, feedback); the cull appends page requests / touches straight into `feedback` |
/// | `gcbvh_nodes`/`gcbvh_header`/`gcbvh_items` | `helio-pass-vg-bvh` outputs (`vg_gcbvh_*`) |
/// | `dense_mask` | one `u32` per 16x16 tile, row-major, nonzero = dense last frame |
#[derive(Clone, Copy)]
pub struct VgCullInputs<'a> {
    pub instances: &'a wgpu::Buffer,
    pub objects: &'a wgpu::Buffer,
    pub stream: helio_vg_stream::VgStreamBuffers<'a>,
    pub gcbvh_nodes: &'a wgpu::Buffer,
    pub gcbvh_header: &'a wgpu::Buffer,
    pub gcbvh_items: &'a wgpu::Buffer,
    pub dense_mask: Option<&'a wgpu::Buffer>,
    /// The scene's `materials` table (`GpuMaterial` rows, 24 words). `Some` turns on
    /// alpha-mask routing; `None` is the opaque-only path with no material loads.
    pub materials: Option<&'a wgpu::Buffer>,
    /// Rows of `instances` (the traversal ignores BVH leaves beyond it).
    pub instance_count: u32,
    /// Dense-mask grid `[tiles_x, tiles_y]`.
    pub dense_tiles: [u32; 2],
}

/// Per-frame values the graph knows.
#[derive(Clone, Copy, Debug)]
pub struct VgCullFrame {
    pub width: u32,
    pub height: u32,
}

/// Buffers sized for the current capacities. Replaced (new `Arc`) when they
/// grow; pass wrappers re-read it each frame.
pub struct VgCullBuffers {
    pub visible: wgpu::Buffer,
    pub state: wgpu::Buffer,
    pub queues: wgpu::Buffer,
    pub history: wgpu::Buffer,
    pub bin_capacity: u32,
    pub queue_capacity: u32,
    /// `u32` words per history half.
    pub history_words: u32,
}

impl VgCullBuffers {
    fn new(
        device: &wgpu::Device,
        bin_capacity: u32,
        queue_capacity: u32,
        history_words: u32,
    ) -> Self {
        let storage = wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_SRC
            | wgpu::BufferUsages::COPY_DST;
        let mk = |label: &str, bytes: u64, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: bytes.max(16),
                usage,
                mapped_at_creation: false,
            })
        };
        Self {
            visible: mk(
                "VgCull Visible",
                crate::BIN_COUNT as u64 * bin_capacity as u64 * 8,
                storage,
            ),
            state: mk(
                "VgCull State",
                state::WORDS as u64 * 4,
                storage | wgpu::BufferUsages::INDIRECT,
            ),
            queues: mk("VgCull Queues", QUEUES * queue_capacity as u64 * 8, storage),
            history: mk("VgCull History", 2 * history_words as u64 * 4, storage),
            bin_capacity,
            queue_capacity,
            history_words,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuParams {
    screen_width: u32,
    screen_height: u32,
    hiz_mip_count: u32,
    mode: u32,
    tau_px: f32,
    sw_edge_px: f32,
    instance_count: u32,
    bin_capacity: u32,
    queue_capacity: u32,
    history_bits_mask: u32,
    hist_read_base: u32,
    hist_write_base: u32,
    flags: u32,
    dense_tiles_x: u32,
    dense_tiles_y: u32,
    max_groups: u32,
    density_budget: f32,
    density_scale_max: f32,
    _pad: [u32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuRound {
    src: u32,
    dst: u32,
    phase: u32,
    hiz_valid: u32,
}

struct Readback {
    staging: wgpu::Buffer,
    ready: Arc<Mutex<Option<Vec<u32>>>>,
    copied: bool,
    in_flight: bool,
    revision: u64,
}

struct Pipelines {
    density: wgpu::ComputePipeline,
    seed: wgpu::ComputePipeline,
    prepare: wgpu::ComputePipeline,
    traverse: wgpu::ComputePipeline,
    finalize: wgpu::ComputePipeline,
}

type BindCache = Option<(
    Vec<wgpu::Buffer>,
    Option<wgpu::TextureView>,
    wgpu::BindGroup,
)>;

pub struct VgCullCore {
    pub config: VgCullConfig,
    pipelines: Pipelines,
    bgl0: wgpu::BindGroupLayout,
    bg1: wgpu::BindGroup,
    bg2: wgpu::BindGroup,
    params_buf: wgpu::Buffer,
    round_buf: wgpu::Buffer,
    dispatch_args: wgpu::Buffer,
    sampler: wgpu::Sampler,
    dummy_view: wgpu::TextureView,
    dummy_buf: wgpu::Buffer,
    buffers: Arc<VgCullBuffers>,
    bind_cache: [BindCache; 2],
    frame: u64,
    readback: Readback,
    last_stats: Option<Vec<u32>>,
    pending_capacity_stats: Option<Vec<u32>>,
    size: (u32, u32),
    prepared: bool,
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

pub fn mip_levels(w: u32, h: u32) -> u32 {
    (u32::BITS - w.max(h).max(1).leading_zeros()).max(1)
}

impl VgCullCore {
    pub fn new(device: &wgpu::Device, config: VgCullConfig) -> Self {
        let source = crate::shader_source();
        let module = helio_core::shader::module(device, "VgCull", source.as_str());

        let mut entries0 = vec![
            storage_entry(0, true),
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ];
        // 2..=3 instances/objects, 4..=10 stream (read-only), 11 stream feedback.
        for b in 2..=10 {
            entries0.push(storage_entry(b, true));
        }
        entries0.push(storage_entry(11, false));
        // 12..=15 gcbvh nodes/header/items, dense mask.
        for b in 12..=15 {
            entries0.push(storage_entry(b, true));
        }
        entries0.push(wgpu::BindGroupLayoutEntry {
            binding: 16,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        });
        entries0.push(wgpu::BindGroupLayoutEntry {
            binding: 17,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
            count: None,
        });
        for b in 18..=21 {
            entries0.push(storage_entry(b, false));
        }
        entries0.push(storage_entry(22, true));
        let bgl0 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("VgCull BGL0"),
            entries: &entries0,
        });
        let bgl1 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("VgCull BGL1 (round)"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: NonZeroU64::new(std::mem::size_of::<GpuRound>() as u64),
                },
                count: None,
            }],
        });
        let bgl2 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("VgCull BGL2 (dispatch args)"),
            entries: &[storage_entry(0, false)],
        });

        let pl2 = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("VgCull PL"),
            bind_group_layouts: &[Some(&bgl0), Some(&bgl1)],
            immediate_size: 0,
        });
        let pl3 = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("VgCull PL prepare"),
            bind_group_layouts: &[Some(&bgl0), Some(&bgl1), Some(&bgl2)],
            immediate_size: 0,
        });
        let pipeline = |label: &str, layout: &wgpu::PipelineLayout, entry: &str| {
            helio_core::shader::create_compute_pipeline(
                device,
                wgpu::ComputePipelineDescriptor {
                    label: Some(label),
                    layout: Some(layout),
                    module: &module,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    cache: None,
                },
            )
        };
        let pipelines = Pipelines {
            density: pipeline("VgCull Density", &pl2, "cs_density"),
            seed: pipeline("VgCull Seed", &pl2, "cs_seed"),
            prepare: pipeline("VgCull RoundPrepare", &pl3, "cs_round_prepare"),
            traverse: pipeline("VgCull Traverse", &pl2, "cs_traverse"),
            finalize: pipeline("VgCull Finalize", &pl2, "cs_finalize"),
        };

        let params_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("VgCull Params"),
            size: std::mem::size_of::<GpuParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let round_blocks = 2 * config.rounds.max(1) as u64 + 2;
        let round_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("VgCull Rounds"),
            size: round_blocks * ROUND_STRIDE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let dispatch_args = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("VgCull DispatchArgs"),
            size: 16,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::INDIRECT
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let bg1 = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("VgCull BG1"),
            layout: &bgl1,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &round_buf,
                    offset: 0,
                    size: NonZeroU64::new(std::mem::size_of::<GpuRound>() as u64),
                }),
            }],
        });
        let bg2 = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("VgCull BG2"),
            layout: &bgl2,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: dispatch_args.as_entire_binding(),
            }],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("VgCull HiZ Sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let dummy_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("VgCull Dummy HiZ"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let dummy_view = dummy_tex.create_view(&Default::default());
        let dummy_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("VgCull Dummy Buffer"),
            size: 16,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let history_words = (config.history_bits.next_power_of_two().max(1024)) / 32;
        let buffers = Arc::new(VgCullBuffers::new(
            device,
            config.initial_bin_capacity.min(MAX_BIN_CAPACITY).max(16),
            config.initial_queue_capacity.max(256),
            history_words,
        ));
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("VgCull Stats Readback"),
            size: state::WORDS as u64 * 4,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            config,
            pipelines,
            bgl0,
            bg1,
            bg2,
            params_buf,
            round_buf,
            dispatch_args,
            sampler,
            dummy_view,
            dummy_buf,
            buffers,
            bind_cache: [None, None],
            frame: 0,
            readback: Readback {
                staging,
                ready: Arc::new(Mutex::new(None)),
                copied: false,
                in_flight: false,
                revision: 0,
            },
            last_stats: None,
            pending_capacity_stats: None,
            size: (1, 1),
            prepared: false,
        }
    }

    pub fn buffers(&self) -> &Arc<VgCullBuffers> {
        &self.buffers
    }

    /// Frames begun so far (phase-0 hi-z is trusted from the second).
    pub fn frame_index(&self) -> u64 {
        self.frame
    }

    /// The state words (bins, stats, ...) of the most recent completed
    /// read-back, one to a few frames old.
    pub fn last_state_words(&self) -> Option<&[u32]> {
        self.last_stats.as_deref()
    }

    /// Increments when a new asynchronous GPU stats sample becomes available.
    pub fn stats_revision(&self) -> u64 {
        self.readback.revision
    }

    /// Whether `begin_frame` ran this frame (the late pass skips otherwise).
    pub fn prepared(&self) -> bool {
        self.prepared
    }

    /// Start of frame: finish any stats read-back (growing capacities), size
    /// advance the history
    /// parity and upload the frame constants.
    pub fn begin_frame(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: &VgCullFrame,
        inputs: &VgCullInputs<'_>,
    ) {
        self.poll_readback();
        let b = &self.buffers;
        let mut bin_cap = b.bin_capacity;
        let mut queue_cap = b.queue_capacity;
        if let Some(words) = self.pending_capacity_stats.take() {
            let st = |i: u32| words[(state::STATS + i) as usize];
            let max_bin = (0..crate::BIN_COUNT)
                .map(|i| words[i as usize])
                .max()
                .unwrap_or(0);
            if st(stats::VISIBLE_OVERFLOW) > 0 || max_bin > bin_cap / 4 * 3 {
                bin_cap = (max_bin.max(bin_cap) * 2)
                    .next_power_of_two()
                    .min(self.config.max_bin_capacity.clamp(16, MAX_BIN_CAPACITY));
            }
            if st(stats::JOB_OVERFLOW) > 0 || st(stats::PEAK_QUEUE) > queue_cap / 4 * 3 {
                queue_cap = (queue_cap * 2).min(1 << 24);
            }
            if std::env::var_os("VG_DEBUG").is_some() {
                eprintln!(
                    "VgCull stats: bins {:?} jobs {} tested {} emitted {} deferred {} culled(frustum {} occl {} cone {} sse {}) page_req {} overflow(job {} vis {})",
                    &words[..crate::BIN_COUNT as usize],
                    words[(state::STATS + stats::JOBS) as usize],
                    words[(state::STATS + stats::CLUSTERS_TESTED) as usize],
                    words[(state::STATS + stats::EMITTED) as usize],
                    words[(state::STATS + stats::DEFERRED) as usize],
                    words[(state::STATS + stats::CULLED_FRUSTUM) as usize],
                    words[(state::STATS + stats::CULLED_OCCLUSION) as usize],
                    words[(state::STATS + stats::CULLED_CONE) as usize],
                    words[(state::STATS + stats::CULLED_SSE) as usize],
                    words[(state::STATS + stats::PAGE_REQUESTS) as usize],
                    words[(state::STATS + stats::JOB_OVERFLOW) as usize],
                    words[(state::STATS + stats::VISIBLE_OVERFLOW) as usize],
                );
            }
            self.last_stats = Some(words);
        }
        if bin_cap != b.bin_capacity || queue_cap != b.queue_capacity {
            self.buffers = Arc::new(VgCullBuffers::new(
                device,
                bin_cap,
                queue_cap,
                b.history_words,
            ));
            self.bind_cache = [None, None];
        }
        let b = &self.buffers;
        self.frame += 1;
        self.size = (frame.width.max(1), frame.height.max(1));
        let parity = (self.frame & 1) as u32;
        let mut flags = 0;
        if self.config.two_phase_occlusion {
            flags |= FLAG_TWO_PHASE;
        }
        if inputs.dense_mask.is_some() {
            flags |= FLAG_DENSE_MASK;
        }
        if self.config.stats {
            flags |= FLAG_STATS;
        }
        if !self.config.root_shortcut {
            flags |= FLAG_NO_ROOT_SHORTCUT;
        }
        if inputs.materials.is_some() {
            flags |= FLAG_MASKED;
            if self.config.sw_masked {
                flags |= FLAG_SW_MASKED;
            }
        }
        let params = GpuParams {
            screen_width: self.size.0,
            screen_height: self.size.1,
            hiz_mip_count: mip_levels(self.size.0, self.size.1),
            mode: mode_bits(self.config.mode),
            tau_px: self.config.tau_px,
            sw_edge_px: self.config.sw_edge_px,
            instance_count: inputs.instance_count,
            bin_capacity: b.bin_capacity,
            queue_capacity: b.queue_capacity,
            history_bits_mask: b.history_words * 32 - 1,
            hist_read_base: (1 - parity) * b.history_words,
            hist_write_base: parity * b.history_words,
            flags,
            dense_tiles_x: inputs.dense_tiles[0].max(1),
            dense_tiles_y: inputs.dense_tiles[1].max(1),
            max_groups: self.config.max_groups.max(1),
            density_budget: self.config.density_budget_tris.max(0.0),
            density_scale_max: helio_vg_format::density::MAX_SCALE,
            _pad: [0; 2],
        };
        helio_core::upload::write_buffer(queue, &self.params_buf, 0, bytemuck::bytes_of(&params));
        self.prepared = true;
    }

    fn poll_readback(&mut self) {
        let rb = &mut self.readback;
        if rb.copied && !rb.in_flight {
            let ready = rb.ready.clone();
            let buf = rb.staging.clone();
            rb.staging
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |r| {
                    if r.is_ok() {
                        let words: Vec<u32> = {
                            let Ok(view) = buf.slice(..).get_mapped_range() else {
                                return;
                            };
                            bytemuck::cast_slice::<u8, u32>(&view).to_vec()
                        };
                        *ready.lock().unwrap() = Some(words);
                    }
                });
            rb.in_flight = true;
            rb.copied = false;
        }
        if rb.in_flight {
            if let Some(words) = rb.ready.lock().unwrap().take() {
                rb.staging.unmap();
                rb.in_flight = false;
                rb.revision = rb.revision.wrapping_add(1);
                self.pending_capacity_stats = Some(words.clone());
                self.last_stats = Some(words);
            }
        }
    }

    /// Records one phase (0 = early, 1 = late). `hiz` is that phase's Hi-Z
    /// (previous frame for 0, the phase-0 build for 1); `None` skips occlusion
    /// for the phase.
    pub fn record_phase(
        &mut self,
        rec: &mut CommandRecorder<'_>,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        camera: &wgpu::Buffer,
        inputs: &VgCullInputs<'_>,
        hiz: Option<&wgpu::TextureView>,
        phase: u32,
    ) {
        let b = self.buffers.clone();
        let rounds = self.config.rounds.max(1);
        if phase == 0 {
            // Everything but the density multiplier, which the next solve slews from.
            rec.clear_buffer(&b.state, 0, Some(state::DENSITY as u64 * 4));
            rec.clear_buffer(&b.state, (state::DENSITY as u64 + 1) * 4, None);
            let parity = (self.frame & 1) as u64;
            rec.clear_buffer(
                &b.history,
                parity * b.history_words as u64 * 4,
                Some(b.history_words as u64 * 4),
            );
        }
        // Phase 0 trusts last frame's Hi-Z only from the second frame on
        // (frame 1 has no previous depth). Stale-but-valid is still safe.
        let hiz_valid =
            self.config.two_phase_occlusion && hiz.is_some() && (phase == 1 || self.frame >= 2);

        for r in 0..rounds {
            let (src, mut dst) = if phase == 0 {
                if r % 2 == 0 {
                    (0, 1)
                } else {
                    (1, 0)
                }
            } else if r == 0 {
                (2, 0)
            } else {
                let s = (r - 1) % 2;
                (s, 1 - s)
            };
            if r == rounds - 1 {
                dst = 3;
            }
            let block = phase as u64 * rounds as u64 + r as u64;
            let round = GpuRound {
                src,
                dst,
                phase,
                hiz_valid: hiz_valid as u32,
            };
            queue.write_buffer(
                &self.round_buf,
                block * ROUND_STRIDE,
                bytemuck::bytes_of(&round),
            );
        }
        let fin_block = 2 * rounds as u64 + phase as u64;
        let fin = GpuRound {
            src: 0,
            dst: 0,
            phase,
            hiz_valid: 0,
        };
        queue.write_buffer(
            &self.round_buf,
            fin_block * ROUND_STRIDE,
            bytemuck::bytes_of(&fin),
        );

        let bg0 = self.bind_group0(device, camera, inputs, hiz, phase as usize, &b);
        let mut pass = rec.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some(if phase == 0 {
                "VgCull Early"
            } else {
                "VgCull Late"
            }),
            timestamp_writes: None,
        });
        pass.set_bind_group(0, &bg0, &[]);
        if phase == 0 {
            pass.set_bind_group(1, &self.bg1, &[0]);
            pass.set_pipeline(&self.pipelines.density);
            pass.dispatch_workgroups(1, 1, 1);
            pass.set_pipeline(&self.pipelines.seed);
            pass.dispatch_workgroups(1, 1, 1);
        }
        for r in 0..rounds {
            let block = phase as u64 * rounds as u64 + r as u64;
            pass.set_bind_group(1, &self.bg1, &[(block * ROUND_STRIDE) as u32]);
            pass.set_pipeline(&self.pipelines.prepare);
            pass.set_bind_group(2, &self.bg2, &[]);
            pass.dispatch_workgroups(1, 1, 1);
            pass.set_pipeline(&self.pipelines.traverse);
            pass.dispatch_workgroups_indirect(&self.dispatch_args, 0);
        }
        pass.set_bind_group(1, &self.bg1, &[(fin_block * ROUND_STRIDE) as u32]);
        pass.set_pipeline(&self.pipelines.finalize);
        pass.dispatch_workgroups(1, 1, 1);
        drop(pass);

        let last = if self.config.two_phase_occlusion {
            1
        } else {
            0
        };
        if phase == last && !self.readback.copied && !self.readback.in_flight {
            rec.copy_buffer_to_buffer(
                &b.state,
                0,
                &self.readback.staging,
                0,
                state::WORDS as u64 * 4,
            );
            self.readback.copied = true;
        }
    }

    fn bind_group0(
        &mut self,
        device: &wgpu::Device,
        camera: &wgpu::Buffer,
        i: &VgCullInputs<'_>,
        hiz: Option<&wgpu::TextureView>,
        slot: usize,
        b: &VgCullBuffers,
    ) -> wgpu::BindGroup {
        let dense = i.dense_mask.unwrap_or(&self.dummy_buf);
        let materials = i.materials.unwrap_or(&self.dummy_buf);
        let mut bufs: Vec<&wgpu::Buffer> = vec![camera, &self.params_buf, i.instances, i.objects];
        bufs.extend(i.stream.as_array());
        bufs.extend([
            i.gcbvh_nodes,
            i.gcbvh_header,
            i.gcbvh_items,
            dense,
            &b.state,
            &b.queues,
            &b.visible,
            &b.history,
        ]);
        bufs.push(materials);
        let key: Vec<wgpu::Buffer> = bufs.iter().map(|b| (*b).clone()).collect();
        if let Some((k, v, bg)) = &self.bind_cache[slot] {
            if *k == key && v.as_ref() == hiz {
                return bg.clone();
            }
        }
        let view = hiz.unwrap_or(&self.dummy_view);
        let mut entries: Vec<wgpu::BindGroupEntry<'_>> = Vec::new();
        // bufs[0..12] = bindings 0..=11, bufs[12..16] = 12..=15, then texture (16),
        // sampler (17), then bufs[16..20] = 18..=21.
        for (n, buf) in bufs.iter().enumerate().take(16) {
            entries.push(wgpu::BindGroupEntry {
                binding: n as u32,
                resource: buf.as_entire_binding(),
            });
        }
        entries.push(wgpu::BindGroupEntry {
            binding: 16,
            resource: wgpu::BindingResource::TextureView(view),
        });
        entries.push(wgpu::BindGroupEntry {
            binding: 17,
            resource: wgpu::BindingResource::Sampler(&self.sampler),
        });
        for (n, buf) in bufs.iter().enumerate().skip(16).take(4) {
            entries.push(wgpu::BindGroupEntry {
                binding: n as u32 + 2,
                resource: buf.as_entire_binding(),
            });
        }
        entries.push(wgpu::BindGroupEntry {
            binding: 22,
            resource: bufs[20].as_entire_binding(),
        });
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("VgCull BG0"),
            layout: &self.bgl0,
            entries: &entries,
        });
        self.bind_cache[slot] = Some((key, hiz.cloned(), bg.clone()));
        bg
    }
}
