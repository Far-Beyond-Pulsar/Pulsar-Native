//! GPU-side culling + depth sort for a growable, handle-addressed instance
//! pool — built for `helio-pass-sprite-batch`'s `SpriteBatchPass`, but
//! deliberately has no Cargo dependency on that crate: it only needs raw
//! `Arc<wgpu::Buffer>` handles (instance data + alive flags) and two counts,
//! matching how `helio-pass-shadow-cull` stays decoupled from
//! `helio-pass-shadow` — the two are wired together by whoever builds the
//! render graph (`SpriteCullPass::new` takes the buffers, then
//! `SpriteBatchPass::use_gpu_culling` takes this pass's outputs), not by one
//! crate importing the other's types.
//!
//! Runs as its own [`RenderPass`], added to the graph *before* the pass that
//! renders the culled/sorted result — this is what lets that pass's
//! `execute()` never know the visible instance count on the CPU at all.
//! Every frame, entirely on the GPU:
//!
//! 1. `cs_cull` (`shaders/sprite_cull.wgsl`) — one thread per pool slot.
//!    Alive + in-view slots are atomically compacted into `indices_a`/`keys_a`,
//!    and the surviving count is atomically accumulated directly into
//!    [`SpriteCullPass::indirect_buf`]'s `instance_count` field.
//! 2. `cs_prepare` (`shaders/sprite_sort.wgsl`) — a single thread. Turns that
//!    GPU-written visible count into a small uniform buffer and an indirect dispatch
//!    arg buffer for step 3, so the sort's dispatch size tracks the actual
//!    per-frame visible count instead of the pool's worst-case capacity.
//! 3. The sort (`helio_core::radix_sort`): a stable LSD radix sort of the
//!    compacted list, 4-bit digits, eight passes.
//!
//! The CPU touches none of this per frame beyond ~27 small dispatches, none of
//! them sized from CPU-known data: the cull dispatch alone is fixed
//! (`slot_capacity`, known up front), but the sort's dispatches are *indirect* (`dispatch_workgroups_indirect`),
//! sized by `cs_prepare` from the cull pass's GPU-written visible count —
//! not from `max_visible` (the pool-sized worst case). Without that,
//! zooming a camera in to where only a few thousand sprites are visible
//! would still dispatch enough workgroups to cover `max_visible` sprites on
//! every sort pass, every frame — the fixed ceiling has to
//! size the *buffers* (a real GPU allocation can't grow mid-frame), but the
//! *dispatch* should track what's actually there. There is no per-instance
//! CPU work in `prepare()`/`execute()` regardless of pool size either way.
//!
//! The instance byte layout this reads (`position`/`size`/`depth` at
//! specific offsets in `shaders/sprite_cull.wgsl`) is a *protocol*, not a
//! Cargo-level type dependency — it must stay byte-identical to
//! `helio_pass_sprite_batch::SpriteInstance`'s `#[repr(C)]` layout, the same
//! way `helio-pass-shadow-cull`'s shader hardcodes `helio-core`'s
//! `GpuInstance` layout without depending on that crate for it.
//!
//! # Fixed capacity
//!
//! This pass is wired to specific `instances`/`slot_alive` buffers *once* at
//! construction and does not handle them being reallocated afterward — size
//! the pool (e.g. `SpriteBatchPass::reserve`) *before* wiring GPU culling in.

use bytemuck::{Pod, Zeroable};
use helio_core::{CommandRecorder, PassContext, PrepareContext, RenderPass, Result};
use std::sync::Arc;

const WG_SIZE: u32 = 256;
/// `DrawIndexedIndirectArgs` is 5 × u32 = 20 bytes: index_count,
/// instance_count, first_index, base_vertex, first_instance.
const INDIRECT_ARGS_SIZE: u64 = 20;
const INDIRECT_INSTANCE_COUNT_OFFSET: u64 = 4;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
// Field order and padding must match `shaders/sprite_cull.wgsl::CullUniforms`.
struct CullUniforms {
    view_min: [f32; 2],
    view_max: [f32; 2],
    slot_count: u32,
    max_visible: u32,
    _pad0: u32,
    scene_mode: u32,
}

