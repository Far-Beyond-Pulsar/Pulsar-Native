//! Corona — fully GPU-native particle system.
//!
//! Per-frame GPU pipeline:
//!   0. Layout       — the end of the pool's used range → indirect dispatch args
//!   1. Simulate     — physics + aging; kills particles whose emitter lost their slot
//!   2. Emit         — ring-buffer spawn (tags particle.velocity.w with emitter row + epoch)
//!   3. ScanLocal    — prefix scan per 256-block (Hillis-Steele) + sort-key reset
//!   4. ScanBlocks   — sequential cumulative sum per emitter; writes emitter_alive
//!   5. Scatter      — scatter alive indices into compact_buf + depth to sort_key_buf
//!   6. BuildMulti   — write one DrawArgs per emitter
//!   copy_buffer_to_buffer: draw_args_staging → draw_args_buf
//!   7+. Sort        — bitonic sort (descending) per emitter for back-to-front order
//!   8.  Render      — one draw_indirect per emitter; atlas sprite from emitter.texture_index
//!
//! # The particle pool
//!
//! Every emitter draws from one shared pool of [`CORONA_MAX_PARTICLES`]
//! particles: its row's `particle_offset`/`particle_count` are a contiguous
//! range of it (Pulsar-Native#1059). In the engine the environment join
//! allocates them, packing placed emitters into the leading rows and giving
//! each a range sized by its requested `max_particles` (a GPU prefix sum,
//! clamped when the pool is full). The pass trusts no CPU copy of them: the
//! shaders clamp each range to the pool and treat one not starting on a
//! 256-particle boundary as empty (a scan block must hold one emitter's
//! particles), `cs_layout` finds the end of the used range every frame, and
//! the particle-wide passes dispatch indirectly over just that range. A
//! particle remembers its emitter row and the row's epoch; a row whose
//! emitter (the row's `spawn_cursor` word, which the join sets to the
//! emitter's identity) or range changes starts a new epoch, and the
//! particles of an old one die. The spawn cursor stays pass-owned GPU state
//! (`spawn_cursor_buf`).

pub mod feature;
pub mod scene_rows;

pub use feature::CoronaFeature;

use bytemuck::{Pod, Zeroable};
use helio_core::graph::ResourceBuilder;
use helio_core::{PassContext, PrepareContext, RenderPass, Result as HelioResult};
use pulsar_scenedb::gpu::BufferKey;

pub mod components;
pub mod gpu_types;
pub use components::CoronaEmitterRow;
pub use components::CORONA_EMITTERS;
pub use gpu_types::*;

// ── Constants ────────────────────────────────────────────────────────────────

const DEFAULT_MAX_PARTICLES: u32 = crate::CORONA_MAX_PARTICLES;
/// Emitter rows the pass reads (see the module doc's pool).
const MAX_EMITTERS: u32 = crate::CORONA_MAX_EMITTERS;
const WG: u32 = 256;
// corona.wgsl's `WG` is the range alignment the join allocates with.
const _: () = assert!(WG == crate::CORONA_RANGE_ALIGNMENT);
const ATLAS_SIZE: u32 = 128; // 128×128 atlas, 4×4 cells of 32×32 each
const ATLAS_CELLS: u32 = 4; // cells per row/column
const _CELL_SIZE: u32 = ATLAS_SIZE / ATLAS_CELLS; // 32

// ── CPU structs ──────────────────────────────────────────────────────────────

/// Matches GpuCoronaUniforms in corona.wgsl (12 × u32/f32 = 48 bytes).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CoronaUniforms {
    delta_time: f32,
    total_particles: u32,
    emitter_count: u32,
    frame_count: u32,
    // Written per sort-dispatch via copy_buffer_to_buffer from sort_steps_buf.
    sort_k: u32,
    sort_j: u32,
    sort_lo: u32,
    sort_n: u32,
    /// How far the frame's world origin moved since the live particles were
    /// last simulated (`xyz`). Particles keep positions across frames in
    /// the frame's space, so the simulation subtracts it to stay put.
    origin_shift: [f32; 4],
}

/// One entry in sort_steps_buf (16 bytes). Pre-built at prepare() time.
/// Copied into uniforms[16..32] before each sort dispatch.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SortStep {
    k: u32,
    j: u32,
    lo: u32,
    n: u32,
}

// ── Pass ─────────────────────────────────────────────────────────────────────

pub struct CoronaPass {
    // ── Pipelines ────────────────────────────────────────────────────────────
    simulate_pipeline: wgpu::ComputePipeline,
    emit_pipeline: wgpu::ComputePipeline,
    layout_pipeline: wgpu::ComputePipeline,
    scan_local_pipeline: wgpu::ComputePipeline,
    scan_blocks_pipeline: wgpu::ComputePipeline,
    scatter_pipeline: wgpu::ComputePipeline,
    build_multi_pipeline: wgpu::ComputePipeline,
    sort_local_pipeline: wgpu::ComputePipeline,
    sort_global_pipeline: wgpu::ComputePipeline,
    sort_local_merge_pipeline: wgpu::ComputePipeline,
    render_pipeline: wgpu::RenderPipeline,

