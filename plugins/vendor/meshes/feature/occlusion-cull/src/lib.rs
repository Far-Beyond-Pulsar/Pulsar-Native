//! Hi-Z occlusion-culling and visible-range compaction pass.
//!
//! Runs AFTER IndirectDispatchPass (frustum cull) each frame, using the PREVIOUS
//! frame's Hi-Z pyramid (temporal approach). One workgroup per draw-call group
//! cooperatively Hi-Z-tests each instance that already survived frustum culling
//! (read from `compacted_indices`) and compacts real survivors into
//! `compacted_indices_2`, updating the source indirect instance counts. A
//! second GPU dispatch packs non-empty indirect draw records inside each
//! material range and writes survivor counts for indirect-count rendering.
//!
//! The first frame that actually has live instances has no Hi-Z pyramid yet,
//! so instead of testing anything it copies `compacted_indices` straight
//! through to `compacted_indices_2` unchanged (see `hiz_warmed_up`'s doc for
//! why this is gated on "first frame with real instances", not `frame_num ==
//! 0` -- the two are not the same frame). Bind-group is rebuilt lazily when
//! buffer pointers change (e.g. scene grows).

use std::sync::Arc;

pub mod feature;
pub use feature::MeshCulling;

use bytemuck::{Pod, Zeroable};
use helio_core::{PassContext, PrepareContext, RenderPass, Result as HelioResult};

pub use helio_pass_gbuffer::CulledBatchFrameData;
use helio_pass_gbuffer::{DrawSegment, ShadingBucket};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CullParams {
    screen_width: u32,
    screen_height: u32,
    /// Rows `draw_calls` holds; the dispatch itself covers the live groups.
    draw_capacity: u32,
    hiz_mip_count: u32,
    /// Baked PVS grid (`occlusion_cull.wgsl` documents the layout); 0 = none.
    pvs_available: u32,
    pvs_grid: [u32; 3],
    pvs_min: [f32; 3],
    pvs_cell_size: f32,
    /// u32 words per source cell: the baker's u64 words × 2.
    pvs_words_per_cell: u32,
    _pad: [u32; 3],
}

/// The baked PVS grid shape last uploaded, and the bitfield it came from.
#[derive(Clone, Copy, PartialEq)]
struct PvsGrid {
    grid: [u32; 3],
    min: [f32; 3],
    cell_size: f32,
    words_per_cell: u32,
    /// Identity of the baked bitfield (address, length): re-upload on change.
    source: (usize, usize),
}

/// Below this many instances, `compacted_indices_2_buf` still allocates at
/// this floor -- matches `ObjectBatchPass`'s own `MIN_SCRATCH_CAPACITY` idiom.
const MIN_CAPACITY: u32 = 256;

/// Smallest draw segment, in indirect records.
const MIN_SEGMENT_CAPACITY: u32 = 64;
/// A material key absent from the range tables this many frames loses its
/// segment. Editing a material graph makes a new key, so keys must expire.
const SEGMENT_EVICT_FRAMES: u64 = 300;

/// `compact_ranges.wgsl`'s `RangeCapacity`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RangeCompactParams {
    slots: u32,
    bucket: u32,
    segment_count: u32,
    _pad: u32,
}

/// `compact_ranges.wgsl`'s `Segment`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuSegment {
    material_class: u32,
    graph_hash_lo: u32,
    graph_hash_hi: u32,
    bucket: u32,
    first: u32,
    capacity: u32,
    _pad: [u32; 2],
}

/// Room for a key with `groups` draw groups before culling: twice that,
/// rounded up, so a key grows a while before its segment must.
fn segment_capacity(groups: u32) -> u32 {
    groups
        .saturating_mul(2)
        .next_power_of_two()
        .max(MIN_SEGMENT_CAPACITY)
}

/// The material keys that have draw segments, and where each segment lies.
#[derive(Default)]
struct SegmentKeys {
    segments: Vec<DrawSegment>,
    /// Frame each segment's key was last in the range tables.
    last_seen: Vec<u64>,
    frame: u64,
}

type RangeTuple = (u32, u64, u32, u32);

