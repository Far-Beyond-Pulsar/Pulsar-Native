//! Virtual geometry: the global instance BVH (GCBVH top level), built and
//! maintained entirely on the GPU.
//!
//! # What it does
//!
//! Reads the VG scene derivation's `vg_instances` rows ([`GpuVgInstance`]) and
//! the scene join's `static_objects` rows (world bounding sphere, liveness) by
//! [`BufferKey`], and keeps a binary AABB tree over the live instances:
//!
//! * **Rebuild** (instance structure changed): Morton codes of the instance
//!   centres, a stable radix sort (the 1-bit LSD scheme of
//!   `helio-pass-object-batch`), batch-leaf formation, Karras LBVH topology,
//!   bottom-up bounds.
//! * **Refit** (only transforms moved): topology stands; leaf bounds are
//!   recomputed from the live object bounds and unioned bottom-up. A full
//!   rebuild is still forced every [`BvhConfig::rebuild_after_refits`] refits,
//!   since drifted instances degrade a refit-only tree.
//! * **Skip** (nothing the pass reads changed): last frame's buffers stand.
//!   Static instances are never re-uploaded: this pass owns no per-instance
//!   upload at all; its inputs are SceneDB-mirrored buffers.
//!
//! Nothing is read back: every dispatch after the first is sized by indirect
//! arguments the GPU wrote from its own live count, and the tree header carries
//! ready-made indirect arguments for consumers.
//!
//! # Batch leaves
//!
//! After sorting, aligned runs of up to [`BvhConfig::batch_max_instances`]
//! (<= 64) consecutive items merge into ONE leaf when every item is tiny and
//! the run's combined box is no longer than [`BvhConfig::batch_cell_extent`]
//! on any axis, so a city block of bolts costs one node, not hundreds.
//!
//! # Buffer contract (what `helio-pass-vg-cull` consumes)
//!
//! Inputs, by [`BufferKey`] from the scene projection:
//! * [`keys::INSTANCES`] = `"vg_instances"`: `array<GpuVgInstance>`; a row is in
//!   the tree iff `flags & INST_ENABLED` and its `object_row` is a live
//!   `static_objects` row with finite bounds.
//! * `"static_objects"`: `array<StaticObjectRow>`.
//! * `"scene_objects_structure"` (optional): rebuild vs refit discriminator.
//!
//! Outputs, published to the graph registry as buffers (declared writes of this
//! pass, so consumers order after it):
//! * [`keys::NODES`] = `"vg_gcbvh_nodes"`: `array<VgBvhNode>` (32 B).
//! * [`keys::HEADER`] = `"vg_gcbvh_header"`: `VgBvhHeader` (128 B) incl. indirect args.
//! * [`keys::ITEMS`]  = `"vg_gcbvh_items"`: `array<u32>`, leaf item -> `vg_instances` row.
//!
//! Node and header layout: `shaders/vg_bvh_nodes.wgsl` ([`NODES_WGSL`]).

use std::num::NonZeroU64;

use bytemuck::{Pod, Zeroable};
use helio_core::{CommandRecorder, PassContext, PrepareContext, RenderPass, Result as HelioResult};
use helio_vg_format::GpuVgInstance;
use pulsar_scenedb::gpu::BufferKey;
use wgpu::util::DeviceExt;

pub mod gpu_types;
pub mod reference;
pub use gpu_types::*;

/// WGSL node/header layout. `include_str!` this ahead of consumer shaders.
pub const NODES_WGSL: &str = include_str!("../shaders/vg_bvh_nodes.wgsl");
/// WGSL of the build/refit kernels (needs [`NODES_WGSL`] prepended).
pub const BUILD_WGSL: &str = include_str!("../shaders/vg_bvh_build.wgsl");