    // Compute writes particle storage, while the vertex stage may only read it
    // on WebGPU. Keep compatible layouts rather than exposing writable storage
    // to the vertex stage.
    compute_bgl: wgpu::BindGroupLayout,
    render_bgl: wgpu::BindGroupLayout,

    // ── GPU buffers ──────────────────────────────────────────────────────────
    uniform_buf: wgpu::Buffer, // CoronaUniforms (32 bytes, UNIFORM | COPY_DST)
    particle_buf: wgpu::Buffer,
    /// Fallback buffer, bound only until `"corona_emitters"` exists (before
    /// any `CoronaEmitterRow` has ever been inserted) — never written
    /// to otherwise; SceneDB's own buffer is bound directly once it exists
    /// (see `execute()`), read_write, so the compute shader's own per-frame
    /// `spawn_cursor` advance persists in place across frames with no CPU
    /// involvement at all.
    emitter_buf: wgpu::Buffer,
    /// Per-emitter-row spawn cursor and restart epoch — purely transient,
    /// pass-owned GPU state, deliberately separate from the emitter rows.
    /// See its creation site in `new()` for why.
    spawn_cursor_buf: wgpu::Buffer,
    compact_buf: wgpu::Buffer,
    emitter_alive_buf: wgpu::Buffer, // non-atomic u32 per emitter
    draw_args_staging: wgpu::Buffer, // STORAGE | COPY_SRC
    draw_args_buf: wgpu::Buffer,     // INDIRECT | COPY_DST
    prefix_buf: wgpu::Buffer,        // u32 per particle slot
    block_sums_buf: wgpu::Buffer,    // u32 per 256-block
    sort_key_buf: wgpu::Buffer,      // f32 per particle slot
    // Pre-built sort steps; 16 bytes per step, STORAGE | COPY_SRC.
    // Entries are copied into uniform_buf[16..32] before each sort dispatch.
    sort_steps_buf: wgpu::Buffer,
    /// `cs_layout`'s output: the particle-wide passes' workgroups (x, 1, 1)
    /// and the end of the pool's used range (STORAGE | COPY_SRC).
    pool_layout_buf: wgpu::Buffer,
    /// The first 12 bytes of `pool_layout_buf`, as indirect dispatch args.
    pool_dispatch_buf: wgpu::Buffer,

    // ── Particle texture (4×4 atlas, 128×128) ────────────────────────────────
    _particle_tex: wgpu::Texture,
    particle_view: wgpu::TextureView,
    particle_sampler: wgpu::Sampler,

    // ── Bind groups (rebuilt when camera, particle or emitter buffer changes) ─
    compute_bg: Option<wgpu::BindGroup>,
    render_bg: Option<wgpu::BindGroup>,
    /// The particle slots `vs_main` indexes `compact_buf` with
    /// (`helio_pass_gbuffer::draw_slots`).
    draw_slots: wgpu::Buffer,
    bg_key: Option<[wgpu::Buffer; 3]>, // (particle_buf, emitter_buf, camera_buf)

    // ── State ────────────────────────────────────────────────────────────────
    max_particles: u32,
    emitter_count: u32,
    #[allow(dead_code)]
    max_sort_steps: u32, // capacity of sort_steps_buf in step count
    /// The world origin the live particles' positions are relative to (zero
    /// for world space), and how far the frame's origin moved from it that
    /// no simulation step has applied yet. Moves accumulate while the pass
    /// is idle (no emitters) so a particle never misses one.
    particles_origin: glam::DVec3,
    pending_shift: glam::DVec3,

    // Sort dispatches by particle range. Empty: the ranges are allocated on
    // the GPU (see the module doc), so the CPU has none to build them from.
    sort_steps: Vec<SortStep>,
    /// Enable per-emitter back-to-front depth sort before rendering. With
    /// GPU-allocated ranges there are no CPU-built sort steps, so this has
    /// no effect until the steps are generated on the GPU too.
    pub depth_sort_enabled: bool,
}

/// The particle pool and the per-slot buffers sized with it.
struct ParticlePool {
    particle_buf: wgpu::Buffer,
    compact_buf: wgpu::Buffer,
    prefix_buf: wgpu::Buffer,
    block_sums_buf: wgpu::Buffer,
    sort_key_buf: wgpu::Buffer,
    draw_slots: wgpu::Buffer,
}

