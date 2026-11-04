//! CPU reference of the GPU build: the same algorithm, step for step.
//!
//! Used by the unit tests (tree validity) and the headless GPU test (GPU output
//! compared against this). Every float expression mirrors
//! `shaders/vg_bvh_build.wgsl` so the two agree bit for bit on ordinary scenes.

use crate::gpu_types::*;
use helio_pass_gbuffer::StaticObjectRow;
use helio_vg_format::{GpuVgInstance, INST_ENABLED};

const BIG: f32 = 1.0e30;
/// Morton keys are 30 bits (10 per axis).
pub const MORTON_BITS: u32 = 30;

/// The output of a build: what the GPU leaves in its three buffers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RefTree {
    /// `vg_gcbvh_items`: instance rows in Morton order.
    pub items: Vec<u32>,
    /// Morton key of each item.
    pub keys: Vec<u32>,
    pub nodes: Vec<GpuBvhNode>,
    pub leaf_count: u32,
    pub batch_leaf_count: u32,
}

/// World bounding sphere of an instance (centre, radius) if it belongs in the
/// tree: enabled, object row alive, finite.
pub fn instance_sphere(
    inst: &GpuVgInstance,
    objects: &[StaticObjectRow],
) -> Option<([f32; 3], f32)> {
    if inst.flags & INST_ENABLED == 0 {
        return None;
    }
    let obj = objects.get(inst.object_row as usize)?;
    let b = obj.bounds;
    let ok = obj.mesh_generation != 0
        && b[0].abs() < BIG
        && b[1].abs() < BIG
        && b[2].abs() < BIG
        && b[3] >= 0.0
        && b[3] < BIG;
    ok.then_some(([b[0], b[1], b[2]], b[3]))
}

pub fn expand_bits(v: u32) -> u32 {
    let mut x = v & 0x3ff;
    x = (x | (x << 16)) & 0x0300_00ff;
    x = (x | (x << 8)) & 0x0300_f00f;
    x = (x | (x << 4)) & 0x030c_30c3;
    x = (x | (x << 2)) & 0x0924_9249;
    x
}

fn morton(c: [f32; 3], bmin: [f32; 3], bmax: [f32; 3]) -> u32 {
    let mut q = [0u32; 3];
    for k in 0..3 {
        let ext = (bmax[k] - bmin[k]).max(1.0e-20);
        let n = ((c[k] - bmin[k]) / ext).clamp(0.0, 1.0);
        q[k] = ((n * 1024.0) as u32).min(1023);
    }
    (expand_bits(q[0]) << 2) | (expand_bits(q[1]) << 1) | expand_bits(q[2])
}

