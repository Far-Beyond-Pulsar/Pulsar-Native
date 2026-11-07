//! The meshes' scene join: the frontend's authored mesh rows to the object
//! rows the meshes' passes draw from, on the GPU
//! (Pulsar-Native#1035, Phase 2; `03-gpu-reflection-and-rendering.md`).
//! Lights have their own derivation (`helio_pass_light_cull::scene_rows`).
//!
//! A frontend whose components are separate entities linked to their owner
//! objects (a mesh instance joined to its object's transform and visibility)
//! installs a [`SceneJoin`] as a scene derivation
//! (`helio::RendererBuilder::with_scene_derivation`). Each frame its input
//! rows changed, the join recomputes one `StaticObjectRow`-shaped row per
//! section of every placed mesh instance, published as `"static_objects"`
//! for the object-batch, G-buffer, shadow and virtual-geometry passes. A
//! section names its material slot; the mesh's slots' rows of the
//! materials' table (the material instances' `"materials"` rows,
//! `helio_mats::instance`) are a var-len row of the mesh entity: the join
//! writes the slot's table row into the object row, reads the instance's
//! shading class and graph hash from its key row, and writes no material
//! data.
//!
//! "Placed" means: the instance is attached and enabled, the generation it
//! recorded for its owner is the owner's current one, the owner is not
//! hidden and has a transform. Nothing else is drawn, so a mesh added,
//! edited, disabled, re-enabled or removed through any write path appears
//! or disappears the frame its rows change, with no notification, armed
//! subscription or CPU-side projection involved. The join owns its outputs
//! (they are execution data, rebuilt from the inputs) and keeps no copy of
//! the scene.
//!
//! The frontend names its buffers in [`SceneJoinKeys`]; the expected row
//! layouts are the `*_ROW_BYTES` constants below and the structs in
//! `shaders/scene_join_*.wgsl`. An input whose rows do not have the
//! expected size is reported once and that half of the join is skipped
//! (nothing drawn) rather than read with the wrong layout.
//!
//! Requires up to 13 storage buffers in one compute stage (native backends
//! report far more; WebGPU's default of 8 does not suffice).

pub use helio_core::{
    BufferHandle, BufferKey, SceneBufferProjection, SceneDerivation, SceneDerivationContext,
    SceneDerivationOutput, ENTITY_GENERATIONS_KEY,
};

pub use helio_scene_join::{
    join_source, GENERATION_ROW_BYTES, HIDDEN_ROW_BYTES, MESH_BOUNDS_ROW_BYTES, OWNER_ROW_BYTES,
    TRANSFORM_ROW_BYTES,
};
use helio_scene_join::{origin_bits, origin_pair};

/// A var-len handle: `offset`, `count` (u32 each).
pub const HANDLE_ROW_BYTES: u64 = 8;
/// A mesh instance's object-row flags (`u32`).
pub const MESH_FLAGS_ROW_BYTES: u64 = 4;
/// One mesh section: draw range and its material slot.
pub const MESH_SECTION_ROW_BYTES: u64 = 16;
/// One material table row of a mesh's slot (`u32`).
pub const MATERIAL_ROW_BYTES: u64 = 4;
/// A material instance's key row (`helio_mats::instance::MaterialKeyComponent`).
pub const MATERIAL_KEY_ROW_BYTES: u64 = 16;

const OBJECT_ROW_BYTES: u64 = 252;
const WORKGROUP: u32 = 64;

/// The mesh join's shader as compiled (shader validation without a device).
pub fn mesh_join_source() -> String {
    join_wgsl(include_str!("../shaders/scene_join_meshes.wgsl"))
}

/// A join shader as compiled: the shared join declarations.
fn join_wgsl(source: &str) -> String {
    join_source(source)
}

