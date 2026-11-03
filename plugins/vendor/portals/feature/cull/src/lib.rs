//! Per-portal-*chain* GPU frustum culling.
//!
//! For each active portal chain (a runtime-sized sequence of portal indices),
//! tests
//! every draw-call group's instances — mapped through that chain's *composed*
//! transform — against the main camera frustum, and compacts survivors into
//! two shared output buffers (`portal_indirect_buf` / `portal_compacted_indices_buf`,
//! plus a parallel `portal_compacted_chains_buf` recording which chain each
//! survivor was selected under). `helio-pass-portal-instances` reads all
//! three to draw the duplicated, chain-clipped content — one indirect draw
//! call per draw group, same shape as the ordinary non-portal G-buffer pass,
//! not one per chain.
//!
//! # Two passes: `select` then `finalize`
//!
//! Reserved capacity is sized **per draw group, not per chain** — chains can
//! number in the hundreds (`portal_count^depth`) while draw groups stay in
//! the dozens to low hundreds for realistic scenes, and only a handful of
//! chains ever have any given group's content in frustum simultaneously,
//! so reserving per-chain capacity would multiply the wrong axis. Because
//! several chains' `select` workgroups append into the *same* draw group's
//! region concurrently, the final survivor count isn't known until they've
//! all finished — `finalize` is a tiny second dispatch (one thread per draw
//! group) that turns the settled count into a `DrawIndexedIndirect`.
//!
//! # Buffers produced
//!
//! | Buffer                          | Format                                                      |
//! |----------------------------------|--------------------------------------------------------------|
//! | `portal_indirect_buf`            | `PORTAL_DRAW_CAPACITY × 20` bytes                             |
//! | `portal_compacted_indices_buf`   | `PORTAL_DRAW_CAPACITY × PORTAL_GROUP_CHAIN_CAPACITY × 4` bytes |
//! | `portal_compacted_chains_buf`    | same shape as `portal_compacted_indices_buf`                  |
//!
//! All fixed-size, allocated once — not resized to track scene growth —
//! mirroring `helio-pass-shadow-cull`'s own atlas buffers. A scene exceeding
//! a cap silently drops the excess (see `portal_cull.wgsl`'s bounds checks)
//! rather than corrupting adjacent memory.
//!
//! # Integration
//!
//! ```ignore
//! let cull_pass = PortalCullPass::new(device);
//! let indirect_buf = Arc::clone(&cull_pass.portal_indirect_buf);
//! let compacted_buf = Arc::clone(&cull_pass.portal_compacted_indices_buf);
//! let compacted_chains_buf = Arc::clone(&cull_pass.portal_compacted_chains_buf);
//! graph.add_pass(Box::new(cull_pass));
//!
//! graph.add_pass(Box::new(
//!     PortalInstancePass::new(device, indirect_buf, compacted_buf, compacted_chains_buf),
//! ));
//! ```

pub mod feature;
/// The portal feature's host settings.
pub mod settings;
pub use feature::{portals_enabled, PortalCullFeature, PortalSettings};

use std::sync::Arc;

mod segments;
use segments::PortalCompaction;
pub use segments::{PortalSegmentBuffers, SharedPortalSegments};

mod portal_math;
pub use portal_math::{
    crossing_detected, plane_signed_distance, portal_pose_facing, PortalPair, PortalPose,
};
mod resolver;
pub use resolver::{
    PortalOccurrence, PortalProjection, PortalRecord, ResolutionError, ResolvedPortalChain,
    SubLevelActorRecord, SubLevelContents, SubLevelResolver, SubLevelRuntimeContext,
};
mod projection;
pub use projection::{
    PortalProjectionBridge, PortalProjectionConfig, PortalProjectionFrame, PortalProjectionKey,
    ProjectionError, RuntimePortalKey, IDENTITY_COORDINATE_SPACE_SLOT,
};
mod contract;
/// The portal rows a host's projection uploads (a scene derivation).
pub mod scene_rows;
pub use components::PortalChain;
pub use components::PortalProjectionCountsRow;
pub use components::PortalRow;
pub use components::PortalViewRow;
pub use components::PORTALS;
pub use components::PORTAL_PROJECTION_COUNTS;
pub use components::PORTAL_VIEWS;
pub use contract::{
    portal_counts, GpuPortalView, PortalCounts, MAX_PORTAL_CHAINS, PORTAL_CHAIN_HANDLE_BUFFER,
    PORTAL_CHAIN_PORTAL_POOL_BUFFER, PORTAL_COUNTS,
};
pub use helio_pass_gbuffer::CoordinateSpacesFrameData;

