//! Virtual-geometry shared formats (plan: `.agents/plans/virtual-geometry.md`).
//!
//! Single source of truth for every struct and bit layout that crosses the
//! offline builder (`helio-vg-build`), the streamer (`helio-vg-stream`) and
//! the GPU passes. `shaders/vg_format.wgsl` mirrors it; tests pin sizes and
//! validate the WGSL. No wgpu dependency: this crate is pure data.

use bytemuck::{Pod, Zeroable};

/// WGSL declarations to prepend to every virtual-geometry shader.
pub const WGSL: &str = include_str!("../shaders/vg_format.wgsl");

pub const CLUSTER_MAX_TRIS: u32 = 128;
pub const CLUSTER_MAX_VERTS: u32 = 128;
pub const CLUSTER_MIN_TRIS: u32 = 32;
/// Streaming page size in bytes.
pub const PAGE_BYTES: u32 = 128 * 1024;
/// Groups are 4..=8 clusters.
pub const GROUP_MIN_CLUSTERS: u32 = 4;
pub const GROUP_MAX_CLUSTERS: u32 = 8;
/// Larger groups the builder may form once few clusters remain (limited in
/// practice by one group having to fit a 128 KiB page); the group ref stores
/// the count in 16 bits, so readers must not assume `GROUP_MAX_CLUSTERS`.
pub const GROUP_MAX_CLUSTERS_LARGE: u32 = 32;
/// Hierarchy fan-out (child slots per node).
pub const NODE_FANOUT: usize = 4;

/// Bits of a visibility id holding the triangle index within its cluster.
pub const VIS_TRI_BITS: u32 = 7;
pub const VIS_TRI_MASK: u32 = (1 << VIS_TRI_BITS) - 1;
/// Bits of a visibility id holding the visible-cluster index.
pub const VIS_CLUSTER_BITS: u32 = 25;
/// Hard ceiling on visible clusters per frame implied by the id layout.
pub const MAX_VISIBLE_CLUSTERS: u32 = 1 << VIS_CLUSTER_BITS;

/// Hierarchy child ref: top bit set = leaf (index into the group-ref table).
pub const NODE_LEAF_BIT: u32 = 0x8000_0000;
pub const NODE_INVALID: u32 = 0xffff_ffff;

/// Bits per axis of the asset-global position lattice.
pub const LATTICE_BITS: u32 = 18;
pub const LATTICE_MAX: u32 = (1 << LATTICE_BITS) - 1;
/// Words of fixed header in a cluster position block (anchor x3, bits, attr offset).
pub const POS_BLOCK_HEADER_WORDS: usize = 5;
/// Bytes per vertex of the attribute stream: normal snorm8x4, tangent
/// snorm8x4 (w = bitangent sign), uv0 f16x2, uv1 f16x2.
pub const ATTR_STRIDE: u32 = 16;
/// A visible-cluster `cluster_slot` is `pool_slot << PAGE_CLUSTER_SHIFT | local_cluster`.
pub const PAGE_CLUSTER_SHIFT: u32 = 11;
pub const PAGE_CLUSTER_MASK: u32 = (1 << PAGE_CLUSTER_SHIFT) - 1;
/// Unified visible index (`bin * bin_capacity + k`) must stay below this
/// (3-RT ids start at `1 << 24`).
pub const VISIBLE_INDEX_LIMIT: u32 = 1 << 24;
/// Workgroups per row of the 2D indirect dispatches that process one
/// workgroup per cluster (software raster, dense): workgroup `(x, y)` is
/// cluster `first + y * SW_GROUP_WIDTH + x`, guarded by `< first + n`.
pub const SW_GROUP_WIDTH: u32 = 32768;
/// Word offset of the per-phase `{first, count}` bin ranges inside the cull's
/// `state` buffer, and the layout of that table: `phase * CULL_RANGES_PER_PHASE
/// + bin * 2` (phase 0 = early draw set, phase 1 = the late delta; bins 0..=3
/// HW, 4 SW, 5 dense, 6 HW alpha-masked, 7 SW alpha-masked).
pub const CULL_STATE_RANGES: u32 = 40;
pub const CULL_BIN_COUNT: u32 = 8;
/// Bins `>= CULL_FIRST_MASKED_BIN` hold clusters whose material is alpha
/// tested (6 = hardware masked, 7 = software masked).
pub const CULL_FIRST_MASKED_BIN: u32 = 6;
pub const CULL_RANGES_PER_PHASE: u32 = CULL_BIN_COUNT * 2;