/// Where the frontend's authored rows live. See the module doc for what
/// each holds.
#[derive(Clone, Copy, Debug)]
pub struct SceneJoinKeys {
    /// Instance -> owner links, keyed by the instance entity.
    pub owners: BufferKey,
    /// Entity generations (`helio_core::ENTITY_GENERATIONS_KEY`).
    pub generations: BufferKey,
    /// Object hidden flags, keyed by the object entity.
    pub hidden: BufferKey,
    /// Object transforms, keyed by the object entity.
    pub transforms: BufferKey,
    /// Mesh instances' vertex ranges (handle table of the vertex pool).
    pub vertex_handles: BufferKey,
    /// Mesh instances' index ranges (handle table of the index pool).
    pub index_handles: BufferKey,
    pub mesh_bounds: BufferKey,
    pub mesh_flags: BufferKey,
    /// Mesh instances' section ranges (handle table of `mesh_sections`).
    pub section_handles: BufferKey,
    /// Optional per-instance four-influence skin stream and bone palette ranges.
    pub skin_handles: BufferKey,
    pub palette_handles: BufferKey,
    /// The section pool; one output object row per pool slot.
    pub mesh_sections: BufferKey,
    /// Mesh instances' slot material rows (handle table of `material_rows`).
    pub material_row_handles: BufferKey,
    /// Every mesh's slots' rows of the materials' table.
    pub material_rows: BufferKey,
    /// The material instances' key rows (`helio_mats::instance::MATERIAL_KEYS`),
    /// at the rows `material_rows` names.
    pub material_keys: BufferKey,
}

pub const OBJECTS_KEY: BufferKey = BufferKey::of("static_objects");
/// The materials' table the object rows index (the material instances'
/// rows, not this join's).
pub const MATERIALS_KEY: BufferKey = helio_mats::MATERIALS.key();
/// A marker whose `content_generation` moves only when the *structure* of the
/// object rows changes (objects added, removed, hidden, or their mesh,
/// material or flags changed), not when objects merely move. The object batch
/// keeps its sort and grouping while this is unchanged. The name is shared
/// with `helio-pass-object-batch`, which reads it.
pub const OBJECTS_STRUCTURE_KEY: BufferKey = BufferKey::of("scene_objects_structure");
/// The object slots (rows of `static_objects`) a frame that only moved objects
/// rewrote, as `[total, fresh, slot, ..]` words: this frame's moved slots plus
/// the previous frame's, so a consumer can settle both. Published only for a
/// run whose list is complete (`content_generation` equals the objects' own);
/// otherwise every object row must be treated as moved.
pub const OBJECTS_MOVED_KEY: BufferKey = BufferKey::of("scene_objects_moved");

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    rows: u32,
    second: u32,
    _pad: [u32; 2],
    /// The frame's world origin as an f32 pair (`origin_hi + origin_lo`),
    /// subtracted pair-wise from the world positions of the rows.
    origin_hi: [f32; 4],
    origin_lo: [f32; 4],
}

/// A buffer the join owns and publishes.
struct Output {
    label: &'static str,
    buffer: wgpu::Buffer,
    rows: u32,
    row_bytes: u64,
    epoch: u64,
    content_generation: u64,
}

impl Output {
    fn new(device: &wgpu::Device, label: &'static str, row_bytes: u64) -> Self {
        Self {
            label,
            buffer: Self::allocate(device, label, row_bytes, 1),
            rows: 1,
            row_bytes,
            epoch: 0,
            content_generation: 0,
        }
    }

    fn allocate(device: &wgpu::Device, label: &str, row_bytes: u64, rows: u32) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: row_bytes * u64::from(rows.max(1)),
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        })
    }

    /// Hold at least `rows` rows (doubling), returning whether it grew.
    fn ensure(&mut self, device: &wgpu::Device, rows: u32) -> bool {
        if rows <= self.rows {
            return false;
        }
        let rows = rows.next_power_of_two();
        self.buffer = Self::allocate(device, self.label, self.row_bytes, rows);
        self.rows = rows;
        self.epoch += 1;
        true
    }

    fn handle(&self) -> BufferHandle {
        BufferHandle {
            buffer: self.buffer.clone(),
            epoch: self.epoch,
            row_bytes: self.row_bytes,
            content_generation: self.content_generation,
        }
    }
}

