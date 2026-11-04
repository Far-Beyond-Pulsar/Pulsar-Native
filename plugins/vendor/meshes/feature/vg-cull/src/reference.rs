//! CPU reference of the cull's selection rules (`shaders/vg_cull.wgsl`).
//!
//! Same formulas in `f32`, used by the hole/overlap simulation and as the
//! oracle of the GPU test. Covers the cut rule, the hierarchy entry test,
//! the residency fallback and tier binning; frustum/occlusion are exercised
//! on the GPU only (the simulation uses an "everything visible" view).

use std::collections::BTreeSet;

use helio_vg_format::{
    GpuCluster, GpuGroupRef, GpuHierarchyNode, GpuVgInstance, TierLock, VgMode, INST_CONE_CULL,
    INST_FORCE_ROOT_ONLY, NODE_INVALID, NODE_LEAF_BIT,
};

use crate::{
    hw_bin_for_tris, BIN_DENSE, BIN_SW, EDGE_FACTOR, INF_ERROR, MIN_LOD_ERROR_SCALE, ROOT_ONLY_TAU,
};

/// What the error projection needs from the camera.
#[derive(Clone, Copy, Debug)]
pub struct RefView {
    /// World-to-view matrix, column-major (`GpuCameraUniforms::view`).
    pub view: [f32; 16],
    /// `proj[1][1]` = cot(fov_y / 2).
    pub proj_y: f32,
    pub near: f32,
    pub screen_height: f32,
}

impl RefView {
    /// Camera at `eye` looking down `-Z` (identity rotation).
    pub fn looking_down_neg_z(eye: [f32; 3], fov_y: f32, near: f32, screen_height: f32) -> Self {
        let mut view = [0.0; 16];
        view[0] = 1.0;
        view[5] = 1.0;
        view[10] = 1.0;
        view[15] = 1.0;
        view[12] = -eye[0];
        view[13] = -eye[1];
        view[14] = -eye[2];
        Self {
            view,
            proj_y: 1.0 / (fov_y * 0.5).tan(),
            near,
            screen_height,
        }
    }

    /// Camera position in world space (`-R^T t` of the view matrix).
    pub fn eye(&self) -> [f32; 3] {
        let v = &self.view;
        let t = [v[12], v[13], v[14]];
        let e = |i: usize| -(v[i * 4] * t[0] + v[i * 4 + 1] * t[1] + v[i * 4 + 2] * t[2]);
        [e(0), e(1), e(2)]
    }

    /// Positive distance along the view axis.
    pub fn view_depth(&self, c: [f32; 3]) -> f32 {
        let v = &self.view;
        -(v[2] * c[0] + v[6] * c[1] + v[10] * c[2] + v[14])
    }
}

/// Projected size in pixels of the object-space error `err` at a bounding
/// sphere. Mirror of `proj_err` in the shader: nearest view depth of the
/// sphere clamped to the near plane, so a sphere that contains another never
/// projects smaller.
pub fn project_error(v: &RefView, err: f32, center: [f32; 3], radius: f32) -> f32 {
    if err >= INF_ERROR {
        return 3.0e38;
    }
    let d = (v.view_depth(center) - radius).max(v.near);
    err * v.proj_y * 0.5 * v.screen_height / d
}

/// An instance as the cull sees it.
#[derive(Clone, Copy, Debug)]
pub struct RefInstance {
    /// Object-to-world, column-major.
    pub matrix: [f32; 16],
    pub inst: GpuVgInstance,
}