pub fn cluster_slot(pool_slot: u32, local: u32) -> u32 {
    debug_assert!(local <= PAGE_CLUSTER_MASK);
    (pool_slot << PAGE_CLUSTER_SHIFT) | local
}
pub fn cluster_slot_page(slot: u32) -> u32 {
    slot >> PAGE_CLUSTER_SHIFT
}
pub fn cluster_slot_local(slot: u32) -> u32 {
    slot & PAGE_CLUSTER_MASK
}

/// Per-asset lattice (`p = origin + q * step`), uploaded as
/// `vg_asset_quant[asset_slot]`. Equal to the first 16 bytes of the `.vgeo`
/// meta block at [`VGEO_META_OFFSET`]. 16 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct GpuAssetQuant {
    pub origin: [f32; 3],
    pub step: f32,
}
/// File offset of the builder's meta block (directly after [`VgeoHeader`]).
pub const VGEO_META_OFFSET: usize = 136;

/// One entry of the visible-cluster list written by the cull. 8 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct GpuVisibleCluster {
    /// Row of `vg_instances`.
    pub instance: u32,
    /// [`cluster_slot`] of the cluster in the page pool.
    pub cluster_slot: u32,
}

/// `vg_asset_ready` row for the scene derivation. 16 bytes; sorted ascending
/// by `vertex_offset`; padding rows have `vertex_offset == u32::MAX`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct VgAssetReadyRow {
    pub vertex_offset: u32,
    pub vertex_count: u32,
    pub asset_slot: u32,
    pub flags: u32,
}
pub const VG_ASSET_READY_FLAG_READY: u32 = 1;
impl VgAssetReadyRow {
    pub const PADDING: Self = Self {
        vertex_offset: u32::MAX,
        vertex_count: 0,
        asset_slot: 0,
        flags: 0,
    };
}

/// Shared position/index/attribute decode. Needs `vg_page_pool` and
/// `vg_asset_quant` declared before it (see [`decode_bindings_wgsl`]).
pub const DECODE_WGSL: &str = include_str!("../shaders/vg_decode.wgsl");
/// Attribute decode (`VgVertexAttr`, `vgd_attr`); append after [`DECODE_WGSL`] in
/// passes that shade (needs the float16-in-float32 capability).
pub const DECODE_ATTR_WGSL: &str = include_str!("../shaders/vg_decode_attr.wgsl");

/// Declarations of the two buffers [`DECODE_WGSL`] reads.
pub fn decode_bindings_wgsl(group: u32, pool_binding: u32, quant_binding: u32) -> String {
    format!(
        "@group({group}) @binding({pool_binding}) var<storage, read> vg_page_pool: array<u32>;\n@group({group}) @binding({quant_binding}) var<storage, read> vg_asset_quant: array<vec4<f32>>;\n"
    )
}

pub mod debug_view;
pub mod density;
pub mod testing;

/// `.vgeo` / page identification.
pub const VGEO_MAGIC: [u8; 4] = *b"VGEO";
pub const PAGE_MAGIC: [u8; 4] = *b"VGPG";
/// Bump on any layout or builder-semantics change; part of the cache key.
/// 2: measured (not estimated) cluster errors, 0.6 group reduction ratio.
/// 3: groups of up to `GROUP_MAX_CLUSTERS_LARGE` clusters at coarse levels.
pub const FORMAT_VERSION: u32 = 4;

