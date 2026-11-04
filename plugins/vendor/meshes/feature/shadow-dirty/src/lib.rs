//! GPU invalidation of cached shadow faces. Hashes all instances in each draw,
//! including transforms and coordinate spaces, and tests both old and new bounds.
//! Dirty bits remain set until the bounded shadow scheduler renders their tile.

use bytemuck::{Pod, Zeroable};
use helio_core::{PassContext, PrepareContext, RenderPass, Result as HelioResult};
use std::sync::Arc;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Maximum shadow atlas faces.  Must match `MAX_FACES` in the WGSL shader and
/// `MAX_SHADOW_FACES` in `helio-pass-shadow`.
const MAX_SHADOW_FACES: usize = helio_light_api::MAX_SHADOW_FACES as usize;

/// Regions of `face_geom_count_buf`, each `MAX_SHADOW_FACES` `u32`s: the
/// movable, static and transmissive caster counts, in that order.
pub const FACE_GEOM_COUNT_REGIONS: usize = 3;

const WORKGROUP_SIZE: u32 = 64;

/// Bytes of one `Changed` entry in the shader (three 16-byte vectors).
const CHANGED_BYTES: usize = 48;

// ── Uniforms ──────────────────────────────────────────────────────────────────

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ShadowDirtyUniforms {
    face_count: u32,
    /// Non-zero: track which pages of a face a moved caster touches
    /// (`ShadowBudget::page_invalidation`). Zero dirties every page.
    pages: u32,
    _pad: [u32; 2],
}

// ── Pass struct ───────────────────────────────────────────────────────────────

pub struct ShadowDirtyPass {
    /// `casters`, `detect`, `faces`, `finish` (see `shadow_dirty.wgsl`), run in
    /// that order.
    pipelines: [wgpu::ComputePipeline; 4],
    #[allow(dead_code)]
    bgl: wgpu::BindGroupLayout,

    /// Uniform buffer holding per-frame parameters.
    uniform_buf: wgpu::Buffer,

    /// Previous-frame bounds and hash of each movable draw call.
    /// Layout: `array<Previous>` indexed by draw-call index (NOT instance index),
    /// sized for every draw the batch can produce; only the first live-count
    /// entries are valid.
    prev_positions_buf: wgpu::Buffer,

    /// The movable caster count the last dispatch saw (one `u32`, GPU-only):
    /// a change dirties every face.
    last_movable_count_buf: wgpu::Buffer,

    /// Per-face dirty flag: 0 = clean, 1 = dirty (atomic u32 array, MAX_SHADOW_FACES entries).
    /// Shared with `ShadowPass` — published via `Arc` so the shadow pass can bind it.
    pub face_dirty_buf: Arc<wgpu::Buffer>,

    /// Per-face geometry draw counts (atomic u32 array, [`FACE_GEOM_COUNT_REGIONS`]
    /// regions of MAX_SHADOW_FACES entries: movable, static, transmissive).
    /// ShadowPass uses these as the `count_buffer` argument to
    /// `multi_draw_indexed_indirect_count`: 0 for a clean face.
    pub face_geom_count_buf: Arc<wgpu::Buffer>,

    /// Per-face 8x8 page mask of the dynamic layer that must be redrawn, two
    /// `u32`s per face (rows 0-3, then rows 4-7; bit `row * 8 + column`, with
    /// rows counted up from the bottom of clip space). Accumulates with
    /// `face_dirty_buf` until `ShadowPass` services the face.
    pub face_pages_buf: Arc<wgpu::Buffer>,

    /// Whether to track pages (`ShadowBudget::page_invalidation`).
    pages: bool,

    /// Per-caster flags written by ShadowMatrixPass when a light matrix changes.
    light_dirty_buf: Arc<wgpu::Buffer>,

    /// Bind group (lazy; rebuilt whenever the `instances` or `shadow_mats` buffer
    /// pointer changes due to `GrowableBuffer` reallocation).
    bind_group: Option<wgpu::BindGroup>,
    bind_group_key: Option<[wgpu::Buffer; 7]>,

    /// Movable draws whose instances changed this frame (`Changed` in the
    /// shader), one entry each, and how many.
    changed_buf: wgpu::Buffer,
    changed_count_buf: wgpu::Buffer,
}

