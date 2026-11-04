//! GPU-driven object sort/group/batch pipeline.
//!
//! See `shaders/object_batch.wgsl`'s module doc for the full 8-stage
//! pipeline design. This crate is the Rust-side pipeline construction,
//! buffer growth management, dispatch sequencing, and the small async
//! CPU readback of the (bounded, GPU-computed) material-class range tables
//! and shadow-partition draw counts every `multi_draw_indexed_indirect`
//! call site downstream still needs to know on the CPU.
//!
//! # What this pass owns vs. what it doesn't
//!
//! This pass owns every buffer it writes (`instances`/`aabbs`/`draw_calls`/
//! the three range tables/the two shadow-partition indirect lists) --
//! nothing here mutates `GpuScene`'s own fields directly. Wiring these
//! buffers into the render graph in place of `Scene::rebuild_instance_
//! buffers`'s CPU-populated `GpuScene` fields (the same "point at an
//! externally-owned buffer" seam `Scene::rebind_transform_buffer` already
//! established for SceneDB's `Transform` buffer) is a separate integration
//! step, tracked apart from this crate.

pub mod feature;
pub mod scene_join;
pub use feature::ObjectBatchFeature;

use bytemuck::{Pod, Zeroable};
use helio_core::{CommandRecorder, PassContext, PrepareContext, RenderPass, Result as HelioResult};
use helio_mesh_api::StaticObjectRow;
use pulsar_scenedb::gpu::BufferKey;

mod readback;

const WG: u32 = 256;
/// The scene join's structure marker (`crate::scene_join::
/// OBJECTS_STRUCTURE_KEY`): moves only when objects are added, removed or
/// change what they are, not when they move.
const STRUCTURE_KEY: &str = "scene_objects_structure";
/// The scene join's list of the object slots a frame that only moved objects
/// rewrote (`crate::scene_join::OBJECTS_MOVED_KEY`).
const MOVED_KEY: &str = "scene_objects_moved";
/// Scene buffer of per-`static_objects`-row flags: a non-zero row is drawn by
/// another renderer (virtual geometry), so the main view's frustum cull skips
/// it. Shadows still see the row: the classic geometry is the shadow proxy.
const CLASSIC_SUPPRESSED_KEY: &str = "classic_suppressed";
/// Byte offset of the one-workgroup-per-group dispatch size in
/// `dispatch_args_groups` (see `cs_prepare_groups`).
const GROUP_DISPATCH_OFFSET: u64 = 12;
/// Below this many rows, scratch buffers still allocate at this floor --
/// avoids reallocating on every single insert/remove around a tiny live
/// count, matching `GrowableBuffer`'s own "small initial capacity" idiom
/// used everywhere else in this codebase.
const MIN_SCRATCH_CAPACITY: u32 = 256;

// ── GPU-mirrored uniform/struct shapes ──────────────────────────────────────

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct BatchUniforms {
    capacity: u32,
    max_objects: u32,
    max_groups: u32,
    max_ranges: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FrameUniformGpu {
    count: u32,
    num_blocks: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RangeBlockUniformGpu {
    num_blocks: u32,
}

/// Matches `shaders/object_batch.wgsl`'s `GpuDrawCallOut` -- 20 bytes, same
/// shape as `helio_mesh_api::GpuDrawCall`  /// for it since it never needs the typed version, only the byte layout for
/// buffer sizing).
const DRAW_CALL_BYTES: u64 = 20;
/// Matches `shaders/object_batch.wgsl`'s `GpuInstanceDataOut` -- 224 bytes,
/// see `helio_mesh_api::GpuInstanceData`'s own doc for the exact field breakdown.
const INSTANCE_BYTES: u64 = 224;
/// Matches `shaders/object_batch.wgsl`'s `GpuInstanceAabbOut` -- 32 bytes (min, pad, max, pad),
/// the layout `indirect_dispatch.wgsl` reads as `GpuAabb`.
const AABB_BYTES: u64 = 32;
/// Matches `shaders/object_batch.wgsl`'s `PrevRow` -- 80 bytes (transform +
/// identity vec4).
const PREV_ROW_BYTES: u64 = 80;
/// Matches `shaders/object_batch.wgsl`'s `GpuRangeOut` -- 20 bytes.
const RANGE_BYTES: u64 = 20;
/// Matches `shaders/object_batch.wgsl`'s `DrawIndexedIndirectArgsOut` -- 20
/// bytes, same shape as `helio_mesh_api::DrawIndexedIndirectArgs`.
const INDIRECT_ARGS_BYTES: u64 = 20;

/// Initial `draw_counts` capacity in `u32` slots (four fixed counts plus
/// room for a typical number of material ranges).
const MIN_DRAW_COUNT_SLOTS: usize = 64;

fn create_draw_counts_buffer(device: &wgpu::Device, slots: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ObjBatch DrawCounts"),
        size: (slots * 4) as u64,
        usage: wgpu::BufferUsages::INDIRECT
            | wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn create_range_count_buffer(device: &wgpu::Device) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ObjBatch RangeCount"),
        // Four count words followed by one 12-byte dispatch argument for
        // each of the opaque, transparent, and forward range tables.
        size: 13 * 4,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC
            | wgpu::BufferUsages::INDIRECT,
        mapped_at_creation: false,
    })
}

fn create_storage_buffer(device: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size.max(4),
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
}

/// Storage buffer written by a compute kernel and consumed as indirect draw
/// arguments (the shadow pass's multi-draw-indirect source).
fn create_indirect_buffer(device: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size.max(4),
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::INDIRECT
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
}

/// Same as [`create_storage_buffer`] but also usable as a `<uniform>`
/// binding (the `FrameUniform`/`dispatch_args`-style "write as storage in
/// one kernel, read as uniform in another" dual-bind trick already used by
/// the sprites and particles plugins' passes) and as an indirect-dispatch
/// source.
fn create_dual_buffer(device: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size.max(4),
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::UNIFORM
            | wgpu::BufferUsages::INDIRECT
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
}

fn bgl_entry_storage(
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

fn bgl_entry_uniform(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn bg_entry(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: buffer.as_entire_binding(),
    }
}

fn make_compute_pipeline(
    device: &wgpu::Device,
    label: &str,
    bgl: &wgpu::BindGroupLayout,
    module: &wgpu::ShaderModule,
    entry_point: &str,
) -> wgpu::ComputePipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[Some(bgl)],
        immediate_size: 0,
    });
    helio_core::shader::create_compute_pipeline(
        &device,
        wgpu::ComputePipelineDescriptor {
            label: Some(label),
            layout: Some(&layout),
            module,
            entry_point: Some(entry_point),
            compilation_options: Default::default(),
            cache: None,
        },
    )
}

fn make_compute_pipeline_with(
    device: &wgpu::Device,
    label: &str,
    bgls: &[Option<&wgpu::BindGroupLayout>],
    module: &wgpu::ShaderModule,
    entry_point: &str,
) -> wgpu::ComputePipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: bgls,
        immediate_size: 0,
    });
    helio_core::shader::create_compute_pipeline(
        &device,
        wgpu::ComputePipelineDescriptor {
            label: Some(label),
            layout: Some(&layout),
            module,
            entry_point: Some(entry_point),
            compilation_options: Default::default(),
            cache: None,
        },
    )
}

/// One CPU-visible material-class-batched draw range, mirroring the CPU
/// reference's `(material_class, graph_hash, start, count)` tuple exactly
/// -- `start`/`count` index the `draw_calls` array, not `instances`.
/// What [`ObjectBatchPass::frame_for_testing`] recorded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatchRun {
    /// Nothing the pass reads changed.
    Skipped,
    /// The full sort and grouping.
    Full,
    /// Only objects moved: every sorted slot's placement re-gathered.
    Placements,
    /// Only objects moved, and the join named which: just those re-gathered.
    Moved,
}

pub type RangeTuple = (u32, u64, u32, u32);

/// The bind groups of one moved-slot list: for the gather, and for sizing its
/// dispatch (the latter also holds the indirect arguments as storage, which
/// the gather itself must not bind).
struct MovedBinds {
    gather: wgpu::BindGroup,
    args: wgpu::BindGroup,
}

