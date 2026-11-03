//! The `RenderPass` implementation: buffers, pipelines and the dispatches.

use std::sync::Arc;

use crate::{
    plan_bands, FoliageQuality, FoliageRange, GpuBladeInstance, GpuFoliageBands, GpuFoliageLayer,
    GpuFoliageTile, GpuFoliageType, GpuGroundArgs, GpuGroundHit, GpuGroundQuery, GpuTileFrame,
    GroundLink, PlaneGround, TileState, FOLIAGE_BAND_TILES, FOLIAGE_MAX_INTERACTORS,
    FOLIAGE_TILE_SIZE_METERS, GROUND_ARGS, GROUND_HITS, GROUND_QUERIES,
};
use glam::DVec3;
use helio_core::{PassContext, PrepareContext, RenderPass, Result as HelioResult};
use pulsar_scenedb::gpu::BufferKey;
use wgpu::util::DeviceExt;

use crate::residency::TileRing;
use crate::uniforms::{FoliageCullUniforms, PlaceUniforms};
use crate::{
    COUNTER_PLACED_BLADES, COUNTER_PLACEMENT_OVERFLOW, COUNTER_VISIBLE_OVERFLOW,
    DEFAULT_WPO_EXTENT_METERS, FOLIAGE_COUNTER_COUNT, FOLIAGE_LOD_FADE_BAND_METERS,
    FOLIAGE_LOD_VERTEX_COUNTS, FOLIAGE_VISIBLE_PER_LOD_CAPACITY,
};

const TILE_BYTES: u64 = std::mem::size_of::<GpuFoliageTile>() as u64;
const BLADE_BYTES: u64 = std::mem::size_of::<GpuBladeInstance>() as u64;
const TYPE_BYTES: u64 = std::mem::size_of::<GpuFoliageType>() as u64;
const LAYER_BYTES: u64 = std::mem::size_of::<GpuFoliageLayer>() as u64;
const FRAME_BYTES: u64 = std::mem::size_of::<GpuTileFrame>() as u64;

/// How far over the eye's height along a tile's up a ground query starts, and how far
/// under it it ends: ground within this band of the eye's height is found, which a reach
/// of a few hundred metres never leaves outside cliffs that tall.
const SEARCH_ABOVE_METRES: f32 = 256.0;
const SEARCH_BELOW_METRES: f32 = 512.0;

/// Densest ground assumed when the pass is not sized for one, in tufts a square metre.
const DEFAULT_TUFTS_PER_M2: f32 = 8.0;

/// Queries one frame may ask the ground (a few far tiles, or many near ones).
const QUERY_CAPACITY: u32 = 1 << 16;

/// Tiles one frame may queue, placed and queried each.
const MAX_QUEUED_TILES: u32 = 64;

/// Tiles one band's ring hands out a frame.
const BAND_TILES_PER_FRAME: u32 = 8;

/// Tallest foliage the bounds allow for, metres: the tallest authored plant grown by the
/// far bands' growth.
const MAX_FOLIAGE_HEIGHT: f32 = 1.5;

/// A tile whose ground was asked last frame: placed this frame from the answers if its
/// slot still holds it.
#[derive(Clone, Copy, Debug)]
struct Asked {
    band: usize,
    slot: u32,
    coord: [i32; 2],
    /// Its first query.
    first: u32,
}

/// Hard ceiling on the foliage type table.
///
/// `GpuBladeInstance` stores the type id in 8 bits, so 256 is not a budget choice — it is
/// the representable maximum.
pub const MAX_FOLIAGE_TYPES: u32 = 256;

/// Hard ceiling on the foliage layer table (a budget: the layer id is not packed into any
/// record). The placement shader loops over the table once per candidate.
pub const MAX_FOLIAGE_LAYERS: u32 = 64;

/// Compute workgroup size shared by `cs_place`, `cs_tile_cull` and `cs_cluster_cull`.
const WORKGROUP_SIZE: u32 = 64;
/// Candidates a placement or query workgroup takes of a tile: a tile is placed by as many
/// workgroups as it has parts, all at once.
const PLACE_PART: u32 = 1024;
/// Parts in a frame at most: every queued tile at the largest candidate grid.
const MAX_PARTS: u32 = 2 * MAX_QUEUED_TILES * crate::MAX_CANDIDATES_PER_TILE.div_ceil(PLACE_PART);
/// Bytes of a `FoliageInteractorRow` row, and of a gathered one.
const SOURCE_INTERACTOR_BYTES: u64 =
    std::mem::size_of::<crate::components::FoliageInteractorRow>() as u64;
/// Bytes of a gathered interactor (`Interactor` in foliage_interactors.wgsl).
pub const INTERACTOR_BYTES: u64 = 48;

/// Foliage residency in bands, GPU placement on a ground, tile/cluster culling and
/// per-LOD compaction.
///
/// See the crate docs for the design and for the interface contract with
/// `FoliageGBufferPass`. The `pub` buffer fields below are that contract: the graph
/// builder clones the `Arc`s into the consumer's constructor.
pub struct FoliagePlacePass {
    // ── Shared with FoliageGBufferPass ──────────────────────────────────────────
    /// `GpuBladeInstance[]`: each band's slabs, one fixed slab per ring slot.
    ///
    /// Fixed slabs rather than a bump allocator, and that is a correctness choice as much
    /// as a simplicity one: with a bump allocator a tile's `blade_offset` would depend on
    /// the order tiles happened to be placed in, and the arena would fragment under ring
    /// churn.
    pub blade_arena: Arc<wgpu::Buffer>,

    /// `GpuFoliageTile[]` — every band's ring slots. Written by the CPU for coordinates
    /// and residency state, by `cs_place` for `blade_count` and the vertical bounds.
    pub tile_table: Arc<wgpu::Buffer>,

    /// `u32` record references in four contiguous regions, one per LOD, addressed with
    /// [`crate::lod_region_offset`].
    pub visible_blades: Arc<wgpu::Buffer>,

    /// Exactly four `wgpu::util::DrawIndirectArgs` at byte offsets 0/16/32/48, in LOD
    /// order; `first_instance` is 0 for all four.
    pub foliage_indirect: Arc<wgpu::Buffer>,

    /// `GpuTileFrame[]`, one per slot: where its records stand (origin, axes, up) in
    /// render space. Written by the CPU when a slot takes a tile and when the world origin
    /// moves; read by placement, the cull and the draw.
    pub tile_frames: Arc<wgpu::Buffer>,

    /// The band table ([`GpuFoliageBands`]), a uniform: written once, read by placement,
    /// the cull and the draw.
    pub bands_buffer: Arc<wgpu::Buffer>,

    /// This frame's interactors (`FoliageInteractorRow` rows with a radius) in
    /// render space, for the draw: a count, then at most [`FOLIAGE_MAX_INTERACTORS`]
    /// rows of [`INTERACTOR_BYTES`] (`Interactors` in foliage_interactors.wgsl).
    pub interactors: Arc<wgpu::Buffer>,
    /// The gather's constants (the frame's origin, the source's rows), its pipeline and
    /// its bind group (rebuilt when SceneDB reallocates the column).
    gather_uniforms: wgpu::Buffer,
    gather_rows: u32,
    gather_pipeline: wgpu::ComputePipeline,
    gather_bind_group: Option<(wgpu::Buffer, wgpu::BindGroup)>,
    placeholder_interactor: wgpu::Buffer,

    /// Per-LOD visible counts plus the overflow telemetry. See the `COUNTER_*` constants.
    counters: Arc<wgpu::Buffer>,