impl ParticlePool {
    fn new(device: &wgpu::Device, capacity: u32) -> Self {
        let per_slot = |label, bytes: u64| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: capacity as u64 * bytes,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            })
        };
        let particle_size = std::mem::size_of::<crate::GpuCoronaParticle>() as u64;
        Self {
            particle_buf: per_slot("Corona Particles", particle_size),
            compact_buf: per_slot("Corona Compact", 4),
            prefix_buf: per_slot("Corona Prefix", 4),
            block_sums_buf: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Corona BlockSums"),
                size: capacity.div_ceil(WG) as u64 * 4,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            }),
            sort_key_buf: per_slot("Corona SortKeys", 4),
            draw_slots: helio_pass_gbuffer::DrawSlots::default()
                .buffer(device, capacity)
                .clone(),
        }
    }
}

impl CoronaPass {
    /// Replaces the particle pool with one of `capacity` slots, empty. The
    /// bind groups are rebuilt on the next execute (their key holds the pool).
    fn set_pool_capacity(&mut self, device: &wgpu::Device, capacity: u32) {
        let pool = ParticlePool::new(device, capacity);
        self.particle_buf = pool.particle_buf;
        self.compact_buf = pool.compact_buf;
        self.prefix_buf = pool.prefix_buf;
        self.block_sums_buf = pool.block_sums_buf;
        self.sort_key_buf = pool.sort_key_buf;
        self.draw_slots = pool.draw_slots;
        self.max_particles = capacity;
        self.bg_key = None;
    }

    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        camera_buf: &wgpu::Buffer,
        surface_format: wgpu::TextureFormat,
    ) -> Self {
        let compute_shader = helio_core::shader::module(
            device,
            "Corona Compute Shader",
            helio_core::include_wgsl!("../shaders/corona.wgsl"),
        );
        let render_shader = helio_core::shader::module(
            device,
            "Corona Render Shader",
            helio_core::include_wgsl!("../shaders/corona_render.wgsl"),
        );

        // ── Buffers ──────────────────────────────────────────────────────────

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Corona Uniforms"),
            size: std::mem::size_of::<CoronaUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // A one-slot pool until the scene has emitters (see `is_member`).
        let ParticlePool {
            particle_buf,
            compact_buf,
            prefix_buf,
            block_sums_buf,
            sort_key_buf,
            draw_slots,
        } = ParticlePool::new(device, 1);

        let emitter_size = std::mem::size_of::<crate::GpuCoronaEmitter>() as u64;
        let emitter_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Corona Emitters"),
            size: emitter_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let pool_layout_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Corona Pool Layout"),
            size: 16,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let pool_dispatch_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Corona Pool Dispatch"),
            size: 12,
            usage: wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Per-emitter-row spawn cursor and restart state (`EmitterState` in
        // corona.wgsl, 8 words): purely transient, pass-owned GPU state that
        // `cs_layout`/`cs_emit` read and advance in place every frame, kept
        // OUT of the emitter rows deliberately -- those are re-written
        // whenever their source changes (an emitter moves, a property is
        // edited), and a cursor living in the same row would be stomped back
        // to a stale value each time, restarting the emission ring.
        // Zero-initialized once; nothing but the shaders touch it again.
        let spawn_cursor_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Corona Emitter State"),
            size: MAX_EMITTERS as u64 * 32,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(
            &spawn_cursor_buf,
            0,
            bytemuck::cast_slice(&vec![0u32; MAX_EMITTERS as usize * 8]),
        );

        let emitter_alive_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Corona Emitter Alive"),
            size: MAX_EMITTERS as u64 * 4,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });

        let draw_args_size =
            MAX_EMITTERS as u64 * std::mem::size_of::<crate::GpuCoronaDrawIndirect>() as u64;
        let draw_args_staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Corona DrawArgs Staging"),
            size: draw_args_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let draw_args_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Corona DrawArgs"),
            size: draw_args_size,
            usage: wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Initial sort_steps_buf capacity.
        let initial_sort_cap = 256u32;
        let sort_steps_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Corona SortSteps"),
            size: initial_sort_cap as u64 * std::mem::size_of::<SortStep>() as u64,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // ── 4×4 sprite atlas ─────────────────────────────────────────────────

        let tex_data = Self::make_atlas(ATLAS_SIZE, ATLAS_CELLS);
        let particle_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Corona Atlas"),
            size: wgpu::Extent3d {
                width: ATLAS_SIZE,
                height: ATLAS_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &particle_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &tex_data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(ATLAS_SIZE * 4),
                rows_per_image: Some(ATLAS_SIZE),
            },
            wgpu::Extent3d {
                width: ATLAS_SIZE,
                height: ATLAS_SIZE,
                depth_or_array_layers: 1,
            },
        );
        let particle_view = particle_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let particle_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Corona Sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });

        let compute_bgl = Self::create_bgl(device, false);
        let render_bgl = Self::create_bgl(device, true);

        let compute_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Corona Compute PL"),
            bind_group_layouts: &[Some(&compute_bgl)],
            immediate_size: 0,
        });
        let render_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Corona Render PL"),
            bind_group_layouts: &[Some(&render_bgl)],
            immediate_size: 0,
        });

        // ── Compute pipelines ────────────────────────────────────────────────

        let mk_compute = |entry: &str| -> wgpu::ComputePipeline {
            helio_core::shader::create_compute_pipeline(
                &device,
                wgpu::ComputePipelineDescriptor {
                    label: Some(&format!("Corona {entry}")),
                    layout: Some(&compute_pl),
                    module: &compute_shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    cache: None,
                },
            )
        };

        let simulate_pipeline = mk_compute("cs_simulate");
        let emit_pipeline = mk_compute("cs_emit");
        let layout_pipeline = mk_compute("cs_layout");
        let scan_local_pipeline = mk_compute("cs_scan_local");
        let scan_blocks_pipeline = mk_compute("cs_scan_blocks");
        let scatter_pipeline = mk_compute("cs_scatter");
        let build_multi_pipeline = mk_compute("cs_build_multi");
        let sort_local_pipeline = mk_compute("cs_sort_local");
        let sort_global_pipeline = mk_compute("cs_sort_global");
        let sort_local_merge_pipeline = mk_compute("cs_sort_local_merge");

        // ── Render pipeline ──────────────────────────────────────────────────

        let render_pipeline = helio_core::shader::create_render_pipeline(
            &device,
            wgpu::RenderPipelineDescriptor {
                label: Some("Corona Render"),
                layout: Some(&render_pl),
                vertex: wgpu::VertexState {
                    module: &render_shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(helio_pass_gbuffer::DRAW_SLOT_LAYOUT)],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &render_shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: surface_format,
                        blend: Some(wgpu::BlendState {
                            color: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::SrcAlpha,
                                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                                operation: wgpu::BlendOperation::Add,
                            },
                            alpha: wgpu::BlendComponent::OVER,
                        }),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(helio_core::depth::NEARER_EQUAL),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            },
        );

        // ── Initial bind group ───────────────────────────────────────────────

        let bg_key = Some([
            particle_buf.clone(),
            emitter_buf.clone(),
            camera_buf.clone(),
        ]);

        let compute_bg = Some(Self::build_bg(
            device,
            &compute_bgl,
            &uniform_buf,
            &particle_buf,
            &emitter_buf,
            &compact_buf,
            &emitter_alive_buf,
            &draw_args_staging,
            camera_buf,
            &prefix_buf,
            &block_sums_buf,
            &sort_key_buf,
            &particle_view,
            &particle_sampler,
            &spawn_cursor_buf,
            &pool_layout_buf,
        ));
        let render_bg = Some(Self::build_bg(
            device,
            &render_bgl,
            &uniform_buf,
            &particle_buf,
            &emitter_buf,
            &compact_buf,
            &emitter_alive_buf,
            &draw_args_staging,
            camera_buf,
            &prefix_buf,
            &block_sums_buf,
            &sort_key_buf,
            &particle_view,
            &particle_sampler,
            &spawn_cursor_buf,
            &pool_layout_buf,
        ));

        let max_sort_steps = initial_sort_cap;

        Self {
            simulate_pipeline,
            emit_pipeline,
            layout_pipeline,
            scan_local_pipeline,
            scan_blocks_pipeline,
            scatter_pipeline,
            build_multi_pipeline,
            sort_local_pipeline,
            sort_global_pipeline,
            sort_local_merge_pipeline,
            render_pipeline,
            compute_bgl,
            render_bgl,
            uniform_buf,
            particle_buf,
            emitter_buf,
            spawn_cursor_buf,
            compact_buf,
            emitter_alive_buf,
            draw_args_staging,
            draw_args_buf,
            prefix_buf,
            block_sums_buf,
            sort_key_buf,
            sort_steps_buf,
            pool_layout_buf,
            pool_dispatch_buf,
            _particle_tex: particle_tex,
            particle_view,
            particle_sampler,
            compute_bg,
            render_bg,
            draw_slots,
            bg_key,
            max_particles: 1,
            emitter_count: 0,
            max_sort_steps,
            particles_origin: glam::DVec3::ZERO,
            pending_shift: glam::DVec3::ZERO,
            sort_steps: Vec::new(),
            depth_sort_enabled: false,
        }
    }

    // ── BGL helpers ──────────────────────────────────────────────────────────

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

    fn storage_entry(
        binding: u32,
        vis: wgpu::ShaderStages,
        ro: bool,
    ) -> wgpu::BindGroupLayoutEntry {
        wgpu::BindGroupLayoutEntry {
            binding,
            visibility: vis,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: ro },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }
    }

    fn create_bgl(device: &wgpu::Device, for_render: bool) -> wgpu::BindGroupLayout {
        use wgpu::ShaderStages as SS;

        let storage_visibility = if for_render { SS::VERTEX } else { SS::COMPUTE };
        let uniform_visibility = if for_render {
            SS::VERTEX | SS::FRAGMENT
        } else {
            SS::COMPUTE
        };
        let label = if for_render {
            "Corona Render BGL"
        } else {
            "Corona Compute BGL"
        };

        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(label),
            entries: &[
                Self::uniform_entry(0, uniform_visibility),
                Self::storage_entry(1, storage_visibility, for_render),
                Self::storage_entry(2, storage_visibility, for_render),
                Self::storage_entry(3, storage_visibility, for_render),
                Self::storage_entry(4, SS::COMPUTE, false),
                Self::storage_entry(5, SS::COMPUTE, false),
                Self::storage_entry(6, uniform_visibility, true),
                Self::storage_entry(7, SS::COMPUTE, false),
                Self::storage_entry(8, SS::COMPUTE, false),
                Self::storage_entry(9, SS::COMPUTE, false),
                wgpu::BindGroupLayoutEntry {
                    binding: 10,
                    visibility: SS::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 11,
                    visibility: SS::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                // Per-emitter-row spawn cursor and epoch: pass-owned, purely transient
                // GPU state (see `spawn_cursor_buf`'s doc), never mirrored by
                // SceneDB and never read by the render bind group -- present
                // here too only so `build_bg` has one uniform entry list for
                // both layouts.
                Self::storage_entry(12, SS::COMPUTE, false),
                // The pool layout (`pool_layout_buf`): compute only, like 12.
                Self::storage_entry(13, SS::COMPUTE, false),
            ],
        })
    }

    // ── Bind group builder ────────────────────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    fn build_bg(
        device: &wgpu::Device,
        bgl: &wgpu::BindGroupLayout,
        uniforms: &wgpu::Buffer,
        particles: &wgpu::Buffer,
        emitters: &wgpu::Buffer,
        compact: &wgpu::Buffer,
        alive: &wgpu::Buffer,
        draw_staging: &wgpu::Buffer,
        camera: &wgpu::Buffer,
        prefix: &wgpu::Buffer,
        block_sums: &wgpu::Buffer,
        sort_keys: &wgpu::Buffer,
        tex_view: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
        spawn_cursors: &wgpu::Buffer,
        pool_layout: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Corona BG"),
            layout: bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: particles.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: emitters.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: compact.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: alive.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: draw_staging.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: camera.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: prefix.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: block_sums.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: sort_keys.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 10,
                    resource: wgpu::BindingResource::TextureView(tex_view),
                },
                wgpu::BindGroupEntry {
                    binding: 11,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 12,
                    resource: spawn_cursors.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 13,
                    resource: pool_layout.as_entire_binding(),
                },
            ],
        })
    }

    // ── Sort step pre-computation ─────────────────────────────────────────────

    /// Compute all bitonic sort dispatches for one emitter (particle_offset=lo, particle_count=n).
    /// Appends to `out`. Steps are: initial local sort, then for each k-stage:
    ///   global steps (j ≥ 256), then one local-merge step (j = 128..1).
    #[allow(dead_code)]
    fn push_sort_steps(lo: u32, n: u32, out: &mut Vec<SortStep>) {
        if n == 0 {
            return;
        }

        // Initial block sort (k = 2..256, all in shared memory).
        // j == 0 signals cs_sort_local (not cs_sort_global).
        out.push(SortStep {
            k: 256,
            j: 0,
            lo,
            n,
        });

        // Global stages for k = 512, 1024, ..., n.
        let mut k = 512u32;
        while k <= n {
            // Global steps: j = k/2 down to 256 (inclusive).
            let mut j = k >> 1;
            while j >= 256 {
                out.push(SortStep { k, j, lo, n });
                j >>= 1;
            }
            // Local merge step: j = 128..1 in shared memory.
            // j == 0xFFFF_FFFF signals cs_sort_local_merge.
            out.push(SortStep {
                k,
                j: u32::MAX,
                lo,
                n,
            });
            k <<= 1;
        }
    }

    /// Grow `sort_steps_buf` if needed and upload `sort_steps`.
    #[allow(dead_code)]
    fn upload_sort_steps(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        sort_steps: &[SortStep],
        sort_steps_buf: &mut wgpu::Buffer,
        max_sort_steps: &mut u32,
    ) {
        let needed = sort_steps.len() as u32;
        if needed > *max_sort_steps {
            *max_sort_steps = needed.next_power_of_two().max(256);
            *sort_steps_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Corona SortSteps"),
                size: *max_sort_steps as u64 * std::mem::size_of::<SortStep>() as u64,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }

        if !sort_steps.is_empty() {
            queue.write_buffer(sort_steps_buf, 0, bytemuck::cast_slice(sort_steps));
        }
    }

    // ── 4×4 Sprite atlas generation ───────────────────────────────────────────
    //
    // 16 procedural sprites arranged in a 4×4 grid:
    //   Row 0 (0-3):  Soft blobs — varying softness / core sharpness
    //   Row 1 (4-7):  Rings — varying thickness
    //   Row 2 (8-11): Stars — varying point count / spike sharpness
    //   Row 3 (12-15):Sparkles — elongated cross / streaks

    fn make_atlas(atlas_size: u32, cells: u32) -> Vec<u8> {
        let cell = atlas_size / cells; // 32
        let mut data = vec![0u8; (atlas_size * atlas_size * 4) as usize];

        for sprite in 0..16u32 {
            let col = sprite % cells;
            let row = sprite / cells;
            let ox = col * cell;
            let oy = row * cell;

            for py in 0..cell {
                for px in 0..cell {
                    let half = cell as f32 * 0.5;
                    let dx = (px as f32 + 0.5 - half) / half;
                    let dy = (py as f32 + 0.5 - half) / half;
                    let r = (dx * dx + dy * dy).sqrt();
                    let a = match row {
                        0 => {
                            // Soft blobs: vary from very soft to hard-edged
                            let sharpness = 1.0 + col as f32 * 2.0;
                            (1.0 - r.powf(sharpness)).clamp(0.0, 1.0)
                        }
                        1 => {
                            // Rings: vary inner radius
                            let inner = 0.3 + col as f32 * 0.12;
                            let outer = 0.85;
                            let ring = 1.0
                                - ((r - (inner + outer) * 0.5).abs() / ((outer - inner) * 0.5))
                                    .clamp(0.0, 1.0);
                            ring * ring
                        }
                        2 => {
                            // Stars: 4-8 points
                            let points = 4.0 + col as f32 * 1.5;
                            let angle = dy.atan2(dx);
                            let star_r = r / (0.5 + 0.5 * (angle * points).cos().abs());
                            (1.0 - star_r * 1.5).clamp(0.0, 1.0)
                        }
                        _ => {
                            // Sparkles: soft cross/streak with varying elongation
                            let elongation = 1.0 + col as f32 * 1.5;
                            let rx = dx / elongation;
                            let ry = dy;
                            let dr = (rx * rx + ry * ry).sqrt();
                            let cx = dx;
                            let cy = dy * elongation;
                            let dc = (cx * cx + cy * cy).sqrt();
                            let combined = (1.0 - dr).max(1.0 - dc).clamp(0.0, 1.0);
                            combined * combined
                        }
                    };
                    let alpha = (a.clamp(0.0, 1.0) * 255.0) as u8;
                    let base = ((oy + py) * atlas_size + ox + px) as usize * 4;
                    data[base] = 255;
                    data[base + 1] = 255;
                    data[base + 2] = 255;
                    data[base + 3] = alpha;
                }
            }
        }
        data
    }
}

