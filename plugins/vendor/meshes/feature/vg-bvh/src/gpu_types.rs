//! Rust mirrors of `shaders/vg_bvh_nodes.wgsl`. Layouts are pinned by tests.

use bytemuck::{Pod, Zeroable};

/// Set in a node's `a` word when it is a leaf.
pub const NODE_LEAF_BIT: u32 = 0x8000_0000;
/// `root_node` of an empty tree.
pub const NODE_INVALID: u32 = 0xffff_ffff;
/// Most instances one batch leaf may hold.
pub const MAX_LEAF_ITEMS: u32 = 64;

/// One BVH node. 32 bytes. See `vg_bvh_nodes.wgsl` for the encoding.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct GpuBvhNode {
    pub min: [f32; 3],
    pub a: u32,
    pub max: [f32; 3],
    pub b: u32,
}

impl GpuBvhNode {
    pub fn is_leaf(&self) -> bool {
        self.a & NODE_LEAF_BIT != 0
    }
    /// Items in a leaf (1..=64).
    pub fn leaf_count(&self) -> u32 {
        self.a & 0xff
    }
    /// First index into the items buffer (leaf only).
    pub fn leaf_first(&self) -> u32 {
        self.b
    }
    /// `[left, right]` node indices (internal only).
    pub fn children(&self) -> [u32; 2] {
        [self.a, self.b]
    }
}

/// Tree summary plus ready-made indirect-dispatch arguments. 128 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct GpuBvhHeader {
    pub root_node: u32,
    pub node_count: u32,
    pub leaf_count: u32,
    pub instance_count: u32,
    pub build_generation: u32,
    pub refit_generation: u32,
    pub batch_leaf_count: u32,
    pub internal_count: u32,
    pub bounds_min: [f32; 3],
    pub _pad0: u32,
    pub bounds_max: [f32; 3],
    pub _pad1: u32,
    pub dispatch_leaves: [u32; 3],
    pub dispatch_instances: [u32; 3],
    pub dispatch_nodes: [u32; 3],
    pub _pad2: [u32; 7],
}

impl GpuBvhHeader {
    /// What the header holds before the first build.
    pub fn empty() -> Self {
        Self {
            root_node: NODE_INVALID,
            ..Self::zeroed()
        }
    }
}

/// Byte offsets into the header buffer usable as indirect-dispatch sources.
pub mod header_offsets {
    /// `ceil(leaf_count / 64), 1, 1`
    pub const DISPATCH_LEAVES: u64 = 64;
    /// `ceil(instance_count / 64), 1, 1`
    pub const DISPATCH_INSTANCES: u64 = 76;
    /// `ceil(node_count / 64), 1, 1`
    pub const DISPATCH_NODES: u64 = 88;
}

/// Tunables of the build.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BvhConfig {
    /// Most instances merged into one batch leaf, 1..=64. `1` disables
    /// batching (every instance is its own leaf).
    pub batch_max_instances: u32,
    /// An instance is "tiny" (batchable) when the side of its bounding box is
    /// at most this (world units).
    pub tiny_extent: f32,
    /// A batch's combined bounding box may not be longer than this on any
    /// axis (world units): the "shares a cell" test.
    pub batch_cell_extent: f32,
    /// A full rebuild is forced after this many refits in a row, because
    /// refitting keeps the build-time topology and the tree degrades as
    /// instances drift. `0` never forces one.
    pub rebuild_after_refits: u32,
}

impl Default for BvhConfig {
    fn default() -> Self {
        Self {
            batch_max_instances: 64,
            tiny_extent: 0.5,
            batch_cell_extent: 8.0,
            rebuild_after_refits: 240,
        }
    }
}

impl BvhConfig {
    pub fn chunk_size(&self) -> u32 {
        self.batch_max_instances.clamp(1, MAX_LEAF_ITEMS)
    }
}
