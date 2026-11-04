//! The virtual-geometry debug views as a compute pass inside the shade pass
//! (`shaders/vg_debug.wgsl`, view ids in [`helio_vg_format::debug_view`]).
//!
//! Nothing here exists until a view is selected: the pipeline is created on
//! the first selected frame and [`VgShade::encode_rec`] records the pass only
//! when [`VgShadeInputs::debug`] is `Some`, so an inactive view costs no GPU
//! time and no shader code in the hot shade path.

use bytemuck::{Pod, Zeroable};
use helio_vg_format::debug_view::VgDebugView;
use helio_vg_stream::VgStreamBuffers;

use crate::{buf, storage_entry, uniform_entry, VgShade, VgShadeInputs, VgShadeTargets};

pub const DEBUG_WGSL: &str = include_str!("../shaders/vg_debug.wgsl");

/// Extra frame data the views read, beyond what the shade already binds.
#[derive(Clone, Copy)]
pub struct VgShadeDebug<'a> {
    pub view: VgDebugView,
    /// The streaming store's buffers (residency, hierarchy, group refs, ...).
    pub stream: VgStreamBuffers<'a>,
    /// The cull's `state` (phase-0 counts).
    pub cull_state: &'a wgpu::Buffer,
    pub bin_capacity: u32,
    /// The software raster's per-tile counters.
    pub tile_counters: &'a wgpu::Buffer,
    /// The raster's own dense-tile classification of this frame.
    pub dense_mask: &'a wgpu::Buffer,
    /// The tiles Tier 3 resolved this frame (the dense pass's union mask), when a
    /// dense pass ran. The Raster Tier view paints those tiles dense/RT instead of
    /// by the cull bin of the pixel's cluster; `dense_mask` stays the raster's own
    /// classification (Dense Tile Mask view).
    pub resolved_mask: Option<&'a wgpu::Buffer>,
    /// Tile grid `[x, y]` of the two buffers above.
    pub tiles: [u32; 2],
    /// The LOD error threshold in pixels.
    pub tau_px: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub(crate) struct DebugParams {
    width: u32,
    height: u32,
    mode: u32,
    bin_capacity: u32,
    tiles_x: u32,
    tiles_y: u32,
    tau_px: f32,
    dense_resolved: u32,
}

pub(crate) struct DebugPipeline {
    bgl0: wgpu::BindGroupLayout,
    bgl1: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
    params: wgpu::Buffer,
}

/// The full `vg_debug` module source.
pub fn debug_source() -> String {
    format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        helio_vg_format::WGSL,
        // Group 0, bindings 7..=13 (page pool .. asset quant); no feedback.
        helio_vg_stream::bindings_wgsl(0, 7, false),
        helio_vg_stream::WGSL,
        helio_vg_format::debug_view::wgsl_constants(),
        format_args!(
            "const VGDBG_ST_PHASE0: u32 = {}u;",
            helio_pass_vg_cull::state::PHASE0_COUNTS
        ),
        crate::COMMON_WGSL,
        crate::shared_decode(),
        DEBUG_WGSL,
    )
}

impl DebugPipeline {
    fn new(device: &wgpu::Device) -> Self {
        let mut entries = vec![storage_entry(0, true), uniform_entry(1)];
        entries.extend((2..=18).map(|b| storage_entry(b, true)));
        let bgl0 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("VgShade debug 0"),
            entries: &entries,
        });
        let bgl1 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("VgShade debug 1"),
            entries: &[
                crate::texture_entry(0, crate::CS, crate::UINT),
                crate::storage_texture_entry(1, wgpu::TextureFormat::Rgba8Unorm),
                // Per-pixel source recorded by `vis_resolve`.
                crate::texture_entry(2, crate::CS, crate::UINT),
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("VgShade debug"),
            bind_group_layouts: &[Some(&bgl0), Some(&bgl1)],
            immediate_size: 0,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("VgShade debug"),
            source: wgpu::ShaderSource::Wgsl(debug_source().into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("VgShade vg_debug"),
            layout: Some(&layout),
            module: &module,
            entry_point: Some("vg_debug"),
            compilation_options: Default::default(),
            cache: None,
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("VgShade debug params"),
            size: std::mem::size_of::<DebugParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            bgl0,
            bgl1,
            pipeline,
            params,
        }
    }
}

impl VgShade {
    /// Records the selected debug view over the freshly shaded G-buffer
    /// albedo. Called by [`VgShade::encode_rec`] after the shade dispatches.
    /// Creates the debug pipeline on first use.
    pub(crate) fn ensure_debug(&mut self, device: &wgpu::Device) {
        self.debug.get_or_insert_with(|| DebugPipeline::new(device));
    }

    pub(crate) fn encode_debug(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut helio_core::CommandRecorder<'_>,
        width: u32,
        height: u32,
        inputs: &VgShadeInputs<'_>,
        targets: &VgShadeTargets<'_>,
        debug: &VgShadeDebug<'_>,
    ) {
        let pipeline = self.debug.as_ref().expect("ensure_debug ran");
        queue.write_buffer(
            &pipeline.params,
            0,
            bytemuck::bytes_of(&DebugParams {
                width,
                height,
                mode: debug.view.id(),
                bin_capacity: debug.bin_capacity,
                tiles_x: debug.tiles[0].max(1),
                tiles_y: debug.tiles[1].max(1),
                tau_px: debug.tau_px,
                dense_resolved: u32::from(debug.resolved_mask.is_some()),
            }),
        );
        let s = &debug.stream;
        let group0 = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("VgShade debug 0"),
            layout: &pipeline.bgl0,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buf(inputs.camera),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: buf(&pipeline.params),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: buf(inputs.static_objects),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: buf(inputs.coordinate_spaces),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: buf(inputs.visible),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: buf(inputs.rt_visible.unwrap_or(&self.dummy_keys)),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: buf(inputs.instances),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: buf(s.page_pool),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: buf(s.residency),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: buf(s.nodes),
                },
                wgpu::BindGroupEntry {
                    binding: 10,
                    resource: buf(s.group_refs),
                },
                wgpu::BindGroupEntry {
                    binding: 11,
                    resource: buf(s.asset_headers),
                },
                wgpu::BindGroupEntry {
                    binding: 12,
                    resource: buf(s.asset_bases),
                },
                wgpu::BindGroupEntry {
                    binding: 13,
                    resource: buf(s.asset_quant),
                },
                wgpu::BindGroupEntry {
                    binding: 14,
                    resource: buf(inputs.materials),
                },
                wgpu::BindGroupEntry {
                    binding: 15,
                    resource: buf(debug.cull_state),
                },
                wgpu::BindGroupEntry {
                    binding: 16,
                    resource: buf(debug.tile_counters),
                },
                wgpu::BindGroupEntry {
                    binding: 17,
                    resource: buf(debug.dense_mask),
                },
                wgpu::BindGroupEntry {
                    binding: 18,
                    resource: buf(debug.resolved_mask.unwrap_or(&self.dummy_keys)),
                },
            ],
        });
        let payload_view = self
            .payload_view
            .as_ref()
            .expect("encode_rec resized the targets");
        let source_view = self
            .source_view
            .as_ref()
            .expect("encode_rec resized the targets");
        let group1 = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("VgShade debug 1"),
            layout: &pipeline.bgl1,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(payload_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(targets.albedo),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(source_view),
                },
            ],
        });
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("VgShade vg_debug"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline.pipeline);
        pass.set_bind_group(0, &group0, &[]);
        pass.set_bind_group(1, &group1, &[]);
        pass.dispatch_workgroups(width.div_ceil(8), height.div_ceil(8), 1);
    }
}