// ── RenderPass impl ──────────────────────────────────────────────────────────

impl RenderPass for CoronaPass {
    fn scene_buffers(&self) -> &'static [&'static str] {
        &["corona_emitters"]
    }

    fn name(&self) -> &'static str {
        "Corona"
    }

    /// A member while some `"corona_emitters"` row is live.
    fn is_member(&mut self, scene: &mut helio_core::SceneContent<'_>) -> bool {
        scene.holds_rows("corona_emitters")
    }

    fn on_leave(&mut self, device: &wgpu::Device) {
        // The pool comes back, empty, when emitters do.
        self.set_pool_capacity(device, 1);
    }

    fn reads(&self) -> &'static [&'static str] {
        &["pre_aa", "full_res_depth", "corona_emitters", "depth"]
    }

    fn writes(&self) -> &'static [&'static str] {
        // Draws (LoadOp::Load) directly onto pre_aa — see render_pass_descriptor()
        // below. Declaring this lets the render graph see the real dependency
        // for subpass fusion.
        &["pre_aa"]
    }

    fn declare_resources(&self, builder: &mut ResourceBuilder) {
        builder.read("pre_aa");
        builder.read("full_res_depth");
        builder.read("corona_emitters");
    }

    fn prepare(&mut self, ctx: &PrepareContext) -> HelioResult<()> {
        // Resolve the `"corona_emitters"` rows by key every frame — no
        // renderer method, no stored SceneDB handle, no CPU compaction. Their
        // particle ranges are read on the GPU (see the module doc), so the
        // CPU needs only how many rows there are.
        let emitter_bytes = std::mem::size_of::<crate::GpuCoronaEmitter>() as u64;
        self.emitter_count = ctx
            .scene_buffers
            .get(BufferKey::of("corona_emitters"))
            .map_or(0, |handle| {
                (handle.buffer.size() / emitter_bytes).min(u64::from(MAX_EMITTERS)) as u32
            });
        if self.max_particles != DEFAULT_MAX_PARTICLES {
            self.set_pool_capacity(ctx.device, DEFAULT_MAX_PARTICLES);
            // A fresh pool holds no particles: emission restarts.
            ctx.write_buffer(
                &self.spawn_cursor_buf,
                0,
                bytemuck::cast_slice(&vec![0u32; MAX_EMITTERS as usize * 8]),
            );
        }

        let origin = ctx.world_origin;
        self.pending_shift += origin - self.particles_origin;
        self.particles_origin = origin;

        let uniforms = CoronaUniforms {
            // The host-driven frame clock: particles freeze with it.
            delta_time: ctx.time_delta,
            total_particles: self.max_particles,
            emitter_count: self.emitter_count,
            frame_count: ctx.frame_num as u32,
            sort_k: 0,
            sort_j: 0,
            sort_lo: 0,
            sort_n: 0,
            origin_shift: self.pending_shift.as_vec3().extend(0.0).to_array(),
        };
        ctx.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&uniforms));
        Ok(())
    }

    fn render_pass_descriptor_with_storage<'a>(
        &'a self,
        target: &'a wgpu::TextureView,
        depth: &'a wgpu::TextureView,
        resources: &'a helio_core::ResourceRegistry<'a>,
        storage: &'a mut helio_core::RenderFrameStorage,
    ) -> Option<wgpu::RenderPassDescriptor<'a>> {
        let pre_aa = resources.get(helio_core::ResourceKey::new("pre_aa"));
        let target_view = pre_aa.unwrap_or(target);
        // `pre_aa` is internal-resolution (render-scaled); the raw `target`
        // fallback is full output resolution. Depth must track whichever one
        // color actually resolved to, or wgpu rejects the pass for mismatched
        // attachment extents whenever render_scale < 1.0.
        let depth_view = if pre_aa.is_some() {
            depth
        } else {
            resources
                .get(helio_core::ResourceKey::new("full_res_depth"))
                .unwrap_or(depth)
        };
        let color_attachments: &'a [Option<wgpu::RenderPassColorAttachment<'a>>] = storage
            .retain_boxed_slice(Box::new([Some(wgpu::RenderPassColorAttachment {
                view: target_view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })]));
        Some(wgpu::RenderPassDescriptor {
            label: Some("Corona Render"),
            color_attachments,
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth_view,
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

    fn execute(&mut self, ctx: &mut PassContext) -> HelioResult<()> {
        if self.emitter_count == 0 {
            return Ok(());
        }

        // ── Bind group rebuild when buffers change ────────────────────────────

        let emitter_buf = ctx
            .scene_buffers
            .get(BufferKey::of("corona_emitters"))
            .map(|handle| &handle.buffer)
            .unwrap_or(&self.emitter_buf);
        let key = [
            self.particle_buf.clone(),
            emitter_buf.clone(),
            ctx.camera.clone(),
        ];

        if self.bg_key.as_ref() != Some(&key) {
            self.compute_bg = Some(Self::build_bg(
                ctx.device,
                &self.compute_bgl,
                &self.uniform_buf,
                &self.particle_buf,
                emitter_buf,
                &self.compact_buf,
                &self.emitter_alive_buf,
                &self.draw_args_staging,
                ctx.camera,
                &self.prefix_buf,
                &self.block_sums_buf,
                &self.sort_key_buf,
                &self.particle_view,
                &self.particle_sampler,
                &self.spawn_cursor_buf,
                &self.pool_layout_buf,
            ));
            self.render_bg = Some(Self::build_bg(
                ctx.device,
                &self.render_bgl,
                &self.uniform_buf,
                &self.particle_buf,
                emitter_buf,
                &self.compact_buf,
                &self.emitter_alive_buf,
                &self.draw_args_staging,
                ctx.camera,
                &self.prefix_buf,
                &self.block_sums_buf,
                &self.sort_key_buf,
                &self.particle_view,
                &self.particle_sampler,
                &self.spawn_cursor_buf,
                &self.pool_layout_buf,
            ));
            self.bg_key = Some(key);
        }

        let compute_bg = self.compute_bg.as_ref().unwrap();
        let render_bg = self.render_bg.as_ref().unwrap();
        let ec = self.emitter_count;

        let mut cmds = ctx.compute_cmds();

        // ── Pass 0: Layout (the pool's used range → dispatch args) ───────────
        {
            let mut p = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Corona Layout"),
                timestamp_writes: None,
            });
            p.set_pipeline(&self.layout_pipeline);
            p.set_bind_group(0, compute_bg, &[]);
            p.dispatch_workgroups(1, 1, 1);
        }
        // STORAGE output → INDIRECT args, as for the draw args below.
        cmds.copy_buffer_to_buffer(&self.pool_layout_buf, 0, &self.pool_dispatch_buf, 0, 12);

        // ── Pass 1: Simulate ─────────────────────────────────────────────────
        {
            let mut p = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Corona Simulate"),
                timestamp_writes: None,
            });
            p.set_pipeline(&self.simulate_pipeline);
            p.set_bind_group(0, compute_bg, &[]);
            p.dispatch_workgroups_indirect(&self.pool_dispatch_buf, 0);
        }
        // The simulation above moved every live particle by the shift.
        self.pending_shift = glam::DVec3::ZERO;

        // ── Pass 2: Emit ─────────────────────────────────────────────────────
        {
            let mut p = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Corona Emit"),
                timestamp_writes: None,
            });
            p.set_pipeline(&self.emit_pipeline);
            p.set_bind_group(0, compute_bg, &[]);
            p.dispatch_workgroups(ec, 1, 1);
        }

        // ── Pass 3: Scan local (prefix scan + sort-key sentinel reset) ────────
        {
            let mut p = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Corona ScanLocal"),
                timestamp_writes: None,
            });
            p.set_pipeline(&self.scan_local_pipeline);
            p.set_bind_group(0, compute_bg, &[]);
            p.dispatch_workgroups_indirect(&self.pool_dispatch_buf, 0);
        }

        // ── Pass 4: Scan blocks (cumulative per-emitter offsets) ──────────────
        {
            let mut p = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Corona ScanBlocks"),
                timestamp_writes: None,
            });
            p.set_pipeline(&self.scan_blocks_pipeline);
            p.set_bind_group(0, compute_bg, &[]);
            p.dispatch_workgroups(ec, 1, 1);
        }

        // ── Pass 5: Scatter (compact_buf + sort_key_buf) ─────────────────────
        {
            let mut p = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Corona Scatter"),
                timestamp_writes: None,
            });
            p.set_pipeline(&self.scatter_pipeline);
            p.set_bind_group(0, compute_bg, &[]);
            p.dispatch_workgroups_indirect(&self.pool_dispatch_buf, 0);
        }

        // ── Pass 6: Build draw args ───────────────────────────────────────────
        {
            let mut p = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Corona BuildMulti"),
                timestamp_writes: None,
            });
            p.set_pipeline(&self.build_multi_pipeline);
            p.set_bind_group(0, compute_bg, &[]);
            p.dispatch_workgroups(ec, 1, 1);
        }

        // Copy STORAGE staging → INDIRECT buffer (the STORAGE+INDIRECT conflict fix).
        let args_size = ec as u64 * std::mem::size_of::<crate::GpuCoronaDrawIndirect>() as u64;
        cmds.copy_buffer_to_buffer(
            &self.draw_args_staging,
            0,
            &self.draw_args_buf,
            0,
            args_size,
        );

        // ── Passes 7+: Bitonic sort per emitter (opt-in) ─────────────────────
        // Each bitonic stage requires a separate compute pass. For emitters with
        // 262K particles this is ~66 dispatches each. Leave depth_sort_enabled=false
        // for additive effects where draw order doesn't matter.

        if !self.depth_sort_enabled || self.sort_steps.is_empty() {
            // Skip sort — proceed directly to render.
        } else {
            let step_size = std::mem::size_of::<SortStep>() as u64;

            for (step_idx, step) in self.sort_steps.iter().enumerate() {
                // Copy {k, j, lo, n} from sort_steps_buf into the sort_* fields of
                // uniform_buf (offset 16 = after the first 4 u32 base fields).
                cmds.copy_buffer_to_buffer(
                    &self.sort_steps_buf,
                    step_idx as u64 * step_size,
                    &self.uniform_buf,
                    16, // byte offset of sort_k in CoronaUniforms
                    step_size,
                );

                let particle_count = step.n;
                let blocks = particle_count.div_ceil(WG);

                if step.j == 0 {
                    // cs_sort_local: initial block sort (k=2..256 in shared memory).
                    let mut p = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("Corona SortLocal"),
                        timestamp_writes: None,
                    });
                    p.set_pipeline(&self.sort_local_pipeline);
                    p.set_bind_group(0, compute_bg, &[]);
                    p.dispatch_workgroups(blocks, 1, 1);
                } else if step.j == u32::MAX {
                    // cs_sort_local_merge: tail steps (j=128..1) for a global k-stage.
                    let mut p = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("Corona SortLocalMerge"),
                        timestamp_writes: None,
                    });
                    p.set_pipeline(&self.sort_local_merge_pipeline);
                    p.set_bind_group(0, compute_bg, &[]);
                    p.dispatch_workgroups(blocks, 1, 1);
                } else {
                    // cs_sort_global: one compare-swap step for j >= 256.
                    let mut p = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("Corona SortGlobal"),
                        timestamp_writes: None,
                    });
                    p.set_pipeline(&self.sort_global_pipeline);
                    p.set_bind_group(0, compute_bg, &[]);
                    p.dispatch_workgroups(blocks, 1, 1);
                }
            }
        } // end depth_sort_enabled else branch

        // ── Render pass ──────────────────────────────────────────────────────

        let mut rp = ctx.render_cmds().unwrap();
        rp.set_pipeline(&self.render_pipeline);
        rp.set_bind_group(0, render_bg, &[]);
        rp.set_vertex_buffer(0, self.draw_slots.slice(..));

        // One draw_indirect per emitter — each draws only its alive, sorted particles.
        let stride = std::mem::size_of::<crate::GpuCoronaDrawIndirect>() as u64;
        for i in 0..ec {
            rp.draw_indirect(&self.draw_args_buf, i as u64 * stride);
        }

        Ok(())
    }
}