/// GPU-side compute pass: culls a growable sprite pool against a view rect
/// and depth-sorts the survivors, entirely on the GPU. See the module doc
/// comment for the full design.
pub struct SpriteCullPass {
    max_visible: u32,

    cull_pipeline: wgpu::ComputePipeline,
    cull_bgl: wgpu::BindGroupLayout,
    legacy_instances_buf: Arc<wgpu::Buffer>,
    legacy_alive_buf: Arc<wgpu::Buffer>,
    visible_indices_buf: Arc<wgpu::Buffer>,
    sort_keys_buf: wgpu::Buffer,
    scene_instances_epoch: Option<u64>,
    active_slot_count: u32,
    scene_mode: u32,
    cull_uniform_buf: wgpu::Buffer,
    cull_bind_group: wgpu::BindGroup,
    view_min: [f32; 2],
    view_max: [f32; 2],
    view_dirty: bool,
    /// The view's centre when the view rect follows the render target
    /// (1 unit = 1 pixel, as `SpriteBatchPass` frames it by default); see
    /// [`SpriteCullPass::follow_target`].
    follow_target: Option<[f32; 2]>,
    target_size: (u32, u32),
    /// A pass built with [`SpriteCullPass::skip_while_empty`] is a graph
    /// member only while the scene's `"sprite_instances"` hold a sprite, so
    /// an overlay over a scene without sprites records nothing.
    skip_while_empty: bool,

    prepare_pipeline: wgpu::ComputePipeline,
    prepare_bind_group: wgpu::BindGroup,

    /// The depth sort (`helio_core::radix_sort`), bound to the visible
    /// pairs.
    sort: helio_core::radix_sort::RadixSort,
    sort_binds: helio_core::radix_sort::RadixSortBinds,

    /// `[num_blocks, 1, 1]`, GPU-written once per frame by `cs_prepare` — the
    /// indirect dispatch args the sort dispatches against,
    /// so their dispatch size tracks the real per-frame visible count.
    dispatch_args_buf: wgpu::Buffer,

    /// Final draw-order buffer: the sort's result lands back in the "a"
    /// ping-pong buffer.
    pub draw_order_buf: Arc<wgpu::Buffer>,
    /// `DrawIndexedIndirectArgs`, `instance_count` GPU-written by `cs_cull`.
    /// Bind this to `SpriteBatchPass::use_gpu_culling`.
    pub indirect_buf: Arc<wgpu::Buffer>,
}

