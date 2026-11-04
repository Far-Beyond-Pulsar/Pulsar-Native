//! Foliage G-buffer pass — grass blades and cards rasterised into the shared 8-target
//! G-buffer.
//!
//! Phase 2 of the foliage plan (`docs/foliage_rendering_plan.md`). This is the consumer
//! half of the grass pipeline: `FoliagePlacePass` decides *which* blades are visible and
//! at what LOD, and this pass turns those four compacted index lists into four
//! `draw_indirect` calls that fill albedo, normal, ORM, emissive and velocity.
//!
//! # Why it draws into the G-buffer and not a composite
//!
//! `BillboardPass` exists and would have been the cheap way to get vegetation on screen.
//! It composites into `pre_aa` *after* deferred lighting, so anything drawn through it
//! receives no shadows, no SSAO, no GI and no correct TAA. Foliage that does not go
//! through the G-buffer cannot be lit like the rest of the scene, and that is not a
//! quality knob — it is the difference between grass that sits in the world and grass
//! that floats on top of a screenshot of it.
//!
//! # Four draws, no vertex buffer, no index buffer
//!
//! `PrimitiveTopology::TriangleStrip`, `strip_index_format: None`, `cull_mode: None`,
//! and every vertex derived procedurally from `@builtin(vertex_index)`. Per-instance
//! strip restart is guaranteed by the WebGPU spec's primitive-assembly algorithm —
//! assembly runs per instance and a strip is split on the restart value only for
//! *indexed* draws — so a blade is one primitive with no degenerate stitch triangles.
//! `strip_index_format: None` has no effect on a non-indexed draw, but on Vulkan a
//! `Some` value sets `primitiveRestartEnable`, and `None` is the correct state here.
//!
//! Exactly four `draw_indirect` calls covers all the grass in the world on every
//! backend, including WebGPU, which has no `MULTI_DRAW_INDIRECT_COUNT`.
//!
//! # The 8-target pipeline with masked writes
//!
//! Grass writes neither `sss` nor `extra`, and the obvious optimisation —
//! a 5-target pipeline — is invalid. A pipeline's fragment targets must match the render
//! pass's colour attachments in count and format (`RenderPassContext::check_compatible`
//! compares the lists element-wise), so a 5-target pipeline needs its own render pass,
//! which forfeits subpass fusion. Breaking the chain forces a store and reload of every
//! touched attachment; at 1080p and 52 bytes/sample that is ~100 MiB each way on a
//! tile-based GPU, far more than the lever was ever going to save.
//!
//! So: all eight targets, identical formats to `GBufferPass`'s, and
//! [`wgpu::ColorWrites::empty()`] on the two grass does not write, with those
//! `@location`s omitted from the fragment shader. **The empty write mask is mandatory,
//! not cosmetic** — a declared fragment target with no shader output has an undefined
//! value, so without the mask those G-buffer channels are filled with garbage wherever
//! grass covers a pixel, and the deferred pass reads them everywhere.
//!
//! `lightmap_uv` *is* written: `(-1, -1.5)`, foliage's tag. Negative x is "no
//! lightmap" (its clear value `(0, 0)` reads as a lightmap at the atlas's corner, and
//! the deferred pass lit grass by that empty texel alone: black); y -1.5 (clear of the
//! voxel pass's -2 and of portal chains, below -2.5) keeps screen-space AO off grass,
//! whose dense blades overwhelm the kernel (black fields) and which carries its own
//! height AO.
//!
//! # Zero overhead when absent
//!
//! When the SceneDB `foliage_types` column is absent, [`FoliageGBufferPass::prepare`] early-returns
//! before any upload and [`FoliageGBufferPass::execute`] records nothing. It still
//! returns `Some(descriptor)` from [`FoliageGBufferPass::render_pass_descriptor`]
//! whenever the G-buffer views exist. That is not an oversight: a pass that
//! conditionally returns `None` based on per-frame state can never participate in
//! subpass fusion, because the executor probes chain membership *once*, at lock time.
//! `BillboardPass` documents the same trap around `helio_pass_billboard`'s `BillboardPass::prepare`.
//!
//! # Pass order
//!
//! This pass must sit **immediately after `GBufferPass` and before
//! `VirtualGeometryPass`**, returning the byte-identical eight views in the same order.
//! Chain formation requires an exact attachment-view match
//! (`helio-core/src/graph/scheduling.rs:82-88`), and `VirtualGeometryPass` binds only
//! seven attachments — it omits `gbuffer_velocity` — so it can neither fuse with this
//! pass nor be skipped over by the chain scan (it is a raster pass, not
//! `chain_transparent`). Placing foliage after VG makes the fusion impossible.

pub mod feature;
pub use feature::FoliageFeature;

mod geometry;

pub use geometry::*;

use std::num::NonZeroU64;
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use helio_core::graph::ResourceBuilder;
use helio_core::{PassContext, PrepareContext, RenderPass, Result as HelioResult};
use helio_pass_foliage_place::{FoliageQuality, GpuFoliageType};
use pulsar_scenedb::gpu::BufferKey;

// ═══════════════════════════════════════════════════════════════════════════════
// Interface contract with `FoliagePlacePass`
// ═══════════════════════════════════════════════════════════════════════════════

/// Stride of one `wgpu::util::DrawIndirectArgs` in the `foliage_indirect` buffer.
///
/// `vertex_count`, `instance_count`, `first_vertex`, `first_instance` — four `u32`.
/// The four LOD commands live at byte offsets 0, 16, 32 and 48.
pub const DRAW_INDIRECT_STRIDE: u64 = 16;

// ── Packed `visible_blades[]` entry ─────────────────────────────────────────────
//
// The live encoding. A `visible_blades[]` entry is **not** a flat arena index: it is the
// owning tile's ring slot in the high 16 bits and the blade's tile-local index in the low
// 16. `foliage_cull.wgsl` writes it, `foliage_gbuffer.wgsl` reads it back with the same
// shift and mask, and both must change together.
//
// Packing rather than a flat index because the consumer needs the *tile* regardless — a
// blade's stored position is tile-local, so reconstructing a world position requires the
// tile's origin and `bounds_center`. Handing the slot over directly saves a division
// per vertex in the hottest shader in the foliage path. `tile_slot_of_blade` above is the
// flat-index alternative, kept only for callers holding a raw arena index.
//
// Both halves have room: the ring is 4096 slots and a tile's slab is a few hundred
// blades, against a 65 536 ceiling each.