struct BindGroupLayouts {
    gather: wgpu::BindGroupLayout,
    prepare: wgpu::BindGroupLayout,
    final_gather: wgpu::BindGroupLayout,
    moved: wgpu::BindGroupLayout,
    moved_args: wgpu::BindGroupLayout,
    group_local_scan: wgpu::BindGroupLayout,
    group_block_scan: wgpu::BindGroupLayout,
    group_write: wgpu::BindGroupLayout,
    group_write_sentinel: wgpu::BindGroupLayout,
    prepare_groups: wgpu::BindGroupLayout,
    build_draw_calls: wgpu::BindGroupLayout,
    range_local_scan: wgpu::BindGroupLayout,
    range_block_scan: wgpu::BindGroupLayout,
    range_write: wgpu::BindGroupLayout,
    shadow_partition: wgpu::BindGroupLayout,
}

struct Pipelines {
    gather: wgpu::ComputePipeline,
    prepare: wgpu::ComputePipeline,
    final_gather: wgpu::ComputePipeline,
    moved_args: wgpu::ComputePipeline,
    final_gather_moved: wgpu::ComputePipeline,
    group_local_scan: wgpu::ComputePipeline,
    group_block_scan: wgpu::ComputePipeline,
    group_write: wgpu::ComputePipeline,
    group_write_sentinel: wgpu::ComputePipeline,
    prepare_groups: wgpu::ComputePipeline,
    build_draw_calls: wgpu::ComputePipeline,
    range_local_scan: wgpu::ComputePipeline,
    range_block_scan: wgpu::ComputePipeline,
    range_write: wgpu::ComputePipeline,
    shadow_partition: wgpu::ComputePipeline,
}

/// Every scratch buffer this pipeline owns, sized to `capacity` rows (or
/// `capacity.div_ceil(WG)` blocks) -- see `ensure_capacity`.
struct ScratchBuffers {
    gather_count: wgpu::Buffer,
    frame_uniform: wgpu::Buffer,
    dispatch_args: wgpu::Buffer,
    keys_a: wgpu::Buffer,
    keys_b: wgpu::Buffer,
    indices_a: wgpu::Buffer,
    indices_b: wgpu::Buffer,
    instances_out: wgpu::Buffer,
    aabbs_out: wgpu::Buffer,
    local_group_rank: wgpu::Buffer,
    block_group_totals: wgpu::Buffer,
    group_count: wgpu::Buffer,
    group_starts: wgpu::Buffer,
    dispatch_args_groups: wgpu::Buffer,
    group_material_class: wgpu::Buffer,
    group_graph_hash_lo: wgpu::Buffer,
    group_graph_hash_hi: wgpu::Buffer,
    group_shading: wgpu::Buffer,
    local_range_rank: wgpu::Buffer,
    block_range_totals: wgpu::Buffer,
    range_count: wgpu::Buffer,
    opaque_ranges: wgpu::Buffer,
    transparent_ranges: wgpu::Buffer,
    forward_ranges: wgpu::Buffer,
    shadow_static_indirect: wgpu::Buffer,
    shadow_movable_indirect: wgpu::Buffer,
    /// Static transparent-only casters, drawn into the coloured
    /// transmittance layer rather than the depth atlas.
    shadow_transmissive_indirect: wgpu::Buffer,
    shadow_counts: wgpu::Buffer,
    /// Per `static_objects` row: last drawn transform + identity, for
    /// per-frame motion vectors (see `object_batch.wgsl`'s `PrevRow`).
    prev_rows: wgpu::Buffer,
    /// Per `static_objects` row: its position in the sorted order, from the
    /// last full run (see `cs_final_gather`).
    sorted_position: wgpu::Buffer,
    /// Dispatch size of the moved-rows gather, derived from the moved list.
    moved_args: wgpu::Buffer,
}

impl ScratchBuffers {
    fn new(device: &wgpu::Device, capacity: u32) -> Self {
        let n = capacity as u64;
        let blocks = capacity.div_ceil(WG) as u64;
        Self {
            gather_count: create_storage_buffer(device, "ObjBatch GatherCount", 16),
            frame_uniform: create_dual_buffer(device, "ObjBatch FrameUniform", 8),
            dispatch_args: create_dual_buffer(device, "ObjBatch DispatchArgs", 12),
            keys_a: create_storage_buffer(device, "ObjBatch KeysA", n * 4),
            keys_b: create_storage_buffer(device, "ObjBatch KeysB", n * 4),
            indices_a: create_storage_buffer(device, "ObjBatch IndicesA", n * 4),
            indices_b: create_storage_buffer(device, "ObjBatch IndicesB", n * 4),
            instances_out: create_storage_buffer(device, "ObjBatch Instances", n * INSTANCE_BYTES),
            aabbs_out: create_storage_buffer(device, "ObjBatch Aabbs", n * AABB_BYTES),
            local_group_rank: create_storage_buffer(device, "ObjBatch LocalGroupRank", n * 4),
            block_group_totals: create_storage_buffer(
                device,
                "ObjBatch BlockGroupTotals",
                blocks * 4,
            ),
            group_count: create_storage_buffer(device, "ObjBatch GroupCount", 16),
            group_starts: create_storage_buffer(device, "ObjBatch GroupStarts", (n + 1) * 4),
            // Two dispatch sizes: `cs_prepare_groups` documents both.
            dispatch_args_groups: create_dual_buffer(device, "ObjBatch DispatchArgsGroups", 24),
            group_material_class: create_storage_buffer(device, "ObjBatch GroupClass", n * 4),
            group_graph_hash_lo: create_storage_buffer(device, "ObjBatch GroupHashLo", n * 4),
            group_graph_hash_hi: create_storage_buffer(device, "ObjBatch GroupHashHi", n * 4),
            group_shading: create_storage_buffer(device, "ObjBatch GroupShading", n * 4),
            // Three packed prefix ranks per draw group (opaque, transparent,
            // forward), so every material-range table has deterministic order.
            local_range_rank: create_storage_buffer(device, "ObjBatch LocalRangeRank", n * 12),
            block_range_totals: create_storage_buffer(
                device,
                "ObjBatch BlockRangeTotals",
                blocks * 12,
            ),
            range_count: create_range_count_buffer(device),
            opaque_ranges: create_storage_buffer(device, "ObjBatch OpaqueRanges", n * RANGE_BYTES),
            transparent_ranges: create_storage_buffer(
                device,
                "ObjBatch TransparentRanges",
                n * RANGE_BYTES,
            ),
            forward_ranges: create_storage_buffer(
                device,
                "ObjBatch ForwardRanges",
                n * RANGE_BYTES,
            ),
            shadow_static_indirect: create_indirect_buffer(
                device,
                "ObjBatch ShadowStaticIndirect",
                n * INDIRECT_ARGS_BYTES,
            ),
            shadow_movable_indirect: create_indirect_buffer(
                device,
                "ObjBatch ShadowMovableIndirect",
                n * INDIRECT_ARGS_BYTES,
            ),
            shadow_transmissive_indirect: create_indirect_buffer(
                device,
                "ObjBatch ShadowTransmissiveIndirect",
                n * INDIRECT_ARGS_BYTES,
            ),
            // Counts plus two commutative fingerprints of static caster row
            // membership. Counts alone miss same-count static/movable swaps.
            shadow_counts: create_storage_buffer(device, "ObjBatch ShadowCounts", 24),
            prev_rows: create_storage_buffer(device, "ObjBatch PrevRows", n * PREV_ROW_BYTES),
            sorted_position: create_storage_buffer(device, "ObjBatch SortedPosition", n * 4),
            moved_args: create_indirect_buffer(device, "ObjBatch MovedArgs", 16),
        }
    }
}

pub struct ObjectBatchPass {
    bgls: BindGroupLayouts,
    pipelines: Pipelines,
    /// The (key, row) sort (`helio_core::radix_sort`), bound to the scratch
    /// pairs.
    sort: helio_core::radix_sort::RadixSort,
    sort_binds: Option<helio_core::radix_sort::RadixSortBinds>,
    batch_uniform: wgpu::Buffer,
    draw_calls_out: wgpu::Buffer,
    /// Same per-group data as `draw_calls_out`, reordered to `wgpu`'s
    /// hardware indirect-draw ABI -- see `object_batch.wgsl`'s
    /// `DrawIndexedIndirectArgsOut` doc for why this needs its own buffer.
    indirect_out: wgpu::Buffer,
    scratch: ScratchBuffers,
    scratch_capacity: u32,

