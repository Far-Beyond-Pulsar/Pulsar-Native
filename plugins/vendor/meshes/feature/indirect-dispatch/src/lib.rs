//! GPU frustum culling and indirect draw command generation.
//!
//! This pass runs a compute shader that:
//! 1. Tests each instance's bounding sphere against the 6 frustum planes
//! 2. Compacts surviving instances into `compacted_indices`, per draw-call group
//! 3. Writes each group's real visible instance count into its DrawIndexedIndirect
//! 4. Is O(1) CPU cost — single compute dispatch regardless of scene size,
//!    sized on the GPU (one workgroup per live group, from
//!    `ObjectBatchFrameData::group_dispatch`), so a group is culled the
//!    frame it appears, not once the CPU reads its count back
//!
//! One workgroup (64 lanes) handles one draw-call group, cooperatively testing
//! every instance in that group and packing survivors via workgroup-shared
//! atomics. Consumers drawing through the shared `instances`/`indirect` pair
//! (e.g. `GBufferPass`) must index `instances` through `compacted_indices`
//! rather than directly by `instance_index` — see `helio-pass-gbuffer`.

use bytemuck::{Pod, Zeroable};
use helio_core::{PassContext, PrepareContext, RenderPass, Result as HelioResult};

pub mod gpu_types;
pub use gpu_types::*;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CullUniforms {
    frustum_planes: [[f32; 4]; 6], // 6 planes × 4 floats = 96 bytes
    /// Rows `draw_calls` holds; the dispatch itself covers the live groups.
    draw_capacity: u32,
    _pad: [u32; 3],
}

/// Below this many entries, `indirect_buf`/`compacted_indices_buf` still
/// allocate at this floor -- avoids reallocating on every single object
/// insert/remove around a tiny scene, matching `ObjectBatchPass`'s own
/// `MIN_SCRATCH_CAPACITY` idiom.
const MIN_CAPACITY: u32 = 256;

pub struct IndirectDispatchPass {
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    uniform_buf: wgpu::Buffer,
    cull_stats_buf: wgpu::Buffer,
    /// This pass's own output -- no longer a central `GpuScene` field (see
    /// `IndirectDispatchFrameData`'s doc, now in this crate): frustum-culled
    /// indirect draw args, one `DrawIndexedIndirect` (20 bytes) per
    /// draw-call group.
    indirect_buf: wgpu::Buffer,
    /// Frustum-culling survivors, one `u32` per live instance (worst case).
    compacted_indices_buf: wgpu::Buffer,
    /// Rows `indirect_buf`/`compacted_indices_buf` are currently sized for.
    draw_capacity: u32,
    instance_capacity: u32,
    /// Lazy bind group — rebuilt whenever the underlying buffer pointers change
    /// (GrowableBuffers reallocate on resize, invalidating old bind groups).
    bind_group: Option<wgpu::BindGroup>,
    /// Tuple of raw buffer pointers used as a staleness key.
    bind_group_key: Option<[wgpu::Buffer; 8]>,
}

impl IndirectDispatchPass {
    pub fn new(device: &wgpu::Device, cull_stats_buf: wgpu::Buffer) -> Self {
        let shader = helio_core::shader::module(
            device,
            "IndirectDispatch Shader",
            helio_core::include_wgsl!("../shaders/indirect_dispatch.wgsl"),
        );

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("CullUniforms"),
            size: std::mem::size_of::<CullUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("IndirectDispatch BGL"),
            entries: &[
                // binding 0: camera storage
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
                // binding 1: cull uniforms
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
                // binding 2: instances (read)
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
                // binding 3: draw calls (read)
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
                // binding 4: AABBs (read)
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // binding 5: indirect output (read_write)
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // binding 6: culling stats (read_write, atomic counters)
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
                // binding 7: compacted instance indices (read_write, GPU-written)
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // binding 8: coordinate_spaces (read) — see indirect_dispatch.wgsl
                wgpu::BindGroupLayoutEntry {
                    binding: 8,
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

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("IndirectDispatch PL"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = helio_core::shader::create_compute_pipeline(
            &device,
            wgpu::ComputePipelineDescriptor {
                label: Some("IndirectDispatch Pipeline"),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                cache: None,
            },
        );

        let indirect_buf = create_indirect_buf(device, MIN_CAPACITY);
        let compacted_indices_buf = create_compacted_indices_buf(device, MIN_CAPACITY);

        Self {
            pipeline,
            bind_group_layout,
            uniform_buf,
            cull_stats_buf: cull_stats_buf.clone(),
            indirect_buf,
            compacted_indices_buf,
            draw_capacity: MIN_CAPACITY,
            instance_capacity: MIN_CAPACITY,
            bind_group: None,
            bind_group_key: None,
        }
    }

    /// Grows `indirect_buf`/`compacted_indices_buf` to at least
    /// `draw_count`/`instance_count` rows (next-power-of-two, floor
    /// `MIN_CAPACITY`) if either has grown since the last call. Returns
    /// `true` if anything reallocated (the caller must then rebuild the
    /// bind group).
    fn ensure_capacity(
        &mut self,
        device: &wgpu::Device,
        draw_count: u32,
        instance_count: u32,
    ) -> bool {
        let mut grew = false;
        if draw_count > self.draw_capacity {
            self.draw_capacity = draw_count.next_power_of_two().max(MIN_CAPACITY);
            self.indirect_buf = create_indirect_buf(device, self.draw_capacity);
            grew = true;
        }
        if instance_count > self.instance_capacity {
            self.instance_capacity = instance_count.next_power_of_two().max(MIN_CAPACITY);
            self.compacted_indices_buf =
                create_compacted_indices_buf(device, self.instance_capacity);
            grew = true;
        }
        grew
    }
}

fn create_indirect_buf(device: &wgpu::Device, capacity: u32) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("IndirectDispatch Indirect"),
        size: (capacity as u64 * 20).max(4),
        // COPY_SRC: `OcclusionCullPass::record_range_compaction` copies this
        // list into its own `compacted_indirect_buf` before compacting ranges.
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::INDIRECT
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
}

fn create_compacted_indices_buf(device: &wgpu::Device, capacity: u32) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("IndirectDispatch CompactedIndices"),
        size: (capacity as u64 * 4).max(4),
        // COPY_SRC: `OcclusionCullPass`'s "no Hi-Z pyramid yet" bypass path
        // copies this buffer straight into its own `compacted_indices_2_buf`
        // (see that pass's `execute()`) instead of running the real Hi-Z
        // test. That path was previously unreachable in practice (gated on
        // `frame_num == 0`, which always saw `draw_count == 0` due to
        // `ObjectBatchPass`'s readback lag -- see `OcclusionCullPass::
        // hiz_warmed_up`'s doc), so this missing flag never surfaced as a
        // validation error until that gate was fixed to actually run the
        // bypass on the first frame with real instances.
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
}

impl RenderPass for IndirectDispatchPass {
    fn name(&self) -> &'static str {
        "IndirectDispatch"
    }