/// The scene join as a scene derivation. See the module doc.
pub struct SceneJoin {
    keys: SceneJoinKeys,
    mesh_pipeline: wgpu::ComputePipeline,
    mesh_uniforms: wgpu::Buffer,
    /// Bound in place of an optional input that is not registered yet.
    empty: wgpu::Buffer,
    objects: Output,
    /// Signatures of the mesh join's inputs at its last run, split into the
    /// structure and the placement (see `derive`).
    last_structure_inputs: Option<u64>,
    last_placement_inputs: Option<u64>,
    /// Moves with every structural change; published on `structure_marker`.
    structure_generation: u64,
    structure_marker: wgpu::Buffer,
    /// The buffers the mesh join was last bound to, and the bind group made
    /// from them.
    mesh_bind: Option<(Vec<wgpu::Buffer>, wgpu::BindGroup)>,
    reported: Vec<BufferKey>,
    moved: MovedJoin,
}

/// What a transform flush looked like to the join's last run, to tell whether
/// the next flush follows it directly (see `MovedJoin::eligible`).
#[derive(Clone, Copy, PartialEq, Eq)]
struct PlacementState {
    epoch: u64,
    content_generation: u64,
    size: u64,
    origin: [u64; 3],
    sequence: Option<u64>,
}

/// The moved-owner join: a frame whose only change is some owners'
/// transforms rewrites just those owners' object rows instead of every row.
/// See `shaders/scene_join_moved.wgsl`.
struct MovedJoin {
    pipelines: [wgpu::ComputePipeline; 3],
    /// Every binding, for the finish kernel that sizes the carry dispatch.
    layout: wgpu::BindGroupLayout,
    /// Without the indirect arguments, which the dispatch they size must not
    /// also bind as storage.
    layout_plain: wgpu::BindGroupLayout,
    uniforms: wgpu::Buffer,
    /// Per-owner list heads then per-instance links, rebuilt by every full
    /// join (see `scene_join_meshes.wgsl`'s `link`).
    link: wgpu::Buffer,
    link_words: u64,
    /// The owner count at the full join that built `link`.
    link_base: u32,
    /// Per object slot: the run that last moved it.
    stamp: wgpu::Buffer,
    stamp_rows: u32,
    /// Two moved lists: this frame's is built from the other's.
    lists: [wgpu::Buffer; 2],
    list_words: u64,
    current: usize,
    carry_args: wgpu::Buffer,
    binds: Vec<(Vec<wgpu::Buffer>, wgpu::BindGroup, wgpu::BindGroup)>,
    /// A full join has built `link` for the current structure.
    link_built: bool,
    last_placement: Option<PlacementState>,
    /// The list left by the last run covers every row that moved since the
    /// consumers last settled: false right after a full join.
    previous_complete: bool,
    /// The objects' `content_generation` the published list belongs to, or 0.
    generation: u64,
    run: u32,
}

