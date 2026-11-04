//! GPU tiled light culling pass (Forward+).
//!
//! Divides the screen into 16×16-pixel tiles, then runs a compute shader that
//! tests every light sphere against each tile's view-space frustum.  The result
//! is two storage buffers:
//!
//! * `tile_light_counts[tile_idx]`  — number of lights that hit this tile
//! * `tile_light_lists[tile_idx * MAX_LIGHTS_PER_TILE + i]` — light index i
//!
//! These buffers are published into `ResourceRegistry` so `DeferredLightPass` can
//! skip every light that doesn't touch the current pixel's tile.

pub mod feature;
pub mod scene_rows;
pub use feature::LightCullFeature;

use bytemuck::{Pod, Zeroable};
use helio_core::graph::ResourceBuilder;
use helio_core::{PassContext, PrepareContext, RenderPass, Result as HelioResult};

// The light list and its binning are the lights' contract (the api's).
use helio_light_api::*;
use pulsar_scenedb::gpu::BufferKey;

#[cfg(test)]
mod tests;

// ─────────────────────────────────────────────────────────────────────────────
// GPU-side uniform mirroring LightCullParams in the WGSL shader.
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct LightCullParams {
    num_tiles_x: u32,
    num_tiles_y: u32,
    num_lights: u32,
    screen_width: u32,
    screen_height: u32,
    light_mode_direct_index: u32,
    _pad1: u32,
    _pad2: u32,
}

// ─────────────────────────────────────────────────────────────────────────────
// Pass struct
// ─────────────────────────────────────────────────────────────────────────────

pub struct LightCullPass {
    pipeline: wgpu::ComputePipeline,
    bgl: wgpu::BindGroupLayout,
    params_buf: wgpu::Buffer,
    compact_pipeline: wgpu::ComputePipeline,
    active_indices: wgpu::Buffer,
    compact_bind_group: Option<wgpu::BindGroup>,
    compact_key: Option<(u64, u64, u32)>,
    /// Storage buffer: u32 per light-slot per tile.
    /// Size: num_tiles * MAX_LIGHTS_PER_TILE * 4 bytes.
    pub tile_light_lists: wgpu::Buffer,
    /// Storage buffer: one u32 count per tile.
    /// Size: num_tiles * 4 bytes.
    pub tile_light_counts: wgpu::Buffer,
    /// Cached bind group, rebuilt when camera or lights buffer changes.
    bind_group: Option<wgpu::BindGroup>,
    /// Key: (camera, lights buffer epoch, light_entity_indices,
    /// transforms) — used to skip needless bind-group rebuilds. The
    /// lights buffer is keyed by its SceneDB epoch, not by the address of
    /// this frame's handle, which a reallocated buffer can reuse.
    bind_group_key: Option<(wgpu::Buffer, u64, wgpu::Buffer, wgpu::Buffer)>,
    /// Light culling cache key: (camera_generation, lights buffer (epoch,
    /// content generation), light count, use_direct_index) — used to skip
    /// culling compute when nothing the shader reads has changed. The
    /// content generation changes when lights are edited in place, which
    /// the epoch alone does not.
    cull_cache_key: Option<(u64, (u64, u64), u32, bool)>,
    num_tiles_x: u32,
    num_tiles_y: u32,
    width: u32,
    height: u32,
}

impl LightCullPass {
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let num_tiles_x = width.div_ceil(TILE_SIZE);
        let num_tiles_y = height.div_ceil(TILE_SIZE);
        let num_tiles = num_tiles_x
            .checked_mul(num_tiles_y)
            .expect("tile grid overflow: viewport dimensions too large");

        let shader = helio_core::shader::module(
            device,
            "LightCull Shader",
            helio_core::include_wgsl!("../shaders/light_cull.wgsl"),
        );

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("LightCull BGL"),
            entries: &[
                // 0: camera storage
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
                // 1: LightCullParams uniform
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
                // 2: lights storage read
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
                // 3: SceneDB `Transform` storage read -- entity-indexed the same
                // way `lights` is, see `light_cull.wgsl`'s binding doc.
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
                // 4: light_entity_indices storage read -- parallel to `lights`
                // only in CPU-resolved mode, see `light_cull.wgsl`'s binding doc.
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
                // 5: tile_light_lists read_write
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
                // 6: tile_light_counts read_write
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
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("LightCull PL"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });

        let pipeline = helio_core::shader::create_compute_pipeline(
            &device,
            wgpu::ComputePipelineDescriptor {
                label: Some("LightCull Pipeline"),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                cache: None,
            },
        );

