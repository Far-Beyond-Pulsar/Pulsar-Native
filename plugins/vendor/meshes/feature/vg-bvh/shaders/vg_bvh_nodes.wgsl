// Global instance BVH (GCBVH top level) -- node layout shared with the cull pass.
//
// `helio-pass-vg-cull` `include_str!`s this file ahead of its own source and
// declares its own bindings, e.g.
//
//     @group(G) @binding(B) var<storage, read> vg_gcbvh_nodes:  array<VgBvhNode>;
//     @group(G) @binding(B) var<storage, read> vg_gcbvh_header: VgBvhHeader;
//     @group(G) @binding(B) var<storage, read> vg_gcbvh_items:  array<u32>;
//     @group(G) @binding(B) var<storage, read> vg_instances:    array<VgInstance>; // helio-vg-format
//
// Tree shape: BINARY, AABBs in WORLD space.
//
//   * `leaf_count` = M leaves, `M - 1` internal nodes, `node_count` = 2M - 1.
//   * Internal nodes occupy indices [0, M-1); the ROOT is node 0 (for M == 1
//     the single leaf is the root and also sits at index 0).
//     Leaf nodes occupy [M-1, 2M-1).
//   * Internal node:  `a` = left child node index, `b` = right child node
//     index (both < VG_BVH_LEAF_BIT, both are indices into the node array).
//   * Leaf node:      `a` = VG_BVH_LEAF_BIT | item_count   (1..=64)
//                     `b` = first item (index into `vg_gcbvh_items`)
//     The leaf covers items `vg_gcbvh_items[b .. b + item_count)`; each item is
//     a ROW INDEX into the `vg_instances` buffer (so the instance's
//     `object_row` / `asset_slot` / flags are `vg_instances[item]`).
//     A leaf with item_count == 1 is a plain instance leaf; item_count > 1 is a
//     BATCH leaf holding several tiny, spatially adjacent instances. The leaf's
//     AABB bounds all of its items; a traversal that passes the leaf's tests
//     then iterates its items and tests each against its own bounds
//     (`static_objects[object_row].bounds` = world bounding sphere).
//   * An empty tree has `leaf_count == 0`, `node_count == 0` and
//     `root_node == VG_BVH_INVALID`.
//
// Dispatch sizing without CPU readback: `VgBvhHeader` carries ready-made
// indirect-dispatch arguments (64 threads per workgroup) at fixed offsets.
//
// Everything is plain 4-byte scalars/vec3+u32 so the Rust mirrors in
// `gpu_types.rs` (`GpuBvhNode` 32 B, `GpuBvhHeader` 128 B) are byte-identical.

const VG_BVH_LEAF_BIT: u32 = 0x80000000u;
const VG_BVH_INVALID: u32 = 0xffffffffu;
const VG_BVH_MAX_LEAF_ITEMS: u32 = 64u;

struct VgBvhNode {
    bmin: vec3<f32>,
    a: u32,
    bmax: vec3<f32>,
    b: u32,
}

struct VgBvhHeader {
    root_node: u32,
    node_count: u32,
    leaf_count: u32,
    /// Live instances in the tree (items in `vg_gcbvh_items`).
    instance_count: u32,
    /// Bumped by every full rebuild.
    build_generation: u32,
    /// Bumped by every rebuild or refit (anything that moved a bound).
    refit_generation: u32,
    /// Leaves holding more than one instance.
    batch_leaf_count: u32,
    internal_count: u32,
    /// Root bounds (all live instances).
    bounds_min: array<f32, 3>,
    _pad0: u32,
    bounds_max: array<f32, 3>,
    _pad1: u32,
    /// Indirect args: ceil(leaf_count / 64), 1, 1.
    dispatch_leaves: array<u32, 3>,
    /// Indirect args: ceil(instance_count / 64), 1, 1.
    dispatch_instances: array<u32, 3>,
    /// Indirect args: ceil(node_count / 64), 1, 1 (x may be 0 when empty).
    dispatch_nodes: array<u32, 3>,
    _pad2: array<u32, 7>,
}

fn vg_bvh_is_leaf(n: VgBvhNode) -> bool {
    return (n.a & VG_BVH_LEAF_BIT) != 0u;
}

/// Items in a leaf (1..=64). Only meaningful for leaf nodes.
fn vg_bvh_leaf_count(n: VgBvhNode) -> u32 {
    return n.a & 0xffu;
}

/// First index into `vg_gcbvh_items`. Only meaningful for leaf nodes.
fn vg_bvh_leaf_first(n: VgBvhNode) -> u32 {
    return n.b;
}

/// Child node index of an INTERNAL node: `i == 0` left, otherwise right.
fn vg_bvh_child(n: VgBvhNode, i: u32) -> u32 {
    return select(n.a, n.b, i != 0u);
}