    gather_bg: Option<wgpu::BindGroup>,
    prepare_bg: Option<wgpu::BindGroup>,
    final_gather_bg: Option<wgpu::BindGroup>,
    group_local_scan_bg: Option<wgpu::BindGroup>,
    group_block_scan_bg: Option<wgpu::BindGroup>,
    group_write_bg: Option<wgpu::BindGroup>,
    group_write_sentinel_bg: Option<wgpu::BindGroup>,
    prepare_groups_bg: Option<wgpu::BindGroup>,
    build_draw_calls_bg: Option<wgpu::BindGroup>,
    range_local_scan_bg: Option<wgpu::BindGroup>,
    range_block_scan_bg: Option<wgpu::BindGroup>,
    range_write_bg: Option<wgpu::BindGroup>,
    shadow_partition_bg: Option<wgpu::BindGroup>,
    /// `(scratch_capacity, static_objects buffer identity, materials buffer
    /// identity)` -- any change forces every bind group above to rebuild
    /// (cheap; not a hot-path cost next to the compute work itself).
    bind_group_key: Option<(u32, Option<u64>, wgpu::Buffer)>,

    readback: readback::RangeReadback,

    /// Inputs the last recorded run read: `static_objects` and `materials`
    /// (epoch, content generation) plus row and scratch capacity. The
    /// pipeline is a pure function of these and its own persistent
    /// buffers, so an unchanged key reproduces the previous outputs.
    last_inputs: Option<[u64; 8]>,
    /// True once a run has seen `last_inputs` twice in a row: the first
    /// repeat rolls every row's previous-frame transform forward to the
    /// current one, after which outputs stop changing and runs are skipped.
    settled: bool,
    /// Decided in `prepare`, consumed by `execute`.
    skip_this_frame: bool,
    /// The scene join's structure marker (`(epoch, content_generation)`)
    /// the last recorded run saw. While it is unchanged the sort, the
    /// grouping and the draw tables stand, and a change in the object rows
    /// can only be objects that moved.
    last_structure: Option<(u64, u64)>,
    /// Decided in `prepare`, consumed by `execute`: only placements changed,
    /// so only the final gather (which copies each object's transform into
    /// its sorted slot) is recorded, not the whole sort.
    transform_only: bool,
    /// Decided in `prepare`, consumed by `execute`: the placements run can be
    /// limited to `moved_list`'s slots (see `record_moved_placements`).
    use_moved: bool,
    /// The scene join's moved-slot list this frame, when it has one.
    moved_list: Option<wgpu::Buffer>,
    /// Bind groups of the moved-slot gather, by list buffer.
    moved_bgs: Vec<(wgpu::Buffer, MovedBinds)>,
    /// A full run has not been copied for readback yet (the copy trails the
    /// run by a frame; see `poll_and_kick_off`).
    readback_pending: bool,

    /// Fallback bound in `static_objects`'/`materials`' place before either
    /// has ever been resolved (mirrors every other SceneDB-direct pass's
    /// "bind *some* valid buffer so bind-group creation can't fail" fallback
    /// -- `capacity`/dispatch counts are all 0 whenever this is bound, so it
    /// is never actually dereferenced).
    fallback_buf: wgpu::Buffer,

    /// SceneDB buffer `classic_suppressed` (one `u32` per `static_objects` row)
    /// when published, else a fallback. A non-zero row gets
    /// `INSTANCE_FLAG_CLASSIC_SUPPRESSED`, so the main view skips it.
    suppress_buf: wgpu::Buffer,
    suppress_key: Option<wgpu::Buffer>,

    /// GPU copy of every draw count consumers pass to
    /// `multi_draw_indexed_indirect_count` (Helio#306); `None` when the
    /// device lacks `MULTI_DRAW_INDIRECT_COUNT`. Layout documented on
    /// [`helio_mesh_api::ObjectBatchFrameData::draw_counts`].
    draw_counts: Option<wgpu::Buffer>,
    supports_multi_draw_count: bool,
}

impl ObjectBatchPass {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = helio_core::shader::module(
            device,
            "ObjectBatch Shader",
            helio_core::include_wgsl!("../shaders/object_batch.wgsl"),
        );