/// Names of the buffers this pass reads and publishes.
pub mod keys {
    /// Input: `array<GpuVgInstance>` (scene buffer).
    pub const INSTANCES: &str = helio_mesh_api::VG_INSTANCES;
    /// Input: the scene join's object rows (scene buffer).
    pub const STATIC_OBJECTS: &str = "static_objects";
    /// Input: the scene join's structure marker, as in `helio-pass-object-batch`.
    pub const STRUCTURE: &str = "scene_objects_structure";
    /// Input: the scene join's moved-slot list (see crate docs).
    pub const MOVED: &str = "scene_objects_moved";
    /// Output: `array<VgBvhNode>`.
    pub const NODES: &str = "vg_gcbvh_nodes";
    /// Output: `VgBvhHeader`.
    pub const HEADER: &str = "vg_gcbvh_header";
    /// Output: `array<u32>`, leaf item -> `vg_instances` row.
    pub const ITEMS: &str = "vg_gcbvh_items";
}

const WG: u32 = 256;
const SORT_BITS: usize = reference::MORTON_BITS as usize;
const MIN_CAPACITY: u32 = 256;
const SORT_BIT_STRIDE: u64 = 256;
const NODE_BYTES: u64 = std::mem::size_of::<GpuBvhNode>() as u64;
const INSTANCE_BYTES: u64 = std::mem::size_of::<GpuVgInstance>() as u64;

/// Byte offsets of the 3-word dispatch-argument slots in the args buffer.
const ARGS_ITEMS: u64 = 0;
const ARGS_CHUNKS: u64 = 12;
const ARGS_LEAVES: u64 = 24;
const ARGS_INTERNAL: u64 = 36;
const ARGS_NODES: u64 = 48;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct BuildCfgGpu {
    capacity: u32,
    chunk_size: u32,
    tiny_extent: f32,
    cell_extent: f32,
}

/// What a frame does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BvhMode {
    /// Nothing the pass reads changed.
    Skip,
    /// Topology stands; bounds recomputed.
    Refit,
    /// Everything rebuilt.
    Rebuild,
}

// ── Kernel table ────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
enum Res {
    Cfg,
    SortBit,
    Instances,
    Objects,
    Header,
    SceneBounds,
    BlockTotals,
    LocalRank,
    KeysA,
    IdxA,
    SrcKeys,
    SrcIdx,
    DstKeys,
    DstIdx,
    BlockHist,
    ChunkLeaf,
    ChunkMerge,
    ChunkBase,
    Nodes,
    Parents,
    RefitBounds,
    Counters,
    Args,
}

struct Kernel {
    name: &'static str,
    entry: &'static str,
    /// `(resource, read_only)`; the position is the binding index.
    bindings: &'static [(Res, bool)],
}

use Res::*;

const K_GATHER_LOCAL: usize = 0;
const K_BLOCK_SCAN: usize = 1;
const K_GATHER_WRITE: usize = 2;
const K_HIST: usize = 3;
const K_SCAN: usize = 4;
const K_SCATTER: usize = 5;
const K_CHUNK_EVAL: usize = 6;
const K_CHUNK_SCAN: usize = 7;
const K_MAKE_LEAVES: usize = 8;
const K_KARRAS: usize = 9;
const K_REFIT: usize = 10;
const K_FINALIZE: usize = 11;