/// Bits the owning tile slot is shifted left by in a `visible_blades[]` entry.
pub const VISIBLE_TILE_SHIFT: u32 = 16;

/// Mask selecting the tile-local blade index from a `visible_blades[]` entry.
pub const VISIBLE_LOCAL_MASK: u32 = 0xffff;

/// Pack a `(tile_slot, tile_local_index)` pair into a `visible_blades[]` entry.
#[inline]
pub const fn pack_visible_blade(tile_slot: u32, local_index: u32) -> u32 {
    (tile_slot << VISIBLE_TILE_SHIFT) | (local_index & VISIBLE_LOCAL_MASK)
}

/// Inverse of [`pack_visible_blade`].
#[inline]
pub const fn unpack_visible_blade(packed: u32) -> (u32, u32) {
    (packed >> VISIBLE_TILE_SHIFT, packed & VISIBLE_LOCAL_MASK)
}

/// Element offset of LOD `lod`'s region in `visible_blades[]`, given the per-region
/// stride.
///
/// # The region layout this pass assumes
///
/// `visible_blades` is one buffer holding four **equal-size, contiguous** regions, in
/// LOD order, and region `r` begins at element `r * stride`. The stride is derived from
/// the buffer's own size in [`FoliageGBufferPass::new`] as
/// `buffer.size() / 4 / size_of::<u32>()`, so the producer sizes the buffer and the
/// consumer follows — there is no third place for the two to disagree. For
/// `FoliagePlacePass`'s 16 MiB buffer that comes out at `1 << 20` elements per region,
/// which is what its `FOLIAGE_VISIBLE_PER_LOD_CAPACITY` states.
///
/// Equal-size regions waste a lot of space, and that is deliberate: per-LOD occupancy is
/// bimodal and camera-dependent — a camera looking straight down puts nearly everything
/// in L0, one on the horizon puts nearly everything in L3 — so any unequal split has an
/// ordinary camera angle that overflows it. A single stride also means the region base is
/// one multiply in the shader instead of a table lookup.
///
/// The producer must additionally write `first_instance = 0` and `first_vertex = 0` into
/// every `DrawIndirectArgs`: the region base is applied by this pass from its per-LOD
/// uniform, and WebGPU starts `@builtin(instance_index)` at `firstInstance`, so a
/// non-zero value there would offset the region twice.
#[inline]
pub const fn visible_region_offset(lod: u32, stride: u32) -> u32 {
    lod * stride
}

// ═══════════════════════════════════════════════════════════════════════════════
// Pipeline contract
// ═══════════════════════════════════════════════════════════════════════════════

/// The eight G-buffer colour formats, in the exact order and format `GBufferPass`
/// declares them.
///
/// Duplicated here rather than imported because importing them would make this crate
/// depend on `helio-pass-gbuffer` for a constant. The list is pinned against the real
/// one in `tests/pipeline_contract.rs`; if that ever fails, the fix is here, not there.
pub const GBUFFER_TARGET_FORMATS: [wgpu::TextureFormat; 8] = [
    wgpu::TextureFormat::Rgba8Unorm,  // 0 albedo
    wgpu::TextureFormat::Rgba16Float, // 1 normal
    wgpu::TextureFormat::Rgba8Unorm,  // 2 orm
    wgpu::TextureFormat::Rgba16Float, // 3 emissive
    wgpu::TextureFormat::Rg16Float,   // 4 lightmap_uv
    wgpu::TextureFormat::Rgba16Float, // 5 sss
    wgpu::TextureFormat::Rgba16Float, // 6 extra
    wgpu::TextureFormat::Rgba16Float, // 7 velocity (depth residual reserved)
];

/// The two targets grass does not write, which therefore carry
/// [`wgpu::ColorWrites::empty()`].
///
/// See the crate docs: this is the corrected §12 perf lever, and the mask is what makes
/// it *correct* rather than merely cheap.
pub const UNWRITTEN_TARGET_INDICES: [usize; 2] = [5, 6];

/// The pipeline's fragment target list.
///
/// A function rather than a `const` because [`wgpu::ColorTargetState`] is not
/// const-constructible; the test in `tests/pipeline_contract.rs` reads it, so the thing
/// the pipeline is built from is the thing that is asserted.
pub fn color_target_states() -> [Option<wgpu::ColorTargetState>; 8] {
    std::array::from_fn(|index| {
        Some(wgpu::ColorTargetState {
            format: GBUFFER_TARGET_FORMATS[index],
            // No blending: blending grass into the G-buffer would write a half-weight
            // normal and a half-weight depth, which is not a surface any deferred
            // lighting model can interpret. LODs switch whole tufts instead.
            blend: None,
            write_mask: if UNWRITTEN_TARGET_INDICES.contains(&index) {
                wgpu::ColorWrites::empty()
            } else {
                wgpu::ColorWrites::ALL
            },
        })
    })
}

// ═══════════════════════════════════════════════════════════════════════════════
// Uniforms
// ═══════════════════════════════════════════════════════════════════════════════

/// Tint each blade by its LOD instead of shading it, so LOD band boundaries are visible.
///
/// Enabled with `HELIO_FOLIAGE_DEBUG=lod`. Density and coverage artefacts in foliage all
/// look alike from a screenshot — a ring, a step, a gap — and which LOD boundary they sit
/// on is the single most useful fact for diagnosing them. Guessing that from a
/// description costs a build/run cycle each time; colouring it costs one.
///
/// L0 red, L1 green, L2 blue, L3 yellow.
pub const FLAG_DEBUG_LOD: u32 = 2;

/// Whether `HELIO_FOLIAGE_DEBUG=lod` is set.
///
/// Read once and cached: `execute` runs every frame, and an environment lookup per frame
/// would be a syscall on the hot path for a value that cannot change.
fn debug_lod_enabled() -> bool {
    use std::sync::OnceLock;
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("HELIO_FOLIAGE_DEBUG")
            .map(|value| value.eq_ignore_ascii_case("lod"))
            .unwrap_or(false)
    })
}