impl SegmentKeys {
    /// Adds the keys in this frame's (read-back) range tables, grows segments
    /// they outgrew and drops keys unseen for `SEGMENT_EVICT_FRAMES`. Returns
    /// whether any segment moved, so the table is re-uploaded only then and
    /// unchanged frames record the same commands.
    fn observe(&mut self, tables: [(ShadingBucket, &[RangeTuple]); 3]) -> bool {
        self.frame += 1;

        // A key's groups before culling bound its survivors. One key can span
        // several ranges, so sum them.
        let mut groups: Vec<((u32, u64, ShadingBucket), u32)> = Vec::new();
        for (bucket, ranges) in tables {
            for &(class, hash, _start, count) in ranges {
                let key = (class, hash, bucket);
                match groups.iter_mut().find(|(k, _)| *k == key) {
                    Some((_, total)) => *total += count,
                    None => groups.push((key, count)),
                }
            }
        }

        let mut changed = false;
        for (key, count) in groups {
            let existing = self
                .segments
                .iter()
                .position(|s| (s.material_class, s.graph_hash, s.bucket) == key);
            match existing {
                Some(index) => {
                    self.last_seen[index] = self.frame;
                    if count > self.segments[index].capacity {
                        self.segments[index].capacity = segment_capacity(count);
                        changed = true;
                    }
                }
                None => {
                    self.segments.push(DrawSegment {
                        material_class: key.0,
                        graph_hash: key.1,
                        bucket: key.2,
                        first: 0,
                        capacity: segment_capacity(count),
                    });
                    self.last_seen.push(self.frame);
                    changed = true;
                }
            }
        }
        let frame = self.frame;
        let live = |seen: &u64| frame - seen <= SEGMENT_EVICT_FRAMES;
        let mut keep = self
            .last_seen
            .iter()
            .map(live)
            .collect::<Vec<_>>()
            .into_iter();
        let before = self.segments.len();
        self.segments.retain(|_| keep.next().unwrap());
        self.last_seen.retain(live);
        changed |= self.segments.len() != before;

        if changed {
            let mut first = 0u32;
            for segment in &mut self.segments {
                segment.first = first;
                first += segment.capacity;
            }
        }
        changed
    }

    /// Indirect records all segments take.
    fn records(&self) -> u32 {
        self.segments.last().map_or(0, |s| s.first + s.capacity)
    }
}

/// Per-material draw segments (see `helio_pass_gbuffer::DrawSegments`): the
/// keys, the table's GPU copy, and the buffers range compaction fills.
struct SegmentTable {
    keys: SegmentKeys,
    /// `GpuSegment` rows; sized to a power of two of segments.
    table_buf: wgpu::Buffer,
    indirect_buf: wgpu::Buffer,
    counts_buf: wgpu::Buffer,
    /// Indirect records `indirect_buf` holds.
    record_capacity: u32,
    /// Whether draws can read `counts_buf` (`MULTI_DRAW_INDIRECT_COUNT`).
    counts_supported: bool,
    /// Bumped whenever the table changes.
    generation: u64,
}

impl SegmentTable {
    fn new(device: &wgpu::Device) -> Self {
        Self {
            keys: SegmentKeys::default(),
            table_buf: create_segment_buf(device, "OcclusionCull Segment Table", 32, false),
            indirect_buf: create_segment_buf(device, "OcclusionCull Segment Indirect", 20, true),
            counts_buf: create_segment_buf(device, "OcclusionCull Segment Counts", 4, true),
            record_capacity: 1,
            counts_supported: false,
            generation: 0,
        }
    }

    fn segments(&self) -> &[DrawSegment] {
        &self.keys.segments
    }

    /// Follows `batch`'s range tables (see [`SegmentKeys::observe`]) and
    /// uploads the table when it changed.
    fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        batch: &helio_pass_gbuffer::ObjectBatchFrameData<'_>,
    ) {
        self.counts_supported = batch.draw_counts.is_some();
        let changed = self.keys.observe([
            (ShadingBucket::Opaque, batch.opaque_ranges),
            (ShadingBucket::Transparent, batch.transparent_ranges),
            (ShadingBucket::Forward, batch.forward_ranges),
        ]);
        if !changed {
            return;
        }
        self.generation += 1;

        let records = self.keys.records();
        if records > self.record_capacity {
            self.record_capacity = records.next_power_of_two();
            self.indirect_buf = create_segment_buf(
                device,
                "OcclusionCull Segment Indirect",
                self.record_capacity as u64 * 20,
                true,
            );
        }
        let count_bytes = (self.keys.segments.len().max(1) as u64 * 4).next_power_of_two();
        if count_bytes > self.counts_buf.size() {
            self.counts_buf =
                create_segment_buf(device, "OcclusionCull Segment Counts", count_bytes, true);
        }
        let rows: Vec<GpuSegment> = self
            .keys
            .segments
            .iter()
            .map(|s| GpuSegment {
                material_class: s.material_class,
                graph_hash_lo: s.graph_hash as u32,
                graph_hash_hi: (s.graph_hash >> 32) as u32,
                bucket: s.bucket as u32,
                first: s.first,
                capacity: s.capacity,
                _pad: [0; 2],
            })
            .collect();
        let table_bytes = (rows.len().max(1) as u64 * 32).next_power_of_two();
        if table_bytes > self.table_buf.size() {
            self.table_buf =
                create_segment_buf(device, "OcclusionCull Segment Table", table_bytes, false);
        }
        if !rows.is_empty() {
            queue.write_buffer(&self.table_buf, 0, bytemuck::cast_slice(&rows));
        }
    }
}