// ── Cluster ─────────────────────────────────────────────────────────────────

/// One cluster of up to 128 triangles. 64 bytes.
///
/// Cut rule (no neighbour communication): draw iff
/// `project(self_error, sphere) <= tau && project(parent_error, parent_sphere) > tau`.
/// Builder invariants: `parent_error >= self_error`, `parent_sphere`
/// contains `sphere`; root clusters have `parent_error = f32::INFINITY`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct GpuCluster {
    /// Tight culling and self-error sphere, asset space (xyz, radius).
    pub sphere: [f32; 4],
    /// Sphere of the group that consumed this cluster (parent error space).
    pub parent_sphere: [f32; 4],
    /// Backface cone: axis xyz as snorm8, `cutoff = cos(half-angle)` of the
    /// cone of triangle normals as snorm8 in w (rounded *down* by the builder,
    /// i.e. widened). A viewer at `cam` may cull the whole cluster iff
    /// `dot(c - cam, axis) >= sqrt(1 - cutoff^2) * |c - cam| + radius`
    /// (see [`cone_culled`]). Cutoff of 1.0 disables the cull.
    pub cone: u32,
    pub self_error: f32,
    pub parent_error: f32,
    /// bits 0..8 triangle count, 8..16 vertex count, 16..32 material slot.
    pub counts_material: u32,
    /// Page-relative byte offset of this cluster's position stream.
    pub vertex_offset: u32,
    /// Page-relative byte offset of its u8 triangle index stream.
    pub index_offset: u32,
    /// Asset-local group-ref index of the group whose simplification
    /// *produced* this cluster, i.e. the hierarchy leaf whose clusters are
    /// this cluster's finer children (`NODE_INVALID` for leaf clusters).
    /// `group_refs[group_id].page_id` is the page that must be resident to
    /// refine this cluster. Add the asset's group-ref base on the GPU.
    pub group_id: u32,
    pub flags: u32,
}

impl GpuCluster {
    pub const FLAG_ROOT: u32 = 1;
    pub const FLAG_LEAF: u32 = 2;

    pub fn pack_counts(tris: u32, verts: u32, material: u32) -> u32 {
        debug_assert!(tris <= CLUSTER_MAX_TRIS && verts <= CLUSTER_MAX_VERTS && material <= 0xffff);
        (tris & 0xff) | ((verts & 0xff) << 8) | ((material & 0xffff) << 16)
    }
    pub fn tri_count(&self) -> u32 {
        self.counts_material & 0xff
    }
    pub fn vert_count(&self) -> u32 {
        (self.counts_material >> 8) & 0xff
    }
    pub fn material_slot(&self) -> u32 {
        self.counts_material >> 16
    }
}

fn snorm8(v: f32) -> u32 {
    ((v.clamp(-1.0, 1.0) * 127.0).round() as i32 as i8 as u8) as u32
}
fn unsnorm8(b: u32) -> f32 {
    ((b & 0xff) as u8 as i8) as f32 / 127.0
}

/// Pack a normalized cone axis and cutoff. The builder must widen the cone
/// to cover the quantization error *before* packing; this only quantizes.
pub fn pack_cone(axis: [f32; 3], cutoff: f32) -> u32 {
    snorm8(axis[0]) | (snorm8(axis[1]) << 8) | (snorm8(axis[2]) << 16) | (snorm8(cutoff) << 24)
}
pub fn unpack_cone(c: u32) -> ([f32; 3], f32) {
    (
        [unsnorm8(c), unsnorm8(c >> 8), unsnorm8(c >> 16)],
        unsnorm8(c >> 24),
    )
}