impl ShadowDirtyPass {
    /// Allocate all GPU resources.  Pass the shared buffers to `ShadowPass::new()`.
    pub fn new(device: &wgpu::Device, light_dirty_buf: Arc<wgpu::Buffer>) -> Self {
        // ── Shader ────────────────────────────────────────────────────────────
        let shader = helio_core::shader::module(
            device,
            "ShadowDirty Shader",
            helio_core::include_wgsl!("../shaders/shadow_dirty.wgsl"),
        );

        // ── Bind Group Layout ─────────────────────────────────────────────────
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ShadowDirty BGL"),
            entries: &[
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
                // 0: instances (read-only storage)
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
                // 1: movable_draws (read-only storage)
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
                // 2: prev_positions (read-write storage)
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
                // 3: shadow_mats (read-only storage)
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
                // 4: face_dirty (read-write storage, atomic)
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 5: face_geom_count (read-write storage)
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
                // 6: uniforms
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 7: per-caster light dirty flags from ShadowMatrixPass
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
                // 9: Object Batch's GPU counts (movable caster count at word 2)
                wgpu::BindGroupLayoutEntry {
                    binding: 9,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 10: the movable count the last dispatch saw
                wgpu::BindGroupLayoutEntry {
                    binding: 10,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 12: movable draws that changed this frame
                wgpu::BindGroupLayoutEntry {
                    binding: 12,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 13: how many of them
                wgpu::BindGroupLayoutEntry {
                    binding: 13,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 11: per-face dirty page masks
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
            ],
        });

        // ── Pipeline ──────────────────────────────────────────────────────────
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ShadowDirty PL"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });

        let pipeline = |entry_point: &'static str| {
            helio_core::shader::create_compute_pipeline(
                &device,
                wgpu::ComputePipelineDescriptor {
                    label: Some(entry_point),
                    layout: Some(&pl),
                    module: &shader,
                    entry_point: Some(entry_point),
                    compilation_options: Default::default(),
                    cache: None,
                },
            )
        };
        let pipelines = [
            pipeline("casters"),
            pipeline("detect"),
            pipeline("faces"),
            pipeline("finish"),
        ];

        // ── Buffers ───────────────────────────────────────────────────────────

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ShadowDirty/Uniforms"),
            size: std::mem::size_of::<ShadowDirtyUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // prev_positions: one vec4f per movable draw slot.
        // MAX_SHADOW_FACES is a safe upper bound — scenes rarely have
        // more than a few dozen movable shadow casters.
        let prev_positions_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ShadowDirty/PrevPositions"),
            size: (MAX_SHADOW_FACES * 32) as u64, // 256 × vec4f
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Starts at u32::MAX, which no live count equals: the first dispatch
        // dirties every face.
        let last_movable_count_buf = wgpu::util::DeviceExt::create_buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("ShadowDirty/LastMovableCount"),
                contents: &u32::MAX.to_le_bytes(),
                usage: wgpu::BufferUsages::STORAGE,
            },
        );

        // face_dirty: one atomic<u32> per shadow face. Cleared by the command
        // encoder before the compute dispatch, which provides ordering across
        // every workgroup (a shader workgroup barrier cannot do that).
        let face_dirty_buf = Arc::new(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ShadowDirty/FaceDirty"),
            size: (MAX_SHADOW_FACES * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::INDIRECT
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));

        // face_geom_count: one u32 per shadow face in each of the movable,
        // static and transmissive regions. Written by this shader, read by ShadowPass.
        let face_geom_count_buf = Arc::new(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ShadowDirty/FaceGeomCount"),
            size: (FACE_GEOM_COUNT_REGIONS * MAX_SHADOW_FACES * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::INDIRECT
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));

        let face_pages_buf = Arc::new(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ShadowDirty/FacePages"),
            size: (MAX_SHADOW_FACES * 8) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));

        // One `Changed` (48 bytes) per movable draw slot; grown with the draw
        // capacity in `execute`.
        let changed_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ShadowDirty/Changed"),
            size: (MAX_SHADOW_FACES * CHANGED_BYTES) as u64,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let changed_count_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ShadowDirty/ChangedCount"),
            size: 16,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipelines,
            changed_buf,
            changed_count_buf,
            bgl,
            uniform_buf,
            prev_positions_buf,
            last_movable_count_buf,
            face_dirty_buf,
            face_geom_count_buf,
            face_pages_buf,
            pages: true,
            light_dirty_buf,
            bind_group: None,
            bind_group_key: None,
        }
    }
}

// ── RenderPass impl ───────────────────────────────────────────────────────────