/// Full build.
pub fn build(instances: &[GpuVgInstance], objects: &[StaticObjectRow], cfg: &BvhConfig) -> RefTree {
    // Compaction in row order (the GPU's deterministic scan gives the same).
    let mut rows = Vec::new();
    let mut spheres = Vec::new();
    for (i, inst) in instances.iter().enumerate() {
        if let Some(s) = instance_sphere(inst, objects) {
            rows.push(i as u32);
            spheres.push(s);
        }
    }
    let count = rows.len();
    if count == 0 {
        return RefTree::default();
    }
    let mut bmin = [f32::INFINITY; 3];
    let mut bmax = [f32::NEG_INFINITY; 3];
    for (c, _) in &spheres {
        for k in 0..3 {
            bmin[k] = bmin[k].min(c[k]);
            bmax[k] = bmax[k].max(c[k]);
        }
    }
    let mut order: Vec<usize> = (0..count).collect();
    let keys_unsorted: Vec<u32> = spheres
        .iter()
        .map(|(c, _)| morton(*c, bmin, bmax))
        .collect();
    // Stable, like the LSD radix sort.
    order.sort_by_key(|&p| keys_unsorted[p]);
    let items: Vec<u32> = order.iter().map(|&p| rows[p]).collect();
    let keys: Vec<u32> = order.iter().map(|&p| keys_unsorted[p]).collect();
    let sorted_spheres: Vec<([f32; 3], f32)> = order.iter().map(|&p| spheres[p]).collect();

    // Chunks -> leaves.
    let cs = cfg.chunk_size() as usize;
    let mut leaves: Vec<(u32, u32)> = Vec::new(); // (first, count)
    let mut batch_leaves = 0u32;
    for c in 0..count.div_ceil(cs) {
        let start = c * cs;
        let n = cs.min(count - start);
        let mut mn = [BIG; 3];
        let mut mx = [-BIG; 3];
        let mut tiny = true;
        for &(ctr, r) in &sorted_spheres[start..start + n] {
            for k in 0..3 {
                mn[k] = mn[k].min(ctr[k] - r);
                mx[k] = mx[k].max(ctr[k] + r);
            }
            tiny &= 2.0 * r <= cfg.tiny_extent;
        }
        let longest = (0..3)
            .map(|k| mx[k] - mn[k])
            .fold(f32::NEG_INFINITY, f32::max);
        let merge = cs >= 2 && n >= 2 && tiny && longest <= cfg.batch_cell_extent;
        if merge {
            leaves.push((start as u32, n as u32));
            batch_leaves += 1;
        } else {
            for t in 0..n {
                leaves.push(((start + t) as u32, 1));
            }
        }
    }
    let m = leaves.len();
    let mut nodes = vec![GpuBvhNode::default(); 2 * m - 1];
    for (l, &(first, n)) in leaves.iter().enumerate() {
        nodes[m - 1 + l].a = NODE_LEAF_BIT | n;
        nodes[m - 1 + l].b = first;
    }

    // Karras.
    let leaf_key = |l: i32| keys[leaves[l as usize].0 as usize];
    let delta = |i: i32, j: i32| -> i32 {
        if j < 0 || j >= m as i32 {
            return -1;
        }
        let (ki, kj) = (leaf_key(i), leaf_key(j));
        if ki == kj {
            return 32 + (i as u32 ^ j as u32).leading_zeros() as i32;
        }
        (ki ^ kj).leading_zeros() as i32
    };
    for ii in 0..m.saturating_sub(1) {
        let i = ii as i32;
        let d = if delta(i, i + 1) > delta(i, i - 1) {
            1
        } else {
            -1
        };
        let dmin = delta(i, i - d);
        let mut lmax = 2;
        while delta(i, i + lmax * d) > dmin {
            lmax *= 2;
        }
        let mut l = 0;
        let mut t = lmax / 2;
        while t >= 1 {
            if delta(i, i + (l + t) * d) > dmin {
                l += t;
            }
            t /= 2;
        }
        let j = i + l * d;
        let dnode = delta(i, j);
        let mut s = 0;
        let mut t2 = l;
        loop {
            t2 = (t2 + 1) / 2;
            if delta(i, i + (s + t2) * d) > dnode {
                s += t2;
            }
            if t2 <= 1 {
                break;
            }
        }
        let gamma = i + s * d + d.min(0);
        let (lo, hi) = (i.min(j), i.max(j));
        let left = if lo == gamma {
            (m - 1) as u32 + gamma as u32
        } else {
            gamma as u32
        };
        let right = if hi == gamma + 1 {
            (m - 1) as u32 + (gamma + 1) as u32
        } else {
            (gamma + 1) as u32
        };
        nodes[ii].a = left;
        nodes[ii].b = right;
    }

    let mut tree = RefTree {
        items,
        keys,
        nodes,
        leaf_count: m as u32,
        batch_leaf_count: batch_leaves,
    };
    refit(&mut tree, instances, objects);
    tree
}

/// Recompute every bound from the current object bounds, keeping the topology.
pub fn refit(tree: &mut RefTree, instances: &[GpuVgInstance], objects: &[StaticObjectRow]) {
    if tree.nodes.is_empty() {
        return;
    }
    let m = tree.leaf_count as usize;
    for l in 0..m {
        let n = &mut tree.nodes[m - 1 + l];
        let (first, cnt) = (n.b as usize, n.leaf_count() as usize);
        let mut mn = [BIG; 3];
        let mut mx = [-BIG; 3];
        for &row in &tree.items[first..first + cnt] {
            if let Some((c, r)) = instance_sphere(&instances[row as usize], objects) {
                for k in 0..3 {
                    mn[k] = mn[k].min(c[k] - r);
                    mx[k] = mx[k].max(c[k] + r);
                }
            }
        }
        n.min = mn;
        n.max = mx;
    }
    // Post-order over the internal nodes (min/max are order independent).
    fn union(nodes: &mut [GpuBvhNode], i: usize, m: usize) -> ([f32; 3], [f32; 3]) {
        if i >= m - 1 {
            return (nodes[i].min, nodes[i].max);
        }
        let [l, r] = nodes[i].children();
        let (lmn, lmx) = union(nodes, l as usize, m);
        let (rmn, rmx) = union(nodes, r as usize, m);
        let mut mn = [0.0; 3];
        let mut mx = [0.0; 3];
        for k in 0..3 {
            mn[k] = lmn[k].min(rmn[k]);
            mx[k] = lmx[k].max(rmx[k]);
        }
        nodes[i].min = mn;
        nodes[i].max = mx;
        (mn, mx)
    }
    union(&mut tree.nodes, 0, m);
}