/// Backface cone test shared by the CPU reference and the shader: `axis` is
/// the unit world-space axis, `cutoff = cos(half-angle)`.
pub fn cone_culled(
    cam: [f32; 3],
    center: [f32; 3],
    radius: f32,
    axis: [f32; 3],
    cutoff: f32,
) -> bool {
    if cutoff >= 0.9999 {
        return false;
    }
    let dv = [center[0] - cam[0], center[1] - cam[1], center[2] - cam[2]];
    let len = (dv[0] * dv[0] + dv[1] * dv[1] + dv[2] * dv[2]).sqrt();
    let dot = dv[0] * axis[0] + dv[1] * axis[1] + dv[2] * axis[2];
    dot >= (1.0 - cutoff * cutoff).max(0.0).sqrt() * len + radius
}

// ── Hierarchy ───────────────────────────────────────────────────────────────

/// 4-wide hierarchy node over cluster groups. 96 bytes. Always resident.
///
/// A child subtree is entered iff `project(max_parent_error[i]) > tau`
/// (something below it may still need drawing) and its sphere passes
/// frustum/occlusion tests.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct GpuHierarchyNode {
    pub spheres: [[f32; 4]; NODE_FANOUT],
    pub max_parent_error: [f32; NODE_FANOUT],
    /// Node index, or `NODE_LEAF_BIT | group_ref_index`, or `NODE_INVALID`.
    pub children: [u32; NODE_FANOUT],
}

impl Default for GpuHierarchyNode {
    fn default() -> Self {
        Self {
            spheres: [[0.0; 4]; NODE_FANOUT],
            max_parent_error: [0.0; NODE_FANOUT],
            children: [NODE_INVALID; NODE_FANOUT],
        }
    }
}

/// Error at or above which a parent error means "no parent" (root clusters).
pub const ROOT_PARENT_ERROR: f32 = 1.0e30;

/// The hierarchy leaves (asset-local group-ref indices) holding the root
/// clusters of an asset, or `None` for a malformed hierarchy. Root clusters have an infinite
/// `parent_error`, so they are exactly the clusters of the leaves reached
/// through slots whose `max_parent_error` is infinite.
///
/// The GPU copy of the asset header carries these leaves (see
/// [`gpu_header_with_root_leaves`]), which lets the cull draw the roots of a distant instance
/// without walking the hierarchy (see `helio-pass-vg-cull`).
pub fn root_leaves(nodes: &[GpuHierarchyNode], root_node: u32) -> Option<Vec<u32>> {
    let mut found: Vec<u32> = Vec::new();
    let mut stack = vec![root_node];
    let mut visited = 0usize;
    while let Some(n) = stack.pop() {
        visited += 1;
        if visited > nodes.len() + 1 {
            return None; // cycle
        }
        let node = nodes.get(n as usize)?;
        for s in 0..NODE_FANOUT {
            let child = node.children[s];
            if child == NODE_INVALID || node.max_parent_error[s] < ROOT_PARENT_ERROR {
                continue;
            }
            if child & NODE_LEAF_BIT != 0 {
                let leaf = child & !NODE_LEAF_BIT;
                if !found.contains(&leaf) {
                    found.push(leaf);
                }
            } else {
                stack.push(child);
            }
        }
    }
    (!found.is_empty()).then_some(found)
}

/// GPU view of an asset header: up to [`MAX_ROOT_LEAVES`] root leaves (`leaf + 1`) are
/// stored in the words no shader reads (`cluster_count`, `root_page_count`, `flags`),
/// 0 = unused. More leaves than that store none (no early-out for the asset).
pub const MAX_ROOT_LEAVES: usize = 3;

pub fn gpu_header_with_root_leaves(
    mut header: GpuAssetHeader,
    nodes: &[GpuHierarchyNode],
) -> GpuAssetHeader {
    let leaves = root_leaves(nodes, header.root_node)
        .filter(|l| l.len() <= MAX_ROOT_LEAVES)
        .unwrap_or_default();
    let word = |i: usize| leaves.get(i).map_or(0, |l| l + 1);
    header.cluster_count = word(0);
    header.root_page_count = word(1);
    header.flags = word(2);
    header
}

