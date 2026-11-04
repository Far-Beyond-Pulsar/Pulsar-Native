//! Hand-built synthetic cluster DAG shared by the simulation and GPU tests.
//!
//! 1-D geometry: leaf clusters are unit-radius spheres along +X. Groups of up
//! to four clusters are "simplified" into half as many parent clusters whose
//! sphere is the group's extent and whose error is `2^(level+1) - 1`. With
//! `interleave` the outputs of a group are spread over the next level so the
//! consumers of one group's outputs differ (a genuine DAG, not a tree).
#![allow(dead_code)]

use helio_pass_vg_cull::reference::RefAsset;
use helio_vg_format::{GpuCluster, GpuGroupRef, GpuHierarchyNode, NODE_INVALID, NODE_LEAF_BIT};

pub const INF: f32 = f32::INFINITY;

pub struct SynthDag {
    pub clusters: Vec<GpuCluster>,
    pub group_refs: Vec<GpuGroupRef>,
    pub nodes: Vec<GpuHierarchyNode>,
    pub root_node: u32,
    /// One page per group ref (page id == group ref index).
    pub page_first_slot: Vec<u32>,
    /// Group ref that consumes cluster `i` (`None` for roots).
    pub consumer_group: Vec<Option<u32>>,
    /// Clusters produced by group ref `g`.
    pub produced_by: Vec<Vec<u32>>,
    pub level: Vec<u32>,
    pub leaf_count: usize,
}

impl SynthDag {
    pub fn page_count(&self) -> usize {
        self.group_refs.len()
    }

    /// Pool slot of each page for a residency mask: slot == page id.
    pub fn slots(&self, resident: &[bool]) -> Vec<Option<u32>> {
        resident
            .iter()
            .enumerate()
            .map(|(p, &r)| r.then_some(p as u32))
            .collect()
    }

    /// Cluster record at `(pool slot, local index)`.
    pub fn lookup(&self) -> impl Fn(u32, u32) -> GpuCluster + '_ {
        move |slot, local| self.clusters[(self.page_first_slot[slot as usize] + local) as usize]
    }

    /// Index into `clusters` of a `cluster_slot`.
    pub fn index_of(&self, cs: u32) -> usize {
        (self.page_first_slot[helio_vg_format::cluster_slot_page(cs) as usize]
            + helio_vg_format::cluster_slot_local(cs)) as usize
    }

    /// `cluster_slot` of `clusters[index]`.
    pub fn slot_of(&self, index: usize) -> u32 {
        let page = (0..self.page_first_slot.len())
            .rev()
            .find(|&p| self.page_first_slot[p] as usize <= index)
            .unwrap();
        helio_vg_format::cluster_slot(page as u32, index as u32 - self.page_first_slot[page])
    }

    pub fn asset<'a>(
        &'a self,
        slots: &'a [Option<u32>],
        lookup: &'a dyn Fn(u32, u32) -> GpuCluster,
    ) -> RefAsset<'a> {
        RefAsset {
            root_node: self.root_node,
            nodes: &self.nodes,
            group_refs: &self.group_refs,
            page_slot: slots,
            cluster: lookup,
        }
    }

    /// Resident set closed upward from `seed` pages: a group's page resident
    /// implies the page of every group consuming its outputs is.
    pub fn close_upward(&self, seed: &[bool]) -> Vec<bool> {
        let mut r = seed.to_vec();
        loop {
            let mut changed = false;
            for g in 0..r.len() {
                if r[g] {
                    for &c in &self.produced_by[g] {
                        if let Some(h) = self.consumer_group[c as usize] {
                            if !r[h as usize] {
                                r[h as usize] = true;
                                changed = true;
                            }
                        }
                    }
                }
            }
            if !changed {
                return r;
            }
        }
    }

    /// Root group (its inputs are the roots) is the last group ref.
    pub fn root_group(&self) -> usize {
        self.group_refs.len() - 1
    }
}