impl MovedJoin {
    fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Scene Join Moved"),
            source: wgpu::ShaderSource::Wgsl(
                join_source(include_str!("../shaders/scene_join_moved.wgsl")).into(),
            ),
        });
        let storage = |binding: u32, read_only: bool| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let entries = |with_args: bool| {
            let mut entries = vec![
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
                storage(1, true),
                storage(2, true),
                storage(3, true),
                storage(4, false),
                storage(5, true),
                storage(6, false),
                storage(7, false),
                storage(8, false),
                storage(9, true),
            ];
            if with_args {
                entries.push(storage(10, false));
            }
            entries
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Scene Join Moved"),
            entries: &entries(true),
        });
        let layout_plain = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Scene Join Moved (no args)"),
            entries: &entries(false),
        });
        let pipeline = |entry: &str, layout: &wgpu::BindGroupLayout| {
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(entry),
                bind_group_layouts: &[Some(layout)],
                immediate_size: 0,
            });
            helio_core::shader::create_compute_pipeline(
                &device,
                wgpu::ComputePipelineDescriptor {
                    label: Some(entry),
                    layout: Some(&pipeline_layout),
                    module: &module,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    cache: None,
                },
            )
        };
        let list = || {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Scene Join Moved List"),
                size: 16,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        };
        Self {
            pipelines: [
                pipeline("cs_join_moved", &layout_plain),
                pipeline("cs_moved_finish", &layout),
                pipeline("cs_moved_carry", &layout_plain),
            ],
            layout,
            layout_plain,
            uniforms: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Scene Join Moved Uniforms"),
                size: std::mem::size_of::<Uniforms>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            link: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Scene Join Link"),
                size: 16,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            link_words: 4,
            link_base: 0,
            stamp: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Scene Join Stamp"),
                size: 16,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            }),
            stamp_rows: 4,
            lists: [list(), list()],
            list_words: 4,
            current: 0,
            carry_args: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Scene Join Moved Args"),
                size: 16,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::INDIRECT
                    | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            binds: Vec::new(),
            link_built: false,
            last_placement: None,
            previous_complete: false,
            generation: 0,
            run: 0,
        }
    }

    /// Size the buffers a full join fills for `owners` owner rows,
    /// `instances` instance rows and `slots` object slots.
    fn ensure(&mut self, device: &wgpu::Device, owners: u32, instances: u32, slots: u32) {
        let words = u64::from(owners) + u64::from(instances) + 1;
        if self.link_words != words {
            self.link_words = words;
            self.link = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Scene Join Link"),
                size: words * 4,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        self.link_base = owners;
        if self.stamp_rows < slots.max(1) {
            self.stamp_rows = slots.max(1);
            self.stamp = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Scene Join Stamp"),
                size: u64::from(self.stamp_rows) * 4,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            });
        }
        let list_words = u64::from(slots) + 2;
        if self.list_words < list_words {
            self.list_words = list_words;
            let list = || {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("Scene Join Moved List"),
                    size: list_words * 4,
                    usage: wgpu::BufferUsages::STORAGE
                        | wgpu::BufferUsages::COPY_DST
                        | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                })
            };
            self.lists = [list(), list()];
        }
    }
}