/// Leaf payload: where a group's clusters live. 8 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct GpuGroupRef {
    pub page_id: u32,
    /// bits 0..16 first cluster index within the page, 16..32 cluster count.
    pub first_cluster_count: u32,
}

impl GpuGroupRef {
    pub fn new(page_id: u32, first: u32, count: u32) -> Self {
        debug_assert!(first <= 0xffff && count <= 0xffff);
        Self {
            page_id,
            first_cluster_count: (first & 0xffff) | (count << 16),
        }
    }
    pub fn first_cluster(&self) -> u32 {
        self.first_cluster_count & 0xffff
    }
    pub fn cluster_count(&self) -> u32 {
        self.first_cluster_count >> 16
    }
}

/// Per-asset summary row. 96 bytes.
///
/// After the counters comes what the screen-wide density solve needs without
/// reading any page: the asset's bounding sphere and [`LodTable`] (selected
/// triangles against object-space error tolerance).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct GpuAssetHeader {
    pub root_node: u32,
    pub node_count: u32,
    pub group_count: u32,
    pub page_count: u32,
    pub cluster_count: u32,
    /// Pages that must be resident before the asset is VG-active.
    pub root_page_count: u32,
    pub triangle_count: u32,
    pub flags: u32,
    /// Bounding sphere of the asset (object space): centre xyz, radius.
    pub bounds: [f32; 4],
    /// Error tolerance of [`LodTable`] entry 0 (the largest root error).
    pub lod_error_max: f32,
    /// [`LodTable`] entries, 4 per word, entry `k` in byte `k % 4` of word `k / 4`.
    pub lod_table: [u32; LOD_TABLE_WORDS],
    pub _reserved: [u32; 3],
}

/// Entries of a [`LodTable`].
pub const LOD_TABLE_ENTRIES: usize = 32;
pub const LOD_TABLE_WORDS: usize = LOD_TABLE_ENTRIES / 4;

/// Selected triangles of one asset at a uniform object-space error tolerance:
/// entry `k` is the cut at `error_max * 2^(-k/2)`, stored as `log2(triangles) *
/// 8` in one byte (about 9% steps). The cull's density solve interpolates it
/// per instance, so the screen-wide tolerance is computed rather than fed back.
/// `vg_lod_tris` in `vg_format.wgsl` mirrors [`LodTable::tris_at`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LodTable {
    pub error_max: f32,
    pub q: [u8; LOD_TABLE_ENTRIES],
}

impl LodTable {
    pub fn quantize(tris: f64) -> u8 {
        (tris.max(1.0).log2() * 8.0).round().clamp(0.0, 255.0) as u8
    }

    pub fn from_header(h: &GpuAssetHeader) -> Self {
        let mut q = [0u8; LOD_TABLE_ENTRIES];
        for (k, v) in q.iter_mut().enumerate() {
            *v = (h.lod_table[k / 4] >> ((k % 4) * 8)) as u8;
        }
        Self {
            error_max: h.lod_error_max,
            q,
        }
    }

    pub fn words(&self) -> [u32; LOD_TABLE_WORDS] {
        let mut w = [0u32; LOD_TABLE_WORDS];
        for (k, &v) in self.q.iter().enumerate() {
            w[k / 4] |= u32::from(v) << ((k % 4) * 8);
        }
        w
    }

    /// Triangles selected at object-space tolerance `e` (a tolerance at or above
    /// `error_max` selects the roots, one far below selects the full mesh).
    pub fn tris_at(&self, e: f32) -> f32 {
        if !(self.error_max > 0.0) || !e.is_finite() {
            return 0.0;
        }
        let x = if e <= 0.0 {
            31.0
        } else {
            (2.0 * (self.error_max / e).log2()).clamp(0.0, 31.0)
        };
        let k = x.floor() as usize;
        let t = x - k as f32;
        let a = f32::from(self.q[k]);
        let b = f32::from(self.q[(k + 1).min(LOD_TABLE_ENTRIES - 1)]);
        ((a + (b - a) * t) / 8.0).exp2()
    }
}