fn create_segment_buf(
    device: &wgpu::Device,
    label: &str,
    size: u64,
    indirect: bool,
) -> wgpu::Buffer {
    let mut usage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;
    if indirect {
        usage |= wgpu::BufferUsages::INDIRECT;
    }
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size.max(32),
        usage,
        mapped_at_creation: false,
    })
}

pub struct OcclusionCullPass {
    pipeline: wgpu::ComputePipeline,
    bgl: wgpu::BindGroupLayout,
    range_compact_pipeline: wgpu::ComputePipeline,
    range_compact_bgl: wgpu::BindGroupLayout,
    range_compact_params: wgpu::Buffer,
    range_compact_bind_group: Option<wgpu::BindGroup>,
    range_compact_key: Option<[wgpu::Buffer; 10]>,
    segments: SegmentTable,
    cull_params_buf: wgpu::Buffer,
    hiz_sampler: Arc<wgpu::Sampler>,
    cull_stats_buf: wgpu::Buffer,
    /// This pass's own output -- no longer a central `GpuScene` field (see
    /// `CulledBatchFrameData`'s doc, now in this crate): final (frustum +
    /// occlusion) surviving instance slots, one `u32` per live instance
    /// (worst case).
    compacted_indices_2_buf: wgpu::Buffer,
    /// Packed indirect records consumed by render passes after occlusion.
    compacted_indirect_buf: wgpu::Buffer,
    instance_capacity: u32,

    /// The baked PVS bitfield on the GPU (Helio#256), uploaded once per bake
    /// from the `baked_pvs` frame input. Four zero bytes when none is baked
    /// (`pvs_available = 0` then keeps the shader from reading it).
    pvs_buf: wgpu::Buffer,
    pvs_grid: Option<PvsGrid>,

    /// Cached bind group, invalidated when buffer pointers change.
    bind_group: Option<wgpu::BindGroup>,
    /// True once this pass has actually run its real Hi-Z test against a
    /// depth buffer built from a frame that drew real geometry. `frame_num
    /// == 0` is NOT an equivalent condition. The dispatches follow the GPU's
    /// live group count, but only `ObjectBatchPass`'s async readback tells
    /// the CPU that instances exist, frames later, so the bypass below runs
    /// until `batch.readback_instance_count` is non-zero: by then the frames
    /// the readback describes have drawn real geometry into depth.
    ///
    /// History: this pass used to early-out while the read-back draw count
    /// was 0, which on frame 0 it always is. The bypass then never executed,
    /// `compacted_indices_2_buf` stays zeroed, and the very first real
    /// dispatch (frame 1) Hi-Z-tests against a pyramid built from frame 0's
    /// EMPTY depth buffer (nothing was drawn, so nothing was written to
    /// it) -- in reversed-Z that background clears to 0.0/far, so every
    /// real object's near-depth reads as "closer than the empty
    /// background" and gets marked occluded. Occlusion culling mutates
    /// `indirect_dispatch.indirect` in place, so that zeroes every
    /// instance count; the next frame's depth buffer is then ALSO empty
    /// (nothing drew), and the cycle never recovers -- a permanent
    /// deadlock, not a one-frame glitch. Gating the bypass on "have I ever
    /// dispatched a real test" instead of "is this frame_num 0" fixes it:
    /// the bypass now runs on whichever frame is actually first to see
    /// `draw_count > 0`, guaranteeing real geometry lands in depth before
    /// Hi-Z testing ever reads from it.
    hiz_warmed_up: bool,
    /// A single-phase temporal query is valid only against unchanged inputs.
    /// Moving views need current-depth recovery before history may reject them.
    history_inputs: Option<(u64, u64, u64, u32, u32)>,
    /// ([camera, instances, draw_calls, indirect, pvs_buf, cull_stats_buf,
    /// compacted_indices, compacted_indices_2, coordinate_spaces], hiz_view)
    bind_group_key: Option<([wgpu::Buffer; 9], wgpu::TextureView)>,
    screen_width: u32,
    screen_height: u32,
}