impl SpriteCullPass {
    /// `instances_buf`/`alive_buf` must be
    /// [`crate::SpriteBatchPass::instances_buffer`]/
    /// [`crate::SpriteBatchPass::alive_buffer`] on the pass this culls for,
    /// sized for at least `slot_capacity` slots. `max_visible` bounds how
    /// many sprites can be visible at once — size it for the worst case your
    /// scene can put on screen simultaneously, not the total pool size.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        instances_buf: Arc<wgpu::Buffer>,
        alive_buf: Arc<wgpu::Buffer>,
        slot_capacity: u32,
        max_visible: u32,
    ) -> Self {
        let max_blocks = max_visible.div_ceil(WG_SIZE).max(1);

        // ── Cull ────────────────────────────────────────────────────────────
        let cull_shader = helio_core::shader::module(
            device,
            "Sprite Cull Shader",
            helio_core::include_wgsl!("../shaders/sprite_cull.wgsl"),
        );
        let cull_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Sprite Cull BGL"),
            entries: &[
                uniform_entry(0),
                storage_entry(1, true),
                storage_entry(2, true),
                storage_entry(3, false),
                storage_entry(4, false),
                storage_entry(5, false),
            ],
        });
        let cull_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Sprite Cull PL"),
            bind_group_layouts: &[Some(&cull_bgl)],
            immediate_size: 0,
        });
        let cull_pipeline = helio_core::shader::create_compute_pipeline(
            &device,
            wgpu::ComputePipelineDescriptor {
                label: Some("Sprite Cull Pipeline"),
                layout: Some(&cull_pl),
                module: &cull_shader,
                entry_point: Some("cs_cull"),
                compilation_options: Default::default(),
                cache: None,
            },
        );
        let cull_uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Sprite Cull Uniforms"),
            size: std::mem::size_of::<CullUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let keys_a = create_u32_buffer(device, "Sprite Sort Keys A", max_visible);
        let keys_b = create_u32_buffer(device, "Sprite Sort Keys B", max_visible);
        let indices_a = Arc::new(create_u32_buffer(
            device,
            "Sprite Sort Indices A",
            max_visible,
        ));
        let indices_b = create_u32_buffer(device, "Sprite Sort Indices B", max_visible);

        let indirect_buf = Arc::new(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Sprite Indirect Draw Args"),
            size: INDIRECT_ARGS_SIZE,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::INDIRECT
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        }));
        // Static fields, written once: index_count=6 (one quad), instance_count=0
        // (reset every frame before culling), first_index=0, base_vertex=0,
        // first_instance=0.
        queue.write_buffer(&indirect_buf, 0, bytemuck::cast_slice(&[6u32, 0, 0, 0, 0]));

        let cull_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Sprite Cull BG"),
            layout: &cull_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: cull_uniform_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: instances_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: alive_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: indices_a.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: keys_a.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: indirect_buf.as_entire_binding(),
                },
            ],
        });

        // ── Prepare (drives indirect dispatch sizing for the sort passes) ────
        let frame_uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Sprite Sort Frame Uniform"),
            // FrameUniform{count,num_blocks} is 8 bytes in WGSL; pad the
            // allocation for backend safety margin, mirroring the old
            // CountUniform buffer this replaces.
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM
                | wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let dispatch_args_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Sprite Sort Dispatch Args"),
            size: 12, // [num_blocks, 1, 1] as u32s
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::INDIRECT
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // y/z workgroup counts are always 1 — only `cs_prepare` ever updates
        // [0] again, once per frame.
        queue.write_buffer(
            &dispatch_args_buf,
            0,
            bytemuck::cast_slice(&[max_blocks, 1u32, 1u32]),
        );

        let prepare_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Sprite Sort Prepare BGL"),
            entries: &[
                storage_entry(0, true),
                storage_entry(1, false),
                storage_entry(2, false),
            ],
        });
        let prepare_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Sprite Sort Prepare PL"),
            bind_group_layouts: &[Some(&prepare_bgl)],
            immediate_size: 0,
        });

        // ── Sort ────────────────────────────────────────────────────────────
        let sort_shader = helio_core::shader::module(
            device,
            "Sprite Sort Shader",
            helio_core::include_wgsl!("../shaders/sprite_sort.wgsl"),
        );

        let prepare_pipeline = helio_core::shader::create_compute_pipeline(
            &device,
            wgpu::ComputePipelineDescriptor {
                label: Some("Sprite Sort Prepare Pipeline"),
                layout: Some(&prepare_pl),
                module: &sort_shader,
                entry_point: Some("cs_prepare"),
                compilation_options: Default::default(),
                cache: None,
            },
        );
        let prepare_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Sprite Sort Prepare BG"),
            layout: &prepare_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: indirect_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: frame_uniform_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: dispatch_args_buf.as_entire_binding(),
                },
            ],
        });

        let sort = helio_core::radix_sort::RadixSort::new(device);
        let sort_binds = sort.bind(
            device,
            &frame_uniform_buf,
            [&keys_a, &keys_b],
            [&*indices_a, &indices_b],
            max_visible,
        );

        Self {
            max_visible,
            cull_pipeline,
            cull_bgl,
            cull_uniform_buf,
            cull_bind_group,
            legacy_instances_buf: instances_buf,
            legacy_alive_buf: alive_buf,
            visible_indices_buf: indices_a.clone(),
            sort_keys_buf: keys_a,
            scene_instances_epoch: None,
            active_slot_count: slot_capacity,
            scene_mode: 0,
            view_min: [0.0, 0.0],
            view_max: [0.0, 0.0],
            view_dirty: true,
            follow_target: None,
            target_size: (0, 0),
            skip_while_empty: false,
            prepare_pipeline,
            prepare_bind_group,
            sort,
            sort_binds,
            dispatch_args_buf,
            draw_order_buf: indices_a,
            indirect_buf,
        }
    }

    fn make_cull_bind_group(
        &self,
        device: &wgpu::Device,
        instances: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Sprite Cull BG"),
            layout: &self.cull_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.cull_uniform_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: instances.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.legacy_alive_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.visible_indices_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: self.sort_keys_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: self.indirect_buf.as_entire_binding(),
                },
            ],
        })
    }

    /// Sets the world-space view rect sprites are culled against — must
    /// match the paired `SpriteBatchPass::set_camera`'s effective view, or
    /// sprites will be culled against the wrong bounds. Unlike the batch
    /// pass, this rect isn't derived from the render target size
    /// automatically (this pass has no render target to read a size from) —
    /// callers using `half_extent: None` on the batch pass must resize this
    /// to match themselves.
    pub fn set_view_rect(&mut self, center: [f32; 2], half_extent: [f32; 2]) {
        self.follow_target = None;
        self.view_min = [center[0] - half_extent[0], center[1] - half_extent[1]];
        self.view_max = [center[0] + half_extent[0], center[1] + half_extent[1]];
        self.view_dirty = true;
    }

    /// Culls against a view rect centred on `center` that follows the
    /// render size the pass is prepared with, 1 unit = 1 pixel: the paired
    /// batch pass's default framing (`SpriteBatchPass::set_camera(center,
    /// None)`).
    pub fn follow_target(mut self, center: [f32; 2]) -> Self {
        self.follow_target = Some(center);
        self.target_size = (0, 0);
        self
    }

    /// Leaves the graph's schedule while the scene's `"sprite_instances"`
    /// hold no sprite (or the scene has none); pair it with a batch pass that
    /// does the same ([`SpriteBatchPass::scene_overlay`]).
    pub fn skip_while_empty(mut self) -> Self {
        self.skip_while_empty = true;
        self
    }
}