impl RefInstance {
    pub fn translated_scaled(t: [f32; 3], s: f32, inst: GpuVgInstance) -> Self {
        let mut m = [0.0; 16];
        m[0] = s;
        m[5] = s;
        m[10] = s;
        m[15] = 1.0;
        m[12] = t[0];
        m[13] = t[1];
        m[14] = t[2];
        Self { matrix: m, inst }
    }
    pub fn scale(&self) -> f32 {
        let col = |i: usize| {
            let m = &self.matrix;
            (m[i * 4].powi(2) + m[i * 4 + 1].powi(2) + m[i * 4 + 2].powi(2)).sqrt()
        };
        col(0).max(col(1)).max(col(2))
    }
    pub fn to_world(&self, p: [f32; 3]) -> [f32; 3] {
        let m = &self.matrix;
        [
            m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12],
            m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13],
            m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14],
        ]
    }
    /// The error threshold this instance uses.
    pub fn tau(&self, global_tau: f32) -> f32 {
        if self.inst.flags & INST_FORCE_ROOT_ONLY != 0 {
            ROOT_ONLY_TAU
        } else {
            global_tau
        }
    }
    fn error_scale(&self) -> f32 {
        self.inst.lod_error_scale.max(MIN_LOD_ERROR_SCALE) * self.scale()
    }
}

/// Projected error of `err` at an asset-space sphere of this instance.
pub fn instance_project(v: &RefView, inst: &RefInstance, err: f32, sphere: [f32; 4]) -> f32 {
    let c = inst.to_world([sphere[0], sphere[1], sphere[2]]);
    project_error(v, err * inst.error_scale(), c, sphere[3] * inst.scale())
}

/// The per-cluster decision, identical for every cluster and independent of
/// its neighbours:
/// `project(parent) > tau && (project(self) <= tau || !children_resident)`.
/// With everything resident this is `render iff self <= tau && parent > tau`.
pub fn cut_draws(
    v: &RefView,
    tau: f32,
    inst: &RefInstance,
    c: &GpuCluster,
    children_resident: bool,
) -> bool {
    let tau = inst.tau(tau);
    let s = instance_project(v, inst, c.self_error, c.sphere);
    let p = instance_project(v, inst, c.parent_error, c.parent_sphere);
    p > tau && (s <= tau || !children_resident)
}

/// Tier bin of a drawn cluster (no dense mask: `dense_overlap` supplied by
/// the caller, `false` when there is none).
pub fn choose_bin(
    v: &RefView,
    inst: &RefInstance,
    c: &GpuCluster,
    mode: VgMode,
    sw_edge_px: f32,
    dense_overlap: bool,
) -> u32 {
    let hw = hw_bin_for_tris(c.tri_count());
    if mode == VgMode::ForceHwRaster {
        return hw;
    }
    match inst.inst.tier_lock() {
        TierLock::HwRaster => return hw,
        TierLock::SwRaster => return BIN_SW,
        TierLock::Dense => return BIN_DENSE,
        TierLock::Auto => {}
    }
    let w = inst.to_world([c.sphere[0], c.sphere[1], c.sphere[2]]);
    let rw = c.sphere[3] * inst.scale();
    let d = v.view_depth(w).max(v.near);
    let r_px = rw * v.proj_y * 0.5 * v.screen_height / d;
    let edge = EDGE_FACTOR * r_px / (c.tri_count().max(1) as f32).sqrt();
    if edge <= sw_edge_px {
        if dense_overlap {
            BIN_DENSE
        } else {
            BIN_SW
        }
    } else {
        hw
    }
}

/// Cone test (`cutoff = cos(half-angle)`, see the crate docs). `axis` is the
/// world space unit axis.
pub fn cone_culled(
    cam: [f32; 3],
    center: [f32; 3],
    radius: f32,
    axis: [f32; 3],
    cutoff: f32,
) -> bool {
    helio_vg_format::cone_culled(cam, center, radius, axis, cutoff)
}

/// Whether `INST_CONE_CULL` is set.
pub fn cone_enabled(inst: &GpuVgInstance) -> bool {
    inst.flags & INST_CONE_CULL != 0
}

/// The cull's cone test for one cluster of one instance, exactly as the
/// shader does it (world axis = `normalize(M * axis)`).
pub fn cluster_cone_culled(v: &RefView, inst: &RefInstance, c: &GpuCluster) -> bool {
    if !cone_enabled(&inst.inst) {
        return false;
    }
    let (axis, cutoff) = helio_vg_format::unpack_cone(c.cone);
    let m = &inst.matrix;
    let a = [
        m[0] * axis[0] + m[4] * axis[1] + m[8] * axis[2],
        m[1] * axis[0] + m[5] * axis[1] + m[9] * axis[2],
        m[2] * axis[0] + m[6] * axis[1] + m[10] * axis[2],
    ];
    let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
    let a = [a[0] / l, a[1] / l, a[2] / l];
    let center = inst.to_world([c.sphere[0], c.sphere[1], c.sphere[2]]);
    cone_culled(v.eye(), center, c.sphere[3] * inst.scale(), a, cutoff)
}