impl OcclusionCullPass {
    /// Create the occlusion-cull pass.
    ///
    /// The HiZ texture view is read from `ctx.registry.hiz` each frame (routed
    /// by the graph). `hiz_sampler` is owned by HiZBuildPass and shared via Arc.
    pub fn new(
        device: &wgpu::Device,
        hiz_sampler: Arc<wgpu::Sampler>,
        screen_width: u32,
        screen_height: u32,
        cull_stats_buf: wgpu::Buffer,
    ) -> Self {
        let shader = helio_core::shader::module(
            device,
            "OcclusionCull Shader",
            helio_core::include_wgsl!("../shaders/occlusion_cull.wgsl"),
        );

        let cull_params_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("OcclusionCull CullParams"),
            size: std::mem::size_of::<CullParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let pvs_buf = create_pvs_buf(device, 4);

        // Bind group layout must match occlusion_cull.wgsl binding declarations.
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("OcclusionCull BGL"),
            entries: &[
                // 0: Camera storage
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 1: CullParams uniform
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
                // 2: GpuInstanceData[] (read-only)
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 3: GpuDrawCall[] (read-only) — for mapping draw index → first instance
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 4: Hi-Z texture
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // 5: Hi-Z sampler (non-filtering, nearest)
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                    count: None,
                },
                // 6: indirect draw buffer (read + write, u32 raw view)
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 7: Baked PVS bitfield (read-only storage)
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 9: Culling stats (read_write, atomic counters)
                wgpu::BindGroupLayoutEntry {
                    binding: 9,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 10: compacted_indices (read-only) — frustum-stage survivors
                wgpu::BindGroupLayoutEntry {
                    binding: 10,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 11: compacted_indices_2 (read_write) — final surviving set
                wgpu::BindGroupLayoutEntry {
                    binding: 11,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 12: coordinate_spaces (read-only) — see occlusion_cull.wgsl
                wgpu::BindGroupLayoutEntry {
                    binding: 12,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("OcclusionCull PL"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });

        let pipeline = helio_core::shader::create_compute_pipeline(
            &device,
            wgpu::ComputePipelineDescriptor {
                label: Some("OcclusionCull Pipeline"),
                layout: Some(&pl),
                module: &shader,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                cache: None,
            },
        );

        let range_compact_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("OcclusionCull RangeCompaction BGL"),
            entries: &[
                storage_layout_entry(0, true),
                storage_layout_entry(1, false),
                storage_layout_entry(2, true),
                storage_layout_entry(3, true),
                storage_layout_entry(4, true),
                storage_layout_entry(5, true),
                storage_layout_entry(6, false),
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: None,
                    },
                    count: None,
                },
                storage_layout_entry(8, true),
                storage_layout_entry(9, false),
                storage_layout_entry(10, false),
            ],
        });
        let compact_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("OcclusionCull RangeCompaction Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/compact_ranges.wgsl").into()),
        });
        let compact_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("OcclusionCull RangeCompaction PL"),
            bind_group_layouts: &[Some(&range_compact_bgl)],
            immediate_size: 0,
        });
        let range_compact_pipeline = helio_core::shader::create_compute_pipeline(
            &device,
            wgpu::ComputePipelineDescriptor {
                label: Some("OcclusionCull RangeCompaction Pipeline"),
                layout: Some(&compact_layout),
                module: &compact_shader,
                entry_point: Some("compact_ranges"),
                compilation_options: Default::default(),
                cache: None,
            },
        );
        let range_compact_params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("OcclusionCull RangeCompaction Params"),
            size: 768,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let compacted_indices_2_buf = create_compacted_indices_2_buf(device, MIN_CAPACITY);
        let compacted_indirect_buf = create_compacted_indirect_buf(device, MIN_CAPACITY);

        Self {
            pipeline,
            bgl,
            range_compact_pipeline,
            range_compact_bgl,
            range_compact_params,
            range_compact_bind_group: None,
            range_compact_key: None,
            segments: SegmentTable::new(device),
            cull_params_buf,
            hiz_sampler,
            cull_stats_buf,
            compacted_indices_2_buf,
            compacted_indirect_buf,
            instance_capacity: MIN_CAPACITY,
            pvs_buf,
            pvs_grid: None,
            bind_group: None,
            hiz_warmed_up: false,
            history_inputs: None,
            bind_group_key: None,
            screen_width,
            screen_height,
        }
    }

    /// Grows `compacted_indices_2_buf` and `compacted_indirect_buf` to at
    /// least `capacity` rows (next-power-of-two, floor `MIN_CAPACITY`).
    /// Returns `true` if they reallocated (the caller must then rebuild the
    /// bind group).
    fn ensure_capacity(&mut self, device: &wgpu::Device, capacity: u32) -> bool {
        if capacity <= self.instance_capacity {
            return false;
        }
        self.instance_capacity = capacity.next_power_of_two().max(MIN_CAPACITY);
        self.compacted_indices_2_buf =
            create_compacted_indices_2_buf(device, self.instance_capacity);
        self.compacted_indirect_buf = create_compacted_indirect_buf(device, self.instance_capacity);
        true
    }

    fn record_range_compaction(
        &mut self,
        ctx: &mut PassContext,
        batch: &helio_pass_gbuffer::ObjectBatchFrameData<'_>,
        source_indirect: &wgpu::Buffer,
    ) {
        let mut cmds = ctx.graphics_cmds();
        // Every group the GPU can produce; the live count is GPU-only.
        let bytes = source_indirect
            .size()
            .min(self.compacted_indirect_buf.size());
        cmds.copy_buffer_to_buffer(source_indirect, 0, &self.compacted_indirect_buf, 0, bytes);

        // Legacy/test frames can have no GPU range slots; preserve the
        // copied list unchanged in that case.
        let slots = batch.range_slot_capacity;
        if slots == 0 {
            return;
        }
        let key = [
            source_indirect.clone(),
            batch.range_counts_gpu.clone(),
            batch.opaque_ranges_gpu.clone(),
            batch.transparent_ranges_gpu.clone(),
            batch.forward_ranges_gpu.clone(),
            batch.draw_counts_gpu.clone(),
            self.compacted_indirect_buf.clone(),
            self.segments.table_buf.clone(),
            self.segments.indirect_buf.clone(),
            self.segments.counts_buf.clone(),
        ];
        if self.range_compact_key.as_ref() != Some(&key) {
            self.range_compact_bind_group =
                Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("OcclusionCull RangeCompaction BG"),
                    layout: &self.range_compact_bgl,
                    entries: &[
                        buffer_entry(0, source_indirect),
                        buffer_entry(1, &self.compacted_indirect_buf),
                        buffer_entry(2, batch.range_counts_gpu),
                        buffer_entry(3, batch.opaque_ranges_gpu),
                        buffer_entry(4, batch.transparent_ranges_gpu),
                        buffer_entry(5, batch.forward_ranges_gpu),
                        buffer_entry(6, batch.draw_counts_gpu),
                        uniform_range_entry(
                            7,
                            &self.range_compact_params,
                            std::mem::size_of::<RangeCompactParams>() as u64,
                        ),
                        buffer_entry(8, &self.segments.table_buf),
                        buffer_entry(9, &self.segments.indirect_buf),
                        buffer_entry(10, &self.segments.counts_buf),
                    ],
                }));
            self.range_compact_key = Some(key);
        }
        let mut pass = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("OcclusionCull RangeCompaction"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.range_compact_pipeline);
        for bucket in 0..3u32 {
            pass.set_bind_group(
                0,
                self.range_compact_bind_group.as_ref().unwrap(),
                &[bucket * 256],
            );
            pass.dispatch_workgroups_indirect(batch.range_counts_gpu, 16 + bucket as u64 * 12);
        }
    }

    /// Update internal-resolution dimensions used by cull uniforms.
    pub fn set_screen_size(&mut self, width: u32, height: u32) {
        self.screen_width = width;
        self.screen_height = height;
    }

    /// Test-only observability for `set_screen_size`'s effect -- lets an
    /// integration test assert this pass's cull-uniform resolution actually
    /// tracks the graph's real resolution across a resize, instead of only
    /// being inferable indirectly from occlusion-test outcomes.
    pub fn screen_size(&self) -> (u32, u32) {
        (self.screen_width, self.screen_height)
    }

    /// Whether a baked PVS is currently uploaded and used by the cull.
    pub fn pvs_active(&self) -> bool {
        self.pvs_grid.is_some()
    }

    /// Follow the `baked_pvs` frame input: upload its bitfield when a bake
    /// arrives or changes, drop it when it goes away.
    fn sync_pvs(&mut self, ctx: &PrepareContext) {
        let baked = ctx
            .registry
            .get::<helio_bake_types::BakedPvsRef<'_>>(helio_core::ResourceKey::new("baked_pvs"));
        let Some(pvs) = baked
            .filter(|pvs| pvs.cell_size > 0.0 && pvs.words_per_cell > 0 && !pvs.bits.is_empty())
        else {
            self.pvs_grid = None;
            return;
        };
        let grid = PvsGrid {
            grid: pvs.grid_dims,
            min: pvs.world_min,
            cell_size: pvs.cell_size,
            words_per_cell: pvs.words_per_cell * 2,
            source: (pvs.bits.as_ptr() as usize, pvs.bits.len()),
        };
        if self.pvs_grid == Some(grid) {
            return;
        }
        let bytes: &[u8] = bytemuck::cast_slice(pvs.bits);
        if self.pvs_buf.size() < bytes.len() as u64 {
            self.pvs_buf = create_pvs_buf(ctx.device, bytes.len() as u64);
        }
        ctx.write_buffer(&self.pvs_buf, 0, bytes);
        self.pvs_grid = Some(grid);
    }
}