use bytemuck::{Pod, Zeroable};
use helio_core::{PassContext, PrepareContext, RenderPass, Result as HelioResult};
use pulsar_scenedb::gpu::BufferKey;
pub mod components;

/// Fixed cap on draw-call groups considered. Realistic scenes have dozens to
/// low hundreds of distinct mesh+material combinations; this is generous
/// headroom, not a tight budget.
pub const PORTAL_DRAW_CAPACITY: u32 = 512;

/// How many (instance, chain) survivor slots are reserved **per draw
/// group** — not per chain, see the module doc for why that's the right
/// axis. `PORTAL_DRAW_CAPACITY × PORTAL_GROUP_CHAIN_CAPACITY` is the actual
/// total allocation, so this is deliberately modest.
pub const PORTAL_GROUP_CHAIN_CAPACITY: u32 = 1024;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CullUniforms {
    frustum_planes: [[f32; 4]; 6],
    /// Draw groups dispatched: every group the batch can produce, up to
    /// `PORTAL_DRAW_CAPACITY`. The live count is read on the GPU.
    draw_capacity: u32,
    chain_count: u32,
    group_capacity: u32,
    _pad: u32,
}

pub struct PortalCullPass {
    select_pipeline: wgpu::ComputePipeline,
    finalize_pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    uniform_buf: wgpu::Buffer,

    /// One `DrawIndexedIndirect` per draw group (not per chain).
    pub portal_indirect_buf: Arc<wgpu::Buffer>,

    /// Shared compacted original-instance-slot buffer. Draw group `g`'s
    /// region is `[g * PORTAL_GROUP_CHAIN_CAPACITY, (g+1) * PORTAL_GROUP_CHAIN_CAPACITY)`.
    pub portal_compacted_indices_buf: Arc<wgpu::Buffer>,

    /// Parallel to `portal_compacted_indices_buf` — which chain each
    /// compacted entry was selected under.
    pub portal_compacted_chains_buf: Arc<wgpu::Buffer>,

    /// Per-draw-group selected-instance totals (across all chains
    /// combined), zeroed every frame by the CPU: the atomic counter
    /// `select` claims write slots from.
    portal_stats_buf: wgpu::Buffer,

    /// Packs the surviving records per material key for the portal draw.
    compaction: PortalCompaction,
    /// The packed records `PortalInstancePass` draws (see `segments`).
    pub segments: SharedPortalSegments,

    bind_group: Option<wgpu::BindGroup>,
    /// (camera, instances, draw_calls, coordinate_spaces, portal_views,
    /// chain_handles, chain_portals, compacted_chains, batch GPU counts)
    bind_group_key: Option<[wgpu::Buffer; 9]>,

    /// Draw groups dispatched this frame (see `CullUniforms::draw_capacity`).
    draw_capacity: u32,
    chain_count: u32,
    /// Live chains set by hand ([`Self::set_active_chain_count`]); `None`
    /// reads them from the frame's [`PORTAL_COUNTS`].
    active_chain_count: Option<u32>,
    /// The chains the frame's [`PORTAL_COUNTS`] published.
    published_chain_count: u32,
}