/// Per-frame foliage globals. Mirrors `FoliageGlobals` in `foliage_gbuffer.wgsl`
/// (48 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, Pod, Zeroable)]
pub struct FoliageGlobals {
    /// Render target size in pixels. The velocity target is in pixels/frame, so this is
    /// load-bearing rather than informational — a stale size scales every foliage motion
    /// vector and TAA rejects them as outliers.
    pub screen_size: [f32; 2],
    /// Frame index.
    pub frame: u32,
    /// `FLAG_*` bits.
    pub flags: u32,
    /// xy = the wind clock: the frame clock `t` and the previous frame's `t - dt`
    /// (`PrepareContext::time`, `time_delta`; host-driven, so wind stops with a frozen
    /// or paused clock), zw unused. A `vec4` because WGSL aligns one to 16 bytes: a run
    /// of scalars here against the shader's `vec4` once fed every blade a zero height.
    pub clock: [f32; 4],
    /// `FoliageQuality::lod_distance_scale`, already sanitised here so the shader does
    /// not have to repeat `select_blade_lod`'s defensive branch on it.
    pub lod_quality_scale: f32,
    /// Width of the band (metres) the near tufts switch from L0 to L1 across.
    pub lod_fade_band: f32,
    /// Global multiplier on how far interactors press foliage over.
    pub interaction_strength: f32,
    /// Elements per `visible_blades` region (a LOD's): `cs_boxes` reads every region.
    pub region_stride: u32,
}

const _: () = {
    assert!(
        std::mem::size_of::<FoliageGlobals>() == 48,
        "FoliageGlobals must be 48 bytes and match `struct FoliageGlobals` in \
         foliage_gbuffer.wgsl field for field; a mismatch here does not fail to compile, \
         it silently feeds the vertex shader the wrong numbers"
    );
};

/// Per-LOD constants, bound with a dynamic offset so all four draws share one pipeline
/// and one buffer. Mirrors `FoliageLod` in `foliage_gbuffer.wgsl` (32 bytes).
///
/// A dynamic-offset uniform rather than immediate/push constants because immediate data
/// availability varies by backend, and rather than `first_instance` because that would
/// put the region base under the producer's control on one side of the contract and this
/// pass's on the other.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, Pod, Zeroable)]
pub struct FoliageLodUniform {
    pub lod: u32,
    pub segments: u32,
    pub vertex_count: u32,
    pub region_base: u32,
    pub width_scale: f32,
    pub height_scale: f32,
    /// 1 when this LOD draws a flat card rather than a tapered blade strip.
    pub is_card: u32,
    pub _pad: u32,
}

/// Stride between per-LOD uniform slots.
///
/// 256 rather than `size_of::<FoliageLodUniform>()` because a dynamic uniform offset
/// must be a multiple of the adapter's `min_uniform_buffer_offset_alignment`, which
/// wgpu caps at 256. Using the maximum is valid on every adapter (256 is a multiple of
/// any smaller power-of-two alignment) and costs 896 wasted bytes, once.
pub const LOD_UNIFORM_STRIDE: u32 = 256;

/// Maximum foliage types, fixed by `GpuBladeInstance`'s 8-bit type id.
pub const MAX_FOLIAGE_TYPES: usize = 256;

// ═══════════════════════════════════════════════════════════════════════════════
// Per-frame decision
// ═══════════════════════════════════════════════════════════════════════════════

/// What the publisher gave us this frame, reduced to the parts that decide whether the
/// pass does anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FoliageTables {
    /// Number of entries in the SceneDB `foliage_types` projection.
    pub type_count: u32,
    /// Content generation for the projected foliage type table. Wind does not
    /// advance it because wind is a separate SceneDB projection.
    pub generation: u64,
}

/// The outcome of [`decide_frame`]: everything `prepare` and `execute` need to know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FoliageFrameDecision {
    /// Whether `execute` records anything at all.
    pub enabled: bool,
    /// Whether the type table needs re-uploading (first frame, or the tables changed).
    pub upload_types: bool,
    /// Whether the wind uniform needs uploading. Every enabled frame: wind carries two
    /// timestamps and both move every frame.
    pub upload_wind: bool,
    /// Number of `draw_indirect` calls `execute` records.
    pub draw_count: u32,
}