const KERNELS: [Kernel; 12] = [
    Kernel {
        name: "VgBvh GatherLocal",
        entry: "cs_gather_local",
        bindings: &[
            (Cfg, true),
            (Instances, true),
            (Objects, true),
            (SceneBounds, false),
            (LocalRank, false),
            (BlockTotals, false),
        ],
    },
    Kernel {
        name: "VgBvh BlockScan",
        entry: "cs_block_scan",
        bindings: &[
            (Cfg, true),
            (Header, false),
            (BlockTotals, false),
            (Args, false),
        ],
    },
    Kernel {
        name: "VgBvh GatherWrite",
        entry: "cs_gather_write",
        bindings: &[
            (Cfg, true),
            (Instances, true),
            (Objects, true),
            (SceneBounds, false),
            (LocalRank, false),
            (BlockTotals, false),
            (KeysA, false),
            (IdxA, false),
        ],
    },
    Kernel {
        name: "VgBvh Histogram",
        entry: "cs_histogram",
        bindings: &[
            (SortBit, true),
            (Header, true),
            (SrcKeys, true),
            (BlockHist, false),
        ],
    },
    Kernel {
        name: "VgBvh Scan",
        entry: "cs_scan",
        bindings: &[(Header, true), (BlockHist, false)],
    },
    Kernel {
        name: "VgBvh Scatter",
        entry: "cs_scatter",
        bindings: &[
            (SortBit, true),
            (Header, true),
            (SrcKeys, true),
            (SrcIdx, true),
            (DstKeys, false),
            (DstIdx, false),
            (BlockHist, true),
        ],
    },
    Kernel {
        name: "VgBvh ChunkEval",
        entry: "cs_chunk_eval",
        bindings: &[
            (Cfg, true),
            (Header, true),
            (Instances, true),
            (Objects, true),
            (IdxA, true),
            (ChunkLeaf, false),
            (ChunkMerge, false),
        ],
    },
    Kernel {
        name: "VgBvh ChunkScan",
        entry: "cs_chunk_scan",
        bindings: &[
            (Cfg, true),
            (Header, false),
            (ChunkLeaf, true),
            (ChunkMerge, true),
            (ChunkBase, false),
            (Args, false),
        ],
    },
    Kernel {
        name: "VgBvh MakeLeaves",
        entry: "cs_make_leaves",
        bindings: &[
            (Cfg, true),
            (Header, true),
            (ChunkMerge, true),
            (ChunkBase, true),
            (Nodes, false),
        ],
    },
    Kernel {
        name: "VgBvh Karras",
        entry: "cs_karras",
        bindings: &[
            (Header, true),
            (KeysA, true),
            (Nodes, false),
            (Parents, false),
        ],
    },
    Kernel {
        name: "VgBvh RefitLeaves",
        entry: "cs_refit_leaves",
        bindings: &[
            (Header, true),
            (Instances, true),
            (Objects, true),
            (IdxA, true),
            (Nodes, true),
            (Parents, true),
            (RefitBounds, false),
            (Counters, false),
        ],
    },
    Kernel {
        name: "VgBvh Finalize",
        entry: "cs_finalize",
        bindings: &[(Header, false), (Nodes, false), (RefitBounds, false)],
    },
];

fn storage_buffer(
    device: &wgpu::Device,
    label: &str,
    size: u64,
    extra: wgpu::BufferUsages,
) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size.max(16),
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_SRC
            | wgpu::BufferUsages::COPY_DST
            | extra,
        mapped_at_creation: false,
    })
}

/// Every buffer sized by instance capacity.
struct Scratch {
    capacity: u32,
    scene_bounds: wgpu::Buffer,
    block_totals: wgpu::Buffer,
    local_rank: wgpu::Buffer,
    keys_a: wgpu::Buffer,
    keys_b: wgpu::Buffer,
    idx_a: wgpu::Buffer,
    idx_b: wgpu::Buffer,
    block_hist: wgpu::Buffer,
    chunk_leaf: wgpu::Buffer,
    chunk_merge: wgpu::Buffer,
    chunk_base: wgpu::Buffer,
    nodes: wgpu::Buffer,
    parents: wgpu::Buffer,
    refit_bounds: wgpu::Buffer,
    counters: wgpu::Buffer,
    args: wgpu::Buffer,
}