impl PortalCullPass {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = helio_core::shader::module(
            device,
            "PortalCull Shader",
            helio_core::include_wgsl!("../shaders/portal_cull.wgsl"),
        );

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("PortalCull Uniforms"),
            size: std::mem::size_of::<CullUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let portal_indirect_buf = Arc::new(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("PortalCull/PortalIndirect"),
            size: (PORTAL_DRAW_CAPACITY as u64) * 20,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::INDIRECT
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        }));
        let compacted_len = (PORTAL_DRAW_CAPACITY as u64) * (PORTAL_GROUP_CHAIN_CAPACITY as u64);
        let portal_compacted_indices_buf =
            Arc::new(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("PortalCull/CompactedIndices"),
                size: compacted_len * 4,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }));
        let portal_compacted_chains_buf = Arc::new(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("PortalCull/CompactedChains"),
            size: compacted_len * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        }));
        let portal_stats_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("PortalCull/Stats"),
            size: (PORTAL_DRAW_CAPACITY as u64) * 4,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let segments = PortalSegmentBuffers::shared(device);

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("PortalCull BGL"),
            entries: &[
                storage_entry(0, true), // camera
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
                storage_entry(2, true),   // instances
                storage_entry(3, true),   // draw_calls
                storage_entry(4, true),   // coordinate_spaces
                storage_entry(5, true),   // portal_views
                storage_entry(6, false),  // portal_indirect
                storage_entry(7, false),  // portal_compacted_indices
                storage_entry(8, false),  // portal_stats
                storage_entry(9, true),   // portal chain VarLenHandle table
                storage_entry(10, false), // portal_compacted_chains
                storage_entry(11, true),  // portal chain u32 payload pool
                storage_entry(12, true),  // Object Batch's GPU counts
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("PortalCull PL"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let select_pipeline = helio_core::shader::create_compute_pipeline(
            &device,
            wgpu::ComputePipelineDescriptor {
                label: Some("PortalCull Select Pipeline"),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some("select"),
                compilation_options: Default::default(),
                cache: None,
            },
        );
        let finalize_pipeline = helio_core::shader::create_compute_pipeline(
            &device,
            wgpu::ComputePipelineDescriptor {
                label: Some("PortalCull Finalize Pipeline"),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some("finalize"),
                compilation_options: Default::default(),
                cache: None,
            },
        );

        Self {
            select_pipeline,
            finalize_pipeline,
            bind_group_layout,
            uniform_buf,
            portal_indirect_buf,
            portal_compacted_indices_buf,
            portal_compacted_chains_buf,
            portal_stats_buf,
            compaction: PortalCompaction::new(device, Arc::clone(&segments)),
            segments,
            bind_group: None,
            bind_group_key: None,
            draw_capacity: 0,
            chain_count: 0,
            active_chain_count: None,
            published_chain_count: 0,
        }
    }

    /// Set the live chain row count by hand, in place of the frame's
    /// [`PORTAL_COUNTS`] (a host without a payload, tests). The SceneDB
    /// buffer may be larger than the current dense frame.
    pub fn set_active_chain_count(&mut self, count: u32) {
        self.active_chain_count = Some(count);
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

impl RenderPass for PortalCullPass {
    fn scene_buffers(&self) -> &'static [&'static str] {
        &["portal_views"]
    }

    fn name(&self) -> &'static str {
        "PortalCull"
    }

    /// A member while the host's portal projection has live chains.
    fn is_member(&mut self, scene: &mut helio_core::SceneContent<'_>) -> bool {
        self.published_chain_count = scene
            .scene_buffers()
            .map_or(0, |buffers| portal_counts(buffers).chains);
        self.active_chain_count.unwrap_or(self.published_chain_count) > 0
    }

    fn declare_resources(&self, builder: &mut helio_core::graph::ResourceBuilder) {
        builder.read("object_batch");
        // The G-buffer's draw segments (OcclusionCull publishes them), whose layout the
        // portal draw segments follow.
        builder.read("culled_batch");
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
        // Every group the batch can produce this frame, so a group spawned
        // this frame is culled this frame; the live count is read on the GPU.
        self.draw_capacity = ctx
            .registry
            .get::<helio_pass_gbuffer::ObjectBatchFrameData<'_>>(helio_core::ResourceKey::new(
                "object_batch",
            ))
            .map(|b| b.group_capacity.min(PORTAL_DRAW_CAPACITY))
            .unwrap_or(0);
        // A growable SceneDB buffer reports reserved capacity, not the number
        // of live rows. Never turn that capacity into dispatch work: doing so
        // makes a zero-portal scene run one cull lane per draw group for every
        // reserved chain slot. The host's projection publishes the live count
        // (`PORTAL_COUNTS`).
        self.chain_count = self
            .active_chain_count
            .unwrap_or_else(|| portal_counts(ctx.scene_buffers).chains);
        let planes = helio_core::depth::frustum_planes(ctx.camera_data.view_proj);

        let uniforms = CullUniforms {
            frustum_planes: planes,
            draw_capacity: self.draw_capacity,
            chain_count: self.chain_count,
            group_capacity: PORTAL_GROUP_CHAIN_CAPACITY,
            _pad: 0,
        };
        ctx.queue
            .write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&uniforms));
        // Zero the per-group counters before this frame's dispatch — `select`
        // uses these as its live atomic claim counters.
        let zeros = vec![0u32; PORTAL_DRAW_CAPACITY as usize];
        ctx.queue
            .write_buffer(&self.portal_stats_buf, 0, bytemuck::cast_slice(&zeros));
        Ok(())
    }

    fn execute(&mut self, ctx: &mut PassContext) -> HelioResult<()> {
        if ctx.frame_num < 3 || ctx.frame_num % 120 == 0 {
            log::trace!(
                "[PortalCull] frame={} draw_capacity={} chain_count={}",
                ctx.frame_num,
                self.draw_capacity,
                self.chain_count,
            );
        }
        if self.draw_capacity == 0 || self.chain_count == 0 {
            return Ok(());
        }
        let Some(batch): Option<helio_pass_gbuffer::ObjectBatchFrameData<'_>> = ctx
            .registry
            .get(helio_core::ResourceKey::new("object_batch"))
        else {
            return Ok(());
        };
        let Some(coord_data): Option<helio_pass_gbuffer::CoordinateSpacesFrameData<'_>> = ctx
            .registry
            .get(helio_core::resource_keys::coordinate_spaces())
        else {
            return Ok(());
        };
        let Some(portal_views) = ctx.scene_buffers.get(BufferKey::of("portal_views")) else {
            return Ok(());
        };
        let Some(portal_chain_handles) = ctx
            .scene_buffers
            .get(BufferKey::of(PORTAL_CHAIN_HANDLE_BUFFER))
        else {
            return Ok(());
        };
        let Some(portal_chain_portals) = ctx
            .scene_buffers
            .get(BufferKey::of(PORTAL_CHAIN_PORTAL_POOL_BUFFER))
        else {
            return Ok(());
        };

        let key = [
            ctx.camera.clone(),
            batch.instances.clone(),
            batch.draw_calls.clone(),
            coord_data.coordinate_spaces.clone(),
            portal_views.buffer.clone(),
            portal_chain_handles.buffer.clone(),
            portal_chain_portals.buffer.clone(),
            (*self.portal_compacted_chains_buf).clone(),
            batch.draw_counts_gpu.clone(),
        ];
        if self.bind_group_key.as_ref() != Some(&key) {
            self.bind_group = Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("PortalCull BG"),
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
                        resource: coord_data.coordinate_spaces.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: portal_views.buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: self.portal_indirect_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: self.portal_compacted_indices_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 8,
                        resource: self.portal_stats_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 9,
                        resource: portal_chain_handles.buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 10,
                        resource: self.portal_compacted_chains_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 11,
                        resource: portal_chain_portals.buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 12,
                        resource: batch.draw_counts_gpu.as_entire_binding(),
                    },
                ],
            }));
            self.bind_group_key = Some(key);
        }

        let draw_workgroups = self.draw_capacity;
        let chain_workgroups = self.chain_count;

        let mut cmds = ctx.graphics_cmds();
        {
            let mut pass = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("PortalCull Select"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.select_pipeline);
            pass.set_bind_group(0, self.bind_group.as_ref().unwrap(), &[]);
            pass.dispatch_workgroups(draw_workgroups, chain_workgroups, 1);
        }
        {
            let mut pass = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("PortalCull Finalize"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.finalize_pipeline);
            pass.set_bind_group(0, self.bind_group.as_ref().unwrap(), &[]);
            pass.dispatch_workgroups(draw_workgroups.div_ceil(64), 1, 1);
        }
        drop(cmds);

        // Pack the survivors per material key, for the portal draw's pipelines.
        let culled: Option<helio_pass_gbuffer::CulledBatchFrameData<'_>> = ctx
            .registry
            .get(helio_core::ResourceKey::new("culled_batch"));
        if let Some(culled) = culled {
            let indirect = Arc::clone(&self.portal_indirect_buf);
            self.compaction
                .record(ctx, &batch, &culled.segments, &indirect);
        }

        Ok(())
    }
}