fn create_pvs_buf(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("OcclusionCull Baked PVS"),
        size: size.max(4).next_multiple_of(4),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn create_compacted_indices_2_buf(device: &wgpu::Device, capacity: u32) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("OcclusionCull CompactedIndices2"),
        size: (capacity as u64 * 4).max(4),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn create_compacted_indirect_buf(device: &wgpu::Device, capacity: u32) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("OcclusionCull CompactedIndirect"),
        size: (capacity as u64 * 20).max(20),
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::INDIRECT
            | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn storage_layout_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
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

fn buffer_entry(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: buffer.as_entire_binding(),
    }
}

fn uniform_range_entry(binding: u32, buffer: &wgpu::Buffer, size: u64) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
            buffer,
            offset: 0,
            size: std::num::NonZeroU64::new(size),
        }),
    }
}

impl RenderPass for OcclusionCullPass {
    fn name(&self) -> &'static str {
        "OcclusionCull"
    }

    fn reads(&self) -> &'static [&'static str] {
        &["hiz", "object_batch", "indirect_dispatch"]
    }

    fn declare_resources(&self, builder: &mut helio_core::graph::ResourceBuilder) {
        builder.read("object_batch");
        builder.read("indirect_dispatch");
        builder.write_buffer("culled_batch");
        builder.write_buffer(helio_core::resource_keys::DEPTH_DRAW_SIGNATURE);
    }

    fn publish<'a>(&self, frame: &mut helio_core::ResourceRegistry<'a>) {
        // The source indirect records are refined by Hi-Z, then copied and
        // compacted into this pass's output buffer for downstream geometry.
        //
        // Plain (non-panicking) lookup: this is legitimately optional (a
        // graph that omits `IndirectDispatchPass`, e.g. a focused test
        // graph, has nothing to republish yet) -- the `else { return; }`
        // below already handles absence gracefully, but `frame.read()`
        // falls through to a debug-only panic on a missing key before ever
        // returning `None`, defeating that.
        let Some(_indirect_dispatch) = frame
            .get::<helio_pass_indirect_dispatch::IndirectDispatchFrameData<'a>>(
                helio_core::ResourceKey::new("indirect_dispatch"),
            )
        else {
            return;
        };
        let compacted_indices: &'a wgpu::Buffer =
            unsafe { std::mem::transmute(&self.compacted_indices_2_buf) };
        let indirect: &'a wgpu::Buffer =
            unsafe { std::mem::transmute(&self.compacted_indirect_buf) };
        // Same frame-scoped lifetime bridge as the buffers above.
        let segments: &'a SegmentTable = unsafe { std::mem::transmute(&self.segments) };
        frame.write(
            helio_core::ResourceKey::new("culled_batch"),
            crate::CulledBatchFrameData {
                indirect,
                compacted_indices,
                segments: helio_pass_gbuffer::DrawSegments {
                    indirect: &segments.indirect_buf,
                    counts: segments.counts_supported.then_some(&segments.counts_buf),
                    segments: segments.segments(),
                    table: &segments.table_buf,
                },
            },
            "OcclusionCull",
        );
        // Draws switch pipelines and offsets when the table changes, so
        // depth can change with it while nothing else does.
        helio_core::resource_keys::fold_depth_draw_signature(
            frame,
            segments.generation,
            "OcclusionCull",
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

    fn prepare(&mut self, ctx: &PrepareContext) -> HelioResult<()> {
        // `set_screen_size` was previously dead code -- nothing called it, so
        // `screen_width`/`screen_height` stayed frozen at this pass's
        // construction-time resolution forever, while the "hiz" texture it
        // samples (graph-pooled) DID get correctly reallocated on resize by
        // `HiZBuildPass::on_resize`/its own `ctx.resize` handling in
        // `prepare`. The resulting mismatch corrupts `pick_mip`'s mip-level
        // math and `screen_radius_px`'s UV footprint sizing against the
        // pyramid's real dimensions -- mirrors `HiZBuildPass::prepare`'s own
        // `ctx.resize` sync so both passes agree on the current resolution.
        if ctx.resize {
            self.set_screen_size(ctx.width.max(1), ctx.height.max(1));
        }

        let batch = ctx
            .registry
            .get::<helio_pass_gbuffer::ObjectBatchFrameData<'_>>(helio_core::ResourceKey::new(
                "object_batch",
            ));
        let draw_capacity = batch.map(|b| b.group_capacity).unwrap_or(0);
        let range_slots = batch.map(|b| b.range_slot_capacity).unwrap_or(0);
        if let Some(batch) = batch.as_ref() {
            self.segments.update(ctx.device, ctx.queue, batch);
        }
        for bucket in 0..3u32 {
            let params = RangeCompactParams {
                slots: range_slots,
                bucket,
                segment_count: self.segments.segments().len() as u32,
                _pad: 0,
            };
            ctx.write_buffer(
                &self.range_compact_params,
                bucket as u64 * 256,
                bytemuck::bytes_of(&params),
            );
        }
        // Sized for every group and instance the GPU can produce this frame:
        // the live counts only exist on the GPU.
        self.ensure_capacity(ctx.device, draw_capacity);

        // `baked_pvs` is optional: published by helio-bake's BakeInjectPass
        // only after a bake that included a PVS (`BakeConfig::with_pvs`).
        self.sync_pvs(ctx);
        let pvs = self.pvs_grid;
        let p = CullParams {
            screen_width: self.screen_width,
            screen_height: self.screen_height,
            draw_capacity,
            hiz_mip_count: mip_levels(self.screen_width, self.screen_height),
            pvs_available: pvs.is_some() as u32,
            pvs_grid: pvs.map_or([0; 3], |p| p.grid),
            pvs_min: pvs.map_or([0.0; 3], |p| p.min),
            pvs_cell_size: pvs.map_or(1.0, |p| p.cell_size),
            pvs_words_per_cell: pvs.map_or(0, |p| p.words_per_cell),
            _pad: [0; 3],
        };
        ctx.write_buffer(&self.cull_params_buf, 0, bytemuck::bytes_of(&p));
        Ok(())
    }

    fn execute(&mut self, ctx: &mut PassContext) -> HelioResult<()> {
        let Some(batch) = ctx
            .registry
            .get::<helio_pass_gbuffer::ObjectBatchFrameData<'_>>(helio_core::ResourceKey::new(
                "object_batch",
            ))
        else {
            return Ok(());
        };
        let Some(indirect_dispatch) = ctx
            .registry
            .get::<helio_pass_indirect_dispatch::IndirectDispatchFrameData<'_>>(
            helio_core::ResourceKey::new("indirect_dispatch"),
        ) else {
            return Ok(());
        };
        let Some(coord_data) = ctx
            .registry
            .get::<helio_pass_gbuffer::CoordinateSpacesFrameData<'_>>(
                helio_core::resource_keys::coordinate_spaces(),
            )
        else {
            return Ok(());
        };
        // This frame's segments start empty: counts at zero, and records past
        // each count with zero `instance_count` for non-count draws.
        if !self.segments.segments().is_empty() {
            let mut cmds = ctx.graphics_cmds();
            cmds.clear_buffer(&self.segments.counts_buf, 0, None);
            cmds.clear_buffer(&self.segments.indirect_buf, 0, None);
        }
        // Temporal Hi-Z: the first frame with real instances has no valid
        // pyramid yet (see `hiz_warmed_up`'s doc for why this is NOT the
        // same as `frame_num == 0`) — skip real occlusion testing, but
        // downstream draws always read `compacted_indices_2`, so pass the
        // frustum-culled list through unchanged instead of leaving it
        // stale/uninitialized.
        let inputs = (
            ctx.camera_generation,
            ctx.scene_buffers.content_signature(),
            self.segments.generation,
            self.screen_width,
            self.screen_height,
        );
        let history_matches = self.history_inputs.replace(inputs) == Some(inputs);
        if !self.hiz_warmed_up || !history_matches {
            // The live instance count is GPU-only: copy the whole list.
            let bytes = indirect_dispatch
                .compacted_indices
                .size()
                .min(self.compacted_indices_2_buf.size());
            ctx.graphics_cmds().copy_buffer_to_buffer(
                indirect_dispatch.compacted_indices,
                0,
                &self.compacted_indices_2_buf,
                0,
                bytes,
            );
            // Only declare Hi-Z warmed up once the readback confirms real
            // instances -- that's what guarantees GBuffer has drawn real
            // geometry into depth, which is the one thing the next frame's
            // Hi-Z pyramid actually needs to be valid. Until then, stay
            // un-warmed and retry the bypass next frame instead of moving on
            // to a real test with nothing real backing it.
            if batch.readback_instance_count > 0 {
                self.hiz_warmed_up = true;
            }
            self.record_range_compaction(ctx, &batch, indirect_dispatch.indirect);
            return Ok(());
        }

        // Lazy bind-group rebuild: rebuild whenever any buffer or the
        // HiZ texture view changes (e.g. scene grows, graph reallocates on resize).
        let hiz_view = ctx
            .registry
            .read_texture_view(helio_core::ResourceKey::new("hiz"), "OcclusionCull")
            .expect("OcclusionCull: 'hiz' view not routed by graph — is HiZBuildPass declared?");

        let key = (
            [
                ctx.camera.clone(),
                batch.instances.clone(),
                batch.draw_calls.clone(),
                indirect_dispatch.indirect.clone(),
                self.pvs_buf.clone(),
                self.cull_stats_buf.clone(),
                indirect_dispatch.compacted_indices.clone(),
                self.compacted_indices_2_buf.clone(),
                coord_data.coordinate_spaces.clone(),
            ],
            hiz_view.clone(),
        );
        if self.bind_group_key.as_ref() != Some(&key) {
            self.bind_group = Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("OcclusionCull BG"),
                layout: &self.bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: ctx.camera.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: self.cull_params_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: batch.instances.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: batch.draw_calls.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(hiz_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::Sampler(&self.hiz_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: indirect_dispatch.indirect.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: self.pvs_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 9,
                        resource: self.cull_stats_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 10,
                        resource: indirect_dispatch.compacted_indices.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 11,
                        resource: self.compacted_indices_2_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 12,
                        resource: coord_data.coordinate_spaces.as_entire_binding(),
                    },
                ],
            }));
            self.bind_group_key = Some(key);
        }

        // One workgroup per live draw-call group, counted on the GPU this
        // frame — its 64 lanes cooperatively Hi-Z-test and compact that
        // group's frustum survivors.
        {
            let mut cmds = ctx.graphics_cmds();
            let mut pass = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("OcclusionCull"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, self.bind_group.as_ref().unwrap(), &[]);
            pass.dispatch_workgroups_indirect(
                batch.group_dispatch.buffer,
                batch.group_dispatch.offset,
            );
        }
        self.record_range_compaction(ctx, &batch, indirect_dispatch.indirect);
        Ok(())
    }
}