    /// The ground exchange (`contract::ground`): queries, hits and the queries' header,
    /// published as [`GROUND_QUERIES`], [`GROUND_HITS`] and [`GROUND_ARGS`].
    queries: Arc<wgpu::Buffer>,
    hits: Arc<wgpu::Buffer>,
    ground_args: Arc<wgpu::Buffer>,
    /// The ground: its tiling, and who answers its queries.
    ground: Arc<GroundLink>,
    /// The chart the resident tiles lie on.
    chart: Option<u64>,
    /// The renderer's world origin the frames were written for.
    world_origin: Option<DVec3>,
    /// Batches of queries published so far (the last one is `batch`).
    batch: u64,
    /// Tiles asked in the last batch, in query order.
    asked: Vec<Asked>,

    // ── Internal ────────────────────────────────────────────────────────────────
    placeholder_type: wgpu::Buffer,
    placeholder_layer: wgpu::Buffer,
    place_queue: wgpu::Buffer,
    tile_visibility: wgpu::Buffer,
    place_uniforms: wgpu::Buffer,
    cull_uniforms: wgpu::Buffer,

    place_reset_pipeline: wgpu::ComputePipeline,
    place_bounds_pipeline: wgpu::ComputePipeline,
    place_pipeline: wgpu::ComputePipeline,
    place_finish_pipeline: wgpu::ComputePipeline,
    query_pipeline: wgpu::ComputePipeline,
    /// The frame's parts (`parts` in foliage_place.wgsl), and the placed tiles' height
    /// ranges and record counts while they are placed.
    place_parts: wgpu::Buffer,
    place_scratch: wgpu::Buffer,
    parts_scratch: Vec<[u32; 4]>,
    place_part_count: u32,
    query_part_count: u32,
    ground_plane_pipeline: wgpu::ComputePipeline,
    tile_cull_pipeline: wgpu::ComputePipeline,
    cluster_cull_pipeline: wgpu::ComputePipeline,
    finalize_pipeline: wgpu::ComputePipeline,

    cull_bgl: wgpu::BindGroupLayout,
    place_bind_group: Option<wgpu::BindGroup>,
    place_bind_group_key: Option<(wgpu::Buffer, wgpu::Buffer, wgpu::TextureView)>,
    cull_bind_group: Option<wgpu::BindGroup>,
    cull_bind_group_key: Option<(wgpu::Buffer, wgpu::TextureView, wgpu::Sampler, wgpu::Buffer)>,

    placeholder_hiz: wgpu::TextureView,
    placeholder_hiz_sampler: wgpu::Sampler,
    placeholder_terrain: wgpu::TextureView,
    terrain_sampler: wgpu::Sampler,

    /// The bands, and each one's ring (slots local to the band).
    bands: GpuFoliageBands,
    rings: Vec<TileRing>,
    range: FoliageRange,
    quality: FoliageQuality,
    total_slots: u32,
    cluster_size: u32,
    clusters_per_tile: u32,

    terrain_transform: Option<([f32; 2], f32)>,

    // Per-frame state handed from `prepare` to `execute`.
    active: bool,
    /// Tiles placed from last frame's answers, then tiles queried, this frame.
    place_tile_count: u32,
    queued_tile_count: u32,
    query_count: u32,
    cluster_dispatch_width: u32,
    cluster_dispatch_height: u32,
    tile_dispatch_groups: u32,

    warned_type_overflow: bool,
    /// Whether any type row has a density; see [`crate::foliage_type_liveness`].
    type_liveness: helio_core::SceneBufferLiveness,
    commands_recorded: u64,

    /// Non-blocking readback of [`Self::counters`], for diagnosing an empty field: every
    /// way this pass can fail quietly — no tiles queued, no candidates accepted,
    /// everything culled, the visible region overflowing — looks identical from the
    /// outside, namely bare ground. These counters distinguish them.
    counters_readback: wgpu::Buffer,
    readback_state: ReadbackState,
    debug_counters: [u32; FOLIAGE_COUNTER_COUNT],

    header_scratch: Vec<GpuFoliageTile>,
    frame_scratch: Vec<GpuTileFrame>,
    queue_scratch: Vec<[u32; 2]>,
}

impl FoliagePlacePass {
    /// Create the pass for `quality`'s range and the default density.
    pub fn new(device: &wgpu::Device, quality: FoliageQuality) -> Self {
        Self::new_with(
            device,
            quality,
            FoliageRange {
                range: quality.range(),
                ..FoliageRange::default()
            },
        )
    }