        let cs = wgpu::ShaderStages::COMPUTE;
        let bgls = BindGroupLayouts {
            gather: device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("ObjBatch Gather BGL"),
                entries: &[
                    bgl_entry_uniform(0, cs),
                    bgl_entry_storage(1, cs, true),
                    bgl_entry_storage(2, cs, false),
                    bgl_entry_storage(3, cs, false),
                    bgl_entry_storage(4, cs, false),
                    bgl_entry_storage(5, cs, false),
                ],
            }),
            prepare: device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("ObjBatch Prepare BGL"),
                entries: &[
                    bgl_entry_storage(0, cs, true),
                    bgl_entry_storage(1, cs, false),
                    bgl_entry_storage(2, cs, false),
                ],
            }),
            final_gather: device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("ObjBatch FinalGather BGL"),
                entries: &[
                    bgl_entry_uniform(0, cs),
                    bgl_entry_storage(1, cs, true),
                    bgl_entry_storage(2, cs, true),
                    bgl_entry_storage(3, cs, false),
                    bgl_entry_storage(4, cs, false),
                    bgl_entry_storage(5, cs, false),
                    bgl_entry_storage(6, cs, false),
                    bgl_entry_storage(7, cs, true),
                ],
            }),
            moved: device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("ObjBatch Moved BGL"),
                entries: &[bgl_entry_storage(0, cs, true)],
            }),
            moved_args: device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("ObjBatch MovedArgs BGL"),
                entries: &[
                    bgl_entry_storage(0, cs, true),
                    bgl_entry_storage(1, cs, false),
                ],
            }),
            group_local_scan: device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("ObjBatch GroupLocalScan BGL"),
                entries: &[
                    bgl_entry_uniform(0, cs),
                    bgl_entry_storage(1, cs, true),
                    bgl_entry_storage(2, cs, true),
                    bgl_entry_storage(3, cs, false),
                    bgl_entry_storage(4, cs, false),
                ],
            }),
            group_block_scan: device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("ObjBatch GroupBlockScan BGL"),
                entries: &[
                    bgl_entry_uniform(0, cs),
                    bgl_entry_storage(1, cs, false),
                    bgl_entry_storage(2, cs, false),
                ],
            }),
            group_write: device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("ObjBatch GroupWrite BGL"),
                entries: &[
                    bgl_entry_uniform(0, cs),
                    bgl_entry_storage(1, cs, true),
                    bgl_entry_storage(2, cs, true),
                    bgl_entry_storage(3, cs, true),
                    bgl_entry_storage(4, cs, true),
                    bgl_entry_storage(5, cs, false),
                ],
            }),
            group_write_sentinel: device.create_bind_group_layout(
                &wgpu::BindGroupLayoutDescriptor {
                    label: Some("ObjBatch GroupWriteSentinel BGL"),
                    entries: &[
                        bgl_entry_storage(0, cs, true),
                        bgl_entry_storage(1, cs, false),
                        bgl_entry_storage(2, cs, true),
                    ],
                },
            ),
            prepare_groups: device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("ObjBatch PrepareGroups BGL"),
                entries: &[
                    bgl_entry_storage(0, cs, true),
                    bgl_entry_storage(1, cs, false),
                ],
            }),
            build_draw_calls: device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("ObjBatch BuildDrawCalls BGL"),
                entries: &[
                    bgl_entry_storage(0, cs, true),
                    bgl_entry_storage(1, cs, true),
                    bgl_entry_storage(2, cs, true),
                    bgl_entry_storage(3, cs, true),
                    bgl_entry_storage(4, cs, false),
                    bgl_entry_storage(5, cs, false),
                    bgl_entry_storage(6, cs, false),
                    bgl_entry_storage(7, cs, false),
                    bgl_entry_storage(8, cs, false),
                    bgl_entry_storage(9, cs, true),
                    bgl_entry_storage(10, cs, false),
                ],
            }),
            range_local_scan: device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("ObjBatch RangeLocalScan BGL"),
                entries: &[
                    bgl_entry_storage(0, cs, true),
                    bgl_entry_storage(1, cs, true),
                    bgl_entry_storage(2, cs, true),
                    bgl_entry_storage(3, cs, true),
                    bgl_entry_storage(4, cs, false),
                    bgl_entry_storage(5, cs, false),
                    bgl_entry_storage(6, cs, true),
                ],
            }),
            range_block_scan: device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("ObjBatch RangeBlockScan BGL"),
                entries: &[
                    bgl_entry_uniform(0, cs),
                    bgl_entry_storage(1, cs, false),
                    bgl_entry_storage(2, cs, false),
                ],
            }),
            range_write: device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("ObjBatch RangeWrite BGL"),
                entries: &[
                    bgl_entry_storage(0, cs, true),
                    bgl_entry_storage(1, cs, true),
                    bgl_entry_storage(2, cs, true),
                    bgl_entry_storage(3, cs, true),
                    bgl_entry_storage(4, cs, true),
                    bgl_entry_storage(5, cs, true),
                    bgl_entry_storage(6, cs, true),
                    bgl_entry_storage(7, cs, false),
                    bgl_entry_storage(8, cs, false),
                    bgl_entry_storage(9, cs, false),
                    bgl_entry_storage(10, cs, false),
                ],
            }),
            shadow_partition: device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("ObjBatch ShadowPartition BGL"),
                entries: &[
                    bgl_entry_uniform(0, cs),
                    bgl_entry_storage(1, cs, true),
                    bgl_entry_storage(2, cs, true),
                    bgl_entry_storage(3, cs, false),
                    bgl_entry_storage(4, cs, false),
                    bgl_entry_storage(5, cs, false),
                    bgl_entry_storage(6, cs, true),
                    bgl_entry_storage(7, cs, false),
                ],
            }),
        };

        let pipelines = Pipelines {
            gather: make_compute_pipeline(
                device,
                "ObjBatch Gather",
                &bgls.gather,
                &shader,
                "cs_gather",
            ),
            prepare: make_compute_pipeline(
                device,
                "ObjBatch Prepare",
                &bgls.prepare,
                &shader,
                "cs_prepare",
            ),
            final_gather: make_compute_pipeline(
                device,
                "ObjBatch FinalGather",
                &bgls.final_gather,
                &shader,
                "cs_final_gather",
            ),
            moved_args: make_compute_pipeline_with(
                device,
                "ObjBatch MovedArgs",
                &[None, Some(&bgls.moved_args)],
                &shader,
                "cs_moved_args",
            ),
            final_gather_moved: make_compute_pipeline_with(
                device,
                "ObjBatch FinalGatherMoved",
                &[Some(&bgls.final_gather), Some(&bgls.moved)],
                &shader,
                "cs_final_gather_moved",
            ),
            group_local_scan: make_compute_pipeline(
                device,
                "ObjBatch GroupLocalScan",
                &bgls.group_local_scan,
                &shader,
                "cs_group_local_scan",
            ),
            group_block_scan: make_compute_pipeline(
                device,
                "ObjBatch GroupBlockScan",
                &bgls.group_block_scan,
                &shader,
                "cs_group_block_scan",
            ),
            group_write: make_compute_pipeline(
                device,
                "ObjBatch GroupWrite",
                &bgls.group_write,
                &shader,
                "cs_group_write",
            ),
            group_write_sentinel: make_compute_pipeline(
                device,
                "ObjBatch GroupWriteSentinel",
                &bgls.group_write_sentinel,
                &shader,
                "cs_group_write_sentinel",
            ),
            prepare_groups: make_compute_pipeline(
                device,
                "ObjBatch PrepareGroups",
                &bgls.prepare_groups,
                &shader,
                "cs_prepare_groups",
            ),
            build_draw_calls: make_compute_pipeline(
                device,
                "ObjBatch BuildDrawCalls",
                &bgls.build_draw_calls,
                &shader,
                "cs_build_draw_calls",
            ),
            range_local_scan: make_compute_pipeline(
                device,
                "ObjBatch RangeLocalScan",
                &bgls.range_local_scan,
                &shader,
                "cs_range_local_scan",
            ),
            range_block_scan: make_compute_pipeline(
                device,
                "ObjBatch RangeBlockScan",
                &bgls.range_block_scan,
                &shader,
                "cs_range_block_scan",
            ),
            range_write: make_compute_pipeline(
                device,
                "ObjBatch RangeWrite",
                &bgls.range_write,
                &shader,
                "cs_range_write",
            ),
            shadow_partition: make_compute_pipeline(
                device,
                "ObjBatch ShadowPartition",
                &bgls.shadow_partition,
                &shader,
                "cs_shadow_partition",
            ),
        };

        let batch_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ObjBatch BatchUniforms"),
            size: std::mem::size_of::<BatchUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let scratch_capacity = MIN_SCRATCH_CAPACITY;
        let scratch = ScratchBuffers::new(device, scratch_capacity);
        let draw_calls_out = create_storage_buffer(
            device,
            "ObjBatch DrawCalls",
            scratch_capacity as u64 * DRAW_CALL_BYTES,
        );
        let indirect_out = create_storage_buffer(
            device,
            "ObjBatch Indirect",
            scratch_capacity as u64 * INDIRECT_ARGS_BYTES,
        );

        let fallback_buf = create_dual_buffer(device, "ObjBatch Fallback", 16);

        Self {
            bgls,
            pipelines,
            sort: helio_core::radix_sort::RadixSort::new(device),
            sort_binds: None,
            batch_uniform,
            draw_calls_out,
            indirect_out,
            scratch,
            scratch_capacity,
            gather_bg: None,
            prepare_bg: None,
            final_gather_bg: None,
            group_local_scan_bg: None,
            group_block_scan_bg: None,
            group_write_bg: None,
            group_write_sentinel_bg: None,
            prepare_groups_bg: None,
            build_draw_calls_bg: None,
            range_local_scan_bg: None,
            range_block_scan_bg: None,
            range_write_bg: None,
            shadow_partition_bg: None,
            bind_group_key: None,
            readback: readback::RangeReadback::new(),
            last_inputs: None,
            settled: false,
            skip_this_frame: false,
            last_structure: None,
            transform_only: false,
            use_moved: false,
            moved_list: None,
            moved_bgs: Vec::new(),
            readback_pending: false,
            suppress_buf: fallback_buf.clone(),
            suppress_key: None,
            fallback_buf,
            draw_counts: Some(create_draw_counts_buffer(
                device,
                (MIN_SCRATCH_CAPACITY as usize * 3 + 4).max(MIN_DRAW_COUNT_SLOTS),
            )),
            supports_multi_draw_count: device
                .features()
                .contains(wgpu::Features::MULTI_DRAW_INDIRECT_COUNT),
        }
    }

    /// Grows every scratch buffer to at least `needed` rows, next-power-of-
    /// two rounded (with `MIN_SCRATCH_CAPACITY` as a floor) -- same "grow on
    /// demand, no data preserved across growth" policy `corona`'s
    /// `upload_sort_steps` and every `GrowableBuffer` in this codebase
    /// already use. Returns `true` if it actually reallocated (the caller
    /// must then rebuild every bind group referencing these buffers).
    fn ensure_capacity(&mut self, device: &wgpu::Device, needed: u32) -> bool {
        if needed <= self.scratch_capacity {
            return false;
        }
        let new_capacity = needed.next_power_of_two().max(MIN_SCRATCH_CAPACITY);
        self.scratch = ScratchBuffers::new(device, new_capacity);
        let needed_count_slots = new_capacity as usize * 3 + 4;
        if self
            .draw_counts
            .as_ref()
            .is_none_or(|buffer| buffer.size() < (needed_count_slots * 4) as u64)
        {
            self.draw_counts = Some(create_draw_counts_buffer(device, needed_count_slots));
        }
        self.draw_calls_out = create_storage_buffer(
            device,
            "ObjBatch DrawCalls",
            new_capacity as u64 * DRAW_CALL_BYTES,
        );
        self.indirect_out = create_storage_buffer(
            device,
            "ObjBatch Indirect",
            new_capacity as u64 * INDIRECT_ARGS_BYTES,
        );
        self.scratch_capacity = new_capacity;
        true
    }

    fn rebuild_bind_groups(
        &mut self,
        device: &wgpu::Device,
        static_objects: &wgpu::Buffer,
        materials: &wgpu::Buffer,
    ) {
        self.moved_bgs.clear();
        let s = &self.scratch;
        let cs = wgpu::ShaderStages::COMPUTE;
        let _ = cs;

        self.gather_bg = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ObjBatch Gather BG"),
            layout: &self.bgls.gather,
            entries: &[
                bg_entry(0, &self.batch_uniform),
                bg_entry(1, static_objects),
                bg_entry(2, &s.keys_a),
                bg_entry(3, &s.indices_a),
                bg_entry(4, &s.gather_count),
                bg_entry(5, &s.prev_rows),
            ],
        }));

        self.prepare_bg = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ObjBatch Prepare BG"),
            layout: &self.bgls.prepare,
            entries: &[
                bg_entry(0, &s.gather_count),
                bg_entry(1, &s.frame_uniform),
                bg_entry(2, &s.dispatch_args),
            ],
        }));

        self.sort_binds = Some(self.sort.bind(
            device,
            &s.frame_uniform,
            [&s.keys_a, &s.keys_b],
            [&s.indices_a, &s.indices_b],
            self.scratch_capacity,
        ));

        // The sort's eight (even) passes end back in A.
        self.final_gather_bg = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ObjBatch FinalGather BG"),
            layout: &self.bgls.final_gather,
            entries: &[
                bg_entry(0, &s.frame_uniform),
                bg_entry(1, &s.indices_a),
                bg_entry(2, static_objects),
                bg_entry(3, &s.instances_out),
                bg_entry(4, &s.aabbs_out),
                bg_entry(5, &s.prev_rows),
                bg_entry(6, &s.sorted_position),
                bg_entry(7, &self.suppress_buf),
            ],
        }));

        self.group_local_scan_bg = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ObjBatch GroupLocalScan BG"),
            layout: &self.bgls.group_local_scan,
            entries: &[
                bg_entry(0, &s.frame_uniform),
                bg_entry(1, &s.indices_a),
                bg_entry(2, static_objects),
                bg_entry(3, &s.local_group_rank),
                bg_entry(4, &s.block_group_totals),
            ],
        }));

        self.group_block_scan_bg = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ObjBatch GroupBlockScan BG"),
            layout: &self.bgls.group_block_scan,
            entries: &[
                bg_entry(0, &s.frame_uniform),
                bg_entry(1, &s.block_group_totals),
                bg_entry(2, &s.group_count),
            ],
        }));

        self.group_write_bg = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ObjBatch GroupWrite BG"),
            layout: &self.bgls.group_write,
            entries: &[
                bg_entry(0, &s.frame_uniform),
                bg_entry(1, &s.indices_a),
                bg_entry(2, static_objects),
                bg_entry(3, &s.local_group_rank),
                bg_entry(4, &s.block_group_totals),
                bg_entry(5, &s.group_starts),
            ],
        }));

        self.group_write_sentinel_bg = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ObjBatch GroupWriteSentinel BG"),
            layout: &self.bgls.group_write_sentinel,
            entries: &[
                bg_entry(0, &s.frame_uniform),
                bg_entry(1, &s.group_starts),
                bg_entry(2, &s.group_count),
            ],
        }));

        self.prepare_groups_bg = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ObjBatch PrepareGroups BG"),
            layout: &self.bgls.prepare_groups,
            entries: &[
                bg_entry(0, &s.group_count),
                bg_entry(1, &s.dispatch_args_groups),
            ],
        }));

        self.build_draw_calls_bg = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ObjBatch BuildDrawCalls BG"),
            layout: &self.bgls.build_draw_calls,
            entries: &[
                bg_entry(0, &s.group_count),
                bg_entry(1, &s.group_starts),
                bg_entry(2, &s.indices_a),
                bg_entry(3, static_objects),
                bg_entry(4, &self.draw_calls_out),
                bg_entry(5, &s.group_material_class),
                bg_entry(6, &s.group_graph_hash_lo),
                bg_entry(7, &s.group_graph_hash_hi),
                bg_entry(8, &s.group_shading),
                bg_entry(9, materials),
                bg_entry(10, &self.indirect_out),
            ],
        }));

        self.range_local_scan_bg = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ObjBatch RangeLocalScan BG"),
            layout: &self.bgls.range_local_scan,
            entries: &[
                bg_entry(0, &s.group_count),
                bg_entry(1, &s.group_material_class),
                bg_entry(2, &s.group_graph_hash_lo),
                bg_entry(3, &s.group_graph_hash_hi),
                bg_entry(4, &s.local_range_rank),
                bg_entry(5, &s.block_range_totals),
                bg_entry(6, &s.group_shading),
            ],
        }));

        self.range_block_scan_bg = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ObjBatch RangeBlockScan BG"),
            layout: &self.bgls.range_block_scan,
            entries: &[
                bg_entry(0, &s.dispatch_args_groups),
                bg_entry(1, &s.block_range_totals),
                bg_entry(2, &s.range_count),
            ],
        }));

        self.range_write_bg = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ObjBatch RangeWrite BG"),
            layout: &self.bgls.range_write,
            entries: &[
                bg_entry(0, &s.group_count),
                bg_entry(1, &s.group_material_class),
                bg_entry(2, &s.group_graph_hash_lo),
                bg_entry(3, &s.group_graph_hash_hi),
                bg_entry(4, &s.group_shading),
                bg_entry(5, &s.local_range_rank),
                bg_entry(6, &s.block_range_totals),
                bg_entry(7, &s.opaque_ranges),
                bg_entry(8, &s.transparent_ranges),
                bg_entry(9, &s.forward_ranges),
                bg_entry(10, self.draw_counts.as_ref().expect("draw-count buffer")),
            ],
        }));

        self.shadow_partition_bg = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ObjBatch ShadowPartition BG"),
            layout: &self.bgls.shadow_partition,
            entries: &[
                bg_entry(0, &s.frame_uniform),
                bg_entry(1, &s.indices_a),
                bg_entry(2, static_objects),
                bg_entry(3, &s.shadow_static_indirect),
                bg_entry(4, &s.shadow_movable_indirect),
                bg_entry(5, &s.shadow_counts),
                bg_entry(6, materials),
                bg_entry(7, &s.shadow_transmissive_indirect),
            ],
        }));
    }

    fn moved_binds(&self, device: &wgpu::Device, list: &wgpu::Buffer) -> MovedBinds {
        MovedBinds {
            gather: device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ObjBatch Moved BG"),
                layout: &self.bgls.moved,
                entries: &[bg_entry(0, list)],
            }),
            args: device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ObjBatch MovedArgs BG"),
                layout: &self.bgls.moved_args,
                entries: &[bg_entry(0, list), bg_entry(1, &self.scratch.moved_args)],
            }),
        }
    }

    /// Like `record_placements`, but only for the slots in the scene join's
    /// moved list: the cost follows how many objects moved, not how many
    /// there are. The sorted order and groups stand, as for the full
    /// placements run, and each moved row is re-gathered into the sorted slot
    /// the last full run gave it (`sorted_position`).
    fn record_moved_placements(&self, cmds: &mut CommandRecorder<'_>, moved: &MovedBinds) {
        {
            let mut pass = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("ObjBatch MovedArgs"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipelines.moved_args);
            pass.set_bind_group(1, &moved.args, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        let mut pass = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("ObjBatch FinalGather (moved)"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipelines.final_gather_moved);
        pass.set_bind_group(0, self.final_gather_bg.as_ref().unwrap(), &[]);
        pass.set_bind_group(1, &moved.gather, &[]);
        pass.dispatch_workgroups_indirect(&self.scratch.moved_args, 0);
    }

    /// A run after objects only moved. The sort order, the groups, the draw
    /// calls and the shadow partitions are all functions of what the objects
    /// are, not where they are, and the last full run left them standing in
    /// this pass's persistent buffers. All that is stale is each sorted
    /// slot's copy of its object's transform, bounds and previous transform,
    /// which the final gather rewrites from the live rows through the sorted
    /// indices the last full run kept. One dispatch instead of the sort's
    /// hundred.
    fn record_placements(&self, cmds: &mut CommandRecorder<'_>) {
        if self.use_moved {
            if let Some((_, moved)) = self
                .moved_list
                .as_ref()
                .and_then(|list| self.moved_bgs.iter().find(|(buffer, _)| buffer == list))
            {
                self.record_moved_placements(cmds, moved);
                return;
            }
        }
        let mut pass = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("ObjBatch FinalGather (placements)"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipelines.final_gather);
        pass.set_bind_group(0, self.final_gather_bg.as_ref().unwrap(), &[]);
        pass.dispatch_workgroups_indirect(&self.scratch.dispatch_args, 0);
    }

    /// Records the full 8-stage dispatch sequence. Shared by the graph-
    /// integrated `execute()` and [`Self::run_once_for_testing`] -- same
    /// "test records identically to the real path" discipline `helio-pass-
    /// sprite-cull`'s `record`/`run_once_for_testing` split uses.
    fn record(&self, cmds: &mut CommandRecorder<'_>, capacity: u32) {
        cmds.clear_buffer(&self.scratch.gather_count, 0, None);
        cmds.clear_buffer(&self.scratch.group_count, 0, None);
        cmds.clear_buffer(&self.scratch.range_count, 0, None);
        cmds.clear_buffer(&self.scratch.shadow_counts, 0, None);

        {
            let mut pass = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("ObjBatch Gather"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipelines.gather);
            pass.set_bind_group(0, self.gather_bg.as_ref().unwrap(), &[]);
            pass.dispatch_workgroups(capacity.div_ceil(WG).max(1), 1, 1);
        }
        {
            let mut pass = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("ObjBatch Prepare"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipelines.prepare);
            pass.set_bind_group(0, self.prepare_bg.as_ref().unwrap(), &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }

        {
            let mut pass = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("ObjBatch Sort"),
                timestamp_writes: None,
            });
            self.sort.record(
                &mut pass,
                self.sort_binds.as_ref().unwrap(),
                &self.scratch.dispatch_args,
                0,
            );
        }
        {
            let mut pass = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("ObjBatch FinalGather"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipelines.final_gather);
            pass.set_bind_group(0, self.final_gather_bg.as_ref().unwrap(), &[]);
            pass.dispatch_workgroups_indirect(&self.scratch.dispatch_args, 0);
        }
        {
            let mut pass = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("ObjBatch GroupLocalScan"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipelines.group_local_scan);
            pass.set_bind_group(0, self.group_local_scan_bg.as_ref().unwrap(), &[]);
            pass.dispatch_workgroups_indirect(&self.scratch.dispatch_args, 0);
        }
        {
            let mut pass = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("ObjBatch GroupBlockScan"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipelines.group_block_scan);
            pass.set_bind_group(0, self.group_block_scan_bg.as_ref().unwrap(), &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        {
            let mut pass = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("ObjBatch GroupWrite"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipelines.group_write);
            pass.set_bind_group(0, self.group_write_bg.as_ref().unwrap(), &[]);
            pass.dispatch_workgroups_indirect(&self.scratch.dispatch_args, 0);
        }
        {
            let mut pass = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("ObjBatch GroupWriteSentinel"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipelines.group_write_sentinel);
            pass.set_bind_group(0, self.group_write_sentinel_bg.as_ref().unwrap(), &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        {
            let mut pass = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("ObjBatch PrepareGroups"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipelines.prepare_groups);
            pass.set_bind_group(0, self.prepare_groups_bg.as_ref().unwrap(), &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        {
            let mut pass = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("ObjBatch BuildDrawCalls"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipelines.build_draw_calls);
            pass.set_bind_group(0, self.build_draw_calls_bg.as_ref().unwrap(), &[]);
            pass.dispatch_workgroups_indirect(&self.scratch.dispatch_args_groups, 0);
        }
        {
            let mut pass = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("ObjBatch RangeLocalScan"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipelines.range_local_scan);
            pass.set_bind_group(0, self.range_local_scan_bg.as_ref().unwrap(), &[]);
            pass.dispatch_workgroups_indirect(&self.scratch.dispatch_args_groups, 0);
        }
        {
            let mut pass = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("ObjBatch RangeBlockScan"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipelines.range_block_scan);
            pass.set_bind_group(0, self.range_block_scan_bg.as_ref().unwrap(), &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        {
            let mut pass = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("ObjBatch RangeWrite"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipelines.range_write);
            pass.set_bind_group(0, self.range_write_bg.as_ref().unwrap(), &[]);
            pass.dispatch_workgroups_indirect(&self.scratch.dispatch_args_groups, 0);
        }
        {
            let mut pass = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("ObjBatch ShadowPartition"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipelines.shadow_partition);
            pass.set_bind_group(0, self.shadow_partition_bg.as_ref().unwrap(), &[]);
            pass.dispatch_workgroups_indirect(&self.scratch.dispatch_args, 0);
        }
        if let Some(draw_counts) = self.draw_counts.as_ref() {
            cmds.copy_buffer_to_buffer(&self.scratch.group_count, 0, draw_counts, 0, 4);
            cmds.copy_buffer_to_buffer(&self.scratch.shadow_counts, 0, draw_counts, 4, 12);
        }
    }

    /// GPU-produced, sorted-order instance buffer -- same shape as
    /// `helio_mesh_api::GpuInstanceData`.
    pub fn instances_buffer(&self) -> &wgpu::Buffer {
        &self.scratch.instances_out
    }
    pub fn aabbs_buffer(&self) -> &wgpu::Buffer {
        &self.scratch.aabbs_out
    }
    pub fn draw_calls_buffer(&self) -> &wgpu::Buffer {
        &self.draw_calls_out
    }
    /// `wgpu`-ABI indirect draw args, one per draw-call group, same order
    /// as [`Self::draw_calls_buffer`] -- what `multi_draw_indexed_indirect`
    /// actually reads.
    pub fn indirect_buffer(&self) -> &wgpu::Buffer {
        &self.indirect_out
    }
    pub fn shadow_static_indirect_buffer(&self) -> &wgpu::Buffer {
        &self.scratch.shadow_static_indirect
    }
    pub fn shadow_movable_indirect_buffer(&self) -> &wgpu::Buffer {
        &self.scratch.shadow_movable_indirect
    }
    /// Raw buffers for direct, synchronous verification (integration tests
    /// only -- production code should read [`Self::opaque_ranges`]/
    /// [`Self::transparent_ranges`]/[`Self::forward_ranges`]/[`Self::
    /// counts`] instead, which are the async-readback-backed, non-stalling
    /// versions of the same data).
    /// Per-group shading classification bits and material_class -- used by
    /// `tests/gpu_object_batch_validation.rs`'s `OBJ_BATCH_DEBUG`-gated
    /// diagnostics when a correctness assertion needs more detail than the
    /// production getters expose.
    #[doc(hidden)]
    pub fn debug_group_shading_buffer(&self) -> &wgpu::Buffer {
        &self.scratch.group_shading
    }
    #[doc(hidden)]
    pub fn debug_group_material_class_buffer(&self) -> &wgpu::Buffer {
        &self.scratch.group_material_class
    }
    pub fn group_count_buffer(&self) -> &wgpu::Buffer {
        &self.scratch.group_count
    }
    pub fn range_bucket_counts_buffer(&self) -> &wgpu::Buffer {
        &self.scratch.range_count
    }
    pub fn shadow_counts_buffer(&self) -> &wgpu::Buffer {
        &self.scratch.shadow_counts
    }
    pub fn opaque_ranges_buffer(&self) -> &wgpu::Buffer {
        &self.scratch.opaque_ranges
    }
    pub fn transparent_ranges_buffer(&self) -> &wgpu::Buffer {
        &self.scratch.transparent_ranges
    }
    pub fn forward_ranges_buffer(&self) -> &wgpu::Buffer {
        &self.scratch.forward_ranges
    }

    /// The last harvested opaque/transparent/forward range tables, two or
    /// more frames behind the GPU work that produced them (see the `readback`
    /// module doc). Empty until the first successful readback.
    pub fn opaque_ranges(&self) -> &[RangeTuple] {
        self.readback.opaque()
    }
    pub fn transparent_ranges(&self) -> &[RangeTuple] {
        self.readback.transparent()
    }
    pub fn forward_ranges(&self) -> &[RangeTuple] {
        self.readback.forward()
    }
    /// Last-completed-frame's live draw-call-group count and shadow static/
    /// movable draw counts -- `(draw_count, shadow_static, shadow_movable)`.
    pub fn counts(&self) -> (u32, u32, u32) {
        self.readback.counts()
    }
    /// Live instance count -- same value as [`Self::instances_buffer`]'s
    /// valid prefix length.
    pub fn instance_count(&self) -> u32 {
        self.readback.instance_count()
    }
    /// Static-shadow-atlas cache invalidation signal -- bumps whenever the
    /// static (non-`INSTANCE_FLAG_MOVABLE`) object set's size last changed,
    /// same role `GpuScene::static_objects_generation` used to serve.
    pub fn shadow_static_generation(&self) -> u64 {
        self.readback.shadow_static_generation()
    }

    /// Runs the full pipeline once, outside a `RenderGraph`, blocking until
    /// the GPU is done -- for integration tests that need to read back
    /// results without standing up a full graph + `PassContext`. Mirrors
    /// the sprites plugin's `SpriteCullPass::run_once_for_testing`.
    pub fn run_once_for_testing(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        static_objects: &wgpu::Buffer,
        materials: &wgpu::Buffer,
        capacity: u32,
    ) {
        self.ensure_capacity(device, capacity);
        self.rebuild_bind_groups(device, static_objects, materials);
        queue.write_buffer(
            &self.batch_uniform,
            0,
            bytemuck::bytes_of(&BatchUniforms {
                capacity,
                max_objects: self.scratch_capacity,
                max_groups: self.scratch_capacity,
                max_ranges: self.scratch_capacity,
            }),
        );
        // `RangeBlockUniform` reuses `dispatch_args_groups[0]` directly (the
        // dual-bind trick) -- nothing to write here.

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("ObjBatch Test Encoder"),
        });
        self.record(&mut CommandRecorder::from_encoder(&mut encoder), capacity);
        queue.submit([encoder.finish()]);
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
    }

    /// Runs only the placements update (see `record_placements`) over
    /// `static_objects`, blocking until the GPU is done: for tests that
    /// compare it against a full run. A full run must have happened first
    /// ([`Self::run_once_for_testing`]) over rows that differ from these only
    /// in where the objects are.
    pub fn run_placements_for_testing(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        static_objects: &wgpu::Buffer,
        materials: &wgpu::Buffer,
    ) {
        self.rebuild_bind_groups(device, static_objects, materials);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("ObjBatch Placements Test Encoder"),
        });
        self.record_placements(&mut CommandRecorder::from_encoder(&mut encoder));
        queue.submit([encoder.finish()]);
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
    }

    /// Like [`Self::run_placements_for_testing`] but gathering only the slots
    /// in `moved_list` (`[total, fresh, slot, ..]`, the scene join's
    /// `OBJECTS_MOVED_KEY` layout), blocking until the GPU is done.
    pub fn run_moved_placements_for_testing(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        static_objects: &wgpu::Buffer,
        materials: &wgpu::Buffer,
        moved_list: &wgpu::Buffer,
    ) {
        self.rebuild_bind_groups(device, static_objects, materials);
        let binds = self.moved_binds(device, moved_list);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("ObjBatch Moved Placements Test Encoder"),
        });
        self.record_moved_placements(&mut CommandRecorder::from_encoder(&mut encoder), &binds);
        queue.submit([encoder.finish()]);
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
    }

    /// One frame of what the graph does with this pass -- `prepare`, then
    /// what `execute` records -- on `encoder`, returning which kind of run it
    /// recorded. For tests and benchmarks driving the pass outside a
    /// `RenderGraph`.
    pub fn frame_for_testing(
        &mut self,
        ctx: &PrepareContext,
        encoder: &mut wgpu::CommandEncoder,
    ) -> BatchRun {
        self.prepare(ctx).expect("object batch prepare");
        if self.skip_this_frame {
            return BatchRun::Skipped;
        }
        let capacity = ctx
            .scene_buffers
            .get(BufferKey::of("static_objects"))
            .map_or(0, |h| {
                (h.buffer.size() / std::mem::size_of::<StaticObjectRow>() as u64) as u32
            });
        let mut cmds = CommandRecorder::from_encoder(encoder);
        if !self.transform_only {
            self.record(&mut cmds, capacity);
            BatchRun::Full
        } else if self.use_moved {
            self.record_placements(&mut cmds);
            BatchRun::Moved
        } else {
            self.record_placements(&mut cmds);
            BatchRun::Placements
        }
    }

    /// Runs one step of the asynchronous range/count readback that
    /// `prepare()` performs each frame, for tests driving the pass outside a
    /// `RenderGraph` (pair with [`Self::run_once_for_testing`]).
    pub fn poll_readback_for_testing(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        self.readback
            .poll_and_kick_off(device, queue, &self.scratch, true, true);
    }
}

impl RenderPass for ObjectBatchPass {
    fn scene_buffers(&self) -> &'static [&'static str] {
        &["materials", "static_objects"]
    }

    fn name(&self) -> &'static str {
        "ObjectBatch"
    }

    fn render_pass_descriptor<'a>(
        &'a self,
        _target: &'a wgpu::TextureView,
        _depth: &'a wgpu::TextureView,
        _resources: &'a helio_core::ResourceRegistry<'a>,
    ) -> Option<wgpu::RenderPassDescriptor<'a>> {
        None // Compute-only pass -- no render pass.
    }

    fn declare_resources(&self, builder: &mut helio_core::graph::ResourceBuilder) {
        builder.write_buffer("object_batch");
        builder.write_buffer(helio_core::resource_keys::DEPTH_DRAW_SIGNATURE);
    }

    fn publish<'a>(&self, frame: &mut helio_core::ResourceRegistry<'a>) {
        let (draw_count, shadow_static_draw_count, shadow_movable_draw_count) = self.counts();
        // SAFETY: every borrow in this struct comes out of `self`'s own
        // owned buffers/scratch, which are pass-lifetime (owned by the
        // RenderGraph across many frames), never frame-scoped -- so any
        // single frame's `'a` is always shorter than the pass's real
        // lifetime. `publish(&self, ..)` (not `&'a self`) cannot express
        // that relationship, so the borrow checker sees an unrelated,
        // shorter lifetime here instead; matches `ResourceRegistry::
        // write_texture_binding`'s own transmute for the same reason.
        let data: helio_mesh_api::ObjectBatchFrameData<'a> = unsafe {
            std::mem::transmute(helio_mesh_api::ObjectBatchFrameData {
                instances: &self.scratch.instances_out,
                aabbs: &self.scratch.aabbs_out,
                draw_calls: &self.draw_calls_out,
                indirect: &self.indirect_out,
                readback_draw_count: draw_count,
                readback_instance_count: self.instance_count(),
                group_capacity: self.scratch_capacity,
                group_dispatch: helio_mesh_api::IndirectDispatchArgs {
                    buffer: &self.scratch.dispatch_args_groups,
                    offset: GROUP_DISPATCH_OFFSET,
                },
                opaque_ranges: self.opaque_ranges(),
                transparent_ranges: self.transparent_ranges(),
                forward_ranges: self.forward_ranges(),
                opaque_ranges_gpu: &self.scratch.opaque_ranges,
                transparent_ranges_gpu: &self.scratch.transparent_ranges,
                forward_ranges_gpu: &self.scratch.forward_ranges,
                range_counts_gpu: &self.scratch.range_count,
                draw_counts_gpu: self.draw_counts.as_ref().expect("draw-count buffer"),
                shadow_static_indirect: &self.scratch.shadow_static_indirect,
                readback_shadow_static_draw_count: shadow_static_draw_count,
                shadow_movable_indirect: &self.scratch.shadow_movable_indirect,
                readback_shadow_movable_draw_count: shadow_movable_draw_count,
                shadow_transmissive_indirect: &self.scratch.shadow_transmissive_indirect,
                readback_shadow_transmissive_draw_count: self.readback.shadow_transmissive(),
                shadow_static_generation: self.shadow_static_generation(),
                draw_counts: self
                    .supports_multi_draw_count
                    .then(|| self.draw_counts.as_ref().expect("draw-count buffer")),
                range_slot_capacity: self.scratch_capacity,
            })
        };
        frame.write(
            helio_core::ResourceKey::new("object_batch"),
            data,
            "ObjectBatch",
        );
        // Culling and drawing follow this frame's GPU counts, but a few
        // fallbacks still follow these read-back ones (Hi-Z warm-up, draws
        // without `MULTI_DRAW_INDIRECT_COUNT`), so depth can also change when
        // they do.
        helio_core::resource_keys::fold_depth_draw_signature(
            frame,
            (u64::from(draw_count) << 32) | u64::from(self.instance_count()),
            "ObjectBatch",
        );
        // `[static, movable, transmissive]` shadow-caster counts, written on
        // the GPU this frame (unlike the `readback_shadow_*` fields above,
        // which trail by the readback latency). Lets shadow consumers skip an
        // atlas that holds no casters.
        let counts: &'a wgpu::Buffer = unsafe { std::mem::transmute(&self.scratch.shadow_counts) };
        frame.write_buffer(
            helio_core::ResourceKey::new(helio_light_api::SHADOW_CASTER_COUNTS),
            counts,
            "ObjectBatch",
        );
    }

    fn prepare(&mut self, ctx: &PrepareContext) -> HelioResult<()> {
        let static_objects_handle = ctx.scene_buffers.get(BufferKey::of("static_objects"));
        let capacity = static_objects_handle
            .as_ref()
            .map(|h| (h.buffer.size() / std::mem::size_of::<StaticObjectRow>() as u64) as u32)
            .unwrap_or(0);

        // Growth (a `&mut self` borrow) must fully finish before we take any
        // reference into `self` (the fallback buffer) below.
        let grew = self.ensure_capacity(ctx.device, capacity.max(1));

        // Cloned (a cheap resource-handle clone, not a data copy -- same as
        // `BufferHandle::buffer`'s own "owned clone" doc) rather than
        // borrowed, so it doesn't tie up `self` across the `&mut self`
        // calls below. Change-detection uses the handle's own `epoch`
        // (bumped on every reallocation) rather than a pointer into this
        // freshly-cloned local, which would have a different stack address
        // every single frame and defeat the cache entirely.
        let static_objects_epoch = static_objects_handle.as_ref().map(|h| h.epoch);
        let static_objects_buf: wgpu::Buffer = static_objects_handle
            .as_ref()
            .map(|h| h.buffer.clone())
            .unwrap_or_else(|| self.fallback_buf.clone());
        let materials_handle = ctx.scene_buffers.get(BufferKey::of("materials"));
        let materials_buf = materials_handle
            .as_ref()
            .map(|h| &h.buffer)
            .unwrap_or(&static_objects_buf);
        let suppress_handle = ctx.scene_buffers.get(BufferKey::of(CLASSIC_SUPPRESSED_KEY));
        let suppress_buf: wgpu::Buffer = suppress_handle
            .as_ref()
            .map(|h| h.buffer.clone())
            .unwrap_or_else(|| self.fallback_buf.clone());
        let suppress_changed = self.suppress_key.as_ref() != Some(&suppress_buf);
        if suppress_changed {
            self.suppress_buf = suppress_buf.clone();
            self.suppress_key = Some(suppress_buf);
        }
        let key = (
            self.scratch_capacity,
            static_objects_epoch,
            materials_buf.clone(),
        );
        if grew || suppress_changed || self.bind_group_key.as_ref() != Some(&key) {
            self.rebuild_bind_groups(ctx.device, &static_objects_buf, materials_buf);
            self.bind_group_key = Some(key);
        }

        let generation = |handle: Option<&helio_core::BufferHandle>| {
            handle.map_or((u64::MAX, u64::MAX), |h| (h.epoch, h.content_generation))
        };
        let (objects_epoch, objects_content) = generation(static_objects_handle);
        let (materials_epoch, materials_content) = generation(materials_handle);
        let (suppress_epoch, suppress_content) = generation(suppress_handle);
        let inputs = [
            objects_epoch,
            objects_content,
            materials_epoch,
            materials_content,
            capacity as u64,
            self.scratch_capacity as u64,
            suppress_epoch,
            suppress_content,
        ];
        // The scene join's structure marker, when a join feeds this pass:
        // without one every change is treated as structural.
        let structure = ctx
            .scene_buffers
            .get(BufferKey::of(STRUCTURE_KEY))
            .map(|h| (h.epoch, h.content_generation));
        // Only the objects' rows moved (not the materials, capacities or the
        // structure): same objects, same meshes and materials, same sort
        // order. Their placements are all that differ.
        let placements_only = structure.is_some()
            && structure == self.last_structure
            && self.last_inputs.is_some_and(|last| {
                last[0] == inputs[0] && last[1] != inputs[1] && last[2..] == inputs[2..]
            });
        self.last_structure = structure;
        if self.last_inputs == Some(inputs) {
            self.skip_this_frame = self.settled;
            self.settled = true;
            // The repeat run after a change stays the kind it was.
        } else {
            let previous_inputs = self.last_inputs;
            self.last_inputs = Some(inputs);
            self.settled = false;
            self.skip_this_frame = false;
            self.transform_only = placements_only;
            // The join's moved-slot list covers this change when it was
            // built for exactly this step of the objects' content (and the
            // list before it, which settles last frame's movers' motion
            // vectors, was complete): then only those slots need gathering.
            let moved = ctx.scene_buffers.get(BufferKey::of(MOVED_KEY));
            let follows_last =
                previous_inputs.is_some_and(|last| last[1].wrapping_add(1) == inputs[1]);
            self.use_moved = placements_only
                && follows_last
                && moved.is_some_and(|h| {
                    h.content_generation != 0 && h.content_generation == inputs[1]
                });
            if let (true, Some(list)) = (self.use_moved, moved) {
                if !self
                    .moved_bgs
                    .iter()
                    .any(|(buffer, _)| *buffer == list.buffer)
                {
                    let binds = self.moved_binds(ctx.device, &list.buffer);
                    if self.moved_bgs.len() >= 4 {
                        self.moved_bgs.remove(0);
                    }
                    self.moved_bgs.push((list.buffer.clone(), binds));
                }
                self.moved_list = Some(list.buffer.clone());
            }
        }
        if !self.skip_this_frame && !self.transform_only {
            self.readback_pending = true;
        }

        ctx.write_buffer(
            &self.batch_uniform,
            0,
            bytemuck::bytes_of(&BatchUniforms {
                capacity,
                max_objects: self.scratch_capacity,
                max_groups: self.scratch_capacity,
                max_ranges: self.scratch_capacity,
            }),
        );

        // The tables read back (counts, ranges) come from the sort and the
        // grouping, so a run that only moved objects has nothing new to copy,
        // unless an earlier full run's results are still waiting to be.
        let kick_off_copy =
            !self.skip_this_frame && (!self.transform_only || self.readback_pending);
        let kicked = self.readback.poll_and_kick_off(
            ctx.device,
            ctx.queue,
            &self.scratch,
            kick_off_copy,
            ctx.owns_device,
        );
        if kicked {
            self.readback_pending = false;
        }
        Ok(())
    }

    fn execute(&mut self, ctx: &mut PassContext) -> HelioResult<()> {
        if self.skip_this_frame {
            // Nothing the pipeline reads changed: last frame's outputs stand.
            return Ok(());
        }
        let capacity = {
            let handle = ctx.scene_buffers.get(BufferKey::of("static_objects"));
            handle
                .map(|h| (h.buffer.size() / std::mem::size_of::<StaticObjectRow>() as u64) as u32)
                .unwrap_or(0)
        };
        if self.transform_only {
            self.record_placements(&mut ctx.graphics_cmds());
        } else {
            self.record(&mut ctx.graphics_cmds(), capacity);
        }
        Ok(())
    }
}