fn create_u32_buffer(device: &wgpu::Device, label: &str, count: u32) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: (count.max(1) as u64) * 4,
        // COPY_SRC costs nothing at runtime and is what lets
        // `draw_order_buf` (== `indices_a`) be read back at all, including
        // by `tests/gpu_sort_validation.rs`.
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
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

impl RenderPass for SpriteCullPass {
    fn scene_buffers(&self) -> &'static [&'static str] {
        &["sprite_instances"]
    }

    fn is_member(&mut self, scene: &mut helio_core::SceneContent<'_>) -> bool {
        !self.skip_while_empty || scene.holds_rows("sprite_instances")
    }

    fn name(&self) -> &'static str {
        "SpriteCull"
    }

    fn render_pass_descriptor<'a>(
        &'a self,
        _target: &'a wgpu::TextureView,
        _depth: &'a wgpu::TextureView,
        _resources: &'a helio_core::ResourceRegistry<'a>,
    ) -> Option<wgpu::RenderPassDescriptor<'a>> {
        None // compute-only pass
    }

    fn prepare(&mut self, ctx: &PrepareContext) -> Result<()> {
        let scene = ctx
            .scene_buffers
            .get(pulsar_scenedb::gpu::BufferKey::of("sprite_instances"));
        if let Some(center) = self.follow_target {
            if self.target_size != (ctx.width, ctx.height) {
                self.target_size = (ctx.width, ctx.height);
                let half = [ctx.width as f32 * 0.5, ctx.height as f32 * 0.5];
                self.view_min = [center[0] - half[0], center[1] - half[1]];
                self.view_max = [center[0] + half[0], center[1] + half[1]];
                self.view_dirty = true;
            }
        }
        let epoch = scene.map(|handle| handle.epoch);
        if self.scene_instances_epoch != epoch {
            self.scene_instances_epoch = epoch;
            self.scene_mode = u32::from(scene.is_some());
            self.active_slot_count = scene
                .map(|handle| (handle.buffer.size() / 80) as u32)
                .unwrap_or(self.active_slot_count);
            if let Some(handle) = scene {
                self.cull_bind_group = self.make_cull_bind_group(ctx.device, &handle.buffer);
            } else {
                let legacy = self.legacy_instances_buf.clone();
                self.cull_bind_group = self.make_cull_bind_group(ctx.device, &legacy);
            }
            self.view_dirty = true;
        }

        if self.view_dirty {
            self.view_dirty = false;
            let u = CullUniforms {
                view_min: self.view_min,
                view_max: self.view_max,
                slot_count: self.active_slot_count,
                max_visible: self.max_visible,
                scene_mode: self.scene_mode,
                _pad0: 0,
            };
            ctx.write_buffer(&self.cull_uniform_buf, 0, bytemuck::bytes_of(&u));
        }
        Ok(())
    }

    fn execute(&mut self, ctx: &mut PassContext) -> Result<()> {
        self.record(&mut ctx.graphics_cmds());
        Ok(())
    }
}