/// The zero-overhead guarantee, as a pure function.
///
/// Extracted from `prepare` so the plan's §10 guarantee 2 is testable without a GPU,
/// which is the only way it stays true: "the pass does nothing when there is no foliage"
/// is exactly the kind of property that survives a refactor in spirit and dies in
/// practice, because the cost of breaking it is a constant-size buffer write nobody
/// notices in a profile.
///
/// Three cases:
///
/// - The SceneDB `foliage_types` projection is absent ⇒ nothing at all. Not
///   "upload an empty table", not "record four empty draws" — the pass leaves
///   the projection absent for this reason.
/// - Published but no types registered ⇒ still nothing: a type id indexes the descriptor
///   table directly, so an empty table means no blade can resolve.
/// - Types registered ⇒ four draws, whatever the ring holds. An empty ring is four
///   `draw_indirect` calls with `instance_count == 0`, which is the plan's §10
///   guarantee 3 and costs under 0.02 ms.
pub fn decide_frame(
    tables: Option<FoliageTables>,
    uploaded_generation: Option<u64>,
) -> FoliageFrameDecision {
    match tables {
        Some(tables) if tables.type_count > 0 => FoliageFrameDecision {
            enabled: true,
            upload_types: uploaded_generation != Some(tables.generation),
            upload_wind: true,
            draw_count: LOD_COUNT as u32,
        },
        _ => FoliageFrameDecision {
            enabled: false,
            upload_types: false,
            upload_wind: false,
            draw_count: 0,
        },
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// L0/L1 switch — CPU mirror
// ═══════════════════════════════════════════════════════════════════════════════

/// Whether a near-band tuft of hash `unit` (in `[0, 1)`) draws at `level` at `distance`
/// (CPU mirror of the shader's switch): over `band` metres under the type's L0 distance
/// `boundary` the share of tufts at L1 grows from none to all, each tuft whole at exactly
/// one of L0 and L1 (never two see-through halves). Levels past L1 are whole tufts that
/// the bands fade, not this.
pub fn lod_draws(level: u32, distance: f32, boundary: f32, band: f32, unit: f32) -> bool {
    let t = ((distance - (boundary - band)) / band.max(1.0e-6)).clamp(0.0, 1.0);
    let f = if band > 0.0 {
        t * t * (3.0 - 2.0 * t)
    } else if distance < boundary {
        0.0
    } else {
        1.0
    };
    match level {
        0 => unit < 1.0 - f,
        1 => unit >= 1.0 - f,
        _ => true,
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// The pass
// ═══════════════════════════════════════════════════════════════════════════════

/// Rasterises grass blades and cards into the shared G-buffer.
///
/// Construct it with the four buffers `FoliagePlacePass` owns, following the
/// `ShadowDirtyPass` → `ShadowCullPass` handoff in `helio-presets`: the producer
/// pass is built first and its `Arc<wgpu::Buffer>` fields are cloned into the consumer.
pub struct FoliageGBufferPass {
    pipeline: wgpu::RenderPipeline,
    /// Blocky foliage: the boxes `FoliageBoxesPass` expands, nine corners each.
    box_pipeline: wgpu::RenderPipeline,
    bind_group_layout_0: wgpu::BindGroupLayout,

    // ── Producer-owned buffers (the pinned contract) ─────────────────────────
    blade_arena: Arc<wgpu::Buffer>,
    tile_table: Arc<wgpu::Buffer>,
    /// `GpuTileFrame[]`, each slot's frame (origin, axes, up) in render space.
    tile_frames: Arc<wgpu::Buffer>,
    visible_blades: Arc<wgpu::Buffer>,
    foliage_indirect: Arc<wgpu::Buffer>,

    // ── Pass-owned buffers ───────────────────────────────────────────────────
    globals_buf: Arc<wgpu::Buffer>,
    /// `FoliageBox[]` and the boxes' count and draw arguments (`BoxState`), shared
    /// with [`FoliageBoxesPass`].
    boxes: Arc<wgpu::Buffer>,
    box_state: Arc<wgpu::Buffer>,
    /// Valid only when the SceneDB columns have not been registered yet. Authored
    /// foliage never enters either placeholder; the bind group resolves the mirror's
    /// `foliage_wind`/`foliage_types` buffers directly each frame.
    placeholder_wind: wgpu::Buffer,
    placeholder_type: wgpu::Buffer,
    lod_buf: wgpu::Buffer,
    lod_uniforms_written: bool,

    /// `FoliagePlacePass::interactors`: this frame's interactors in render space.
    interactors: Arc<wgpu::Buffer>,

    bind_group_0: Option<wgpu::BindGroup>,
    bind_group_0_key: Option<(wgpu::Buffer, wgpu::Buffer, wgpu::Buffer)>,
    /// The band table (`FoliagePlacePass::bands_buffer`): where each slot's tufts draw
    /// and how they thin.
    bands: Arc<wgpu::Buffer>,
    bind_group_1: wgpu::BindGroup,

    /// Elements per `visible_blades` region. See [`visible_region_offset`].
    region_stride: u32,

    decision: FoliageFrameDecision,
    uploaded_generation: Option<u64>,
    /// Whether any type row has a density; see
    /// [`helio_pass_foliage_place::foliage_type_liveness`].
    type_liveness: helio_core::SceneBufferLiveness,

    /// Quality preset. Drives the ring radius and the LOD ladder scale, and must match
    /// what `FoliagePlacePass` was configured with — the two independently apply the
    /// same scale, and a mismatch shears the switch.
    pub quality: FoliageQuality,
    /// Width of the band (metres) the near tufts switch from L0 to L1 across, a tuft at a
    /// time ([`lod_draws`]): wide enough that tufts change one by one, narrow enough
    /// that the doubled cull output inside it stays small.
    pub lod_fade_band: f32,
    /// Global multiplier on how far interactors (`FoliageInteractorRow` rows) press
    /// foliage over.
    pub interaction_strength: f32,
}

impl FoliageGBufferPass {
    /// Create the pass against the four buffers `FoliagePlacePass` publishes.
    ///
    /// * `blade_arena` — `GpuBladeInstance[]`, partitioned into equal fixed slabs, one
    ///   per tile ring slot.
    /// * `tile_table` — `GpuFoliageTile[]`, the resident tile ring.
    /// * `visible_blades` — flat `blade_arena` indices in four equal contiguous regions;
    ///   see [`visible_region_offset`] for the region layout.
    /// * `foliage_indirect` — four `DrawIndirectArgs` at byte offsets 0/16/32/48.
    /// * `bands` — `FoliagePlacePass::bands_buffer`, the band table (a uniform).
    /// * `interactors` — `FoliagePlacePass::interactors`, the frame's interactors.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &wgpu::Device,
        blade_arena: Arc<wgpu::Buffer>,
        tile_table: Arc<wgpu::Buffer>,
        tile_frames: Arc<wgpu::Buffer>,
        visible_blades: Arc<wgpu::Buffer>,
        foliage_indirect: Arc<wgpu::Buffer>,
        bands: Arc<wgpu::Buffer>,
        interactors: Arc<wgpu::Buffer>,
    ) -> Self {
        // ── Region stride ─────────────────────────────────────────────────────
        // Derived from the buffer the producer sized, so there is no third place for
        // the two sides to disagree. `max(1)` keeps a degenerate (test-sized) buffer
        // from producing a zero stride that silently aliases all four regions onto
        // region 0.
        let region_stride = ((visible_blades.size() / (LOD_COUNT as u64 * 4)) as u32).max(1);

        // ── Buffers ───────────────────────────────────────────────────────────
        let boxes = Arc::new(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("FoliageGBuffer/Boxes"),
            size: MAX_FOLIAGE_BOXES * BOX_BYTES,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        }));
        let box_state = Arc::new(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("FoliageGBuffer/BoxState"),
            size: BOX_STATE_BYTES,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::INDIRECT
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
        let globals_buf = Arc::new(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("FoliageGBuffer/Globals"),
            size: std::mem::size_of::<FoliageGlobals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
        let placeholder_wind = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("FoliageGBuffer/WindPlaceholder"),
            size: std::mem::size_of::<helio_pass_foliage_place::GpuWind>() as u64,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let placeholder_type = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("FoliageGBuffer/TypesPlaceholder"),
            size: std::mem::size_of::<GpuFoliageType>() as u64,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let lod_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("FoliageGBuffer/LodConstants"),
            size: (LOD_COUNT as u32 * LOD_UNIFORM_STRIDE) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // ── Bind group layouts ────────────────────────────────────────────────
        //
        // Everything here is VERTEX-visible except `globals`, which the fragment stage
        // also reads for the screen size and the dither's frame index. Blade geometry,
        // wind, LOD selection and the interaction bend are all vertex-stage work; the
        // fragment stage only shades.
        let bind_group_layout_0 = layout_0(device, "FoliageGBuffer BGL 0", true);

        let bind_group_layout_1 =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("FoliageGBuffer BGL 1"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        // One buffer, four slots, one dynamic offset per draw.
                        has_dynamic_offset: true,
                        min_binding_size: NonZeroU64::new(
                            std::mem::size_of::<FoliageLodUniform>() as u64
                        ),
                    },
                    count: None,
                }],
            });

        let bind_group_1 = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("FoliageGBuffer BG 1"),
            layout: &bind_group_layout_1,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &lod_buf,
                    offset: 0,
                    // One slot's worth, so `offset + size` stays inside the buffer at
                    // every dynamic offset including the last.
                    size: NonZeroU64::new(LOD_UNIFORM_STRIDE as u64),
                }),
            }],
        });

        // ── Pipeline ──────────────────────────────────────────────────────────
        let shader = helio_core::shader::module_with(
            device,
            "FoliageGBuffer Shader",
            helio_core::include_wgsl!("../shaders/foliage_gbuffer.wgsl"),
            &[
                helio_pass_foliage_place::WIND_SNIPPET,
                helio_pass_foliage_place::COMMON_SNIPPET,
            ],
        );
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("FoliageGBuffer PL"),
            bind_group_layouts: &[Some(&bind_group_layout_0), Some(&bind_group_layout_1)],
            immediate_size: 0,
        });
        let targets = color_target_states();
        let raster = |label: &str, entry: &str| {
            helio_core::shader::create_render_pipeline(
                &device,
                wgpu::RenderPipelineDescriptor {
                    label: Some(label),
                    layout: Some(&pipeline_layout),
                    vertex: wgpu::VertexState {
                        module: &shader,
                        entry_point: Some(entry),
                        compilation_options: Default::default(),
                        // No vertex buffer. Every attribute is derived from `vertex_index` and
                        // the packed instance record — see `geometry`.
                        buffers: &[],
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &shader,
                        entry_point: Some("fs_main"),
                        compilation_options: Default::default(),
                        targets: &targets,
                    }),
                    primitive: wgpu::PrimitiveState {
                        topology: wgpu::PrimitiveTopology::TriangleStrip,
                        // No effect on a non-indexed draw, but on Vulkan a `Some` value sets
                        // `primitiveRestartEnable`, and `None` is the correct state here.
                        strip_index_format: None,
                        // Blades are two-sided: a grass blade is a surface with no back. The
                        // fragment shader flips the normal by `@builtin(front_facing)`.
                        cull_mode: None,
                        ..Default::default()
                    },
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: wgpu::TextureFormat::Depth32Float,
                        // Grass is opaque geometry and must occlude what is behind it.
                        depth_write_enabled: Some(true),
                        depth_compare: Some(helio_core::depth::NEARER_EQUAL),
                        stencil: wgpu::StencilState::default(),
                        bias: wgpu::DepthBiasState::default(),
                    }),
                    // Single-sampled, like every other Helio raster pass.
                    multisample: wgpu::MultisampleState::default(),
                    multiview_mask: None,
                    cache: None,
                },
            )
        };
        let pipeline = raster("FoliageGBuffer Pipeline", "vs_main");
        let box_pipeline = raster("FoliageGBuffer Box Pipeline", "vs_box");

        Self {
            pipeline,
            box_pipeline,
            bind_group_layout_0,
            blade_arena,
            tile_table,
            tile_frames,
            visible_blades,
            foliage_indirect,
            globals_buf,
            boxes,
            box_state,
            placeholder_wind,
            placeholder_type,
            lod_buf,
            lod_uniforms_written: false,
            interactors,
            bind_group_0: None,
            bind_group_0_key: None,
            bands,
            bind_group_1,
            region_stride,
            decision: decide_frame(None, None),
            uploaded_generation: None,
            type_liveness: helio_pass_foliage_place::foliage_type_liveness(),
            quality: FoliageQuality::default(),
            lod_fade_band: 4.0,
            interaction_strength: 1.0,
        }
    }

    /// Elements per `visible_blades` region, derived from the buffer's size.
    /// See [`visible_region_offset`].
    #[inline]
    pub fn region_stride(&self) -> u32 {
        self.region_stride
    }

    /// What the pass decided to do this frame. See [`decide_frame`].
    #[inline]
    pub fn decision(&self) -> FoliageFrameDecision {
        self.decision
    }

    /// The per-LOD constants, in the order they are written into `lod_buf`.
    ///
    /// Public so the LOD ladder's vertex counts can be asserted against
    /// [`LOD_VERTEX_COUNTS`] without a GPU.
    pub fn lod_uniforms(
        region_stride: u32,
        cluster_granularity: u32,
    ) -> [FoliageLodUniform; LOD_COUNT] {
        let _ = cluster_granularity;
        std::array::from_fn(|lod| FoliageLodUniform {
            lod: lod as u32,
            segments: LOD_SEGMENTS[lod],
            vertex_count: LOD_VERTEX_COUNTS[lod],
            region_base: visible_region_offset(lod as u32, region_stride),
            width_scale: 1.0,
            height_scale: 1.0,
            is_card: (lod == 2) as u32,
            _pad: 0,
        })
    }
}