// Compile-time layout sanity: keeps `object_batch.wgsl`'s `StaticObjectRow`
// and this crate's assumed size in lock-step with the real
// `StaticObjectRow` -- a drift here fails `cargo test`, not a draw.
#[cfg(test)]
mod tests {
    use super::*;
    use helio_mesh_api::GpuInstanceData;

    #[test]
    fn static_object_component_size_matches_wgsl_mirror() {
        // 4×u32 (16) + transform(64) + prev_transform(64) + normal_mat(48)
        // + bounds(16) + 11×u32/i32/u32 (44) = 252 bytes -- see `object_batch
        // .wgsl`'s `StaticObjectRow` doc.
        assert_eq!(std::mem::size_of::<StaticObjectRow>(), 252);
    }

    #[test]
    fn output_struct_sizes_match_wgsl() {
        assert_eq!(INSTANCE_BYTES, 224);
        assert_eq!(
            helio_mesh_api::draw_slots::INSTANCE_BYTES,
            std::mem::size_of::<GpuInstanceData>() as u64
        );
        assert_eq!(AABB_BYTES, 32);
        assert_eq!(DRAW_CALL_BYTES, 20);
        assert_eq!(RANGE_BYTES, 20);
        assert_eq!(INDIRECT_ARGS_BYTES, 20);
    }
}