impl SpriteCullPass {
    /// Records the cull, prepare and sort dispatches. Shared
    /// by the `RenderPass::execute()` trait impl and
    /// [`SpriteCullPass::run_once_for_testing`] — the graph-integrated path
    /// and the standalone test path must record identically, or a test pass
    /// proves nothing about the real one.
    fn record(&self, encoder: &mut CommandRecorder<'_>) {
        // Reset the atomic visible-count / instance_count field to zero
        // before culling. The other four `DrawIndexedIndirectArgs` fields
        // are static and were written once at construction.
        encoder.clear_buffer(&self.indirect_buf, INDIRECT_INSTANCE_COUNT_OFFSET, Some(4));

        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("SpriteCull"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.cull_pipeline);
            pass.set_bind_group(0, &self.cull_bind_group, &[]);
            pass.dispatch_workgroups(self.active_slot_count.div_ceil(WG_SIZE), 1, 1);
        }

        // Turns the GPU-computed visible count into `frame_uniform_buf`
        // (read by every sort kernel below) and `dispatch_args_buf` (the
        // indirect dispatch size for histogram/scatter) — this is what lets
        // those dispatches shrink when fewer sprites are visible instead of
        // always covering `max_visible`.
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("SpriteSort Prepare"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.prepare_pipeline);
            pass.set_bind_group(0, &self.prepare_bind_group, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }

        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("SpriteSort"),
                timestamp_writes: None,
            });
            self.sort
                .record(&mut pass, &self.sort_binds, &self.dispatch_args_buf, 0);
        }
    }

    /// Runs cull + sort once, outside a `RenderGraph`, blocking until the
    /// GPU is done. Not for the hot path — this exists for integration tests
    /// that need to read back [`SpriteCullPass::draw_order_buf`]/
    /// [`SpriteCullPass::indirect_buf`] afterward without standing up a full
    /// graph + `PassContext`.
    pub fn run_once_for_testing(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if self.view_dirty {
            self.view_dirty = false;
            let u = CullUniforms {
                view_min: self.view_min,
                view_max: self.view_max,
                slot_count: self.active_slot_count,
                max_visible: self.max_visible,
                scene_mode: self.scene_mode,
                _pad0: 0,
            };
            queue.write_buffer(&self.cull_uniform_buf, 0, bytemuck::bytes_of(&u));
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("SpriteCullPass::run_once_for_testing"),
        });
        self.record(&mut CommandRecorder::from_encoder(&mut encoder));
        queue.submit([encoder.finish()]);
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
    }
}