impl SceneJoin {
    pub fn new(device: &wgpu::Device, keys: SceneJoinKeys) -> Self {
        let pipeline = |label: &str, source: &str, entry: &str| {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(join_wgsl(source).into()),
            });
            helio_core::shader::create_compute_pipeline(
                &device,
                wgpu::ComputePipelineDescriptor {
                    label: Some(label),
                    layout: None,
                    module: &module,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    cache: None,
                },
            )
        };
        let uniforms = |label: &str| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: std::mem::size_of::<Uniforms>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        Self {
            keys,
            mesh_pipeline: pipeline(
                "Scene Join Meshes",
                include_str!("../shaders/scene_join_meshes.wgsl"),
                "cs_join_meshes",
            ),
            mesh_uniforms: uniforms("Scene Join Mesh Uniforms"),
            empty: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Scene Join Empty Input"),
                size: 256,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            }),
            objects: Output::new(device, "Scene Join Objects", OBJECT_ROW_BYTES),
            last_structure_inputs: None,
            last_placement_inputs: None,
            structure_generation: 0,
            structure_marker: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Scene Join Structure Marker"),
                size: 16,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            }),
            mesh_bind: None,
            reported: Vec::new(),
            moved: MovedJoin::new(device),
        }
    }

    /// The moved-owner join (see `MovedJoin`): rewrites the object rows of
    /// the owners `rows` names, then builds the list of every slot a consumer
    /// has to refresh. Returns whether that list is complete, i.e. follows a
    /// run that was itself a moved-owner join.
    fn record_moved(
        &mut self,
        ctx: &SceneDerivationContext<'_>,
        encoder: &mut wgpu::CommandEncoder,
        rows: &helio_core::FlushedRows,
        dispatch_args: Option<&wgpu::Buffer>,
        transforms: &BufferHandle,
        section_handles: &BufferHandle,
        mesh_bounds: Option<&BufferHandle>,
    ) -> bool {
        helio_core::cpu_scope!("SceneJoin: moved");
        let complete = self.moved.previous_complete;
        let previous = self.moved.current;
        let next = 1 - previous;
        self.moved.current = next;
        self.moved.previous_complete = true;
        self.moved.run = self.moved.run.wrapping_add(1).max(1);
        let (origin_hi, origin_lo) = origin_pair(ctx.world_origin);
        ctx.queue.write_buffer(
            &self.moved.uniforms,
            0,
            bytemuck::bytes_of(&Uniforms {
                rows: self.objects.rows,
                second: self.moved.link_base,
                _pad: [self.moved.run, 0],
                origin_hi,
                origin_lo,
            }),
        );
        let bound = [
            &transforms.buffer,
            &section_handles.buffer,
            mesh_bounds.map_or(&self.empty, |h| &h.buffer),
            &self.objects.buffer,
            &rows.buffer,
            &self.moved.link,
            &self.moved.stamp,
            &self.moved.lists[next],
            &self.moved.lists[previous],
            &self.moved.carry_args,
        ];
        let at = match self.moved.binds.iter().position(|(buffers, _, _)| {
            buffers.iter().zip(bound).all(|(cached, now)| cached == now)
        }) {
            Some(at) => at,
            None => {
                let mut entries = vec![entry(0, &self.moved.uniforms)];
                entries.extend(
                    bound
                        .iter()
                        .enumerate()
                        .map(|(i, b)| entry(i as u32 + 1, b)),
                );
                let plain = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("Scene Join Moved"),
                    layout: &self.moved.layout_plain,
                    entries: &entries[..10],
                });
                let full = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("Scene Join Moved Finish"),
                    layout: &self.moved.layout,
                    entries: &entries,
                });
                if self.moved.binds.len() >= 4 {
                    self.moved.binds.remove(0);
                }
                self.moved
                    .binds
                    .push((bound.iter().map(|b| (*b).clone()).collect(), plain, full));
                self.moved.binds.len() - 1
            }
        };
        let (_, plain, full) = &self.moved.binds[at];
        encoder.clear_buffer(&self.moved.lists[next], 0, Some(8));
        let [join, finish, carry] = &self.moved.pipelines;
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Scene Join Moved"),
            timestamp_writes: None,
        });
        pass.set_pipeline(join);
        pass.set_bind_group(0, plain, &[]);
        match dispatch_args {
            // The moved list was built on the GPU: its size is not known here.
            Some(args) => pass.dispatch_workgroups_indirect(args, 0),
            None => pass.dispatch_workgroups(rows.count.div_ceil(WORKGROUP), 1, 1),
        }
        drop(pass);
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Scene Join Moved Finish"),
            timestamp_writes: None,
        });
        pass.set_pipeline(finish);
        pass.set_bind_group(0, full, &[]);
        pass.dispatch_workgroups(1, 1, 1);
        drop(pass);
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Scene Join Moved Carry"),
            timestamp_writes: None,
        });
        pass.set_pipeline(carry);
        pass.set_bind_group(0, plain, &[]);
        pass.dispatch_workgroups_indirect(&self.moved.carry_args, 0);
        complete
    }

    /// `key`'s buffer when it is registered with the expected row size.
    /// A size mismatch is reported once and treated as missing.
    fn input<'a>(
        &mut self,
        inputs: &'a helio_core::SceneBufferProjection,
        key: BufferKey,
        row_bytes: u64,
    ) -> Option<&'a BufferHandle> {
        let handle = inputs.get(key)?;
        if handle.row_bytes != 0 && handle.row_bytes != row_bytes {
            if !self.reported.contains(&key) {
                self.reported.push(key);
                log::error!(
                    "scene join: buffer {key:?} has {}-byte rows, expected {row_bytes}; \
                     the join that reads it is skipped",
                    handle.row_bytes
                );
            }
            return None;
        }
        Some(handle)
    }
}

fn rows_of(handle: &BufferHandle, row_bytes: u64) -> u32 {
    (handle.buffer.size() / row_bytes).min(u64::from(u32::MAX)) as u32
}

/// A value that moves whenever any of `handles` was reallocated, rewritten
/// or resized, or `extra` changed.
fn signature(handles: &[Option<&BufferHandle>], extra: &[u64]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut mix = |value: u64| {
        hash ^= value;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    };
    for handle in handles {
        match handle {
            Some(handle) => {
                mix(handle.epoch);
                mix(handle.content_generation);
                mix(handle.buffer.size());
            }
            None => mix(u64::MAX),
        }
    }
    for value in extra {
        mix(*value);
    }
    hash
}