impl Scratch {
    fn new(device: &wgpu::Device, capacity: u32) -> Self {
        let n = capacity as u64;
        let blocks = capacity.div_ceil(WG) as u64;
        let none = wgpu::BufferUsages::empty();
        Self {
            capacity,
            scene_bounds: storage_buffer(device, "VgBvh SceneBounds", 32, none),
            block_totals: storage_buffer(device, "VgBvh BlockTotals", blocks * 4, none),
            local_rank: storage_buffer(device, "VgBvh LocalRank", n * 4, none),
            keys_a: storage_buffer(device, "VgBvh KeysA", n * 4, none),
            keys_b: storage_buffer(device, "VgBvh KeysB", n * 4, none),
            idx_a: storage_buffer(device, "VgBvh Items", n * 4, none),
            idx_b: storage_buffer(device, "VgBvh IdxB", n * 4, none),
            block_hist: storage_buffer(device, "VgBvh BlockHist", blocks * 8, none),
            chunk_leaf: storage_buffer(device, "VgBvh ChunkLeaf", n * 4, none),
            chunk_merge: storage_buffer(device, "VgBvh ChunkMerge", n * 4, none),
            chunk_base: storage_buffer(device, "VgBvh ChunkBase", n * 4, none),
            nodes: storage_buffer(device, "VgBvh Nodes", 2 * n * NODE_BYTES, none),
            parents: storage_buffer(device, "VgBvh Parents", 2 * n * 4, none),
            refit_bounds: storage_buffer(device, "VgBvh RefitBounds", 2 * n * 32, none),
            counters: storage_buffer(device, "VgBvh Counters", n * 4, none),
            args: storage_buffer(device, "VgBvh Args", 64, wgpu::BufferUsages::INDIRECT),
        }
    }
}

/// Bind groups for one (scratch, instances, objects) combination.
struct Binds {
    /// One per kernel; the sort kernels hold the even-parity group here.
    groups: Vec<wgpu::BindGroup>,
    /// Odd-parity (B -> A) groups of the histogram and scatter kernels.
    hist_odd: wgpu::BindGroup,
    scatter_odd: wgpu::BindGroup,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Inputs {
    instances: (u64, u64),
    objects: (u64, u64),
    structure: Option<(u64, u64)>,
    rows: u32,
}

pub struct VgBvhPass {
    cfg: BvhConfig,
    bgls: Vec<wgpu::BindGroupLayout>,
    pipelines: Vec<wgpu::ComputePipeline>,
    cfg_uniform: wgpu::Buffer,
    sort_bits: wgpu::Buffer,
    header: wgpu::Buffer,
    scratch: Scratch,
    binds: Option<Binds>,
    bind_key: Option<(u64, wgpu::Buffer, wgpu::Buffer)>,
    scratch_generation: u64,
    rows: u32,

    last: Option<Inputs>,
    refits_since_build: u32,
    mode: BvhMode,
}

impl VgBvhPass {
    pub fn new(device: &wgpu::Device) -> Self {
        Self::with_config(device, BvhConfig::default())
    }