fn span(s: [f32; 4]) -> (f32, f32) {
    (s[0] - s[3], s[0] + s[3])
}

fn sphere_of(lo: f32, hi: f32) -> [f32; 4] {
    [(lo + hi) * 0.5, 0.0, 0.0, (hi - lo) * 0.5]
}

pub fn build(leaves: usize, interleave: bool) -> SynthDag {
    assert!(leaves.is_power_of_two() && leaves >= 8);
    // Per-level cluster slots (global pool slot == push order).
    let mut clusters: Vec<GpuCluster> = Vec::new();
    let mut level_of: Vec<u32> = Vec::new();
    let mut levels: Vec<Vec<u32>> = Vec::new();

    let mut lvl0 = Vec::new();
    for i in 0..leaves {
        let tris = 32 + 32 * (i as u32 % 4);
        lvl0.push(clusters.len() as u32);
        clusters.push(GpuCluster {
            sphere: [2.0 * i as f32, 0.0, 0.0, 1.0],
            counts_material: GpuCluster::pack_counts(tris, tris, 0),
            flags: GpuCluster::FLAG_LEAF,
            group_id: NODE_INVALID,
            ..Default::default()
        });
        level_of.push(0);
    }
    levels.push(lvl0);

    struct Group {
        inputs: Vec<u32>,
        sphere: [f32; 4],
        error: f32,
        outputs: Vec<u32>,
    }
    let mut groups: Vec<Group> = Vec::new();

    let mut lv = 0u32;
    loop {
        let cur = levels.last().unwrap().clone();
        if cur.len() <= 2 {
            break;
        }
        let error = (1u32 << (lv + 1)) as f32 - 1.0;
        let chunks: Vec<Vec<u32>> = cur.chunks(4).map(|c| c.to_vec()).collect();
        let ng = chunks.len();
        let mut next_slots: Vec<(usize, usize, u32)> = Vec::new(); // (group, k, slot)
        let first_group = groups.len();
        for (gi, inputs) in chunks.iter().enumerate() {
            let lo = inputs
                .iter()
                .map(|&c| span(clusters[c as usize].sphere).0)
                .fold(f32::MAX, f32::min);
            let hi = inputs
                .iter()
                .map(|&c| span(clusters[c as usize].sphere).1)
                .fold(f32::MIN, f32::max);
            let sphere = sphere_of(lo, hi);
            let outs = (inputs.len() + 1) / 2;
            let mut outputs = Vec::new();
            for k in 0..outs {
                let slot = clusters.len() as u32;
                clusters.push(GpuCluster {
                    sphere,
                    self_error: error,
                    counts_material: GpuCluster::pack_counts(96, 96, 0),
                    group_id: (first_group + gi) as u32,
                    ..Default::default()
                });
                level_of.push(lv + 1);
                outputs.push(slot);
                next_slots.push((gi, k, slot));
            }
            for &c in inputs {
                clusters[c as usize].parent_sphere = sphere;
                clusters[c as usize].parent_error = error;
            }
            groups.push(Group {
                inputs: inputs.clone(),
                sphere,
                error,
                outputs,
            });
        }
        // Next level ordering.
        let mut order = next_slots.clone();
        if interleave {
            order.sort_by_key(|&(g, k, _)| (k, g));
        }
        let _ = ng;
        levels.push(order.iter().map(|&(_, _, s)| s).collect());
        lv += 1;
    }
    // Roots: no parent.
    let roots = levels.last().unwrap().clone();
    for &r in &roots {
        clusters[r as usize].parent_error = INF;
        clusters[r as usize].parent_sphere = clusters[r as usize].sphere;
        clusters[r as usize].flags |= GpuCluster::FLAG_ROOT;
    }
    let root_lo = roots
        .iter()
        .map(|&c| span(clusters[c as usize].sphere).0)
        .fold(f32::MAX, f32::min);
    let root_hi = roots
        .iter()
        .map(|&c| span(clusters[c as usize].sphere).1)
        .fold(f32::MIN, f32::max);
    groups.push(Group {
        inputs: roots.clone(),
        sphere: sphere_of(root_lo, root_hi),
        error: INF,
        outputs: vec![],
    });

    // Group refs: one page per group, pages hold the group's inputs.
    // Pool slots are re-laid page by page so `first_cluster_slot + local` is
    // contiguous per page, which is how the GPU reads them.
    let mut remap = vec![0u32; clusters.len()];
    let mut page_first_slot = Vec::new();
    let mut next = 0u32;
    for g in &groups {
        page_first_slot.push(next);
        for &c in &g.inputs {
            remap[c as usize] = next;
            next += 1;
        }
    }
    let mut pool = vec![GpuCluster::default(); clusters.len()];
    for (i, c) in clusters.iter().enumerate() {
        pool[remap[i] as usize] = *c;
    }
    let mut consumer_group = vec![None; pool.len()];
    let mut level = vec![0u32; pool.len()];
    let mut produced_by = vec![Vec::new(); groups.len()];
    let mut group_refs = Vec::new();
    for (gi, g) in groups.iter().enumerate() {
        group_refs.push(GpuGroupRef::new(gi as u32, 0, g.inputs.len() as u32));
        for &c in &g.inputs {
            if g.error.is_finite() {
                consumer_group[remap[c as usize] as usize] = Some(gi as u32);
            }
        }
        for &o in &g.outputs {
            produced_by[gi].push(remap[o as usize]);
        }
    }
    for (i, l) in level_of.iter().enumerate() {
        level[remap[i] as usize] = *l;
    }

    // Hierarchy: 4-wide tree over the group refs ordered by sphere centre.
    // A leaf item's sphere is the group sphere, which contains its clusters'
    // spheres and parent spheres; `max_parent_error` is the group's error.
    struct Item {
        sphere: [f32; 4],
        mpe: f32,
        child: u32,
    }
    let mut items: Vec<Item> = groups
        .iter()
        .enumerate()
        .map(|(gi, g)| Item {
            sphere: g.sphere,
            mpe: g.error,
            child: NODE_LEAF_BIT | gi as u32,
        })
        .collect();
    items.sort_by(|a, b| a.sphere[0].partial_cmp(&b.sphere[0]).unwrap());
    let mut nodes: Vec<GpuHierarchyNode> = Vec::new();
    let mut layer = items;
    // Build bottom-up; node indices are assigned in push order, the root last.
    loop {
        let mut next_layer = Vec::new();
        for chunk in layer.chunks(4) {
            let lo = chunk
                .iter()
                .map(|i| span(i.sphere).0)
                .fold(f32::MAX, f32::min);
            let hi = chunk
                .iter()
                .map(|i| span(i.sphere).1)
                .fold(f32::MIN, f32::max);
            let mut node = GpuHierarchyNode::default();
            for (s, it) in chunk.iter().enumerate() {
                node.spheres[s] = it.sphere;
                node.max_parent_error[s] = it.mpe;
                node.children[s] = it.child;
            }
            let idx = nodes.len() as u32;
            nodes.push(node);
            next_layer.push(Item {
                sphere: sphere_of(lo, hi),
                mpe: chunk.iter().map(|i| i.mpe).fold(0.0, f32::max),
                child: idx,
            });
        }
        if next_layer.len() == 1 {
            break;
        }
        layer = next_layer;
    }
    // The root node is the last one pushed, but a root with a single node
    // child chain is fine; if the top layer had one node it is the root.
    let root_node = (nodes.len() - 1) as u32;

    SynthDag {
        clusters: pool,
        group_refs,
        nodes,
        root_node,
        page_first_slot,
        consumer_group,
        produced_by,
        level,
        leaf_count: leaves,
    }
}