// ── Instances (produced by the VG scene derivation, read by cull) ──────────

/// Which raster tier an instance is locked to (`Auto` = per-cluster choice).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum TierLock {
    #[default]
    Auto = 0,
    HwRaster = 1,
    SwRaster = 2,
    Dense = 3,
}

impl TierLock {
    pub fn from_bits(v: u32) -> Self {
        match v & INST_TIER_MASK {
            1 => Self::HwRaster,
            2 => Self::SwRaster,
            3 => Self::Dense,
            _ => Self::Auto,
        }
    }
}

pub const INST_ENABLED: u32 = 1;
pub const INST_SHADOW_CASTER: u32 = 2;
pub const INST_FORCE_ROOT_ONLY: u32 = 4;
/// Set on VG-active instances: the authored-LOD selection must skip them.
pub const INST_LOD_SUPPRESS: u32 = 8;
pub const INST_CONE_CULL: u32 = 16;
pub const INST_TIER_SHIFT: u32 = 8;
pub const INST_TIER_MASK: u32 = 3;

/// One row per VG-active instance. 16 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct GpuVgInstance {
    pub asset_slot: u32,
    pub flags: u32,
    pub lod_error_scale: f32,
    /// Row in the scene join's `static_objects` buffer.
    pub object_row: u32,
}

impl GpuVgInstance {
    pub fn tier_lock(&self) -> TierLock {
        TierLock::from_bits(self.flags >> INST_TIER_SHIFT)
    }
    pub fn with_tier_lock(mut self, tier: TierLock) -> Self {
        self.flags = (self.flags & !(INST_TIER_MASK << INST_TIER_SHIFT))
            | ((tier as u32) << INST_TIER_SHIFT);
        self
    }
}

/// Global pipeline mode (`VirtualGeometrySettings`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VgMode {
    #[default]
    Auto,
    ForceNonRtFallback,
    ForceHwRaster,
    Off,
}

// ── Visibility buffer ──────────────────────────────────────────────────────

/// Raster-time id word: `visible_cluster_index << 7 | triangle`.
pub fn vis_pack(visible_cluster: u32, tri: u32) -> u32 {
    debug_assert!(visible_cluster < MAX_VISIBLE_CLUSTERS && tri <= VIS_TRI_MASK);
    (visible_cluster << VIS_TRI_BITS) | (tri & VIS_TRI_MASK)
}
pub fn vis_cluster(id: u32) -> u32 {
    id >> VIS_TRI_BITS
}
pub fn vis_tri(id: u32) -> u32 {
    id & VIS_TRI_MASK
}

/// 64-bit raster key raced on by software tiers: depth bits (reversed-Z, so
/// a larger float is nearer and `atomicMax` wins) in the high word, id in
/// the low word.
pub fn raster_key(depth: f32, id: u32) -> u64 {
    ((depth.to_bits() as u64) << 32) | id as u64
}
pub fn raster_key_depth(key: u64) -> f32 {
    f32::from_bits((key >> 32) as u32)
}
pub fn raster_key_id(key: u64) -> u32 {
    key as u32
}

/// Resolved unified payload word 1: barycentrics (v1, v2; v0 = 1 - u - v)
/// as two unorm16.
pub fn bary_pack(u: f32, v: f32) -> u32 {
    let q = |x: f32| (x.clamp(0.0, 1.0) * 65535.0).round() as u32;
    q(u) | (q(v) << 16)
}
pub fn bary_unpack(w: u32) -> (f32, f32) {
    ((w & 0xffff) as f32 / 65535.0, (w >> 16) as f32 / 65535.0)
}