    pub fn with_config(device: &wgpu::Device, cfg: BvhConfig) -> Self {
        let source = format!("{NODES_WGSL}\n{BUILD_WGSL}");
        let module = helio_core::shader::module(device, "VgBvh Shader", source.as_str());
        let cs = wgpu::ShaderStages::COMPUTE;

        let mut bgls = Vec::new();
        let mut pipelines = Vec::new();
        for kernel in &KERNELS {
            let entries: Vec<wgpu::BindGroupLayoutEntry> = kernel
                .bindings
                .iter()
                .enumerate()
                .map(|(i, (res, read_only))| wgpu::BindGroupLayoutEntry {
                    binding: i as u32,
                    visibility: cs,
                    ty: match res {
                        Cfg => wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        SortBit => wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: true,
                            min_binding_size: None,
                        },
                        _ => wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage {
                                read_only: *read_only,
                            },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                    },
                    count: None,
                })
                .collect();
            let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(kernel.name),
                entries: &entries,
            });
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(kernel.name),
                bind_group_layouts: &[Some(&bgl)],
                immediate_size: 0,
            });
            pipelines.push(helio_core::shader::create_compute_pipeline(
                device,
                wgpu::ComputePipelineDescriptor {
                    label: Some(kernel.name),
                    layout: Some(&layout),
                    module: &module,
                    entry_point: Some(kernel.entry),
                    compilation_options: Default::default(),
                    cache: None,
                },
            ));
            bgls.push(bgl);
        }

        // Bit index of each sort pass, one 256-byte-aligned slot per pass.
        let mut bits = vec![0u8; SORT_BITS * SORT_BIT_STRIDE as usize];
        for i in 0..SORT_BITS {
            bits[i * SORT_BIT_STRIDE as usize..][..4].copy_from_slice(&(i as u32).to_le_bytes());
        }
        let sort_bits = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("VgBvh SortBits"),
            contents: &bits,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let cfg_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("VgBvh Cfg"),
            size: std::mem::size_of::<BuildCfgGpu>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let header = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("VgBvh Header"),
            contents: bytemuck::bytes_of(&GpuBvhHeader::empty()),
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::INDIRECT
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
        });

        Self {
            cfg,
            bgls,
            pipelines,
            cfg_uniform,
            sort_bits,
            header,
            scratch: Scratch::new(device, MIN_CAPACITY),
            binds: None,
            bind_key: None,
            scratch_generation: 0,
            rows: 0,
            last: None,
            refits_since_build: 0,
            mode: BvhMode::Skip,
        }
    }

    /// Replaces the tunables; the next frame rebuilds.
    pub fn set_config(&mut self, cfg: BvhConfig) {
        if cfg != self.cfg {
            self.cfg = cfg;
            self.last = None;
        }
    }

    pub fn config(&self) -> BvhConfig {
        self.cfg
    }

    /// `array<VgBvhNode>`; valid prefix is `header.node_count`.
    pub fn nodes_buffer(&self) -> &wgpu::Buffer {
        &self.scratch.nodes
    }
    /// `VgBvhHeader`.
    pub fn header_buffer(&self) -> &wgpu::Buffer {
        &self.header
    }
    /// `array<u32>`; valid prefix is `header.instance_count`.
    pub fn items_buffer(&self) -> &wgpu::Buffer {
        &self.scratch.idx_a
    }
    /// Bytes of every buffer sized by instance capacity (the tree, sort scratch).
    pub fn scratch_bytes(&self) -> u64 {
        let s = &self.scratch;
        [
            &s.scene_bounds,
            &s.block_totals,
            &s.local_rank,
            &s.keys_a,
            &s.keys_b,
            &s.idx_a,
            &s.idx_b,
            &s.block_hist,
            &s.chunk_leaf,
            &s.chunk_merge,
            &s.chunk_base,
            &s.nodes,
            &s.parents,
            &s.refit_bounds,
            &s.counters,
            &s.args,
        ]
        .iter()
        .map(|b| b.size())
        .sum()
    }

    /// What the last `prepare` decided.
    pub fn mode(&self) -> BvhMode {
        self.mode
    }

    fn ensure_capacity(&mut self, device: &wgpu::Device, rows: u32) -> bool {
        if rows <= self.scratch.capacity {
            return false;
        }
        self.scratch = Scratch::new(device, rows.next_power_of_two().max(MIN_CAPACITY));
        self.scratch_generation += 1;
        true
    }

    fn buffer<'a>(
        &'a self,
        res: Res,
        instances: &'a wgpu::Buffer,
        objects: &'a wgpu::Buffer,
        odd: bool,
    ) -> wgpu::BindingResource<'a> {
        let s = &self.scratch;
        let (src_keys, dst_keys) = if odd {
            (&s.keys_b, &s.keys_a)
        } else {
            (&s.keys_a, &s.keys_b)
        };
        let (src_idx, dst_idx) = if odd {
            (&s.idx_b, &s.idx_a)
        } else {
            (&s.idx_a, &s.idx_b)
        };
        let buf = match res {
            Cfg => &self.cfg_uniform,
            SortBit => {
                return wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &self.sort_bits,
                    offset: 0,
                    size: NonZeroU64::new(16),
                });
            }
            Instances => instances,
            Objects => objects,
            Header => &self.header,
            SceneBounds => &s.scene_bounds,
            BlockTotals => &s.block_totals,
            LocalRank => &s.local_rank,
            KeysA => &s.keys_a,
            IdxA => &s.idx_a,
            SrcKeys => src_keys,
            SrcIdx => src_idx,
            DstKeys => dst_keys,
            DstIdx => dst_idx,
            BlockHist => &s.block_hist,
            ChunkLeaf => &s.chunk_leaf,
            ChunkMerge => &s.chunk_merge,
            ChunkBase => &s.chunk_base,
            Nodes => &s.nodes,
            Parents => &s.parents,
            RefitBounds => &s.refit_bounds,
            Counters => &s.counters,
            Args => &s.args,
        };
        buf.as_entire_binding()
    }

    fn make_group(
        &self,
        device: &wgpu::Device,
        kernel: usize,
        instances: &wgpu::Buffer,
        objects: &wgpu::Buffer,
        odd: bool,
    ) -> wgpu::BindGroup {
        let k = &KERNELS[kernel];
        let entries: Vec<wgpu::BindGroupEntry> = k
            .bindings
            .iter()
            .enumerate()
            .map(|(i, (res, _))| wgpu::BindGroupEntry {
                binding: i as u32,
                resource: self.buffer(*res, instances, objects, odd),
            })
            .collect();
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(k.name),
            layout: &self.bgls[kernel],
            entries: &entries,
        })
    }

    fn rebuild_binds(
        &mut self,
        device: &wgpu::Device,
        instances: &wgpu::Buffer,
        objects: &wgpu::Buffer,
    ) {
        let groups = (0..KERNELS.len())
            .map(|k| self.make_group(device, k, instances, objects, false))
            .collect();
        let binds = Binds {
            groups,
            hist_odd: self.make_group(device, K_HIST, instances, objects, true),
            scatter_odd: self.make_group(device, K_SCATTER, instances, objects, true),
        };
        self.binds = Some(binds);
    }

    fn cfg_gpu(&self) -> BuildCfgGpu {
        BuildCfgGpu {
            capacity: self.rows,
            chunk_size: self.cfg.chunk_size(),
            tiny_extent: self.cfg.tiny_extent,
            cell_extent: self.cfg.batch_cell_extent,
        }
    }

    /// Chooses what this frame does and makes the bind groups current. Returns
    /// the cfg to upload.
    fn plan(
        &mut self,
        device: &wgpu::Device,
        instances: &wgpu::Buffer,
        objects: &wgpu::Buffer,
        inputs: Inputs,
    ) -> BvhMode {
        self.rows = inputs.rows;
        let grew = self.ensure_capacity(device, inputs.rows);
        let key_changed = match &self.bind_key {
            Some((gen, i, o)) => *gen != self.scratch_generation || i != instances || o != objects,
            None => true,
        };
        if grew || key_changed || self.binds.is_none() {
            self.rebuild_binds(device, instances, objects);
            self.bind_key = Some((self.scratch_generation, instances.clone(), objects.clone()));
        }
        let mode = match self.last {
            None => BvhMode::Rebuild,
            Some(_) if grew => BvhMode::Rebuild,
            Some(last) if last == inputs => BvhMode::Skip,
            Some(last) => {
                let structural = inputs.rows != last.rows
                    || inputs.instances != last.instances
                    || inputs.objects.0 != last.objects.0
                    || inputs.structure.is_none()
                    || inputs.structure != last.structure;
                if structural {
                    BvhMode::Rebuild
                } else {
                    BvhMode::Refit
                }
            }
        };
        let mode = if mode == BvhMode::Refit
            && self.cfg.rebuild_after_refits != 0
            && self.refits_since_build >= self.cfg.rebuild_after_refits
        {
            BvhMode::Rebuild
        } else {
            mode
        };
        match mode {
            BvhMode::Rebuild => self.refits_since_build = 0,
            BvhMode::Refit => self.refits_since_build += 1,
            BvhMode::Skip => {}
        }
        self.last = Some(inputs);
        self.mode = mode;
        mode
    }

    fn dispatch(
        &self,
        cmds: &mut CommandRecorder<'_>,
        kernel: usize,
        group: &wgpu::BindGroup,
        offsets: &[u32],
        how: Dispatch,
    ) {
        let mut pass = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some(KERNELS[kernel].name),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipelines[kernel]);
        pass.set_bind_group(0, group, offsets);
        match how {
            Dispatch::Direct(n) => pass.dispatch_workgroups(n, 1, 1),
            Dispatch::Indirect(offset) => {
                pass.dispatch_workgroups_indirect(&self.scratch.args, offset)
            }
        }
    }

    fn record(&self, cmds: &mut CommandRecorder<'_>, mode: BvhMode) {
        let Some(binds) = self.binds.as_ref() else {
            return;
        };
        let g = &binds.groups;
        if mode == BvhMode::Rebuild {
            let blocks = self.rows.div_ceil(WG).max(1);
            cmds.clear_buffer(&self.scratch.scene_bounds, 0, None);
            self.dispatch(
                cmds,
                K_GATHER_LOCAL,
                &g[K_GATHER_LOCAL],
                &[],
                Dispatch::Direct(blocks),
            );
            self.dispatch(
                cmds,
                K_BLOCK_SCAN,
                &g[K_BLOCK_SCAN],
                &[],
                Dispatch::Direct(1),
            );
            self.dispatch(
                cmds,
                K_GATHER_WRITE,
                &g[K_GATHER_WRITE],
                &[],
                Dispatch::Direct(blocks),
            );
            for bit in 0..SORT_BITS {
                let odd = bit % 2 == 1;
                let off = [(bit as u64 * SORT_BIT_STRIDE) as u32];
                let hist = if odd { &binds.hist_odd } else { &g[K_HIST] };
                let scatter = if odd {
                    &binds.scatter_odd
                } else {
                    &g[K_SCATTER]
                };
                self.dispatch(cmds, K_HIST, hist, &off, Dispatch::Indirect(ARGS_ITEMS));
                self.dispatch(cmds, K_SCAN, &g[K_SCAN], &[], Dispatch::Direct(1));
                self.dispatch(
                    cmds,
                    K_SCATTER,
                    scatter,
                    &off,
                    Dispatch::Indirect(ARGS_ITEMS),
                );
            }
            self.dispatch(
                cmds,
                K_CHUNK_EVAL,
                &g[K_CHUNK_EVAL],
                &[],
                Dispatch::Indirect(ARGS_CHUNKS),
            );
            self.dispatch(
                cmds,
                K_CHUNK_SCAN,
                &g[K_CHUNK_SCAN],
                &[],
                Dispatch::Direct(1),
            );
            self.dispatch(
                cmds,
                K_MAKE_LEAVES,
                &g[K_MAKE_LEAVES],
                &[],
                Dispatch::Indirect(ARGS_CHUNKS),
            );
            self.dispatch(
                cmds,
                K_KARRAS,
                &g[K_KARRAS],
                &[],
                Dispatch::Indirect(ARGS_INTERNAL),
            );
        }
        cmds.clear_buffer(&self.scratch.counters, 0, None);
        self.dispatch(
            cmds,
            K_REFIT,
            &g[K_REFIT],
            &[],
            Dispatch::Indirect(ARGS_LEAVES),
        );
        self.dispatch(
            cmds,
            K_FINALIZE,
            &g[K_FINALIZE],
            &[],
            Dispatch::Indirect(ARGS_NODES),
        );
    }

    /// Runs one frame outside a render graph, blocking until the GPU is done:
    /// for tests. `instances` is an `array<GpuVgInstance>`, `objects` an
    /// `array<StaticObjectRow>`.
    pub fn run_for_testing(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        instances: &wgpu::Buffer,
        objects: &wgpu::Buffer,
        mode: BvhMode,
    ) {
        let rows = (instances.size() / INSTANCE_BYTES) as u32;
        let grew = self.ensure_capacity(device, rows);
        self.rows = rows;
        if grew
            || self.binds.is_none()
            || self.bind_key.as_ref().map(|k| (&k.1, &k.2)) != Some((instances, objects))
        {
            self.rebuild_binds(device, instances, objects);
            self.bind_key = Some((self.scratch_generation, instances.clone(), objects.clone()));
        }
        // A grown scratch has no topology to refit.
        let mode = if grew { BvhMode::Rebuild } else { mode };
        self.mode = mode;
        queue.write_buffer(&self.cfg_uniform, 0, bytemuck::bytes_of(&self.cfg_gpu()));
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("VgBvh Test Encoder"),
        });
        self.record(&mut CommandRecorder::from_encoder(&mut encoder), mode);
        queue.submit([encoder.finish()]);
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
    }
}