impl RenderPass for FoliageGBufferPass {
    fn scene_buffers(&self) -> &'static [&'static str] {
        &["foliage_types", "foliage_wind"]
    }

    fn name(&self) -> &'static str {
        "FoliageGBuffer"
    }

    fn reads(&self) -> &'static [&'static str] {
        &["gbuffer", "foliage_draws", "foliage_boxes"]
    }

    fn writes(&self) -> &'static [&'static str] {
        &["gbuffer", "gbuffer_velocity"]
    }

    fn declare_resources(&self, builder: &mut ResourceBuilder) {
        // Read-only. The eight G-buffer targets are declared and owned by `GBufferPass`;
        // this pass loads them and draws into them, exactly as `VirtualGeometryPass`
        // does. Declaring a second writer would allocate a second set of textures and
        // the fusion this pass is positioned for would become impossible.
        builder.read("gbuffer");
    }

    fn render_pass_descriptor_with_storage<'a>(
        &'a self,
        _target: &'a wgpu::TextureView,
        depth: &'a wgpu::TextureView,
        resources: &'a helio_core::ResourceRegistry<'a>,
        storage: &'a mut helio_core::RenderFrameStorage,
    ) -> Option<wgpu::RenderPassDescriptor<'a>> {
        // Returns `Some` whenever the G-buffer exists, regardless of whether there is
        // any foliage this frame. A pass that returns `None` on per-frame state can
        // never participate in subpass fusion, because the executor probes chain
        // membership once at lock time — see the crate docs and
        // `helio_pass_billboard`'s `BillboardPass::prepare`.
        //
        // The views must be byte-identical to `GBufferPass`'s, in the same order:
        // `compute_chains` requires an exact attachment match before it will fuse.
        let gbuffer = resources.read::<helio_core::ViewGroup<'_, 4>>(
            helio_core::ResourceKey::new("gbuffer"),
            "FoliageGBuffer",
        )?;
        let lightmap_uv = resources.read(
            helio_core::ResourceKey::new("gbuffer_lightmap_uv"),
            "FoliageGBuffer",
        )?;
        let sss_target = resources.read(
            helio_core::ResourceKey::new("gbuffer_sss"),
            "FoliageGBuffer",
        )?;
        let extra_target = resources.read(
            helio_core::ResourceKey::new("gbuffer_extra"),
            "FoliageGBuffer",
        )?;
        let velocity_target = resources.read(
            helio_core::ResourceKey::new("gbuffer_velocity"),
            "FoliageGBuffer",
        )?;

        // `LoadOp::Load` on all eight: this pass adds geometry to a G-buffer that
        // `GBufferPass` has already filled. A clear here would erase the scene.
        //
        // Written out longhand rather than through a helper because the *order* is the
        // contract — it has to be readable next to `GBufferPass`'s list and diffable
        // against it.
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
            label: Some("FoliageGBuffer"),
            color_attachments,
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    // Load, not clear: blades depth-test against the opaque scene
                    // `GBufferPass` just drew.
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
        let type_handle = ctx.scene_buffers.get(BufferKey::of("foliage_types"));
        self.type_liveness
            .update(ctx.device, ctx.queue, type_handle);
        let type_handle = type_handle.filter(|handle| self.type_liveness.maybe_live(handle));
        let wind_handle = ctx.scene_buffers.get(BufferKey::of("foliage_wind"));
        let tables = type_handle.map(|handle| FoliageTables {
            type_count: (handle.buffer.size() / std::mem::size_of::<GpuFoliageType>() as u64)
                .min(MAX_FOLIAGE_TYPES as u64) as u32,
            generation: handle.epoch,
        });
        self.decision = decide_frame(tables, self.uploaded_generation);
        if !self.decision.enabled {
            // The zero-overhead path: not one buffer write. See `decide_frame`.
            return Ok(());
        }
        if wind_handle.is_none() {
            return Ok(());
        }
        self.uploaded_generation = tables.map(|t| t.generation);

        // ── Per-LOD constants (once) ──────────────────────────────────────────
        if !self.lod_uniforms_written {
            for (lod, uniform) in
                Self::lod_uniforms(self.region_stride, self.quality.cluster_granularity())
                    .iter()
                    .enumerate()
            {
                ctx.write_buffer(
                    &self.lod_buf,
                    lod as u64 * LOD_UNIFORM_STRIDE as u64,
                    bytemuck::bytes_of(uniform),
                );
            }
            self.lod_uniforms_written = true;
        }

        // ── Globals ───────────────────────────────────────────────────────────
        let globals = FoliageGlobals {
            screen_size: [ctx.width as f32, ctx.height as f32],
            frame: ctx.frame_num as u32,
            flags: if debug_lod_enabled() {
                FLAG_DEBUG_LOD
            } else {
                0
            },
            clock: [ctx.time, ctx.time - ctx.time_delta, 0.0, 0.0],
            // Sanitised here so the shader does not have to repeat
            // `select_blade_lod`'s defensive branch on every vertex.
            lod_quality_scale: {
                let scale = self.quality.lod_distance_scale();
                if scale.is_finite() && scale > 0.0 {
                    scale
                } else {
                    1.0
                }
            },
            lod_fade_band: self.lod_fade_band.max(0.0),
            region_stride: self.region_stride,
            interaction_strength: self.interaction_strength.max(0.0),
        };
        ctx.write_buffer(&self.globals_buf, 0, bytemuck::bytes_of(&globals));
        Ok(())
    }

    fn execute(&mut self, ctx: &mut PassContext) -> HelioResult<()> {
        if ctx.frame_num < 3 || ctx.frame_num % 120 == 0 {
            log::debug!(
                "[foliage][raster] frame={} enabled={} render_pass_open={} interactors={}",
                ctx.frame_num,
                self.decision.enabled,
                ctx.render_cmds().is_some(),
                self.interactors.size(),
            );
        }
        if !self.decision.enabled {
            // Zero recorded commands. Not four empty draws — nothing.
            return Ok(());
        }
        let Some(mut pass) = ctx.render_cmds() else {
            // The executor did not open our render pass, which means
            // `render_pass_descriptor` returned `None` (no G-buffer). Nothing to draw
            // into; not an error.
            return Ok(());
        };

        // ── Bind group 0 ──────────────────────────────────────────────────────
        //
        // Rebuilt when the camera buffer or a SceneDB column reallocates. The producer's
        // buffers are `Arc`s owned by this pass, so they cannot move underneath it and
        // are not part of the key.
        //
        // Built inside a block that ends before the assignment: the scene's buffers are
        // borrowed out of `ctx` or `self`, and holding that borrow across
        // `self.bind_group_0 = ...` would not compile. The created bind group owns its
        // references, so nothing escapes the block.
        let (key, rebuilt) = {
            let wind_buffer = ctx
                .scene_buffers
                .get(BufferKey::of("foliage_wind"))
                .map(|h| &h.buffer)
                .unwrap_or(&self.placeholder_wind);
            let type_buffer = ctx
                .scene_buffers
                .get(BufferKey::of("foliage_types"))
                .map(|h| &h.buffer)
                .unwrap_or(&self.placeholder_type);
            let key = (ctx.camera.clone(), wind_buffer.clone(), type_buffer.clone());
            let rebuilt = if self.bind_group_0_key.as_ref() != Some(&key) {
                log::debug!("FoliageGBuffer: rebuilding bind group 0");
                Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("FoliageGBuffer BG 0"),
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
                            resource: wind_buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: type_buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: self.blade_arena.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 5,
                            resource: self.tile_table.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 6,
                            resource: self.visible_blades.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 7,
                            resource: self.interactors.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 9,
                            resource: self.tile_frames.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 10,
                            resource: self.bands.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 11,
                            resource: self.boxes.as_entire_binding(),
                        },
                    ],
                }))
            } else {
                None
            };
            (key, rebuilt)
        };
        if let Some(bind_group) = rebuilt {
            self.bind_group_0 = Some(bind_group);
            self.bind_group_0_key = Some(key);
        }

        // ── Four draws, one per LOD ───────────────────────────────────────────
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, self.bind_group_0.as_ref().unwrap(), &[]);
        for lod in 0..self.decision.draw_count {
            pass.set_bind_group(1, &self.bind_group_1, &[lod * LOD_UNIFORM_STRIDE]);
            pass.draw_indirect(&self.foliage_indirect, lod as u64 * DRAW_INDIRECT_STRIDE);
        }
        // Blocky foliage: the boxes `FoliageBoxesPass` expanded this frame.
        pass.set_pipeline(&self.box_pipeline);
        pass.set_bind_group(1, &self.bind_group_1, &[0]);
        pass.draw_indirect(&self.box_state, BOX_DRAW_OFFSET);
        Ok(())
    }
}

