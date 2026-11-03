//! The portal draw segments: the portal pass's surviving indirect records,
//! packed per material key.
//!
//! The G-buffer draws one segment per `(material_class, graph_hash)` with that
//! key's pipeline (`helio_pass_gbuffer::DrawSegments`). A portal duplicate must
//! be shaded by its own material the same way, so after the cull this runs the
//! occlusion cull's range compaction over `portal_indirect`, into buffers of the
//! portal pass's own laid out by the G-buffer's segment table (the same key
//! gets the same region offset and capacity). `PortalInstancePass` then draws
//! each segment with its pipeline.

use std::sync::{Arc, Mutex};

use bytemuck::{Pod, Zeroable};
use helio_core::PassContext;

/// What the portal draw reads: records and counts per segment, laid out like
/// the G-buffer's own (`DrawSegments::indirect` / `counts`).
pub struct PortalSegmentBuffers {
    pub indirect: wgpu::Buffer,
    pub counts: wgpu::Buffer,
    /// Moves whenever a buffer is replaced.
    pub generation: u64,
}

/// Shared between `PortalCullPass` (which fills it) and `PortalInstancePass`.
pub type SharedPortalSegments = Arc<Mutex<PortalSegmentBuffers>>;

impl PortalSegmentBuffers {
    pub fn shared(device: &wgpu::Device) -> SharedPortalSegments {
        Arc::new(Mutex::new(Self {
            indirect: segment_buffer(device, "PortalCull/SegmentIndirect", 20, true),
            counts: segment_buffer(device, "PortalCull/SegmentCounts", 4, true),
            generation: 0,
        }))
    }
}

fn segment_buffer(device: &wgpu::Device, label: &str, size: u64, indirect: bool) -> wgpu::Buffer {
    let mut usage =
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC;
    if indirect {
        usage |= wgpu::BufferUsages::INDIRECT;
    }
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage,
        mapped_at_creation: false,
    })
}

/// `compact_portal.wgsl`'s `RangeCapacity`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    slots: u32,
    bucket: u32,
    segment_count: u32,
    _pad: u32,
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

/// The compaction pipeline and its scratch.
pub(crate) struct PortalCompaction {
    pipeline: wgpu::ComputePipeline,
    bgl: wgpu::BindGroupLayout,
    params: wgpu::Buffer,
    /// Compaction's working copy of `portal_indirect`.
    scratch_indirect: wgpu::Buffer,
    /// Compaction's own per-range counts (the object batch's are not ours to write).
    scratch_counts: wgpu::Buffer,
    bind_group: Option<wgpu::BindGroup>,
    key: Option<Vec<wgpu::Buffer>>,
    pub segments: SharedPortalSegments,
}