    /// Create the pass and allocate every buffer it will ever use, for the bands reaching
    /// `range` ([`plan_bands`]).
    ///
    /// Nothing here grows: the band table fixes the slots, the slabs and so the arena, so
    /// there is no reallocation path and no bind-group versioning for our own buffers.
    pub fn new_with(device: &wgpu::Device, quality: FoliageQuality, range: FoliageRange) -> Self {
        let cluster_size = quality.cluster_granularity().max(1);
        let range = FoliageRange {
            range: if range.range.is_finite() && range.range > 0.0 {
                range.range
            } else {
                quality.range()
            },
            tufts_per_m2: if range.tufts_per_m2.is_finite() && range.tufts_per_m2 > 0.0 {
                range.tufts_per_m2
            } else {
                DEFAULT_TUFTS_PER_M2
            },
            cluster_edge: isqrt(cluster_size).max(1),
            ..range
        };
        let bands = plan_bands(&range);
        let total_slots = bands.slots().max(1);
        let records = bands.records().max(1);
        let max_slab = bands.bands().iter().map(|b| b.slab).max().unwrap_or(1);
        let clusters_per_tile = max_slab.div_ceil(cluster_size).max(1);
        log::info!(
            "foliage: {} bands to {:.0} m, {} slots, arena {:.1} MiB ({})",
            bands.count,
            bands.range,
            total_slots,
            (records * BLADE_BYTES) as f64 / (1024.0 * 1024.0),
            bands
                .bands()
                .iter()
                .map(|b| format!(
                    "{:.0}-{:.0} m: {} m tiles, {}² tufts, share {:.2}",
                    b.inner, b.outer, b.tile_size, b.candidate_grid, b.share
                ))
                .collect::<Vec<_>>()
                .join("; "),
        );

        let storage = |label: &str, size: u64, usage: wgpu::BufferUsages| {
            Arc::new(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size.max(4),
                usage: wgpu::BufferUsages::STORAGE | usage,
                mapped_at_creation: false,
            }))
        };
        let copy = wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC;
        let blade_arena = storage("Foliage Blade Arena", records * BLADE_BYTES, copy);
        let tile_table = storage("Foliage Tile Table", total_slots as u64 * TILE_BYTES, copy);
        let tile_frames = storage(
            "Foliage Tile Frames",
            total_slots as u64 * FRAME_BYTES,
            copy,
        );
        let visible_blades = storage(
            "Foliage Visible Blades",
            FOLIAGE_LOD_VERTEX_COUNTS.len() as u64 * FOLIAGE_VISIBLE_PER_LOD_CAPACITY as u64 * 4,
            wgpu::BufferUsages::COPY_DST,
        );
        let foliage_indirect = storage(
            "Foliage Indirect Draws",
            FOLIAGE_LOD_VERTEX_COUNTS.len() as u64
                * std::mem::size_of::<wgpu::util::DrawIndirectArgs>() as u64,
            wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST,
        );
        let bands_buffer = Arc::new(
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Foliage Bands"),
                contents: bytemuck::bytes_of(&bands),
                usage: wgpu::BufferUsages::UNIFORM,
            }),
        );
        let queries = storage(
            "Foliage Ground Queries",
            QUERY_CAPACITY as u64 * std::mem::size_of::<GpuGroundQuery>() as u64,
            wgpu::BufferUsages::empty(),
        );
        let hits = storage(
            "Foliage Ground Hits",
            QUERY_CAPACITY as u64 * std::mem::size_of::<GpuGroundHit>() as u64,
            wgpu::BufferUsages::empty(),
        );
        let ground_args = storage(
            "Foliage Ground Args",
            std::mem::size_of::<GpuGroundArgs>() as u64,
            wgpu::BufferUsages::INDIRECT,
        );
        let counters = storage("Foliage Counters", FOLIAGE_COUNTER_COUNT as u64 * 4, copy);
        let interactors = storage(
            "Foliage Interactors",
            16 + u64::from(FOLIAGE_MAX_INTERACTORS) * INTERACTOR_BYTES,
            wgpu::BufferUsages::COPY_DST,
        );

        let plain = |label: &str, size: u64, usage: wgpu::BufferUsages| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let placeholder_type = plain(
            "Foliage Type Placeholder",
            TYPE_BYTES,
            wgpu::BufferUsages::STORAGE,
        );
        let placeholder_layer = plain(
            "Foliage Layer Placeholder",
            LAYER_BYTES,
            wgpu::BufferUsages::STORAGE,
        );
        let placeholder_interactor = plain(
            "Foliage Interactor Placeholder",
            SOURCE_INTERACTOR_BYTES,
            wgpu::BufferUsages::STORAGE,
        );
        let gather_uniforms = plain(
            "Foliage Gather Uniforms",
            48,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        // Last frame's answered tiles, then this frame's queried ones: (slot, first query).
        let place_queue = plain(
            "Foliage Place Queue",
            2 * MAX_QUEUED_TILES as u64 * 8,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let place_parts = plain(
            "Foliage Place Parts",
            u64::from(MAX_PARTS) * 16,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let place_scratch = plain(
            "Foliage Place Scratch",
            2 * MAX_QUEUED_TILES as u64 * 16,
            wgpu::BufferUsages::STORAGE,
        );
        let tile_visibility = plain(
            "Foliage Tile Visibility",
            total_slots as u64 * 4,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let place_uniforms = plain(
            "Foliage Place Uniforms",
            std::mem::size_of::<PlaceUniforms>() as u64,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let cull_uniforms = plain(
            "Foliage Cull Uniforms",
            std::mem::size_of::<FoliageCullUniforms>() as u64,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );

        // ── Placeholders ───────────────────────────────────────────────────────
        //
        // Both exist so a missing upstream resource degrades into a well-defined bind
        // rather than an unwrap. The Hi-Z placeholder is only ever reached before the
        // pyramid is routed, and `hiz_valid` is 0 whenever it is bound.
        let texture = |label: &str, format: wgpu::TextureFormat| {
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
        let placeholder_hiz = texture("Foliage Placeholder HiZ", wgpu::TextureFormat::R32Float);
        let placeholder_terrain = texture(
            "Foliage Placeholder Terrain",
            wgpu::TextureFormat::Rg16Float,
        );
        let placeholder_hiz_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Foliage Placeholder HiZ Sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        // Clamp-to-edge, not repeat: the capture is a camera-relative ring, and a
        // repeating address mode would sample terrain from the opposite side of it.
        let terrain_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Foliage Terrain Sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        // ── Pipelines ──────────────────────────────────────────────────────────
        let snippets = [crate::COMMON_SNIPPET];
        let place_shader = helio_core::shader::module_with(
            device,
            "Foliage Place Shader",
            helio_core::include_wgsl!("../shaders/foliage_place.wgsl"),
            &snippets,
        );
        let cull_shader = helio_core::shader::module_with(
            device,
            "Foliage Cull Shader",
            helio_core::include_wgsl!("../shaders/foliage_cull.wgsl"),
            &snippets,
        );
        let gather_shader = helio_core::shader::module_with(
            device,
            "Foliage Interactors Shader",
            helio_core::include_wgsl!("../shaders/foliage_interactors.wgsl"),
            &[],
        );

        let texture_entry =
            |binding: u32, sample_type: wgpu::TextureSampleType| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Texture {
                    sample_type,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            };
        let sampler_entry =
            |binding: u32, kind: wgpu::SamplerBindingType| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Sampler(kind),
                count: None,
            };
        let place_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Foliage Place BGL"),
            entries: &[
                uniform_entry(0),
                storage_entry(1, true),
                storage_entry(2, false),
                storage_entry(3, false),
                storage_entry(4, true),
                storage_entry(5, false),
                texture_entry(6, wgpu::TextureSampleType::Float { filterable: true }),
                sampler_entry(7, wgpu::SamplerBindingType::Filtering),
                storage_entry(8, true),
                storage_entry(9, true),
                storage_entry(10, false),
                storage_entry(11, false),
                storage_entry(12, false),
                uniform_entry(13),
                storage_entry(14, true),
                storage_entry(15, false),
            ],
        });
        let cull_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Foliage Cull BGL"),
            entries: &[
                // Camera is a read-only storage array (mono uses index 0; a future
                // single-pass stereo cull can union `cameras[0]` and `cameras[1]`).
                storage_entry(0, true),
                uniform_entry(1),
                storage_entry(2, true),
                storage_entry(3, true),
                storage_entry(4, true),
                storage_entry(5, false),
                storage_entry(6, false),
                storage_entry(7, false),
                storage_entry(8, false),
                texture_entry(9, wgpu::TextureSampleType::Float { filterable: false }),
                sampler_entry(10, wgpu::SamplerBindingType::NonFiltering),
                storage_entry(11, true),
                uniform_entry(12),
            ],
        });
        let layout = |label: &str, bgl: &wgpu::BindGroupLayout| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[Some(bgl)],
                immediate_size: 0,
            })
        };
        let gather_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Foliage Interactors BGL"),
            entries: &[
                uniform_entry(0),
                storage_entry(1, true),
                storage_entry(2, false),
            ],
        });
        let gather_layout = layout("Foliage Interactors PL", &gather_bgl);
        let place_layout = layout("Foliage Place PL", &place_bgl);
        let cull_layout = layout("Foliage Cull PL", &cull_bgl);
        let pipeline = |label: &str,
                        layout: &wgpu::PipelineLayout,
                        module: &wgpu::ShaderModule,
                        entry: &str| {
            helio_core::shader::create_compute_pipeline(
                &device,
                wgpu::ComputePipelineDescriptor {
                    label: Some(label),
                    layout: Some(layout),
                    module,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    cache: None,
                },
            )
        };
        let place_reset_pipeline = pipeline(
            "Foliage Place Reset Pipeline",
            &place_layout,
            &place_shader,
            "cs_place_reset",
        );
        let place_bounds_pipeline = pipeline(
            "Foliage Place Bounds Pipeline",
            &place_layout,
            &place_shader,
            "cs_place_bounds",
        );
        let place_pipeline = pipeline(
            "Foliage Place Pipeline",
            &place_layout,
            &place_shader,
            "cs_place",
        );
        let place_finish_pipeline = pipeline(
            "Foliage Place Finish Pipeline",
            &place_layout,
            &place_shader,
            "cs_place_finish",
        );
        let query_pipeline = pipeline(
            "Foliage Ground Query Pipeline",
            &place_layout,
            &place_shader,
            "cs_query",
        );
        let ground_plane_pipeline = pipeline(
            "Foliage Plane Ground Pipeline",
            &place_layout,
            &place_shader,
            "cs_ground_plane",
        );
        let tile_cull_pipeline = pipeline(
            "Foliage Tile Cull Pipeline",
            &cull_layout,
            &cull_shader,
            "cs_tile_cull",
        );
        let cluster_cull_pipeline = pipeline(
            "Foliage Cluster Cull Pipeline",
            &cull_layout,
            &cull_shader,
            "cs_cluster_cull",
        );
        let finalize_pipeline = pipeline(
            "Foliage Finalize Pipeline",
            &cull_layout,
            &cull_shader,
            "cs_finalize",
        );
        let gather_pipeline = pipeline(
            "Foliage Interactors Pipeline",
            &gather_layout,
            &gather_shader,
            "cs_gather",
        );

        let rings = bands
            .bands()
            .iter()
            .map(|b| {
                TileRing::new(
                    b.slot_count,
                    FOLIAGE_BAND_TILES,
                    b.tile_size,
                    BAND_TILES_PER_FRAME,
                )
            })
            .collect();

        Self {
            blade_arena,
            tile_table,
            visible_blades,
            foliage_indirect,
            tile_frames,
            bands_buffer,
            interactors,
            gather_uniforms,
            gather_rows: 0,
            gather_pipeline,
            gather_bind_group: None,
            placeholder_interactor,
            counters,
            queries,
            hits,
            ground_args,
            ground: Arc::new(GroundLink::built_in(Box::new(PlaneGround))),
            chart: None,
            world_origin: None,
            batch: 0,
            asked: Vec::new(),
            placeholder_type,
            placeholder_layer,
            place_queue,
            tile_visibility,
            place_uniforms,
            cull_uniforms,
            place_reset_pipeline,
            place_bounds_pipeline,
            place_pipeline,
            place_finish_pipeline,
            query_pipeline,
            place_parts,
            place_scratch,
            parts_scratch: Vec::new(),
            place_part_count: 0,
            query_part_count: 0,
            ground_plane_pipeline,
            tile_cull_pipeline,
            cluster_cull_pipeline,
            finalize_pipeline,
            cull_bgl,
            place_bind_group: None,
            place_bind_group_key: None,
            cull_bind_group: None,
            cull_bind_group_key: None,
            placeholder_hiz,
            placeholder_hiz_sampler,
            placeholder_terrain,
            terrain_sampler,
            bands,
            rings,
            range,
            quality,
            total_slots,
            cluster_size,
            clusters_per_tile,
            terrain_transform: None,
            active: false,
            place_tile_count: 0,
            queued_tile_count: 0,
            query_count: 0,
            cluster_dispatch_width: 1,
            cluster_dispatch_height: 1,
            tile_dispatch_groups: total_slots.div_ceil(WORKGROUP_SIZE).max(1),
            warned_type_overflow: false,
            type_liveness: crate::foliage_type_liveness(),
            commands_recorded: 0,
            counters_readback: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Foliage Counters Readback"),
                size: FOLIAGE_COUNTER_COUNT as u64 * 4,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            readback_state: ReadbackState::Idle,
            debug_counters: [0; FOLIAGE_COUNTER_COUNT],
            header_scratch: Vec::new(),
            frame_scratch: Vec::new(),
            queue_scratch: Vec::new(),
        }
    }

    /// Grow on `ground` from now on (`contract::ground`): its chart tiles the bands, and
    /// its answers place the tufts. Every resident tile is placed again.
    pub fn set_ground(&mut self, ground: Arc<GroundLink>) {
        self.ground = ground;
        self.chart = None;
        self.asked.clear();
    }

    /// The ground this pass grows on.
    pub fn ground(&self) -> &Arc<GroundLink> {
        &self.ground
    }

    /// Quality preset this pass was built for.
    #[inline]
    pub fn quality(&self) -> FoliageQuality {
        self.quality
    }

    /// What the bands were planned from.
    #[inline]
    pub fn range(&self) -> &FoliageRange {
        &self.range
    }

    /// The band table ([`Self::bands_buffer`]'s contents).
    #[inline]
    pub fn bands(&self) -> &GpuFoliageBands {
        &self.bands
    }

    /// Slots over every band, i.e. entries in [`FoliagePlacePass::tile_table`].
    #[inline]
    pub fn slot_count(&self) -> u32 {
        self.total_slots
    }

    /// Records per cull cluster, from [`FoliageQuality::cluster_granularity`].
    #[inline]
    pub fn cluster_size(&self) -> u32 {
        self.cluster_size
    }

    /// Per-LOD visible counts and overflow telemetry. See the `COUNTER_*` constants.
    #[inline]
    pub fn counters(&self) -> &Arc<wgpu::Buffer> {
        &self.counters
    }

    /// Frames on which `execute` actually recorded GPU commands (the zero-overhead
    /// guarantee's instrument: with no foliage registered this must never advance).
    #[inline]
    pub fn commands_recorded(&self) -> u64 {
        self.commands_recorded
    }

    /// Latest non-blocking readback of the counters buffer. May trail by a frame or two.
    ///
    /// Slots 0..4 are the per-LOD visible instance counts, then
    /// [`COUNTER_VISIBLE_OVERFLOW`], [`COUNTER_PLACEMENT_OVERFLOW`] and
    /// [`COUNTER_PLACED_BLADES`].
    pub fn debug_counters(&self) -> [u32; FOLIAGE_COUNTER_COUNT] {
        self.debug_counters
    }

    /// Tiles whose ground was queried this frame (placed the next).
    pub fn queued_tile_count(&self) -> u32 {
        self.queued_tile_count
    }

    /// Tiles placed this frame from last frame's answers.
    pub fn placed_tile_count(&self) -> u32 {
        self.place_tile_count
    }

    /// The bands' rings, for tests and debug overlays (slots local to each band).
    #[inline]
    pub fn rings(&self) -> &[TileRing] {
        &self.rings
    }

    /// Advance the non-blocking readback. Called once per frame from `prepare`.
    fn pump_readback(&mut self) {
        match std::mem::replace(&mut self.readback_state, ReadbackState::Idle) {
            ReadbackState::Idle => {}
            ReadbackState::CopySubmitted => {
                let flag = Arc::new(std::sync::Mutex::new(None));
                let sink = Arc::clone(&flag);
                self.counters_readback
                    .slice(..)
                    .map_async(wgpu::MapMode::Read, move |result| {
                        *sink.lock().unwrap() = Some(result);
                    });
                self.readback_state = ReadbackState::Mapping(flag);
            }
            ReadbackState::Mapping(flag) => {
                let done = flag.lock().unwrap().take();
                match done {
                    Some(Ok(())) => {
                        if let Ok(view) = self.counters_readback.slice(..).get_mapped_range() {
                            if let Ok(values) = bytemuck::try_cast_slice::<u8, u32>(&view) {
                                if values.len() >= FOLIAGE_COUNTER_COUNT {
                                    self.debug_counters
                                        .copy_from_slice(&values[..FOLIAGE_COUNTER_COUNT]);
                                }
                            }
                        }
                        self.counters_readback.unmap();
                    }
                    Some(Err(_)) => {}
                    // Still in flight; poll again next frame.
                    None => self.readback_state = ReadbackState::Mapping(flag),
                }
            }
        }
    }

    /// Publish the terrain capture's ring transform: `(min-corner world XZ, side length
    /// in metres)`. Until this is called the built-in ground answers with the plane y = 0;
    /// `None` goes back to it.
    pub fn set_terrain_transform(&mut self, transform: Option<([f32; 2], f32)>) {
        self.terrain_transform = transform.filter(|(origin, extent)| {
            origin[0].is_finite() && origin[1].is_finite() && extent.is_finite() && *extent > 0.0
        });
    }

    /// The frame of band `band`'s tile `coord` (band tiles) on `chart`.
    fn band_frame(&self, chart: u64, band: usize, coord: [i32; 2]) -> Option<crate::TileFrame> {
        let span = self.bands.bands[band].tile_span;
        let base = [coord[0] * span as i32, coord[1] * span as i32];
        self.ground.geometry().tile_frame(chart, base, span)
    }

    /// Upload the frames and headers of the slots the rings touched this frame (every
    /// frame when `all_frames`: the origin or the chart moved). Contiguous slots are
    /// coalesced into one write.
    fn upload_tile_headers(
        &mut self,
        ctx: &PrepareContext,
        generation: u32,
        chart: u64,
        world_origin: DVec3,
        all_frames: bool,
    ) {
        for band in 0..self.rings.len() {
            let b = self.bands.bands[band];
            let mut dirty: Vec<u32> = if all_frames {
                (0..b.slot_count).collect()
            } else {
                self.rings[band].dirty_slots().to_vec()
            };
            let headers: std::collections::HashSet<u32> =
                self.rings[band].dirty_slots().iter().copied().collect();
            dirty.sort_unstable();
            dirty.dedup();
            let mut index = 0usize;
            while index < dirty.len() {
                let start = dirty[index];
                let mut end = index;
                while end + 1 < dirty.len() && dirty[end + 1] == dirty[end] + 1 {
                    end += 1;
                }
                let last = dirty[end];
                self.frame_scratch.clear();
                for local in start..=last {
                    let frame = self.rings[band]
                        .slot_coord(local)
                        .and_then(|coord| self.band_frame(chart, band, coord));
                    self.frame_scratch
                        .push(frame.map_or(GpuTileFrame::default(), |f| f.gpu(world_origin)));
                }
                ctx.write_buffer(
                    &self.tile_frames,
                    u64::from(b.slot_base + start) * FRAME_BYTES,
                    bytemuck::cast_slice(&self.frame_scratch),
                );
                // The headers of the slots the ring changed (a frame rewritten for a moved
                // origin keeps its tile's header and records).
                self.header_scratch.clear();
                let flush = |scratch: &mut Vec<GpuFoliageTile>, next: u32| {
                    if !scratch.is_empty() {
                        let first = b.slot_base + next - scratch.len() as u32;
                        ctx.write_buffer(
                            &self.tile_table,
                            u64::from(first) * TILE_BYTES,
                            bytemuck::cast_slice(scratch),
                        );
                        scratch.clear();
                    }
                };
                let mut scratch = std::mem::take(&mut self.header_scratch);
                for local in start..=last {
                    if !headers.contains(&local) {
                        flush(&mut scratch, local);
                        continue;
                    }
                    scratch.push(match self.rings[band].slot_coord(local) {
                        Some(coord) => GpuFoliageTile {
                            tile_coord: coord,
                            blade_offset: b.arena_base + local * b.slab,
                            blade_count: 0,
                            bounds_center: 0.0,
                            bounds_half: 0.0,
                            // The GPU flips this to `Resident` at the end of `cs_place`.
                            // Publishing `Resident` from here would let the cull read a
                            // slab placement has not written yet.
                            state: TileState::Placing.as_u32(),
                            generation,
                        },
                        None => GpuFoliageTile::EMPTY,
                    });
                }
                flush(&mut scratch, last + 1);
                self.header_scratch = scratch;
                index = end + 1;
            }
        }
    }

    /// The cull's constants and dispatch sizes, and the counters' readback.
    fn prepare_cull(&mut self, ctx: &PrepareContext, type_count: u32) -> HelioResult<()> {
        // 2D dispatch grid, the way `vg_cull.wgsl` does it: a large table on a device
        // with a small single-dimension limit must still fit.
        let max_dispatch = ctx
            .device
            .limits()
            .max_compute_workgroups_per_dimension
            .max(1);
        let cluster_groups = (self.total_slots as u64 * self.clusters_per_tile as u64)
            .div_ceil(WORKGROUP_SIZE as u64)
            .max(1) as u32;
        self.cluster_dispatch_width = cluster_groups.min(max_dispatch).max(1);
        self.cluster_dispatch_height = cluster_groups.div_ceil(self.cluster_dispatch_width);
        self.tile_dispatch_groups = self.total_slots.div_ceil(WORKGROUP_SIZE).max(1);

        let max_dim = ctx.width.max(ctx.height).max(1);
        let hiz_mip_count = (u32::BITS - max_dim.leading_zeros()).max(1);
        // Frame 0 has no pyramid: its texels are not depth anything drew, and testing
        // against them can cull everything. Same guard as `vg_cull.wgsl`.
        let hiz_valid = (ctx.frame_num > 0 && ctx.registry.contains("hiz")) as u32;

        ctx.write_buffer(
            &self.cull_uniforms,
            0,
            bytemuck::bytes_of(&FoliageCullUniforms {
                tile_count: self.total_slots,
                screen_width: ctx.width,
                screen_height: ctx.height,
                hiz_mip_count,
                hiz_valid,
                cluster_size: self.cluster_size,
                clusters_per_tile: self.clusters_per_tile,
                per_lod_capacity: FOLIAGE_VISIBLE_PER_LOD_CAPACITY,
                tile_size: FOLIAGE_TILE_SIZE_METERS,
                lod_quality_scale: self.quality.lod_distance_scale(),
                type_count,
                cluster_dispatch_width: self.cluster_dispatch_width,
                max_foliage_height: MAX_FOLIAGE_HEIGHT,
                wpo_extent: DEFAULT_WPO_EXTENT_METERS,
                lod_fade_band: FOLIAGE_LOD_FADE_BAND_METERS,
                _pad: [0; 1],
            }),
        );

        self.pump_readback();
        if ctx.frame_num % 120 == 0 {
            let c = self.debug_counters;
            log::debug!(
                "[foliage] ground {} chart {:?} resident {:?} batch {} answered {:?} queued_tiles={} \
                 placed={} visible L0={} L1={} L2={} L3={} visible_overflow={} placement_overflow={}",
                if self.ground.is_external() {
                    "elsewhere"
                } else {
                    "built in"
                },
                self.chart,
                self.rings
                    .iter()
                    .map(TileRing::resident_count)
                    .collect::<Vec<_>>(),
                self.batch,
                self.ground.answered(),
                self.queued_tile_count,
                c[COUNTER_PLACED_BLADES],
                c[0],
                c[1],
                c[2],
                c[3],
                c[COUNTER_VISIBLE_OVERFLOW],
                c[COUNTER_PLACEMENT_OVERFLOW],
            );
        }

        self.active = true;
        Ok(())
    }
}