// ── Files ──────────────────────────────────────────────────────────────────

/// `.vgeo` header. 72 bytes. Section byte offsets are from the start of the
/// file; the section contents are owned by `helio-vg-build` (T2) but must
/// stay expressible through this header.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct VgeoHeader {
    pub magic: [u8; 4],
    pub version: u32,
    /// Content id of the source mesh geometry (xxh3-128), low then high.
    pub content_id: [u64; 2],
    pub asset: GpuAssetHeader,
    pub hierarchy_offset: u32,
    pub group_refs_offset: u32,
    pub page_dir_offset: u32,
    pub pages_offset: u32,
}

/// Header at the start of every 128 KiB page. 32 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct PageHeader {
    pub magic: [u8; 4],
    pub version: u32,
    pub page_id: u32,
    pub cluster_count: u32,
    pub clusters_offset: u32,
    pub position_stream_offset: u32,
    pub attribute_stream_offset: u32,
    pub index_stream_offset: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of;

    #[test]
    fn root_leaves_follow_infinite_parent_error_slots() {
        let mut root = GpuHierarchyNode::default();
        root.children[0] = 1;
        root.max_parent_error[0] = f32::INFINITY;
        root.children[1] = NODE_LEAF_BIT | 5;
        root.max_parent_error[1] = 3.0; // finite: not a root leaf
        root.children[2] = NODE_LEAF_BIT | 9;
        root.max_parent_error[2] = f32::INFINITY;
        let mut inner = GpuHierarchyNode::default();
        inner.children[0] = NODE_LEAF_BIT | 2;
        inner.max_parent_error[0] = f32::INFINITY;
        let nodes = [root, inner];
        let mut leaves = root_leaves(&nodes, 0).unwrap();
        leaves.sort();
        assert_eq!(leaves, vec![2, 9]);
        let h = gpu_header_with_root_leaves(
            GpuAssetHeader {
                root_node: 0,
                ..Default::default()
            },
            &nodes,
        );
        assert_eq!((h.cluster_count, h.root_page_count, h.flags), (10, 3, 0)); // discovery order
        assert!(root_leaves(&[GpuHierarchyNode::default()], 0).is_none());
    }

    #[test]
    fn layouts_are_stable() {
        assert_eq!(size_of::<GpuCluster>(), 64);
        assert_eq!(size_of::<GpuHierarchyNode>(), 96);
        assert_eq!(size_of::<GpuGroupRef>(), 8);
        assert_eq!(size_of::<GpuAssetHeader>(), 96);
        assert_eq!(size_of::<GpuVgInstance>(), 16);
        assert_eq!(size_of::<VgeoHeader>(), 136);
        assert_eq!(size_of::<PageHeader>(), 32);
        assert_eq!(size_of::<GpuAssetQuant>(), 16);
        assert_eq!(size_of::<GpuVisibleCluster>(), 8);
        assert_eq!(size_of::<VgAssetReadyRow>(), 16);
    }

    #[test]
    fn decode_wgsl_validates_and_cone_convention() {
        let src = format!(
            "{WGSL}\n{}\n{DECODE_WGSL}
{DECODE_ATTR_WGSL}\n@compute @workgroup_size(1) fn main() {{
                let c = vgd_load_cluster(5u);
                let b = vgd_pos_block(5u, c, 0u);
                let p = vgd_pos(b, 0u);
                let t = vgd_tri(5u, c, 0u);
                let a = vgd_attr(5u, b, t.x);
                vg_page_pool[0] = u32(p.x + a.uv0.x);
            }}",
            decode_bindings_wgsl(0, 0, 1).replace(
                "var<storage, read> vg_page_pool",
                "var<storage, read_write> vg_page_pool"
            )
        );
        let module = naga::front::wgsl::parse_str(&src)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&src)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("validate");
        // cos(half-angle) convention: a cluster whose normals all point along
        // +Z within 30 degrees is culled only from behind the cone's horizon.
        let cut = 30f32.to_radians().cos();
        let axis = [0.0, 0.0, 1.0];
        // Camera far behind (looking along +Z toward the cluster): back-facing.
        assert!(cone_culled(
            [0.0, 0.0, -10.0],
            [0.0, 0.0, 0.0],
            1.0,
            axis,
            cut
        ));
        // Camera in front: front-facing, never culled.
        assert!(!cone_culled(
            [0.0, 0.0, 10.0],
            [0.0, 0.0, 0.0],
            1.0,
            axis,
            cut
        ));
        // Camera to the side inside the 60-degree-from-horizon band: not culled.
        assert!(!cone_culled(
            [10.0, 0.0, 0.0],
            [0.0, 0.0, 0.0],
            0.0,
            axis,
            cut
        ));
    }

    #[test]
    fn vis_and_key_round_trip() {
        let id = vis_pack(MAX_VISIBLE_CLUSTERS - 1, 127);
        assert_eq!(
            (vis_cluster(id), vis_tri(id)),
            (MAX_VISIBLE_CLUSTERS - 1, 127)
        );
        let key = raster_key(0.25, id);
        assert_eq!((raster_key_depth(key), raster_key_id(key)), (0.25, id));
        // Reversed-Z: nearer (larger) depth wins an unsigned compare.
        assert!(raster_key(0.5, 1) > raster_key(0.25, u32::MAX));
        let (u, v) = bary_unpack(bary_pack(0.25, 0.5));
        assert!((u - 0.25).abs() < 1e-4 && (v - 0.5).abs() < 1e-4);
    }

    #[test]
    fn cluster_and_cone_packing() {
        let c = GpuCluster {
            counts_material: GpuCluster::pack_counts(128, 97, 513),
            ..Default::default()
        };
        assert_eq!(
            (c.tri_count(), c.vert_count(), c.material_slot()),
            (128, 97, 513)
        );
        let (a, cut) = unpack_cone(pack_cone([0.0, -1.0, 0.5], 0.75));
        assert!(
            a[0] == 0.0 && a[1] == -1.0 && (a[2] - 0.5).abs() < 0.01 && (cut - 0.75).abs() < 0.01
        );
        let g = GpuGroupRef::new(9, 3, 8);
        assert_eq!((g.page_id, g.first_cluster(), g.cluster_count()), (9, 3, 8));
        let i = GpuVgInstance::default().with_tier_lock(TierLock::SwRaster);
        assert_eq!(i.tier_lock(), TierLock::SwRaster);
    }

    /// The WGSL mirror parses, validates, and its structs have the sizes of
    /// the Rust ones.
    #[test]
    fn wgsl_mirror_matches() {
        let src = format!("{WGSL}\n@compute @workgroup_size(1) fn main() {{}}");
        let module = naga::front::wgsl::parse_str(&src).expect("parse");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("validate");
        let mut layouter = naga::proc::Layouter::default();
        layouter.update(module.to_ctx()).expect("layout");
        let size_of_struct = |name: &str| {
            let (handle, _) = module
                .types
                .iter()
                .find(|(_, t)| t.name.as_deref() == Some(name))
                .unwrap_or_else(|| panic!("missing {name}"));
            layouter[handle].size as usize
        };
        assert_eq!(size_of_struct("VgCluster"), size_of::<GpuCluster>());
        assert_eq!(
            size_of_struct("VgHierarchyNode"),
            size_of::<GpuHierarchyNode>()
        );
        assert_eq!(size_of_struct("VgGroupRef"), size_of::<GpuGroupRef>());
        assert_eq!(size_of_struct("VgAssetHeader"), size_of::<GpuAssetHeader>());
        assert_eq!(size_of_struct("VgInstance"), size_of::<GpuVgInstance>());
    }
}
