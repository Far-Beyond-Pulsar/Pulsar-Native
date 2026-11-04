//! GPU shadow allocation with a bounded asynchronous residency commit.
use bytemuck::{Pod, Zeroable};
use helio_core::{CommandRecorder, PassContext, PrepareContext, RenderPass, Result as HelioResult};
pub mod feature;
pub use feature::ShadowViewsFeature;
// The lights' contract (matrices, budget, settings, flags) is the api's.
use helio_light_api::*;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ShadowMatrixUniforms {
    light_count: u32,
    shadow_atlas_size: u32,
    _pad: [u32; 2],
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CasterParams {
    row_count: u32,
    caster_capacity: u32,
    nonce: u32,
    atlas_size: u32,
    view_proj: [f32; 16],
    inv_view_proj: [f32; 16],
    camera: [f32; 4],
    tuning: [f32; 4],
}
/// Minimum frames between residency solves. The solve is cheap now and a
/// longer gap only delays new lights getting their maps (shadow pop-in); the
/// readback in flight already bounds the rate.
const REBUILD_INTERVAL_FRAMES: u64 = 1;
/// Minimum frames between residency solves when the only reason is that the
/// lights buffer was rewritten (its generation moves whenever any object
/// moves, see prepare): a new, removed or retuned light waits at most this
/// long for its shadow. A camera move or a reallocation still solves at once.
const GENERATION_REBUILD_INTERVAL_FRAMES: u64 = 8;
const TABLE_BYTES: u64 = std::mem::size_of::<ResidencyTable>() as u64;
type MapDone = std::sync::Arc<std::sync::Mutex<Option<bool>>>;
enum Readback {
    Idle,
    Copied(u32),
    Mapping(u32, MapDone),
}

pub struct ShadowMatrixPass {
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    uniform_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    shadow_matrix_buf: wgpu::Buffer,
    desired_matrices: wgpu::Buffer,
    camera_buf: wgpu::Buffer,
    shadow_dirty_buf: wgpu::Buffer,
    shadow_hashes_buf: wgpu::Buffer,
    bound_lights: wgpu::Buffer,
    shadow_atlas_size: u32,
    face_capacity: u32,
    caster_pipelines: [wgpu::ComputePipeline; 4],
    caster_bind_group_layout: wgpu::BindGroupLayout,
    caster_bind_group: wgpu::BindGroup,
    caster_params_buf: wgpu::Buffer,
    proposed: wgpu::Buffer,
    committed: wgpu::Buffer,
    candidates: wgpu::Buffer,
    staging: wgpu::Buffer,
    readback: Readback,
    residency: ResidencyTable,
    nonce: u32,
    rows: u32,
    frame: u64,
    generations: [u64; MAX_SHADOW_CASTERS],
    last_generation: Option<(u64, u64)>,
    /// rame when the last residency solve was dispatched.
    last_solve_frame: u64,
    last_view: [f32; 16],
    last_position: [f32; 4],
    rebuild: bool,
    /// Decided in `prepare` so the nonce and the dispatch always agree.
    rebuilding: bool,
    /// Earliest frame a camera- or retry-driven rebuild may run; a changed
    /// light set resets it so real edits are never delayed.
    next_rebuild: u64,
    commit: bool,
    budget: ShadowBudget,
    viewport_height: f32,
}
fn buffer(
    device: &wgpu::Device,
    label: &str,
    size: u64,
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size.max(16),
        usage,
        mapped_at_creation: false,
    })
}
fn bgl(
    device: &wgpu::Device,
    label: &str,
    kinds: &[wgpu::BufferBindingType],
) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &kinds
            .iter()
            .enumerate()
            .map(|(i, kind)| wgpu::BindGroupLayoutEntry {
                binding: i as u32,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: *kind,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            })
            .collect::<Vec<_>>(),
    })
}
fn bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    buffers: &[&wgpu::Buffer],
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Shadow bindings"),
        layout,
        entries: &buffers
            .iter()
            .enumerate()
            .map(|(i, b)| wgpu::BindGroupEntry {
                binding: i as u32,
                resource: b.as_entire_binding(),
            })
            .collect::<Vec<_>>(),
    })
}
fn pipeline(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    shader: &wgpu::ShaderModule,
    entry: &str,
) -> wgpu::ComputePipeline {
    let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(entry),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    helio_core::shader::create_compute_pipeline(
        &device,
        wgpu::ComputePipelineDescriptor {
            label: Some(entry),
            layout: Some(&pl),
            module: shader,
            entry_point: Some(entry),
            compilation_options: Default::default(),
            cache: None,
        },
    )
}
impl ShadowMatrixPass {
    pub fn new(
        device: &wgpu::Device,
        lights: &wgpu::Buffer,
        matrices: &wgpu::Buffer,
        camera: &wgpu::Buffer,
        dirty: &wgpu::Buffer,
        hashes: &wgpu::Buffer,
        atlas_size: u32,
    ) -> Self {
        use wgpu::BufferBindingType::{Storage, Uniform};
        use wgpu::BufferUsages as U;
        let ro = Storage { read_only: true };
        let rw = Storage { read_only: false };
        let uniform_buf = buffer(device, "Shadow matrix params", 16, U::UNIFORM | U::COPY_DST);
        let desired = buffer(
            device,
            "Pending shadow matrices",
            matrices.size(),
            U::STORAGE | U::COPY_DST | U::COPY_SRC,
        );
        let layout = bgl(device, "Shadow matrices", &[ro, rw, ro, Uniform, rw, rw]);
        let bindings = bind(
            device,
            &layout,
            &[lights, &desired, camera, &uniform_buf, dirty, hashes],
        );
        let shader = helio_core::shader::module(
            device,
            "Shadow matrices",
            helio_core::include_wgsl!("../shaders/shadow_matrices.wgsl"),
        );
        let matrix_pipeline = pipeline(device, &layout, &shader, "compute_shadow_matrices");
        let caster_layout = bgl(device, "Shadow allocation", &[rw, Uniform, rw, ro, rw]);
        let proposed = buffer(
            device,
            "Proposed shadow residency",
            TABLE_BYTES,
            U::STORAGE | U::COPY_SRC,
        );
        let committed = buffer(
            device,
            "Committed shadow residency",
            TABLE_BYTES,
            U::STORAGE | U::COPY_DST,
        );
        let candidates = buffer(
            device,
            "Shadow candidate scores",
            lights.size() / 128 * 16,
            U::STORAGE,
        );
        let params = buffer(
            device,
            "Shadow allocation params",
            std::mem::size_of::<CasterParams>() as u64,
            U::UNIFORM | U::COPY_DST,
        );
        let caster_bindings = bind(
            device,
            &caster_layout,
            &[lights, &params, &proposed, &committed, &candidates],
        );
        let shader = helio_core::shader::module(
            device,
            "Shadow residency",
            helio_core::include_wgsl!("../shaders/shadow_casters.wgsl"),
        );
        let pipelines = [
            "score_lights",
            "select_lights",
            "pack_tiles",
            "commit_lights",
        ]
        .map(|entry| pipeline(device, &caster_layout, &shader, entry));
        Self {
            pipeline: matrix_pipeline,
            bind_group_layout: layout,
            uniform_buf,
            bind_group: bindings,
            shadow_matrix_buf: matrices.clone(),
            desired_matrices: desired,
            camera_buf: camera.clone(),
            shadow_dirty_buf: dirty.clone(),
            shadow_hashes_buf: hashes.clone(),
            bound_lights: lights.clone(),
            shadow_atlas_size: atlas_size,
            face_capacity: (matrices.size() / std::mem::size_of::<GpuShadowMatrix>() as u64) as u32,
            caster_pipelines: pipelines,
            caster_bind_group_layout: caster_layout,
            caster_bind_group: caster_bindings,
            caster_params_buf: params,
            proposed,
            committed,
            candidates,
            staging: buffer(
                device,
                "Shadow residency readback",
                TABLE_BYTES,
                U::COPY_DST | U::MAP_READ,
            ),
            readback: Readback::Idle,
            residency: ResidencyTable::default(),
            nonce: 0,
            rows: 0,
            frame: 0,
            generations: [1; MAX_SHADOW_CASTERS],
            last_generation: None,
            last_solve_frame: 0,
            last_view: [0.0; 16],
            last_position: [0.0; 4],
            rebuild: true,
            rebuilding: false,
            next_rebuild: 0,
            commit: true,
            budget: ShadowBudget::default(),
            viewport_height: 1080.0,
        }
    }
    pub fn with_budget(mut self, budget: ShadowBudget, viewport_height: u32) -> Self {
        self.budget = budget;
        self.viewport_height = viewport_height.max(1) as f32;
        self
    }
    pub fn face_capacity(&self) -> u32 {
        self.face_capacity
    }
    pub fn caster_capacity(&self) -> u32 {
        (self.face_capacity / 6).min(MAX_SHADOW_CASTERS as u32)
    }
    pub fn atlas_size(&self) -> u32 {
        self.shadow_atlas_size
    }
    /// Last asynchronously committed, bounded residency snapshot.
    pub fn residency(&self) -> &ResidencyTable {
        &self.residency
    }
    pub fn matrices(&self) -> &wgpu::Buffer {
        &self.shadow_matrix_buf
    }
    fn poll(&mut self, queue: &wgpu::Queue) {
        match &self.readback {
            Readback::Idle => {}
            Readback::Copied(nonce) => {
                let nonce = *nonce;
                let done = std::sync::Arc::new(std::sync::Mutex::new(None));
                let callback = done.clone();
                self.staging
                    .slice(..)
                    .map_async(wgpu::MapMode::Read, move |r| {
                        if let Ok(mut d) = callback.lock() {
                            *d = Some(r.is_ok());
                        }
                    });
                self.readback = Readback::Mapping(nonce, done);
            }
            Readback::Mapping(nonce, done) => {
                let Some(ok) = done.lock().ok().and_then(|v| *v) else {
                    return;
                };
                if ok {
                    let mapped = self.staging.slice(..).get_mapped_range().unwrap();
                    let table = bytemuck::pod_read_unaligned::<ResidencyTable>(&mapped);
                    if table.header[1] == *nonce && *nonce == self.nonce {
                        for s in 0..self.caster_capacity() as usize {
                            let old = self.residency.residents[s];
                            let mut new = table.residents[s];
                            if old.owner == new.owner {
                                new.strength = old.strength;
                            }
                            if old.owner != new.owner
                                || old.tiles != new.tiles
                                || old.flags != new.flags
                            {
                                self.generations[s] = self.generations[s].wrapping_add(1);
                                for f in 0..6 {
                                    if old.owner == new.owner
                                        && old.tiles[f] == new.tiles[f]
                                        && old.flags == new.flags
                                    {
                                        continue;
                                    }
                                    let t = new.tiles[f];
                                    let a = self.shadow_atlas_size as f32;
                                    let atlas = [
                                        t.x as f32 / a,
                                        t.y as f32 / a,
                                        t.size as f32 / a,
                                        new.strength,
                                    ];
                                    let mut meta = [0u32; 8];
                                    for j in 0..4 {
                                        meta[j] = atlas[j].to_bits();
                                    }
                                    meta[5] = new.flags;
                                    meta[6] = t.size;
                                    meta[7] = 1;
                                    queue.write_buffer(
                                        &self.shadow_matrix_buf,
                                        (s * 6 + f) as u64 * 96 + 64,
                                        bytemuck::cast_slice(&meta),
                                    );
                                    meta[7] = 2;
                                    queue.write_buffer(
                                        &self.desired_matrices,
                                        (s * 6 + f) as u64 * 96 + 64,
                                        bytemuck::cast_slice(&meta),
                                    );
                                }
                            }
                            self.residency.residents[s] = new;
                        }
                        self.commit = true;
                        // Retry missing tiles after fading residents release their space.
                        self.rebuild |= self
                            .residency
                            .residents
                            .iter()
                            .any(|r| r.owner != 0 && r.target == 0);
                    }
                    drop(mapped);
                    self.staging.unmap();
                } else {
                    self.rebuild = true;
                }
                self.readback = Readback::Idle;
            }
        }
    }
    fn rebuild_due(&self) -> bool {
        self.rebuild && matches!(self.readback, Readback::Idle) && self.frame >= self.next_rebuild
    }
    fn dispatch(&self, cmds: &mut CommandRecorder<'_>, index: usize, groups: u32) {
        let mut pass = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Shadow residency"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.caster_pipelines[index]);
        pass.set_bind_group(0, &self.caster_bind_group, &[]);
        pass.dispatch_workgroups(groups.max(1), 1, 1);
    }
}
impl RenderPass for ShadowMatrixPass {
    fn scene_buffers(&self) -> &'static [&'static str] {
        &[helio_light_api::LIGHT_LIST]
    }

    fn name(&self) -> &'static str {
        "ShadowMatrix"
    }
    fn writes(&self) -> &'static [&'static str] {
        &[helio_light_api::SHADOW_MATRICES]
    }
    fn publish<'a>(&self, frame: &mut helio_core::ResourceRegistry<'a>) {
        let matrices = unsafe {
            std::mem::transmute::<&wgpu::Buffer, &'a wgpu::Buffer>(&self.shadow_matrix_buf)
        };
        let desired = unsafe {
            std::mem::transmute::<&wgpu::Buffer, &'a wgpu::Buffer>(&self.desired_matrices)
        };
        let residency =
            unsafe { std::mem::transmute::<&ResidencyTable, &'a ResidencyTable>(&self.residency) };
        frame.write(
            crate::shadow_matrices(),
            ShadowMatricesFrameData {
                shadow_matrices: matrices,
                desired_matrices: Some(desired),
                residency: Some(residency),
                budget: self.budget,
                shadow_count: self.face_capacity,
                per_caster_dirty_gen: self.generations,
                movable_objects_generation: self.frame,
                caster_layout: None,
            },
            self.name(),
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
        helio_core::cpu_scope!("ShadowMatrix: prepare");
        let lights = ctx
            .scene_buffers
            .get(helio_core::BufferKey::of(helio_light_api::LIGHT_LIST));
        self.rows = lights.map_or(0, |l| l.row_capacity());
        if let Some(l) = lights.filter(|l| l.buffer != self.bound_lights) {
            self.bound_lights = l.buffer.clone();
            self.candidates = buffer(
                ctx.device,
                "Shadow candidates",
                u64::from(self.rows) * 16,
                wgpu::BufferUsages::STORAGE,
            );
            self.bind_group = bind(
                ctx.device,
                &self.bind_group_layout,
                &[
                    &self.bound_lights,
                    &self.desired_matrices,
                    &self.camera_buf,
                    &self.uniform_buf,
                    &self.shadow_dirty_buf,
                    &self.shadow_hashes_buf,
                ],
            );
            self.caster_bind_group = bind(
                ctx.device,
                &self.caster_bind_group_layout,
                &[
                    &self.bound_lights,
                    &self.caster_params_buf,
                    &self.proposed,
                    &self.committed,
                    &self.candidates,
                ],
            );
            self.rebuild = true;
            self.commit = true;
            self.next_rebuild = 0;
        }
        let generation = lights.map(|l| (l.epoch, l.content_generation));
        if generation != self.last_generation {
            // The lights buffer is joined from the scene-wide `Transform`
            // column, so its `content_generation` moves whenever ANY object
            // moves, light or not (a gizmo drag moves it every frame).
            // Treating that as "every light changed" bumped every caster's
            // generation, which marks every face changed (a full, top
            // priority rebuild of both layers) and bumped the nonce, which
            // discards the residency solve in flight. Both starved the
            // per-frame shadow budget while an object was being dragged, so
            // its shadow only caught up once it stopped. A moved light is
            // already caught per caster on the GPU (`light_dirty`, from the
            // matrix hashes), and added, removed or retuned lights come back
            // from the re-solve as owner/tile/flag changes, so only the
            // re-solve is requested here. A reallocation (a new `epoch`)
            // does invalidate everything.
            let reallocated = generation.map(|g| g.0) != self.last_generation.map(|g| g.0);
            self.rebuild = true;
            self.commit = true;
            self.last_generation = generation;
            if reallocated {
                self.next_rebuild = 0;
                self.nonce = self.nonce.wrapping_add(1);
                for g in &mut self.generations {
                    *g = g.wrapping_add(1);
                }
            } else {
                self.next_rebuild = self
                    .next_rebuild
                    .max(self.last_solve_frame + GENERATION_REBUILD_INTERVAL_FRAMES);
            }
        }
        {
            helio_core::cpu_scope!("ShadowMatrix: poll residency readback");
            self.poll(ctx.queue);
        }
        let camera = &ctx.camera_data;
        let significant = camera
            .view_proj
            .iter()
            .zip(self.last_view)
            .any(|(a, b)| (*a - b).abs() > 0.01)
            || camera.position_near[..3]
                .iter()
                .zip(&self.last_position[..3])
                .any(|(a, b)| (*a - *b).abs() > 0.05);
        if significant {
            self.rebuild = true;
            self.last_view = camera.view_proj;
            self.last_position = camera.position_near;
        }
        for s in 0..self.caster_capacity() as usize {
            let r = &mut self.residency.residents[s];
            if r.owner == 0 {
                continue;
            }
            let target = r.target as f32 / 65535.0;
            let step = 1.0 / self.budget.fade_frames.max(1) as f32;
            let old = r.strength;
            r.strength += (target - r.strength).clamp(-step, step);
            if old != r.strength {
                for f in 0..6 {
                    let offset = (s * 6 + f) as u64 * 96 + 76;
                    ctx.write_buffer(
                        &self.shadow_matrix_buf,
                        offset,
                        bytemuck::bytes_of(&r.strength),
                    );
                    ctx.write_buffer(
                        &self.desired_matrices,
                        offset,
                        bytemuck::bytes_of(&r.strength),
                    );
                }
                if r.strength == 0.0 {
                    self.rebuild = true;
                }
            }
        }
        // The allocator always reads the current fade values and stable ownership.
        ctx.queue
            .write_buffer(&self.committed, 0, bytemuck::bytes_of(&self.residency));
        let mut max_res = self
            .budget
            .max_resolution
            .min(self.shadow_atlas_size)
            .clamp(128, 2048);
        while max_res > 128 && max_res * max_res * 3 > self.budget.update_texels_per_frame {
            max_res /= 2;
        }
        self.rebuilding = self.rebuild_due();
        if self.rebuilding {
            self.nonce = self.nonce.wrapping_add(1);
        }
        let params = CasterParams {
            row_count: self.rows,
            caster_capacity: self.caster_capacity(),
            nonce: self.nonce,
            atlas_size: self.shadow_atlas_size,
            view_proj: camera.view_proj,
            inv_view_proj: camera.inv_view_proj,
            camera: [
                camera.position_near[0],
                camera.position_near[1],
                camera.position_near[2],
                self.budget.max_distance,
            ],
            tuning: [
                self.viewport_height,
                max_res as f32,
                self.budget.hysteresis,
                0.0,
            ],
        };
        ctx.queue
            .write_buffer(&self.caster_params_buf, 0, bytemuck::bytes_of(&params));
        ctx.write_buffer(
            &self.uniform_buf,
            0,
            bytemuck::bytes_of(&ShadowMatrixUniforms {
                light_count: self.rows,
                shadow_atlas_size: self.shadow_atlas_size,
                _pad: [0; 2],
            }),
        );
        self.frame += 1;
        Ok(())
    }

    fn execute(&mut self, ctx: &mut PassContext) -> HelioResult<()> {
        helio_core::cpu_scope!("ShadowMatrix: execute");
        let mut cmds = ctx.graphics_cmds();
        if self.rows == 0 {
            return Ok(());
        }
        if self.commit {
            ctx.begin_gpu_scope(&mut cmds, "Shadow Matrix: commit residency");
            self.dispatch(&mut cmds, 3, self.rows.div_ceil(64));
            ctx.end_gpu_scope(&mut cmds, "Shadow Matrix: commit residency");
            self.commit = false;
        }
        if self.rebuilding {
            self.rebuilding = false;
            self.next_rebuild = self.frame + REBUILD_INTERVAL_FRAMES;
            self.last_solve_frame = self.frame;
            ctx.begin_gpu_scope(&mut cmds, "Shadow Matrix: residency solve");
            self.dispatch(&mut cmds, 0, self.rows.div_ceil(64));
            self.dispatch(&mut cmds, 1, 1);
            self.dispatch(&mut cmds, 2, 1);
            cmds.copy_buffer_to_buffer(&self.proposed, 0, &self.staging, 0, TABLE_BYTES);
            ctx.end_gpu_scope(&mut cmds, "Shadow Matrix: residency solve");
            self.readback = Readback::Copied(self.nonce);
            self.rebuild = false;
        }
        ctx.begin_gpu_scope(&mut cmds, "Shadow Matrix: build matrices");
        {
            let mut pass = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Shadow matrices"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.dispatch_workgroups(self.rows.div_ceil(64), 1, 1);
        }
        ctx.end_gpu_scope(&mut cmds, "Shadow Matrix: build matrices");
        Ok(())
    }
}
#[cfg(test)]
mod tests;