impl RenderPass for FoliagePlacePass {
    fn scene_buffers(&self) -> &'static [&'static str] {
        &["foliage_layers", "foliage_types", "foliage_interactors"]
    }

    fn name(&self) -> &'static str {
        "FoliagePlace"
    }

    fn reads(&self) -> &'static [&'static str] {
        &["hiz"]
    }

    fn writes(&self) -> &'static [&'static str] {
        &["foliage_draws", GROUND_QUERIES, GROUND_HITS, GROUND_ARGS]
    }

    /// The ground exchange, for a ground answered by another pass (`contract::ground`).
    fn publish<'a>(&self, frame: &mut helio_core::ResourceRegistry<'a>) {
        frame.write_buffer_binding(GROUND_QUERIES, &self.queries, "FoliagePlace");
        frame.write_buffer_binding(GROUND_HITS, &self.hits, "FoliagePlace");
        frame.write_buffer_binding(GROUND_ARGS, &self.ground_args, "FoliagePlace");
    }

    fn render_pass_descriptor<'a>(
        &'a self,
        _target: &'a wgpu::TextureView,
        _depth: &'a wgpu::TextureView,
        _resources: &'a helio_core::ResourceRegistry<'a>,
    ) -> Option<wgpu::RenderPassDescriptor<'a>> {
        None
    }

    fn prepare(&mut self, ctx: &PrepareContext) -> HelioResult<()> {
        // SceneDB is the only foliage authoring input. Zero type rows: no work.
        self.active = false;
        let Some(type_handle) = ctx.scene_buffers.get(BufferKey::of("foliage_types")) else {
            return Ok(());
        };
        self.type_liveness
            .update(ctx.device, ctx.queue, Some(type_handle));
        if !self.type_liveness.maybe_live(type_handle) {
            return Ok(());
        }
        let mut type_count = (type_handle.buffer.size() / TYPE_BYTES) as u32;
        if type_count > MAX_FOLIAGE_TYPES {
            if !self.warned_type_overflow {
                log::warn!(
                    "{type_count} foliage types published but GpuBladeInstance stores the type id in 8 bits; \
                     clamping to {MAX_FOLIAGE_TYPES}"
                );
                self.warned_type_overflow = true;
            }
            type_count = MAX_FOLIAGE_TYPES;
        }
        let mut layer_count = ctx
            .scene_buffers
            .get(BufferKey::of("foliage_layers"))
            .map(|h| (h.buffer.size() / LAYER_BYTES) as u32)
            .unwrap_or(0);
        if layer_count > MAX_FOLIAGE_LAYERS {
            log::warn!(
                "{layer_count} foliage layers published but the layer table holds {MAX_FOLIAGE_LAYERS}; clamping"
            );
            layer_count = MAX_FOLIAGE_LAYERS;
        }

        // The ground's chart around the eye. Tiles of one chart only are resident: a new
        // chart (another cube face under the eye, another ground) places every tile again.
        let world_origin = ctx.world_origin;
        // The interactors are gathered against this frame's origin.
        {
            let hi = world_origin.as_vec3();
            let lo = (world_origin - hi.as_dvec3()).as_vec3();
            self.gather_rows = ctx
                .scene_buffers
                .get(BufferKey::of("foliage_interactors"))
                .map_or(0, |h| (h.buffer.size() / SOURCE_INTERACTOR_BYTES) as u32);
            let mut words = [0u32; 12];
            words[..3].copy_from_slice(&hi.to_array().map(f32::to_bits));
            words[4..7].copy_from_slice(&lo.to_array().map(f32::to_bits));
            words[8] = self.gather_rows;
            ctx.write_buffer(&self.gather_uniforms, 0, bytemuck::cast_slice(&words));
        }
        let position_near = ctx.camera_data.position_near;
        let camera = glam::Vec3::new(position_near[0], position_near[1], position_near[2]);
        let eye = world_origin + camera.as_dvec3();
        let Some(chart) = self.ground.geometry().chart(eye) else {
            // The ground cannot tile yet (not loaded): draw what is resident.
            self.place_tile_count = 0;
            self.queued_tile_count = 0;
            self.query_count = 0;
            return self.prepare_cull(ctx, type_count);
        };
        let chart_changed = self.chart != Some(chart.id);
        self.chart = Some(chart.id);
        let origin_moved = self.world_origin != Some(world_origin);
        self.world_origin = Some(world_origin);

        // Residency is keyed on (tile_coord, generation): a tile is re-placed whenever the
        // type or layer table's contents, or the chart, change. The records' seed follows
        // the type table alone.
        let mix = |handle: &helio_core::BufferHandle| {
            handle.epoch
                ^ handle
                    .content_generation
                    .wrapping_mul(0x9e37_79b9_7f4a_7c15)
        };
        let seed_generation = mix(type_handle) as u32;
        let layers = ctx.scene_buffers.get(BufferKey::of("foliage_layers"));
        let generation = (mix(type_handle)
            ^ layers.map_or(0, |h| mix(h).rotate_left(29))
            ^ chart.id.wrapping_mul(0xc2b2_ae3d_27d4_eb4f).rotate_left(13))
            as u32;
        if chart_changed {
            self.asked.clear();
        }
        let ring_eye = [chart.eye[0] as f32, chart.eye[1] as f32];
        let mut invalidated = false;
        for ring in &mut self.rings {
            let update = ring.update(ring_eye, generation);
            invalidated |= update.invalidated;
            if update.evicted > 0 {
                log::debug!(
                    "foliage ring evicted {} resident tiles to make room",
                    update.evicted
                );
            }
        }

        // Last frame's tiles, placed from their answers where they came: the ground
        // answered the batch (the built-in plane always does), the origin the queries
        // were written in still holds, and the slot still holds the tile. The others are
        // asked again.
        let answered = !self.ground.is_external() || self.ground.answered() == Some(self.batch);
        let asked = std::mem::take(&mut self.asked);
        self.queue_scratch.clear();
        for a in &asked {
            let base = self.bands.bands[a.band].slot_base;
            let held = self.rings[a.band].slot_coord(a.slot - base) == Some(a.coord);
            if held && answered && !origin_moved && !invalidated {
                self.queue_scratch.push([a.slot, a.first]);
            } else if held {
                self.rings[a.band].refresh(a.coord);
            }
        }
        self.place_tile_count = self.queue_scratch.len() as u32;

        // The ground's changes since last frame: the resident tiles under them are placed
        // again (their records draw meanwhile).
        for [lo, hi] in self.ground.take_changes() {
            let mut range: Option<([f64; 2], [f64; 2])> = None;
            for corner in 0..8 {
                let p = DVec3::new(
                    if corner & 1 == 0 { lo.x } else { hi.x },
                    if corner & 2 == 0 { lo.y } else { hi.y },
                    if corner & 4 == 0 { lo.z } else { hi.z },
                );
                let Some(at) = self.ground.geometry().locate(chart.id, p) else {
                    continue;
                };
                range = Some(match range {
                    None => (at, at),
                    Some((a, b)) => (
                        [a[0].min(at[0]), a[1].min(at[1])],
                        [b[0].max(at[0]), b[1].max(at[1])],
                    ),
                });
            }
            if let Some((a, b)) = range {
                for (band, ring) in self.rings.iter_mut().enumerate() {
                    let size = f64::from(self.bands.bands[band].tile_size);
                    let tile = |v: f64| (v / size).floor() as i32;
                    for coord in
                        ring.resident_within([tile(a[0]), tile(a[1])], [tile(b[0]), tile(b[1])])
                    {
                        ring.refresh(coord);
                    }
                }
            }
        }

        // This frame's questions: each band's entering tiles (budgeted by its ring), then
        // those placed again, nearest band first, while the queries fit.
        self.upload_tile_headers(
            ctx,
            seed_generation,
            chart.id,
            world_origin,
            origin_moved || chart_changed,
        );
        self.query_count = 0;
        let mut queued = Vec::new();
        for band in 0..self.rings.len() {
            let b = self.bands.bands[band];
            let cells = b.candidate_grid * b.candidate_grid;
            let locals: Vec<u32> = self.rings[band]
                .place_queue()
                .iter()
                .chain(self.rings[band].refresh_queue())
                .copied()
                .collect();
            for local in locals {
                let Some(coord) = self.rings[band].slot_coord(local) else {
                    continue;
                };
                let fits = self.query_count + cells <= QUERY_CAPACITY
                    && (self.place_tile_count + queued.len() as u32) < 2 * MAX_QUEUED_TILES
                    && (queued.len() as u32) < MAX_QUEUED_TILES;
                if fits {
                    queued.push(Asked {
                        band,
                        slot: b.slot_base + local,
                        coord,
                        first: self.query_count,
                    });
                    self.query_count += cells;
                } else {
                    self.rings[band].refresh(coord);
                }
            }
        }
        self.asked = queued;
        self.queued_tile_count = self.asked.len() as u32;
        if self.queued_tile_count > 0 {
            self.batch += 1;
            if self.ground.is_external() {
                // What the answering pass needs besides the queries: each probe block's
                // frame and where its queries start (as `cs_query` takes it, in f64).
                let mut tiles = Vec::new();
                for a in &self.asked {
                    let b = self.bands.bands[a.band];
                    let Some(frame) = self.band_frame(chart.id, a.band, a.coord) else {
                        continue;
                    };
                    let up = frame.up.normalize_or(DVec3::Y);
                    let start_height =
                        (eye - frame.origin).dot(up) + f64::from(SEARCH_ABOVE_METRES);
                    let blocks = b.probe_blocks.max(1);
                    let per_block = (b.candidate_grid / blocks).pow(2);
                    for block in 0..blocks * blocks {
                        let (bx, by) = (f64::from(block % blocks), f64::from(block / blocks));
                        let n = f64::from(blocks);
                        tiles.push(crate::AskedTile {
                            frame: frame.part([bx / n, by / n], [(bx + 1.0) / n, (by + 1.0) / n]),
                            start_height,
                            first: a.first + block * per_block,
                            count: per_block,
                        });
                    }
                }
                self.ground.ask(self.batch, tiles);
            }
        }
        self.queue_scratch
            .extend(self.asked.iter().map(|a| [a.slot, a.first]));
        if !self.queue_scratch.is_empty() {
            ctx.write_buffer(
                &self.place_queue,
                0,
                bytemuck::cast_slice(&self.queue_scratch),
            );
        }
        // Each tile's candidates in parts of a workgroup each: a frame's few tiles are
        // placed and queried across the GPU, not a workgroup a tile.
        self.parts_scratch.clear();
        let mut part_counts = [0u32; 2];
        for (entry, &[slot, _]) in self.queue_scratch.iter().enumerate() {
            let Some(band) = self.bands.band_of_slot(slot) else {
                continue;
            };
            let cells = self.bands.bands[band].candidate_grid.pow(2);
            let queried = usize::from(entry as u32 >= self.place_tile_count);
            let mut start = 0;
            while start < cells && self.parts_scratch.len() < MAX_PARTS as usize {
                let end = (start + PLACE_PART).min(cells);
                self.parts_scratch.push([entry as u32, start, end, 0]);
                part_counts[queried] += 1;
                start = end;
            }
        }
        [self.place_part_count, self.query_part_count] = part_counts;
        if !self.parts_scratch.is_empty() {
            ctx.write_buffer(
                &self.place_parts,
                0,
                bytemuck::cast_slice(&self.parts_scratch),
            );
        }

        let (terrain_origin, terrain_extent, terrain_valid) = match self.terrain_transform {
            Some((origin, extent)) if ctx.registry.contains("foliage_terrain") => {
                (origin, extent, 1)
            }
            _ => ([0.0, 0.0], 1.0, 0),
        };
        ctx.write_buffer(
            &self.place_uniforms,
            0,
            bytemuck::bytes_of(&PlaceUniforms {
                cluster_edge: isqrt(self.cluster_size).max(1),
                place_tile_count: self.place_tile_count,
                query_tile_count: self.queued_tile_count,
                query_count: self.query_count,
                density_multiplier: self.quality.density_multiplier(),
                type_count,
                max_foliage_height: MAX_FOLIAGE_HEIGHT,
                terrain_valid,
                terrain_origin_x: terrain_origin[0],
                terrain_origin_z: terrain_origin[1],
                terrain_extent,
                layer_count,
                search_above: SEARCH_ABOVE_METRES,
                search_below: SEARCH_BELOW_METRES,
                eye: camera.to_array(),
                place_part_count: self.place_part_count,
                query_part_count: self.query_part_count,
                _pad: 0,
            }),
        );
        self.prepare_cull(ctx, type_count)
    }

    fn execute(&mut self, ctx: &mut PassContext) -> HelioResult<()> {
        if !self.active {
            return Ok(());
        }

        let hiz_view = ctx
            .resource_pool
            .get_view_2d("hiz")
            .unwrap_or(&self.placeholder_hiz);
        let hiz_sampler = ctx
            .registry
            .get(helio_core::ResourceKey::new("hiz_sampler"))
            .unwrap_or(&self.placeholder_hiz_sampler);
        let type_buffer = ctx
            .scene_buffers
            .get(BufferKey::of("foliage_types"))
            .map(|h| &h.buffer)
            .unwrap_or(&self.placeholder_type);
        let layer_buffer = ctx
            .scene_buffers
            .get(BufferKey::of("foliage_layers"))
            .map(|h| &h.buffer)
            .unwrap_or(&self.placeholder_layer);
        let place_key = (
            type_buffer.clone(),
            layer_buffer.clone(),
            self.placeholder_terrain.clone(),
        );
        if self.place_bind_group_key.as_ref() != Some(&place_key) {
            self.place_bind_group =
                Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("Foliage Place BG"),
                    layout: &self.place_pipeline.get_bind_group_layout(0),
                    entries: &[
                        entry(0, self.place_uniforms.as_entire_binding()),
                        entry(1, type_buffer.as_entire_binding()),
                        entry(2, self.tile_table.as_entire_binding()),
                        entry(3, self.blade_arena.as_entire_binding()),
                        entry(4, self.place_queue.as_entire_binding()),
                        entry(5, self.counters.as_entire_binding()),
                        entry(
                            6,
                            wgpu::BindingResource::TextureView(&self.placeholder_terrain),
                        ),
                        entry(7, wgpu::BindingResource::Sampler(&self.terrain_sampler)),
                        entry(8, layer_buffer.as_entire_binding()),
                        entry(9, self.tile_frames.as_entire_binding()),
                        entry(10, self.queries.as_entire_binding()),
                        entry(11, self.hits.as_entire_binding()),
                        entry(12, self.ground_args.as_entire_binding()),
                        entry(13, self.bands_buffer.as_entire_binding()),
                        entry(14, self.place_parts.as_entire_binding()),
                        entry(15, self.place_scratch.as_entire_binding()),
                    ],
                }));
            self.place_bind_group_key = Some(place_key);
        }
        let key = (
            ctx.camera.clone(),
            hiz_view.clone(),
            hiz_sampler.clone(),
            type_buffer.clone(),
        );
        if self.cull_bind_group_key.as_ref() != Some(&key) {
            self.cull_bind_group = Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Foliage Cull BG"),
                layout: &self.cull_bgl,
                entries: &[
                    entry(0, ctx.camera.as_entire_binding()),
                    entry(1, self.cull_uniforms.as_entire_binding()),
                    entry(2, self.tile_table.as_entire_binding()),
                    entry(3, self.blade_arena.as_entire_binding()),
                    entry(4, type_buffer.as_entire_binding()),
                    entry(5, self.visible_blades.as_entire_binding()),
                    entry(6, self.counters.as_entire_binding()),
                    entry(7, self.foliage_indirect.as_entire_binding()),
                    entry(8, self.tile_visibility.as_entire_binding()),
                    entry(9, wgpu::BindingResource::TextureView(hiz_view)),
                    entry(10, wgpu::BindingResource::Sampler(hiz_sampler)),
                    entry(11, self.tile_frames.as_entire_binding()),
                    entry(12, self.bands_buffer.as_entire_binding()),
                ],
            }));
            self.cull_bind_group_key = Some(key);
        }
        let Some(cull_bg) = self.cull_bind_group.as_ref() else {
            return Ok(());
        };

        // `ctx.graphics_cmds()`, not `ctx.compute_cmds()`: the two streams are submitted
        // as [compute, graphics], so anything recorded on the compute stream runs before
        // *all* graphics work and would Hi-Z-test against the previous frame's pyramid.
        let mut encoder = ctx.graphics_cmds();

        // Visible counts are per-frame; the overflow counters are too, so a single bad
        // frame does not look like a permanent budget failure.
        encoder.clear_buffer(&self.counters, 0, None);

        // This frame's interactors, gathered from wherever their rows are.
        encoder.clear_buffer(&self.interactors, 0, Some(16));
        if self.gather_rows > 0 {
            let source = ctx
                .scene_buffers
                .get(BufferKey::of("foliage_interactors"))
                .map(|h| &h.buffer)
                .unwrap_or(&self.placeholder_interactor);
            if self
                .gather_bind_group
                .as_ref()
                .is_none_or(|(key, _)| key != source)
            {
                let group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("Foliage Interactors BG"),
                    layout: &self.gather_pipeline.get_bind_group_layout(0),
                    entries: &[
                        entry(0, self.gather_uniforms.as_entire_binding()),
                        entry(1, source.as_entire_binding()),
                        entry(2, self.interactors.as_entire_binding()),
                    ],
                });
                self.gather_bind_group = Some((source.clone(), group));
            }
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Foliage Interactors"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.gather_pipeline);
            pass.set_bind_group(0, &self.gather_bind_group.as_ref().unwrap().1, &[]);
            pass.dispatch_workgroups(self.gather_rows.div_ceil(WORKGROUP_SIZE), 1, 1);
        }

        if self.place_tile_count > 0 || self.queued_tile_count > 0 {
            let Some(place_bg) = self.place_bind_group.as_ref() else {
                return Ok(());
            };
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Foliage Place"),
                timestamp_writes: None,
            });
            pass.set_bind_group(0, place_bg, &[]);
            // Last frame's tiles from their answers (before this frame's queries take
            // their place in the exchange buffers): their ranges, then their tufts, a
            // workgroup a part, then their headers.
            if self.place_tile_count > 0 && self.place_part_count > 0 {
                let tiles = self.place_tile_count.div_ceil(WORKGROUP_SIZE);
                pass.set_pipeline(&self.place_reset_pipeline);
                pass.dispatch_workgroups(tiles, 1, 1);
                pass.set_pipeline(&self.place_bounds_pipeline);
                pass.dispatch_workgroups(self.place_part_count, 1, 1);
                pass.set_pipeline(&self.place_pipeline);
                pass.dispatch_workgroups(self.place_part_count, 1, 1);
                pass.set_pipeline(&self.place_finish_pipeline);
                pass.dispatch_workgroups(tiles, 1, 1);
            }
            // This frame's questions, a workgroup a part; the built-in ground answers them
            // at once, another ground in its own pass later in the frame.
            if self.queued_tile_count > 0 && self.query_part_count > 0 {
                pass.set_pipeline(&self.query_pipeline);
                pass.dispatch_workgroups(self.query_part_count, 1, 1);
                // Directly: the queries are counted here, and the bind group holds the
                // indirect header writable (one usage per dispatch).
                if !self.ground.is_external() {
                    pass.set_pipeline(&self.ground_plane_pipeline);
                    pass.dispatch_workgroups(
                        self.query_count.div_ceil(WORKGROUP_SIZE).max(1),
                        1,
                        1,
                    );
                }
            }
        }

        for (label, pipeline, groups) in [
            (
                "Foliage Tile Cull",
                &self.tile_cull_pipeline,
                [self.tile_dispatch_groups, 1],
            ),
            (
                "Foliage Cluster Cull",
                &self.cluster_cull_pipeline,
                [self.cluster_dispatch_width, self.cluster_dispatch_height],
            ),
            ("Foliage Finalize", &self.finalize_pipeline, [1, 1]),
        ] {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some(label),
                timestamp_writes: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, cull_bg, &[]);
            pass.dispatch_workgroups(groups[0], groups[1], 1);
        }

        if matches!(self.readback_state, ReadbackState::Idle) {
            encoder.copy_buffer_to_buffer(
                &self.counters,
                0,
                &self.counters_readback,
                0,
                FOLIAGE_COUNTER_COUNT as u64 * 4,
            );
            self.readback_state = ReadbackState::CopySubmitted;
        }

        self.commands_recorded += 1;
        Ok(())
    }
}