/// Structural check of a tree in the GPU's encoding (works on the reference or
/// on buffers read back from the GPU).
///
/// * the header agrees with the node count and the root is node 0;
/// * every node is reachable from the root exactly once;
/// * leaf item ranges tile `0..instance_count` exactly, and every live
///   instance row appears exactly once in `items`;
/// * a parent's box contains its children's, and a leaf's box contains the
///   sphere box of every item it holds.
pub fn validate(
    nodes: &[GpuBvhNode],
    items: &[u32],
    header: &GpuBvhHeader,
    instances: &[GpuVgInstance],
    objects: &[StaticObjectRow],
) -> Result<(), String> {
    let live: Vec<u32> = instances
        .iter()
        .enumerate()
        .filter(|(_, inst)| instance_sphere(inst, objects).is_some())
        .map(|(i, _)| i as u32)
        .collect();
    if header.instance_count as usize != live.len() {
        return Err(format!(
            "instance_count {} != live rows {}",
            header.instance_count,
            live.len()
        ));
    }
    if live.is_empty() {
        if header.node_count != 0 || header.leaf_count != 0 || header.root_node != NODE_INVALID {
            return Err("empty scene must give an empty tree".into());
        }
        return Ok(());
    }
    let m = header.leaf_count as usize;
    if header.node_count as usize != 2 * m - 1 || header.root_node != 0 {
        return Err(format!("bad header: {header:?}"));
    }
    if nodes.len() < 2 * m - 1 || items.len() < live.len() {
        return Err("buffers shorter than the header says".into());
    }
    let mut seen_node = vec![false; 2 * m - 1];
    let mut item_covered = vec![0u32; live.len()];
    let mut batch_leaves = 0;
    let mut stack = vec![0usize];
    while let Some(i) = stack.pop() {
        if i >= 2 * m - 1 {
            return Err(format!("node index {i} out of range"));
        }
        if std::mem::replace(&mut seen_node[i], true) {
            return Err(format!("node {i} reached twice"));
        }
        let n = &nodes[i];
        if n.is_leaf() {
            if i < m - 1 {
                return Err(format!("leaf flag on internal slot {i}"));
            }
            let (first, cnt) = (n.leaf_first() as usize, n.leaf_count() as usize);
            if cnt == 0 || cnt > MAX_LEAF_ITEMS as usize || first + cnt > live.len() {
                return Err(format!("leaf {i} has bad range {first}+{cnt}"));
            }
            if cnt > 1 {
                batch_leaves += 1;
            }
            for p in first..first + cnt {
                item_covered[p] += 1;
                let (c, r) = instance_sphere(&instances[items[p] as usize], objects)
                    .ok_or_else(|| format!("leaf {i} holds dead item {}", items[p]))?;
                for k in 0..3 {
                    if n.min[k] > c[k] - r || n.max[k] < c[k] + r {
                        return Err(format!("leaf {i} does not contain item {}", items[p]));
                    }
                }
            }
        } else {
            if i >= m - 1 {
                return Err(format!("internal flag on leaf slot {i}"));
            }
            for child in n.children() {
                let c = &nodes[child as usize];
                for k in 0..3 {
                    if n.min[k] > c.min[k] || n.max[k] < c.max[k] {
                        return Err(format!("node {i} does not contain child {child}"));
                    }
                }
                stack.push(child as usize);
            }
        }
    }
    if seen_node.iter().any(|s| !s) {
        return Err("a node is unreachable from the root".into());
    }
    if item_covered.iter().any(|&c| c != 1) {
        return Err("leaf ranges do not tile the items exactly once".into());
    }
    let mut sorted_items: Vec<u32> = items[..live.len()].to_vec();
    sorted_items.sort_unstable();
    if sorted_items != live {
        return Err("items are not a permutation of the live rows".into());
    }
    if header.batch_leaf_count != batch_leaves {
        return Err(format!(
            "batch_leaf_count {} != {}",
            header.batch_leaf_count, batch_leaves
        ));
    }
    Ok(())
}

/// The header the reference build corresponds to (generations excluded).
pub fn header_of(tree: &RefTree, live: u32) -> GpuBvhHeader {
    let m = tree.leaf_count;
    let nodes = if m > 0 { 2 * m - 1 } else { 0 };
    let mut h = GpuBvhHeader::empty();
    h.instance_count = live;
    h.leaf_count = m;
    h.node_count = nodes;
    h.internal_count = nodes.saturating_sub(m);
    h.batch_leaf_count = tree.batch_leaf_count;
    h.root_node = if m > 0 { 0 } else { NODE_INVALID };
    if let Some(root) = tree.nodes.first() {
        h.bounds_min = root.min;
        h.bounds_max = root.max;
    }
    h
}