#[derive(Clone, Copy)]
enum Dispatch {
    Direct(u32),
    Indirect(u64),
}

impl RenderPass for VgBvhPass {
    fn name(&self) -> &'static str {
        "VgBvh"
    }

    fn render_pass_descriptor<'a>(
        &'a self,
        _target: &'a wgpu::TextureView,
        _depth: &'a wgpu::TextureView,
        _resources: &'a helio_core::ResourceRegistry<'a>,
    ) -> Option<wgpu::RenderPassDescriptor<'a>> {
        None // Compute only.
    }

    fn declare_resources(&self, builder: &mut helio_core::graph::ResourceBuilder) {
        builder.write_buffer(keys::NODES);
        builder.write_buffer(keys::HEADER);
        builder.write_buffer(keys::ITEMS);
    }

    fn publish<'a>(&self, frame: &mut helio_core::ResourceRegistry<'a>) {
        // SAFETY: the buffers are owned by this pass, which the graph keeps
        // alive across frames; `publish(&self, ..)` cannot express that to the
        // borrow checker (same reasoning as `helio-pass-object-batch`).
        let nodes: &'a wgpu::Buffer = unsafe { std::mem::transmute(&self.scratch.nodes) };
        let header: &'a wgpu::Buffer = unsafe { std::mem::transmute(&self.header) };
        let items: &'a wgpu::Buffer = unsafe { std::mem::transmute(&self.scratch.idx_a) };
        frame.write_buffer(helio_core::ResourceKey::new(keys::NODES), nodes, "VgBvh");
        frame.write_buffer(helio_core::ResourceKey::new(keys::HEADER), header, "VgBvh");
        frame.write_buffer(helio_core::ResourceKey::new(keys::ITEMS), items, "VgBvh");
    }

    fn prepare(&mut self, ctx: &PrepareContext) -> HelioResult<()> {
        let instances = ctx.scene_buffers.get(BufferKey::of(keys::INSTANCES));
        let objects = ctx.scene_buffers.get(BufferKey::of(keys::STATIC_OBJECTS));
        let (Some(instances), Some(objects)) = (instances, objects) else {
            self.mode = BvhMode::Skip;
            return Ok(());
        };
        let inputs = Inputs {
            instances: (instances.epoch, instances.content_generation),
            objects: (objects.epoch, objects.content_generation),
            structure: ctx
                .scene_buffers
                .get(BufferKey::of(keys::STRUCTURE))
                .map(|h| (h.epoch, h.content_generation)),
            rows: (instances.buffer.size() / INSTANCE_BYTES) as u32,
        };
        let mode = self.plan(ctx.device, &instances.buffer, &objects.buffer, inputs);
        if mode != BvhMode::Skip {
            ctx.write_buffer(&self.cfg_uniform, 0, bytemuck::bytes_of(&self.cfg_gpu()));
        }
        Ok(())
    }

    fn execute(&mut self, ctx: &mut PassContext) -> HelioResult<()> {
        if self.mode == BvhMode::Skip {
            return Ok(());
        }
        self.record(&mut ctx.graphics_cmds(), self.mode);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirrors_have_the_wgsl_sizes() {
        assert_eq!(std::mem::size_of::<GpuBvhNode>(), 32);
        assert_eq!(std::mem::size_of::<GpuBvhHeader>(), 128);
        assert_eq!(std::mem::size_of::<BuildCfgGpu>(), 16);
        assert_eq!(std::mem::size_of::<helio_mesh_api::StaticObjectRow>(), 252);
        assert_eq!(header_offsets::DISPATCH_LEAVES, 16 * 4);
        assert_eq!(header_offsets::DISPATCH_INSTANCES, 19 * 4);
        assert_eq!(header_offsets::DISPATCH_NODES, 22 * 4);
    }
}