impl PortalCompaction {
    pub(crate) fn new(device: &wgpu::Device, segments: SharedPortalSegments) -> Self {
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("PortalCull Compaction BGL"),
            entries: &[
                storage_entry(0, true),
                storage_entry(1, false),
                storage_entry(2, true),
                storage_entry(3, true),
                storage_entry(4, true),
                storage_entry(5, true),
                storage_entry(6, false),
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                storage_entry(8, true),
                storage_entry(9, false),
                storage_entry(10, false),
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("PortalCull Compaction Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/compact_portal.wgsl").into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("PortalCull Compaction PL"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipeline = helio_core::shader::create_compute_pipeline(
            &device,
            wgpu::ComputePipelineDescriptor {
                label: Some("PortalCull Compaction Pipeline"),
                layout: Some(&layout),
                module: &shader,
                entry_point: Some("compact_ranges"),
                compilation_options: Default::default(),
                cache: None,
            },
        );
        Self {
            pipeline,
            bgl,
            params: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("PortalCull Compaction Params"),
                size: std::mem::size_of::<Params>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            scratch_indirect: segment_buffer(device, "PortalCull/CompactionIndirect", 20, true),
            scratch_counts: segment_buffer(device, "PortalCull/CompactionCounts", 16, false),
            bind_group: None,
            key: None,
            segments,
        }
    }

    /// Follow the G-buffer's segment table: buffers as large as its, so a key's
    /// region has the same room here.
    fn size_to(
        &mut self,
        device: &wgpu::Device,
        culled: &helio_pass_gbuffer::DrawSegments<'_>,
        groups: u32,
        range_slots: u32,
    ) {
        let mut shared = self.segments.lock().unwrap_or_else(|p| p.into_inner());
        if shared.indirect.size() < culled.indirect.size() {
            shared.indirect = segment_buffer(
                device,
                "PortalCull/SegmentIndirect",
                culled.indirect.size(),
                true,
            );
            shared.generation += 1;
        }
        let counts_wanted = culled.counts.map_or(4, |c| c.size());
        if shared.counts.size() < counts_wanted {
            shared.counts = segment_buffer(device, "PortalCull/SegmentCounts", counts_wanted, true);
            shared.generation += 1;
        }
        let indirect_wanted = u64::from(groups.max(1)) * 20;
        if self.scratch_indirect.size() < indirect_wanted {
            self.scratch_indirect = segment_buffer(
                device,
                "PortalCull/CompactionIndirect",
                indirect_wanted.next_power_of_two(),
                true,
            );
        }
        let counts_scratch = (4 + 3 * u64::from(range_slots)) * 4;
        if self.scratch_counts.size() < counts_scratch {
            self.scratch_counts = segment_buffer(
                device,
                "PortalCull/CompactionCounts",
                counts_scratch.next_power_of_two(),
                false,
            );
        }
    }

    /// Pack `portal_indirect`'s surviving records into the portal segments.
    pub(crate) fn record(
        &mut self,
        ctx: &mut PassContext,
        batch: &helio_pass_gbuffer::ObjectBatchFrameData<'_>,
        culled: &helio_pass_gbuffer::DrawSegments<'_>,
        portal_indirect: &wgpu::Buffer,
    ) {
        let slots = batch.range_slot_capacity;
        if slots == 0 || culled.is_empty() {
            return;
        }
        self.size_to(ctx.device, culled, batch.group_capacity, slots);
        ctx.write_buffer(
            &self.params,
            0,
            bytemuck::bytes_of(&Params {
                slots,
                bucket: helio_pass_gbuffer::ShadingBucket::Opaque as u32,
                segment_count: culled.segments.len() as u32,
                _pad: 0,
            }),
        );
        let shared = self.segments.lock().unwrap_or_else(|p| p.into_inner());
        let mut cmds = ctx.graphics_cmds();
        // This frame's segments start empty.
        cmds.clear_buffer(&shared.counts, 0, None);
        cmds.clear_buffer(&shared.indirect, 0, None);
        let bytes = portal_indirect.size().min(self.scratch_indirect.size());
        cmds.copy_buffer_to_buffer(portal_indirect, 0, &self.scratch_indirect, 0, bytes);

        let key = vec![
            portal_indirect.clone(),
            self.scratch_indirect.clone(),
            batch.range_counts_gpu.clone(),
            batch.opaque_ranges_gpu.clone(),
            batch.transparent_ranges_gpu.clone(),
            batch.forward_ranges_gpu.clone(),
            self.scratch_counts.clone(),
            culled.table.clone(),
            shared.indirect.clone(),
            shared.counts.clone(),
        ];
        if self.key.as_ref() != Some(&key) {
            self.bind_group = Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("PortalCull Compaction BG"),
                layout: &self.bgl,
                entries: &[
                    entry(0, portal_indirect),
                    entry(1, &self.scratch_indirect),
                    entry(2, batch.range_counts_gpu),
                    entry(3, batch.opaque_ranges_gpu),
                    entry(4, batch.transparent_ranges_gpu),
                    entry(5, batch.forward_ranges_gpu),
                    entry(6, &self.scratch_counts),
                    entry(7, &self.params),
                    entry(8, culled.table),
                    entry(9, &shared.indirect),
                    entry(10, &shared.counts),
                ],
            }));
            self.key = Some(key);
        }
        let mut pass = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("PortalCull Compaction"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, self.bind_group.as_ref().unwrap(), &[]);
        // The opaque bucket's dispatch arguments (see the occlusion cull).
        pass.dispatch_workgroups_indirect(batch.range_counts_gpu, 16);
    }
}

fn entry(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: buffer.as_entire_binding(),
    }
}