        // Separate layout: the compaction output is writable here and read-only
        // in the tile kernel. Never bind the same buffer with conflicting usage.
        let compact_pipeline = helio_core::shader::create_compute_pipeline(
            &device,
            wgpu::ComputePipelineDescriptor {
                label: Some("LightCull Compact Pipeline"),
                layout: None,
                module: &shader,
                entry_point: Some("compact_lights"),
                compilation_options: Default::default(),
                cache: None,
            },
        );
        let active_indices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Active light indices"),
            size: 8,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let params_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("LightCull Params"),
            size: std::mem::size_of::<LightCullParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let list_buf_size = (num_tiles * MAX_LIGHTS_PER_TILE * 4) as u64;
        let tile_light_lists = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("TileLightLists"),
            size: list_buf_size.max(4),
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        let count_buf_size = (num_tiles * 4) as u64;
        let tile_light_counts = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("TileLightCounts"),
            size: count_buf_size.max(4),
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            bgl,
            params_buf,
            compact_pipeline,
            active_indices,
            compact_bind_group: None,
            compact_key: None,
            tile_light_lists,
            tile_light_counts,
            bind_group: None,
            bind_group_key: None,
            cull_cache_key: None,
            num_tiles_x,
            num_tiles_y,
            width,
            height,
        }
    }
}