fn mip_levels(w: u32, h: u32) -> u32 {
    let max_dim = w.max(h);
    (u32::BITS - max_dim.leading_zeros()).max(1)
}

#[cfg(test)]
mod segment_tests {
    use super::{segment_capacity, SegmentKeys, ShadingBucket, SEGMENT_EVICT_FRAMES};

    fn observe(keys: &mut SegmentKeys, opaque: &[(u32, u64, u32, u32)]) -> bool {
        keys.observe([
            (ShadingBucket::Opaque, opaque),
            (ShadingBucket::Transparent, &[]),
            (ShadingBucket::Forward, &[]),
        ])
    }

    #[test]
    fn keys_sharing_ranges_get_one_segment_sized_for_their_sum() {
        let mut keys = SegmentKeys::default();
        // Class 7 spans two ranges; class 3 one.
        assert!(observe(
            &mut keys,
            &[(7, 1, 0, 40), (3, 0, 40, 5), (7, 1, 45, 30)]
        ));
        let segments = &keys.segments;
        assert_eq!(segments.len(), 2);
        assert_eq!(
            (segments[0].material_class, segments[0].capacity),
            (7, segment_capacity(70))
        );
        assert_eq!(segments[0].first, 0);
        assert_eq!(segments[1].material_class, 3);
        assert_eq!(segments[1].first, segments[0].capacity);
    }