/// Boxes blocky foliage expands to at most a frame (`FoliageBox`, 64 bytes each):
/// beyond them, shapes are not drawn (counted in `BoxState::overflow`).
pub const MAX_FOLIAGE_BOXES: u64 = 1 << 19;
const BOX_BYTES: u64 = 64;
/// `BoxState`: the count, the overflow, two spare words, then the box draw's
/// `DrawIndirectArgs` at [`BOX_DRAW_OFFSET`].
const BOX_STATE_BYTES: u64 = 32;
const BOX_DRAW_OFFSET: u64 = 16;
/// Workgroups of `cs_boxes`, whose threads stride over every visible record.
const BOX_GROUPS: u32 = 1024;

fn entry(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: buffer.as_entire_binding(),
    }
}

/// Group 0 of the foliage shader: the camera, globals, wind, types, records, tiles, the
/// visible list, interactors, frames and bands, and (the draw's) the expanded boxes.
fn layout_0(device: &wgpu::Device, label: &str, boxes: bool) -> wgpu::BindGroupLayout {
    let stages =
        wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE;
    let buffer = |binding: u32, ty: wgpu::BufferBindingType| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: stages,
        ty: wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    let storage = wgpu::BufferBindingType::Storage { read_only: true };
    let mut entries = vec![
        // Camera is a read-only storage array; `view_index` selects the eye in a future
        // single-pass stereo path (mono reads index 0).
        buffer(0, storage),
        buffer(1, wgpu::BufferBindingType::Uniform),
        buffer(2, storage),                           // wind
        buffer(3, storage),                           // foliage_types
        buffer(4, storage),                           // blade_arena
        buffer(5, storage),                           // tile_table
        buffer(6, storage),                           // visible_blades
        buffer(7, storage),                           // foliage_interactors
        buffer(9, storage),                           // tile_frames
        buffer(10, wgpu::BufferBindingType::Uniform), // bands
    ];
    if boxes {
        entries.push(buffer(11, storage));
    }
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &entries,
    })
}