impl RenderPass for LightCullPass {
    fn scene_buffers(&self) -> &'static [&'static str] {
        &[helio_light_api::LIGHT_LIST]
    }

    fn name(&self) -> &'static str {
        "LightCull"
    }

    fn declare_resources(&self, builder: &mut ResourceBuilder) {
        builder.write_buffer(helio_light_api::TILE_LIGHT_LISTS);
        builder.write_buffer(helio_light_api::TILE_LIGHT_COUNTS);
    }

    fn on_resize(&mut self, device: &wgpu::Device, size: helio_core::RenderSize) {
        let [width, height] = size.internal;
        let num_tiles_x = width.div_ceil(TILE_SIZE);
        let num_tiles_y = height.div_ceil(TILE_SIZE);
        let num_tiles = num_tiles_x
            .checked_mul(num_tiles_y)
            .expect("tile grid overflow: viewport dimensions too large");

        let list_buf_size = (num_tiles * MAX_LIGHTS_PER_TILE * 4) as u64;
        self.tile_light_lists = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("TileLightLists"),
            size: list_buf_size.max(4),
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        let count_buf_size = (num_tiles * 4) as u64;
        self.tile_light_counts = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("TileLightCounts"),
            size: count_buf_size.max(4),
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        self.num_tiles_x = num_tiles_x;
        self.num_tiles_y = num_tiles_y;
        self.width = width;
        self.height = height;
        // Invalidate cached bind group so it gets rebuilt with the new buffers.
        self.bind_group = None;
        self.bind_group_key = None;
        self.cull_cache_key = None;
    }

    fn publish<'a>(&self, frame: &mut helio_core::ResourceRegistry<'a>) {
        // SAFETY: extended out of `self`'s own owned buffers, which are
        // pass-lifetime (owned by the RenderGraph across many frames), never
        // frame-scoped -- see `helio-pass-object-batch::publish`'s identical
        // comment for the full reasoning.
        let tile_light_lists: &'a wgpu::Buffer =
            unsafe { std::mem::transmute(&self.tile_light_lists) };
        let tile_light_counts: &'a wgpu::Buffer =
            unsafe { std::mem::transmute(&self.tile_light_counts) };
        frame.write(
            helio_core::ResourceKey::new(helio_light_api::TILE_LIGHT_LISTS),
            tile_light_lists,
            "LightCull",
        );
        frame.write(
            helio_core::ResourceKey::new(helio_light_api::TILE_LIGHT_COUNTS),
            tile_light_counts,
            "LightCull",
        );
        frame.write(
            helio_core::ResourceKey::new(helio_light_api::CLUSTER_LIGHT_GRID),
            crate::ClusterLightGrid {
                tile_light_lists,
                tile_light_counts,
                num_tiles_x: self.num_tiles_x,
                num_tiles_y: self.num_tiles_y,
            },
            "LightCull",
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
        // Tile grid must match the internal (render-target) resolution, not output.
        // ctx.width/height are internal_w/h from the graph.
        self.num_tiles_x = ctx.width.div_ceil(TILE_SIZE);
        self.num_tiles_y = ctx.height.div_ceil(TILE_SIZE);
        // Prefer the SceneDB-direct `"scene_lights"` buffer (every allocated
        // row, no per-frame CPU query) when populated; else
        // `ctx.scene.movable_light_count`, production's actual light count
        // today (`Renderer::submit_light_frame`, driven by `engine_backend`'s
        // own SceneDB resolve) -- see `light_mode_direct_index`'s doc.
        let scene_lights = ctx
            .scene_buffers
            .get(BufferKey::of(helio_light_api::LIGHT_LIST));
        let use_direct_index = scene_lights.is_some();
        // Every allocated row: the buffer grows with the scene and unused
        // rows are zeroed.
        let num_lights = scene_lights.map_or(0, |lights| lights.row_capacity());
        let required_bytes = (u64::from(num_lights) + 1) * 4;
        if self.active_indices.size() < required_bytes {
            self.active_indices = ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Active light indices"),
                size: required_bytes,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.compact_bind_group = None;
            self.compact_key = None;
            self.bind_group_key = None;
            self.cull_cache_key = None;
        }
        let params = LightCullParams {
            num_tiles_x: self.num_tiles_x,
            num_tiles_y: ctx.height.div_ceil(TILE_SIZE),
            num_lights,
            screen_width: ctx.width,
            screen_height: ctx.height,
            light_mode_direct_index: use_direct_index as u32,
            _pad1: 0,
            _pad2: 0,
        };
        ctx.queue
            .write_buffer(&self.params_buf, 0, bytemuck::bytes_of(&params));
        Ok(())
    }

    fn execute(&mut self, ctx: &mut PassContext) -> HelioResult<()> {
        let scene_lights_handle = ctx
            .scene_buffers
            .get(BufferKey::of(helio_light_api::LIGHT_LIST));
        let use_direct_index = scene_lights_handle.is_some();
        let lights_buf = scene_lights_handle
            .map(|handle| &handle.buffer)
            .unwrap_or(ctx.camera);
        let light_entity_indices_buf = &self.active_indices;
        let movable_light_count = scene_lights_handle.map_or(0, |lights| lights.row_capacity());

        if !use_direct_index && movable_light_count == 0 {
            // No active movable lights via either source: clear light
            // lists/counts to avoid stale data usage. Static/stationary
            // lights are baked and don't need runtime culling.
            let mut cmds = ctx.graphics_cmds();
            cmds.clear_buffer(&self.tile_light_lists, 0, None);
            cmds.clear_buffer(&self.tile_light_counts, 0, None);
            self.cull_cache_key = None; // Invalidate cache
            self.compact_key = None;
            self.compact_bind_group = None;
            return Ok(());
        }

        // Fallback mirrors `ForwardLitPass`'s: before any entity has a
        // `Transform` yet, bind *some* valid buffer so bind-group creation
        // can't fail -- `params.num_lights` is 0 whenever `transforms` would
        // actually be dereferenced at a live light's index, so this is never
        // read in practice.
        let transforms_buf = ctx.camera;

        // ── Light culling cache: skip compute if scene static ─────────────────
        // Use generation counters to detect actual data changes (not pointer
        // addresses) for the CPU-resolved path; the SceneDB-direct path uses
        // its buffer's own epoch instead, since nothing else identifies "did
        // the row data change" for it.
        let camera_gen = ctx.camera_generation;
        let lights_gen = scene_lights_handle
            .map(|h| (h.epoch, h.content_generation))
            .unwrap_or((0, 0));

        let compact_key = (lights_gen.0, lights_gen.1, movable_light_count);
        if self.compact_key != Some(compact_key) {
            if self.compact_bind_group.is_none()
                || self.compact_key.map(|key| key.0) != Some(lights_gen.0)
            {
                self.compact_bind_group =
                    Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("LightCull Compact BG"),
                        layout: &self.compact_pipeline.get_bind_group_layout(0),
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: self.params_buf.as_entire_binding(),
                            },
                            wgpu::BindGroupEntry {
                                binding: 2,
                                resource: lights_buf.as_entire_binding(),
                            },
                            wgpu::BindGroupEntry {
                                binding: 7,
                                resource: self.active_indices.as_entire_binding(),
                            },
                        ],
                    }));
            }
            let mut cmds = ctx.graphics_cmds();
            cmds.clear_buffer(&self.active_indices, 0, Some(4));
            if movable_light_count > 0 {
                let mut compact = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("LightCull Compact"),
                    timestamp_writes: None,
                });
                compact.set_pipeline(&self.compact_pipeline);
                compact.set_bind_group(0, self.compact_bind_group.as_ref().unwrap(), &[]);
                compact.dispatch_workgroups(movable_light_count.div_ceil(256), 1, 1);
            }
            self.compact_key = Some(compact_key);
        }

        let cache_key = (
            camera_gen,
            lights_gen,
            movable_light_count,
            use_direct_index,
        );

        // `self.width/height` are internal-resolution values maintained by
        // on_resize. ctx.width/height are full output resolution, so do not
        // use them as a resize signal here.
        let resolution_changed = false;

        // Check if we can reuse previous frame's culling results
        if self.cull_cache_key == Some(cache_key) && !resolution_changed {
            // Camera, lights, and resolution unchanged - reuse cached tile culling results
            return Ok(());
        }

        // Update cache key
        self.cull_cache_key = Some(cache_key);

        let lights_epoch = scene_lights_handle.map_or(u64::MAX, |h| h.epoch);
        let key = (
            ctx.camera.clone(),
            lights_epoch,
            light_entity_indices_buf.clone(),
            transforms_buf.clone(),
        );

        if self.bind_group_key.as_ref() != Some(&key) {
            self.bind_group = Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("LightCull BG"),
                layout: &self.bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: ctx.camera.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: self.params_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: lights_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: transforms_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: light_entity_indices_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: self.tile_light_lists.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: self.tile_light_counts.as_entire_binding(),
                    },
                ],
            }));
            self.bind_group_key = Some(key);
        }

        let total_tiles = self.num_tiles_x * self.num_tiles_y;
        // Each workgroup has 256 threads, each thread handles one tile.
        let workgroups = total_tiles.div_ceil(256);

        let mut cmds = ctx.graphics_cmds();
        let mut pass = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("LightCull"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, self.bind_group.as_ref().unwrap(), &[]);
        pass.dispatch_workgroups(workgroups, 1, 1);
        Ok(())
    }
}