    #[test]
    fn an_unchanged_frame_moves_nothing_even_if_the_layout_shifts() {
        let mut keys = SegmentKeys::default();
        observe(&mut keys, &[(7, 0, 0, 10), (3, 0, 10, 10)]);
        let before = keys.segments.clone();
        // Same keys, different starts and order, counts within capacity.
        assert!(!observe(&mut keys, &[(3, 0, 0, 20), (7, 0, 20, 1)]));
        assert_eq!(keys.segments, before);
    }

    #[test]
    fn a_key_that_outgrows_its_segment_grows_and_later_ones_move() {
        let mut keys = SegmentKeys::default();
        observe(&mut keys, &[(7, 0, 0, 10), (3, 0, 10, 10)]);
        let capacity = keys.segments[0].capacity;
        assert!(observe(
            &mut keys,
            &[(7, 0, 0, capacity + 1), (3, 0, 0, 10)]
        ));
        assert_eq!(keys.segments[0].capacity, segment_capacity(capacity + 1));
        assert_eq!(keys.segments[1].first, keys.segments[0].capacity);
    }

    #[test]
    fn unseen_keys_expire_and_buckets_are_separate_keys() {
        let mut keys = SegmentKeys::default();
        keys.observe([
            (ShadingBucket::Opaque, &[(1, 0, 0, 1)]),
            (ShadingBucket::Transparent, &[(1, 0, 1, 1)]),
            (ShadingBucket::Forward, &[]),
        ]);
        assert_eq!(
            keys.segments.len(),
            2,
            "same material in two buckets: two segments"
        );
        for _ in 0..SEGMENT_EVICT_FRAMES {
            assert!(!observe(&mut keys, &[(1, 0, 0, 1)]));
        }
        assert!(
            observe(&mut keys, &[(1, 0, 0, 1)]),
            "the transparent key expires"
        );
        assert_eq!(keys.segments.len(), 1);
        assert_eq!(keys.segments[0].bucket, ShadingBucket::Opaque);
        assert_eq!(keys.segments[0].first, 0);
    }
}