fn entry(binding: u32, resource: wgpu::BindingResource<'_>) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry { binding, resource }
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

/// State of the non-blocking counter readback. Never blocks the GPU; values may trail the
/// rendered frame by a frame or two, which is fine for telemetry.
enum ReadbackState {
    Idle,
    CopySubmitted,
    Mapping(Arc<std::sync::Mutex<Option<Result<(), wgpu::BufferAsyncError>>>>),
}

/// Integer square root (exact; `(x as f32).sqrt()` loses precision above 2^24).
fn isqrt(value: u32) -> u32 {
    if value == 0 {
        return 0;
    }
    let value = u64::from(value);
    let mut guess = u64::from((value as f64).sqrt() as u32);
    while guess > 0 && guess * guess > value {
        guess -= 1;
    }
    while (guess + 1) * (guess + 1) <= value {
        guess += 1;
    }
    guess as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isqrt_is_exact_at_and_around_perfect_squares() {
        assert_eq!(isqrt(0), 0);
        assert_eq!(isqrt(1), 1);
        for root in 1u32..1024 {
            let square = root * root;
            assert_eq!(isqrt(square), root);
            assert_eq!(isqrt(square - 1), root - 1);
            assert_eq!(isqrt(square + 1), root);
        }
        assert_eq!(isqrt(u32::MAX), 65535);
        assert_eq!(isqrt(1 << 30), 1 << 15);
    }

    #[test]
    fn every_preset_plans_bands_that_fit_the_query_budget_and_index_space() {
        for quality in [
            FoliageQuality::Low,
            FoliageQuality::Medium,
            FoliageQuality::High,
            FoliageQuality::Ultra,
        ] {
            let bands = plan_bands(&FoliageRange {
                range: quality.range(),
                cluster_edge: isqrt(quality.cluster_granularity()),
                ..FoliageRange::default()
            });
            for b in bands.bands() {
                // A tile's queries fit one frame's budget, and its records the 16-bit
                // tile-local index of a visible reference.
                assert!(
                    b.candidate_grid * b.candidate_grid <= QUERY_CAPACITY,
                    "{quality:?}: {}² candidates",
                    b.candidate_grid
                );
                assert!(b.slab <= crate::MAX_BLADES_PER_TILE);
            }
            // The arena stays bounded however far the bands reach.
            assert!(
                bands.records() * BLADE_BYTES < 96 * 1024 * 1024,
                "{quality:?}: {} records",
                bands.records()
            );
        }
    }

    #[test]
    fn gpu_record_sizes_match_the_wgsl_mirrors() {
        assert_eq!(TILE_BYTES, 32);
        assert_eq!(FRAME_BYTES, 64);
        assert_eq!(BLADE_BYTES, 16);
        assert_eq!(TYPE_BYTES, 128);
        assert_eq!(LAYER_BYTES, 32);
        assert_eq!(std::mem::size_of::<GpuFoliageBands>(), 416);
        assert_eq!(MAX_FOLIAGE_TYPES, 256, "the 8-bit type id caps this");
    }
}