impl FoliageGBufferPass {
    /// The compute pass that expands this pass's blocky foliage into boxes each frame:
    /// add it to the graph after `FoliagePlacePass` and before `GBufferPass` (between the
    /// two raster passes it would break their fusion).
    pub fn boxes_pass(&self, device: &wgpu::Device) -> FoliageBoxesPass {
        FoliageBoxesPass::new(
            device,
            Arc::clone(&self.globals_buf),
            [
                &self.blade_arena,
                &self.tile_table,
                &self.tile_frames,
                &self.visible_blades,
                &self.foliage_indirect,
                &self.bands,
                &self.interactors,
            ]
            .map(Arc::clone),
            Arc::clone(&self.boxes),
            Arc::clone(&self.box_state),
        )
    }
}

/// Expands each frame's visible blocky foliage into boxes (`cs_boxes`): everything a box
/// shares — its record, blade and staircase, the wind and interactors at its centre — is
/// worked out once a box here instead of once a vertex in the draw, which places nine
/// corners a box (`vs_box`). Made by [`FoliageGBufferPass::boxes_pass`].
pub struct FoliageBoxesPass {
    expand: wgpu::ComputePipeline,
    finish: wgpu::ComputePipeline,
    layout_0: wgpu::BindGroupLayout,
    empty_group: wgpu::BindGroup,
    group_2: wgpu::BindGroup,
    group_0: Option<((wgpu::Buffer, wgpu::Buffer, wgpu::Buffer), wgpu::BindGroup)>,
    globals: Arc<wgpu::Buffer>,
    /// Blade arena, tile table, tile frames, visible list, the cull's draws, bands and
    /// interactors (`FoliagePlacePass`'s).
    producer: [Arc<wgpu::Buffer>; 7],
    box_state: Arc<wgpu::Buffer>,
    placeholder_wind: wgpu::Buffer,
    placeholder_type: wgpu::Buffer,
    active: bool,
}