fn entry(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: buffer.as_entire_binding(),
    }
}

impl SceneDerivation for SceneJoin {
    fn name(&self) -> &'static str {
        "SceneJoin"
    }

    fn derive(
        &mut self,
        ctx: &SceneDerivationContext<'_>,
        encoder: &mut wgpu::CommandEncoder,
    ) -> SceneDerivationOutput {
        let keys = self.keys;
        let inputs = ctx.inputs;
        let owners = self.input(inputs, keys.owners, OWNER_ROW_BYTES);
        let generations = self.input(inputs, keys.generations, GENERATION_ROW_BYTES);
        let hidden = self.input(inputs, keys.hidden, HIDDEN_ROW_BYTES);
        let world_transforms = self.input(inputs, keys.transforms, TRANSFORM_ROW_BYTES);
        let vertex_handles = self.input(inputs, keys.vertex_handles, HANDLE_ROW_BYTES);
        let index_handles = self.input(inputs, keys.index_handles, HANDLE_ROW_BYTES);
        let mesh_bounds = self.input(inputs, keys.mesh_bounds, MESH_BOUNDS_ROW_BYTES);
        let mesh_flags = self.input(inputs, keys.mesh_flags, MESH_FLAGS_ROW_BYTES);
        let section_handles = self.input(inputs, keys.section_handles, HANDLE_ROW_BYTES);
        let skin_handles = self.input(inputs, keys.skin_handles, HANDLE_ROW_BYTES);
        let palette_handles = self.input(inputs, keys.palette_handles, HANDLE_ROW_BYTES);
        let sections = self.input(inputs, keys.mesh_sections, MESH_SECTION_ROW_BYTES);
        let material_row_handles =
            self.input(inputs, keys.material_row_handles, HANDLE_ROW_BYTES);
        let material_rows = self.input(inputs, keys.material_rows, MATERIAL_ROW_BYTES);
        let material_keys = self.input(inputs, keys.material_keys, MATERIAL_KEY_ROW_BYTES);
        // Meshes are placed from rows composed only up to their nearest rigid
        // frame (and tagged with that frame's space) when a hierarchy resolve
        // ran ahead of this join; otherwise from the plain transform rows, in the
        // world space.
        let placement_key = if inputs.contains(helio_scene_join::LOCAL_PLACEMENT_KEY) {
            helio_scene_join::LOCAL_PLACEMENT_KEY
        } else {
            keys.transforms
        };
        let transforms = if placement_key == keys.transforms {
            world_transforms
        } else {
            self.input(inputs, placement_key, TRANSFORM_ROW_BYTES)
        };
        let spaces = self.input(inputs, helio_scene_join::OBJECT_SPACE_KEY, 4);
        let mut recorded = false;
        let (origin_hi, origin_lo) = origin_pair(ctx.world_origin);
        let [ox, oy, oz] = origin_bits(ctx.world_origin);

        // ── Meshes ──────────────────────────────────────────────────────
        let instance_rows = section_handles.map_or(0, |h| rows_of(h, HANDLE_ROW_BYTES));
        let output_rows = sections.map_or(0, |h| rows_of(h, MESH_SECTION_ROW_BYTES));
        let grew = self.objects.ensure(ctx.device, output_rows);
        // What the join's rows are made of splits in two. The *structure*
        // (which objects exist, their meshes, materials, flags, visibility)
        // changes rarely and decides how the object batch sorts and groups
        // them. The *placement* (the shared `Transform` column and the world
        // origin) changes every frame an object is dragged or simulated, and
        // only moves numbers inside rows already sorted and grouped. Telling
        // the two apart is what lets moving one object cost one object's
        // worth of downstream work instead of re-sorting the scene.
        let structure_inputs = [
            owners,
            generations,
            hidden,
            vertex_handles,
            index_handles,
            mesh_bounds,
            mesh_flags,
            section_handles,
            skin_handles,
            palette_handles,
            sections,
            spaces,
            // A slot that names another material, or a material that changes
            // class (a graph that loaded), regroups its draws.
            material_row_handles,
            material_rows,
            material_keys,
        ];
        let structure_signature = signature(&structure_inputs, &[self.objects.epoch]);
        let placement_signature = signature(&[transforms], &[ox, oy, oz]);
        let structure_changed = grew || self.last_structure_inputs != Some(structure_signature);
        let placement_changed = self.last_placement_inputs != Some(placement_signature);
        if structure_changed || placement_changed {
            helio_core::cpu_scope!("SceneJoin: meshes");
            self.last_structure_inputs = Some(structure_signature);
            self.last_placement_inputs = Some(placement_signature);
            // Only some owners' transforms moved since the last run, which
            // SceneDB's flush reports row by row: rewrite just their objects.
            let mut moved_ran: Option<bool> = None;
            let flushed = transforms.and_then(|_| inputs.flushed_rows(placement_key));
            // A hierarchy resolve ahead of this join builds its own moved list on
            // the GPU (its count is not known here, so the dispatch is indirect).
            // It is only usable when it describes the rows being read.
            let resolved = inputs.get(helio_scene_join::HIERARCHY_MOVED_ARGS_KEY);
            let gpu_list = resolved
                .filter(|args| {
                    args.content_generation != 0
                        && transforms
                            .is_some_and(|t| args.content_generation == t.content_generation)
                })
                .map(|args| &args.buffer);
            let list_unusable = resolved.is_some() && gpu_list.is_none();
            let placement_now = transforms.map(|t| PlacementState {
                epoch: t.epoch,
                content_generation: t.content_generation,
                size: t.buffer.size(),
                origin: [ox, oy, oz],
                sequence: flushed.map(|f| f.sequence),
            });
            let moved_rows = match (self.moved.last_placement, placement_now, flushed) {
                (Some(last), Some(now), Some(rows))
                    if !structure_changed
                        && self.moved.link_built
                        && last.sequence.is_some_and(|s| rows.sequence == s + 1)
                        && last.epoch == now.epoch
                        && last.size == now.size
                        && last.origin == now.origin
                        && last.content_generation.wrapping_add(1) == now.content_generation
                        && rows.count > 0
                        && !list_unusable
                        // A large share of the scene moving is cheaper (and
                        // simpler) as one pass over all of it.
                        && (gpu_list.is_some()
                            || u64::from(rows.count) * 4 <= u64::from(instance_rows)) =>
                {
                    Some(rows)
                }
                _ => None,
            };
            self.moved.last_placement = placement_now;
            if let (Some(rows), Some(transforms), Some(section_handles)) =
                (moved_rows, transforms, section_handles)
            {
                moved_ran = Some(self.record_moved(
                    ctx,
                    encoder,
                    rows,
                    gpu_list,
                    transforms,
                    section_handles,
                    mesh_bounds,
                ));
            } else {
                self.moved.link_built = false;
                self.moved.previous_complete = false;
                self.moved.generation = 0;
                let owner_rows = transforms.map_or(0, |h| rows_of(h, TRANSFORM_ROW_BYTES));
                self.moved
                    .ensure(ctx.device, owner_rows, instance_rows, self.objects.rows);
                encoder.clear_buffer(&self.moved.link, 0, None);
                encoder.clear_buffer(&self.moved.lists[self.moved.current], 0, Some(8));
                encoder.clear_buffer(&self.objects.buffer, 0, None);
                if let (
                    Some(owners),
                    Some(generations),
                    Some(transforms),
                    Some(vertex_handles),
                    Some(index_handles),
                    Some(section_handles),
                    Some(sections),
                ) = (
                    owners,
                    generations,
                    transforms,
                    vertex_handles,
                    index_handles,
                    section_handles,
                    sections,
                ) {
                    ctx.queue.write_buffer(
                        &self.mesh_uniforms,
                        0,
                        bytemuck::bytes_of(&Uniforms {
                            rows: instance_rows,
                            second: self.objects.rows,
                            _pad: [self.moved.link_base, 0],
                            origin_hi,
                            origin_lo,
                        }),
                    );
                    // The bind group only depends on which buffers are bound, so
                    // a frame that only moved objects reuses the last one.
                    let bound = [
                        &owners.buffer,
                        &generations.buffer,
                        hidden.map_or(&self.empty, |h| &h.buffer),
                        &transforms.buffer,
                        &vertex_handles.buffer,
                        &index_handles.buffer,
                        mesh_bounds.map_or(&self.empty, |h| &h.buffer),
                        mesh_flags.map_or(&self.empty, |h| &h.buffer),
                        &section_handles.buffer,
                        skin_handles.map_or(&self.empty, |h| &h.buffer),
                        palette_handles.map_or(&self.empty, |h| &h.buffer),
                        &sections.buffer,
                        &self.objects.buffer,
                        material_keys.map_or(&self.empty, |h| &h.buffer),
                        &self.moved.link,
                        spaces.map_or(&self.empty, |h| &h.buffer),
                        material_row_handles.map_or(&self.empty, |h| &h.buffer),
                        material_rows.map_or(&self.empty, |h| &h.buffer),
                    ];
                    let reusable = self.mesh_bind.as_ref().is_some_and(|(buffers, _)| {
                        buffers.iter().zip(bound).all(|(cached, now)| cached == now)
                    });
                    if !reusable {
                        let layout = self.mesh_pipeline.get_bind_group_layout(0);
                        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some("Scene Join Meshes"),
                            layout: &layout,
                            entries: &[
                                entry(0, &self.mesh_uniforms),
                                entry(1, bound[0]),
                                entry(2, bound[1]),
                                entry(3, bound[2]),
                                entry(4, bound[3]),
                                entry(5, bound[4]),
                                entry(6, bound[5]),
                                entry(7, bound[6]),
                                entry(8, bound[7]),
                                entry(9, bound[8]),
                                entry(10, bound[9]),
                                entry(11, bound[10]),
                                entry(12, bound[11]),
                                entry(13, bound[12]),
                                entry(14, bound[13]),
                                entry(15, bound[14]),
                                entry(16, bound[15]),
                                entry(17, bound[16]),
                                entry(18, bound[17]),
                            ],
                        });
                        self.mesh_bind =
                            Some((bound.iter().map(|b| (*b).clone()).collect(), bind_group));
                    }
                    let bind_group = &self.mesh_bind.as_ref().expect("bound above").1;
                    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("Scene Join Meshes"),
                        timestamp_writes: None,
                    });
                    pass.set_pipeline(&self.mesh_pipeline);
                    pass.set_bind_group(0, bind_group, &[]);
                    pass.dispatch_workgroups(instance_rows.div_ceil(WORKGROUP).max(1), 1, 1);
                    drop(pass);
                    self.moved.link_built = true;
                }
            }
            // Objects hold placements, so any change moves them. The
            // structure marker only follows the structure: consumers that
            // sort and group on it (`ObjectBatch`) keep their work across a
            // frame that only moved things.
            self.objects.content_generation += 1;
            if let Some(complete) = moved_ran {
                self.moved.generation = if complete {
                    self.objects.content_generation
                } else {
                    0
                };
            }
            if structure_changed {
                self.structure_generation += 1;
            }
            recorded = true;
        }

        let mut buffers = vec![
            (OBJECTS_KEY, self.objects.handle()),
            (
                OBJECTS_MOVED_KEY,
                BufferHandle {
                    buffer: self.moved.lists[self.moved.current].clone(),
                    epoch: self.objects.epoch,
                    row_bytes: 0,
                    content_generation: self.moved.generation,
                },
            ),
            (
                OBJECTS_STRUCTURE_KEY,
                BufferHandle {
                    buffer: self.structure_marker.clone(),
                    epoch: self.objects.epoch,
                    row_bytes: 0,
                    content_generation: self.structure_generation,
                },
            ),
        ];
        SceneDerivationOutput {
            buffers,
            flushed: Vec::new(),
            recorded,
        }
    }
}