impl RenderPass for ShadowDirtyPass {
    fn name(&self) -> &'static str {
        "ShadowDirty"
    }

    fn declare_resources(&self, builder: &mut helio_core::graph::ResourceBuilder) {
        builder.read("object_batch");
        builder.read(helio_light_api::SHADOW_MATRICES);
        builder.write_buffer("shadow_dirty");
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
        helio_core::cpu_scope!("ShadowDirty: prepare");
        let shadow =
            ctx.registry
                .get::<helio_light_api::ShadowMatricesFrameData<'_>>(
                    helio_light_api::shadow_matrices(),
                );
        let face_count = shadow
            .as_ref()
            .map(|s| s.shadow_count)
            .unwrap_or(0)
            .min(MAX_SHADOW_FACES as u32);
        if let Some(shadow) = &shadow {
            self.pages = shadow.budget.page_invalidation;
        }

        // The movable caster count, and whether it changed, are GPU-only:
        // see `last_movable_count_buf`.
        let u = ShadowDirtyUniforms {
            face_count,
            pages: u32::from(self.pages),
            _pad: [0; 2],
        };
        ctx.queue
            .write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&u));
        Ok(())
    }

    fn execute(&mut self, ctx: &mut PassContext) -> HelioResult<()> {
        helio_core::cpu_scope!("ShadowDirty: execute");
        let Some(batch) = ctx
            .registry
            .get::<helio_mesh_api::ObjectBatchFrameData<'_>>(helio_core::ResourceKey::new(
                "object_batch",
            ))
        else {
            return Ok(());
        };
        // Every movable draw the batch can produce this frame; the live count
        // is read on the GPU.
        let draw_capacity = batch.group_capacity;
        let Some(coords) = ctx
            .registry
            .get::<helio_core::CoordinateSpacesFrameData<'_>>(
                helio_core::resource_keys::coordinate_spaces(),
            )
        else {
            return Ok(());
        };
        let required = u64::from(draw_capacity.max(1)) * 32;
        if required > self.prev_positions_buf.size() {
            self.prev_positions_buf = ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Shadow draw history"),
                size: required.next_power_of_two(),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.bind_group_key = None;
        }
        let changed_required = (u64::from(draw_capacity.max(1)) + 1) * CHANGED_BYTES as u64;
        if changed_required > self.changed_buf.size() {
            self.changed_buf = ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ShadowDirty/Changed"),
                size: changed_required.next_power_of_two(),
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            });
            self.bind_group_key = None;
        }
        let Some(shadow_data) =
            ctx.registry
                .get::<helio_light_api::ShadowMatricesFrameData<'_>>(
                    helio_light_api::shadow_matrices(),
                )
        else {
            return Ok(());
        };
        let face_count = shadow_data.shadow_count;

        if face_count == 0 {
            return Ok(());
        }

        // ── Lazy bind group rebuild on GrowableBuffer reallocation ─────────────
        let key = [
            batch.instances.clone(),
            batch.shadow_movable_indirect.clone(),
            shadow_data
                .desired_matrices
                .unwrap_or(shadow_data.shadow_matrices)
                .clone(),
            (*self.light_dirty_buf).clone(),
            coords.coordinate_spaces.clone(),
            batch.draw_counts_gpu.clone(),
            self.changed_buf.clone(),
        ];

        if self.bind_group_key.as_ref() != Some(&key) {
            helio_core::cpu_scope!("ShadowDirty: rebuild bind group");
            self.bind_group = Some(
                ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("ShadowDirty BG"),
                    layout: &self.bgl,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 8,
                            resource: coords.coordinate_spaces.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: batch.instances.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: batch.shadow_movable_indirect.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: self.prev_positions_buf.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: shadow_data
                                .desired_matrices
                                .unwrap_or(shadow_data.shadow_matrices)
                                .as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: self.face_dirty_buf.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 5,
                            resource: self.face_geom_count_buf.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 6,
                            resource: self.uniform_buf.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 7,
                            resource: self.light_dirty_buf.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 9,
                            resource: batch.draw_counts_gpu.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 10,
                            resource: self.last_movable_count_buf.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 11,
                            resource: self.face_pages_buf.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 12,
                            resource: self.changed_buf.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 13,
                            resource: self.changed_count_buf.as_entire_binding(),
                        },
                    ],
                }),
            );
            self.bind_group_key = Some(key);
        }

        let bg = self.bind_group.as_ref().unwrap();

        // Reset the complete output arrays before dispatch. Doing this as
        // encoder commands avoids the cross-workgroup race that occurs when
        // invocation zero clears storage while other workgroups write it.
        let mut cmds = ctx.graphics_cmds();
        // Pending dirty bits survive until ShadowPass services their tile.

        // Dispatch enough threads to cover every movable draw the batch can
        // produce; threads past this frame's live count return at once.
        // At least one thread, so a change to an empty movable set still
        // passes through the force-dirty path.
        let draw_groups = draw_capacity.max(1).div_ceil(WORKGROUP_SIZE);
        let face_groups = face_count.div_ceil(WORKGROUP_SIZE).max(1);
        let caster_groups = face_count.div_ceil(6).div_ceil(WORKGROUP_SIZE).max(1);

        // The changed-draw list starts empty each frame.
        cmds.clear_buffer(&self.changed_count_buf, 0, None);
        ctx.begin_gpu_scope(&mut cmds, "Shadow Dirty: detect");
        {
            let mut pass = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("ShadowDirty"),
                timestamp_writes: None,
            });
            pass.set_bind_group(0, bg, &[]);
            // Dispatches in one pass run in order with their storage writes
            // visible to the next: casters -> detect -> faces -> finish.
            for (pipeline, groups) in
                self.pipelines
                    .iter()
                    .zip([caster_groups, draw_groups, face_groups, 1])
            {
                pass.set_pipeline(pipeline);
                pass.dispatch_workgroups(groups, 1, 1);
            }
        }
        ctx.end_gpu_scope(&mut cmds, "Shadow Dirty: detect");
        Ok(())
    }
}