impl FoliageBoxesPass {
    fn new(
        device: &wgpu::Device,
        globals: Arc<wgpu::Buffer>,
        producer: [Arc<wgpu::Buffer>; 7],
        boxes: Arc<wgpu::Buffer>,
        box_state: Arc<wgpu::Buffer>,
    ) -> Self {
        let layout_0 = layout_0(device, "FoliageBoxes BGL 0", false);
        let empty_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("FoliageBoxes BGL 1"),
            entries: &[],
        });
        let storage = |binding: u32, read_only: bool| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout_2 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("FoliageBoxes BGL 2"),
            entries: &[storage(0, false), storage(1, false), storage(2, true)],
        });
        let shader = helio_core::shader::module_with(
            device,
            "FoliageBoxes Shader",
            helio_core::include_wgsl!("../shaders/foliage_gbuffer.wgsl"),
            &[
                helio_pass_foliage_place::WIND_SNIPPET,
                helio_pass_foliage_place::COMMON_SNIPPET,
            ],
        );
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("FoliageBoxes PL"),
            bind_group_layouts: &[Some(&layout_0), Some(&empty_layout), Some(&layout_2)],
            immediate_size: 0,
        });
        let pipeline = |label: &str, entry: &str| {
            helio_core::shader::create_compute_pipeline(
                &device,
                wgpu::ComputePipelineDescriptor {
                    label: Some(label),
                    layout: Some(&layout),
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    cache: None,
                },
            )
        };
        let empty_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("FoliageBoxes BG 1"),
            layout: &empty_layout,
            entries: &[],
        });
        let group_2 = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("FoliageBoxes BG 2"),
            layout: &layout_2,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: boxes.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: box_state.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: producer[4].as_entire_binding(),
                },
            ],
        });
        let placeholder = |label: &str, size: u64| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            })
        };
        Self {
            expand: pipeline("FoliageBoxes Expand", "cs_boxes"),
            finish: pipeline("FoliageBoxes Finish", "cs_boxes_finish"),
            layout_0,
            empty_group,
            group_2,
            group_0: None,
            globals,
            producer,
            box_state,
            placeholder_wind: placeholder(
                "FoliageBoxes/WindPlaceholder",
                std::mem::size_of::<helio_pass_foliage_place::GpuWind>() as u64,
            ),
            placeholder_type: placeholder(
                "FoliageBoxes/TypesPlaceholder",
                std::mem::size_of::<GpuFoliageType>() as u64,
            ),
            active: false,
        }
    }
}

impl RenderPass for FoliageBoxesPass {
    fn name(&self) -> &'static str {
        "FoliageBoxes"
    }

    fn reads(&self) -> &'static [&'static str] {
        &["foliage_draws"]
    }

    fn writes(&self) -> &'static [&'static str] {
        &["foliage_boxes"]
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
        // As the draw: no type rows or no wind, no foliage.
        self.active = ctx
            .scene_buffers
            .get(BufferKey::of("foliage_types"))
            .is_some()
            && ctx
                .scene_buffers
                .get(BufferKey::of("foliage_wind"))
                .is_some();
        Ok(())
    }

    fn execute(&mut self, ctx: &mut PassContext) -> HelioResult<()> {
        if !self.active {
            return Ok(());
        }
        let wind = ctx
            .scene_buffers
            .get(BufferKey::of("foliage_wind"))
            .map(|h| &h.buffer)
            .unwrap_or(&self.placeholder_wind);
        let types = ctx
            .scene_buffers
            .get(BufferKey::of("foliage_types"))
            .map(|h| &h.buffer)
            .unwrap_or(&self.placeholder_type);
        let key = (ctx.camera.clone(), wind.clone(), types.clone());
        if self.group_0.as_ref().is_none_or(|(k, _)| *k != key) {
            let [blades, tiles, frames, visible, _, bands, interactors] = &self.producer;
            let group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("FoliageBoxes BG 0"),
                layout: &self.layout_0,
                entries: &[
                    entry(0, ctx.camera),
                    entry(1, &self.globals),
                    entry(2, wind),
                    entry(3, types),
                    entry(4, blades),
                    entry(5, tiles),
                    entry(6, visible),
                    entry(7, interactors),
                    entry(9, frames),
                    entry(10, bands),
                ],
            });
            self.group_0 = Some((key, group));
        }
        let mut encoder = ctx.graphics_cmds();
        encoder.clear_buffer(&self.box_state, 0, Some(BOX_DRAW_OFFSET));
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Foliage Boxes"),
            timestamp_writes: None,
        });
        pass.set_bind_group(0, &self.group_0.as_ref().unwrap().1, &[]);
        pass.set_bind_group(1, &self.empty_group, &[]);
        pass.set_bind_group(2, &self.group_2, &[]);
        pass.set_pipeline(&self.expand);
        pass.dispatch_workgroups(BOX_GROUPS, 1, 1);
        pass.set_pipeline(&self.finish);
        pass.dispatch_workgroups(1, 1, 1);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_blade_reference_round_trips() {
        for tile_slot in [0u32, 1, 4095, 65535] {
            for local in [0u32, 1, 2559, 65535] {
                let packed = pack_visible_blade(tile_slot, local);
                assert_eq!(unpack_visible_blade(packed), (tile_slot, local));
            }
        }
        // The halves must not bleed into each other, which is the failure that draws
        // grass from a neighbouring tile at a plausible-looking position.
        assert_eq!(pack_visible_blade(0xffff, 0), 0xffff_0000);
        assert_eq!(pack_visible_blade(0, 0xffff), 0x0000_ffff);
    }

    #[test]
    fn region_offsets_are_contiguous_and_in_lod_order() {
        let stride = 1024;
        let offsets: Vec<u32> = (0..4).map(|l| visible_region_offset(l, stride)).collect();
        assert_eq!(offsets, vec![0, 1024, 2048, 3072]);
    }
}