    fn render_pass_descriptor<'a>(
        &'a self,
        _target: &'a wgpu::TextureView,
        _depth: &'a wgpu::TextureView,
        _resources: &'a helio_core::ResourceRegistry<'a>,
    ) -> Option<wgpu::RenderPassDescriptor<'a>> {
        None
    }

    fn declare_resources(&self, builder: &mut helio_core::graph::ResourceBuilder) {
        builder.read("object_batch");
        builder.write_buffer("indirect_dispatch");
    }

    fn publish<'a>(&self, frame: &mut helio_core::ResourceRegistry<'a>) {
        // The pass owns these buffers for the renderer lifetime; ResourceRegistry
        // stores borrowed views for the current frame.
        let indirect: &'a wgpu::Buffer = unsafe { std::mem::transmute(&self.indirect_buf) };
        let compacted_indices: &'a wgpu::Buffer =
            unsafe { std::mem::transmute(&self.compacted_indices_buf) };
        frame.write(
            helio_core::ResourceKey::new("indirect_dispatch"),
            crate::IndirectDispatchFrameData {
                indirect,
                compacted_indices,
            },
            "IndirectDispatch",
        );
    }

    fn prepare(&mut self, ctx: &PrepareContext) -> HelioResult<()> {
        let Some(batch) = ctx
            .registry
            .get::<helio_pass_gbuffer::ObjectBatchFrameData<'_>>(helio_core::ResourceKey::new(
                "object_batch",
            ))
        else {
            return Ok(());
        };
        // Sized for every group the GPU can produce this frame: the live
        // counts only exist on the GPU.
        self.ensure_capacity(ctx.device, batch.group_capacity, batch.group_capacity);

        let planes = helio_core::depth::frustum_planes(ctx.camera_data.view_proj);
        let uniforms = CullUniforms {
            frustum_planes: planes,
            draw_capacity: batch.group_capacity,
            _pad: [0; 3],
        };
        ctx.queue
            .write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&uniforms));
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
        let Some(coord_data) = ctx
            .registry
            .get::<helio_pass_gbuffer::CoordinateSpacesFrameData<'_>>(
                helio_core::resource_keys::coordinate_spaces(),
            )
        else {
            return Ok(());
        };

        // Rebuild bind group if any source buffer has reallocated (handle changed).
        let key = [
            ctx.camera.clone(),
            batch.instances.clone(),
            batch.draw_calls.clone(),
            batch.aabbs.clone(),
            self.indirect_buf.clone(),
            self.cull_stats_buf.clone(),
            self.compacted_indices_buf.clone(),
            coord_data.coordinate_spaces.clone(),
        ];
        if self.bind_group_key.as_ref() != Some(&key) {
            self.bind_group = Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("IndirectDispatch BG"),
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: ctx.camera.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: self.uniform_buf.as_entire_binding(),
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
                        resource: batch.aabbs.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: self.indirect_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: self.cull_stats_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: self.compacted_indices_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 8,
                        resource: coord_data.coordinate_spaces.as_entire_binding(),
                    },
                ],
            }));
            self.bind_group_key = Some(key);
        }

        // O(1) CPU: one dispatch, GPU culls all draw calls in parallel. One
        // workgroup per live draw-call group, counted on the GPU this frame —
        // its 64 lanes cooperatively compact that group's surviving instances
        // (see indirect_dispatch.wgsl).
        let mut cmds = ctx.graphics_cmds();
        let mut pass = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("IndirectDispatch"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, self.bind_group.as_ref().unwrap(), &[]);
        pass.dispatch_workgroups_indirect(batch.group_dispatch.buffer, batch.group_dispatch.offset);
        Ok(())
    }
}