/// One asset's hierarchy + cluster pool as the cull reads it.
pub struct RefAsset<'a> {
    pub root_node: u32,
    pub nodes: &'a [GpuHierarchyNode],
    pub group_refs: &'a [GpuGroupRef],
    /// Pool slot of each asset-local page, `None` = not resident.
    pub page_slot: &'a [Option<u32>],
    /// The cluster record at `(pool_slot, local_index)`.
    pub cluster: &'a dyn Fn(u32, u32) -> GpuCluster,
}

impl RefAsset<'_> {
    fn slot_of(&self, page: u32) -> Option<u32> {
        self.page_slot.get(page as usize).copied().flatten()
    }

    pub fn page_resident(&self, page: u32) -> bool {
        self.slot_of(page).is_some()
    }

    /// Whether the clusters `c` was simplified from are resident (leaf
    /// clusters have none, which counts as resident).
    pub fn children_resident(&self, c: &GpuCluster) -> bool {
        if c.flags & GpuCluster::FLAG_LEAF != 0 || c.group_id == NODE_INVALID {
            return true;
        }
        self.page_resident(self.group_refs[c.group_id as usize].page_id)
    }
}

/// Result of a reference traversal.
#[derive(Default, Debug, PartialEq, Eq)]
pub struct RefSelection {
    /// Drawn `cluster_slot`s, ascending.
    pub drawn: Vec<u32>,
    /// Asset-local page ids requested.
    pub requested_pages: BTreeSet<u32>,
}

/// Traversal by the same rules as the GPU (entry test, residency, cut), for
/// one instance with no frustum/occlusion/cone culling.
pub fn select_clusters(
    asset: &RefAsset<'_>,
    v: &RefView,
    tau: f32,
    inst: &RefInstance,
) -> RefSelection {
    select_clusters_ex(asset, v, tau, inst, false)
}

/// As [`select_clusters`], optionally applying the cone cull of
/// `INST_CONE_CULL` instances.
pub fn select_clusters_ex(
    asset: &RefAsset<'_>,
    v: &RefView,
    tau: f32,
    inst: &RefInstance,
    apply_cone: bool,
) -> RefSelection {
    let mut out = RefSelection::default();
    let tau_i = inst.tau(tau);
    let mut stack: Vec<(u32, usize)> = Vec::new();
    for s in 0..4 {
        stack.push((asset.root_node, s));
    }
    while let Some((parent, slot)) = stack.pop() {
        let node = &asset.nodes[parent as usize];
        let child = node.children[slot];
        if child == NODE_INVALID {
            continue;
        }
        let p = instance_project(v, inst, node.max_parent_error[slot], node.spheres[slot]);
        if p <= tau_i {
            continue;
        }
        if child & NODE_LEAF_BIT != 0 {
            let g = asset.group_refs[(child & !NODE_LEAF_BIT) as usize];
            let Some(pool) = asset.slot_of(g.page_id) else {
                out.requested_pages.insert(g.page_id);
                continue;
            };
            for i in 0..g.cluster_count() {
                let local = g.first_cluster() + i;
                let c = (asset.cluster)(pool, local);
                let kids = asset.children_resident(&c);
                if !kids {
                    let s = instance_project(v, inst, c.self_error, c.sphere);
                    if s > tau_i {
                        out.requested_pages
                            .insert(asset.group_refs[c.group_id as usize].page_id);
                    }
                }
                if cut_draws(v, tau, inst, &c, kids)
                    && !(apply_cone && cluster_cone_culled(v, inst, &c))
                {
                    out.drawn.push(helio_vg_format::cluster_slot(pool, local));
                }
            }
        } else {
            for s in 0..4 {
                stack.push((child, s));
            }
        }
    }
    out.drawn.sort_unstable();
    out
}
