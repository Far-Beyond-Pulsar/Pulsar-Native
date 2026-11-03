//! GPU compute bounce-physics for a `helio-pass-sprite-batch` instance pool
//! — writes positions (and the Y-sort `depth` field) directly into the
//! shared `instances` storage buffer every frame, entirely on the GPU.
//!
//! Deliberately has no Cargo dependency on `helio-pass-sprite-batch` or
//! `helio-pass-sprite-cull`: it only needs `Arc<wgpu::Buffer>` handles
//! (instance data + alive flags, the same two `SpriteCullPass` binds) plus
//! its own per-slot velocity buffer, matching how all three sprite passes
//! stay decoupled from each other — wired together by whoever builds the
//! render graph, not by importing each other's types.
//!
//! # Ordering and the "don't call `update_sprite` afterward" rule
//!
//! Add this pass to the graph *before* `SpriteCullPass`, which must in turn
//! run *before* `SpriteBatchPass` — cull needs to see this frame's simulated
//! positions, and the batch pass needs cull's output.
//!
//! `SpriteBatchPass::prepare()` uploads its CPU-side `Vec<SpriteInstance>`
//! mirror over whatever byte range was last touched by `insert_sprite`/
//! `update_sprite`. Since this pass writes `position`/`depth` *directly on
//! the GPU*, calling `update_sprite` for a slot this pass is driving would
//! have `SpriteBatchPass::prepare()` clobber that frame's simulated position
//! with stale CPU data on the next upload. The correct pattern: insert every
//! sprite once at startup (one upload, `SpriteBatchPass`'s dirty range then
//! goes permanently clean), and never call `update_sprite` on a
//! GPU-simulated slot again — let this pass own it from then on.

use bytemuck::{Pod, Zeroable};
use helio_core::{PassContext, PrepareContext, RenderPass, Result};
use std::sync::Arc;
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SimUniforms {
    bounds_min: [f32; 2],
    bounds_max: [f32; 2],
    dt: f32,
    slot_count: u32,
    _pad0: u32,
    _pad1: u32,
}

const WG_SIZE: u32 = 256;

/// GPU compute pass: integrates position by velocity and bounces off a fixed
/// world-space box, in place, for every alive slot in a paired
/// `SpriteBatchPass` instance pool. See the module doc comment for wiring
/// order and the CPU-side "don't touch these slots" rule.
pub struct SpriteSimulatePass {
    pipeline: wgpu::ComputePipeline,
    bind_group: wgpu::BindGroup,
    uniform_buf: wgpu::Buffer,
    slot_count: u32,
    bounds_min: [f32; 2],
    bounds_max: [f32; 2],
}

impl SpriteSimulatePass {
    /// `instances_buf`/`alive_buf` must be
    /// `SpriteBatchPass::instances_buffer`/`SpriteBatchPass::alive_buffer` on
    /// the pass this simulates for. `initial_velocities.len()` fixes the
    /// slot count this pass drives — must match (or exceed) the number of
    /// sprites actually inserted, and (like `SpriteCullPass`) is fixed for
    /// this pass's lifetime; reallocating the paired pool's buffers
    /// afterward isn't handled.
    pub fn new(
        device: &wgpu::Device,
        instances_buf: Arc<wgpu::Buffer>,
        alive_buf: Arc<wgpu::Buffer>,
        initial_velocities: &[[f32; 2]],
        bounds_min: [f32; 2],
        bounds_max: [f32; 2],
    ) -> Self {
        let slot_count = initial_velocities.len() as u32;

        let shader = helio_core::shader::module(
            device,
            "Sprite Simulate Shader",
            helio_core::include_wgsl!("../shaders/sprite_simulate.wgsl"),
        );

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Sprite Simulate BGL"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
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

        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Sprite Simulate PL"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipeline = helio_core::shader::create_compute_pipeline(
            &device,
            wgpu::ComputePipelineDescriptor {
                label: Some("Sprite Simulate Pipeline"),
                layout: Some(&pl),
                module: &shader,
                entry_point: Some("cs_simulate"),
                compilation_options: Default::default(),
                cache: None,
            },
        );

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Sprite Simulate Uniforms"),
            size: std::mem::size_of::<SimUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let velocity_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Sprite Velocities"),
            contents: bytemuck::cast_slice(initial_velocities),
            usage: wgpu::BufferUsages::STORAGE,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Sprite Simulate BG"),
            layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: alive_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: instances_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: velocity_buf.as_entire_binding(),
                },
            ],
        });

        Self {
            pipeline,
            bind_group,
            uniform_buf,
            slot_count,
            bounds_min,
            bounds_max,
        }
    }
}

impl RenderPass for SpriteSimulatePass {
    fn name(&self) -> &'static str {
        "SpriteSimulate"
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
        let u = SimUniforms {
            bounds_min: self.bounds_min,
            bounds_max: self.bounds_max,
            dt: ctx.delta_time,
            slot_count: self.slot_count,
            _pad0: 0,
            _pad1: 0,
        };
        ctx.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&u));
        Ok(())
    }

    fn execute(&mut self, ctx: &mut PassContext) -> Result<()> {
        let mut encoder = ctx.graphics_cmds();
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("SpriteSimulate"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.dispatch_workgroups(self.slot_count.div_ceil(WG_SIZE), 1, 1);
        Ok(())
    }
}
