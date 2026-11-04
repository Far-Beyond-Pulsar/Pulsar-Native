// Virtual geometry shared declarations. Mirrors `helio-vg-format/src/lib.rs`;
// a size or field-order change must be made in both (tests enforce sizes).

const VG_CLUSTER_MAX_TRIS: u32 = 128u;
const VG_CLUSTER_MAX_VERTS: u32 = 128u;
const VG_VIS_TRI_BITS: u32 = 7u;
const VG_VIS_TRI_MASK: u32 = 127u;
const VG_VIS_CLUSTER_BITS: u32 = 25u;
const VG_NODE_LEAF_BIT: u32 = 0x80000000u;
const VG_NODE_INVALID: u32 = 0xffffffffu;

const VG_INST_ENABLED: u32 = 1u;
const VG_INST_SHADOW_CASTER: u32 = 2u;
const VG_INST_FORCE_ROOT_ONLY: u32 = 4u;
const VG_INST_LOD_SUPPRESS: u32 = 8u;
const VG_INST_CONE_CULL: u32 = 16u;
const VG_INST_TIER_SHIFT: u32 = 8u;
const VG_INST_TIER_MASK: u32 = 3u;

struct VgCluster {
    sphere: vec4<f32>,
    parent_sphere: vec4<f32>,
    cone: u32,
    self_error: f32,
    parent_error: f32,
    counts_material: u32,
    vertex_offset: u32,
    index_offset: u32,
    group_id: u32,
    flags: u32,
}

struct VgHierarchyNode {
    spheres: array<vec4<f32>, 4>,
    max_parent_error: vec4<f32>,
    children: vec4<u32>,
}

struct VgGroupRef {
    page_id: u32,
    first_cluster_count: u32,
}

struct VgAssetHeader {
    root_node: u32,
    node_count: u32,
    group_count: u32,
    page_count: u32,
    cluster_count: u32,
    root_page_count: u32,
    triangle_count: u32,
    flags: u32,
    // Object-space bounding sphere (xyz centre, w radius).
    bounds: vec4<f32>,
    // Tolerance of lod_table entry 0 and the table (see `LodTable`).
    lod_error_max: f32,
    lod_table: array<u32, 8>,
    reserved: array<u32, 3>,
}

// Mirror of `LodTable::tris_at`: triangles selected at object-space tolerance e.
fn vg_lod_tris(h: VgAssetHeader, e: f32) -> f32 {
    if !(h.lod_error_max > 0.0) { return 0.0; }
    var x = 31.0;
    if e > 0.0 { x = clamp(2.0 * log2(h.lod_error_max / e), 0.0, 31.0); }
    let k = u32(floor(x));
    let t = x - f32(k);
    let k1 = min(k + 1u, 31u);
    let a = f32((h.lod_table[k >> 2u] >> ((k & 3u) * 8u)) & 255u);
    let b = f32((h.lod_table[k1 >> 2u] >> ((k1 & 3u) * 8u)) & 255u);
    return exp2((a + (b - a) * t) / 8.0);
}

struct VgInstance {
    asset_slot: u32,
    flags: u32,
    lod_error_scale: f32,
    object_row: u32,
}

struct VgAssetQuant {
    origin: vec3<f32>,
    step: f32,
}

struct VgVisibleCluster {
    instance: u32,
    cluster_slot: u32,
}

const VG_PAGE_CLUSTER_SHIFT: u32 = 11u;
const VG_PAGE_CLUSTER_MASK: u32 = 2047u;

fn vg_vis_pack(cluster: u32, tri: u32) -> u32 {
    return (cluster << VG_VIS_TRI_BITS) | (tri & VG_VIS_TRI_MASK);
}
fn vg_vis_cluster(id: u32) -> u32 { return id >> VG_VIS_TRI_BITS; }
fn vg_vis_tri(id: u32) -> u32 { return id & VG_VIS_TRI_MASK; }
fn vg_raster_key(depth: f32, id: u32) -> vec2<u32> { return vec2<u32>(bitcast<u32>(depth), id); }
