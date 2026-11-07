// THE decode of virtual-geometry cluster streams (`helio_vg_format::DECODE_WGSL`).
// Every pass includes this one file; the CPU mirror is `helio_vg_build::VgeoReader::decode`.
//
// Requires, declared before this text (see `helio_vg_format::decode_bindings_wgsl`):
//   var<storage, read> vg_page_pool:  array<u32>;     // page slots, PAGE_BYTES each
//   var<storage, read> vg_asset_quant: array<vec4<f32>>; // xyz = lattice origin, w = step
// and `helio_vg_format::WGSL` (VgCluster).
//
// A `cluster_slot` is `pool_slot << 11 | local_cluster_index` (the cull's output).
//
// POSITION BLOCK at page byte `cluster.vertex_offset`:
//   u32 anchor_x, anchor_y, anchor_z     min lattice coordinate over the cluster
//   u32 bx | by << 8 | bz << 16          bits per axis (0..=18)
//   u32 attribute_offset                 page-relative byte offset of the attributes
//   bit stream, LSB first, per vertex: (x - anchor_x) in bx bits, then y, then z
//   position = origin + f32(q) * step    (asset-global 18-bit lattice, crack free)
// INDEX STREAM at page byte `cluster.index_offset`: u8 triplets.
// ATTRIBUTES (`DECODE_ATTR_WGSL`) at `attribute_offset + v * 16`: normal snorm8x4 | tangent snorm8x4
//   (w = bitangent sign) | uv0 f16x2 | uv1 f16x2.

const VGD_PAGE_WORDS: u32 = 32768u;
const VGD_SLOT_SHIFT: u32 = 11u;
const VGD_SLOT_MASK: u32 = 2047u;
const VGD_POS_HEADER_WORDS: u32 = 5u;
const VGD_ATTR_STRIDE: u32 = 16u;

struct VgdPos {
    anchor: vec3<u32>,
    bits: vec3<u32>,
    stride: u32,
    attr_byte: u32,
    stream_word: u32,
    origin: vec3<f32>,
    step: f32,
}

/// Word index in `vg_page_pool` of the first word of the slot's page.
fn vgd_page_word(cluster_slot: u32) -> u32 {
    return (cluster_slot >> VGD_SLOT_SHIFT) * VGD_PAGE_WORDS;
}

fn vgd_load_cluster(cluster_slot: u32) -> VgCluster {
    let base = vgd_page_word(cluster_slot);
    // PageHeader.clusters_offset is word 4.
    let w = base + (vg_page_pool[base + 4u] >> 2u) + (cluster_slot & VGD_SLOT_MASK) * 16u;
    var c: VgCluster;
    c.sphere = vec4<f32>(
        bitcast<f32>(vg_page_pool[w]), bitcast<f32>(vg_page_pool[w + 1u]),
        bitcast<f32>(vg_page_pool[w + 2u]), bitcast<f32>(vg_page_pool[w + 3u]));
    c.parent_sphere = vec4<f32>(
        bitcast<f32>(vg_page_pool[w + 4u]), bitcast<f32>(vg_page_pool[w + 5u]),
        bitcast<f32>(vg_page_pool[w + 6u]), bitcast<f32>(vg_page_pool[w + 7u]));
    c.cone = vg_page_pool[w + 8u];
    c.self_error = bitcast<f32>(vg_page_pool[w + 9u]);
    c.parent_error = bitcast<f32>(vg_page_pool[w + 10u]);
    c.counts_material = vg_page_pool[w + 11u];
    c.vertex_offset = vg_page_pool[w + 12u];
    c.index_offset = vg_page_pool[w + 13u];
    c.group_id = vg_page_pool[w + 14u];
    c.flags = vg_page_pool[w + 15u];
    return c;
}

fn vgd_tri_count(cl: VgCluster) -> u32 { return cl.counts_material & 0xffu; }
fn vgd_vert_count(cl: VgCluster) -> u32 { return (cl.counts_material >> 8u) & 0xffu; }
fn vgd_material(cl: VgCluster) -> u32 { return cl.counts_material >> 16u; }

fn vgd_pos_block(cluster_slot: u32, cl: VgCluster, asset_slot: u32) -> VgdPos {
    let w = vgd_page_word(cluster_slot) + (cl.vertex_offset >> 2u);
    var p: VgdPos;
    p.anchor = vec3<u32>(vg_page_pool[w], vg_page_pool[w + 1u], vg_page_pool[w + 2u]);
    let d = vg_page_pool[w + 3u];
    p.bits = vec3<u32>(d & 0xffu, (d >> 8u) & 0xffu, (d >> 16u) & 0xffu);
    p.stride = p.bits.x + p.bits.y + p.bits.z;
    p.attr_byte = vg_page_pool[w + 4u];
    p.stream_word = w + VGD_POS_HEADER_WORDS;
    let q = vg_asset_quant[asset_slot];
    p.origin = q.xyz;
    p.step = q.w;
    return p;
}

// n <= 18 bits at absolute bit `bit` of the stream starting at word `word0`.
fn vgd_read_bits(word0: u32, bit: u32, n: u32) -> u32 {
    let w = word0 + (bit >> 5u);
    let s = bit & 31u;
    var v = vg_page_pool[w] >> s;
    if (s + n > 32u) {
        v = v | (vg_page_pool[w + 1u] << (32u - s));
    }
    return v & ((1u << n) - 1u);
}

/// Lattice coordinate of local vertex `i`.
fn vgd_lattice(p: VgdPos, i: u32) -> vec3<u32> {
    let b = i * p.stride;
    let qx = vgd_read_bits(p.stream_word, b, p.bits.x);
    let qy = vgd_read_bits(p.stream_word, b + p.bits.x, p.bits.y);
    let qz = vgd_read_bits(p.stream_word, b + p.bits.x + p.bits.y, p.bits.z);
    return p.anchor + vec3<u32>(qx, qy, qz);
}

/// Asset-space position of local vertex `i`.
fn vgd_pos(p: VgdPos, i: u32) -> vec3<f32> {
    return p.origin + vec3<f32>(vgd_lattice(p, i)) * p.step;
}

fn vgd_byte(cluster_slot: u32, page_byte: u32) -> u32 {
    let a = vgd_page_word(cluster_slot) * 4u + page_byte;
    return (vg_page_pool[a >> 2u] >> ((a & 3u) * 8u)) & 0xffu;
}

/// The three local vertex indices of triangle `tri`.
fn vgd_tri(cluster_slot: u32, cl: VgCluster, tri: u32) -> vec3<u32> {
    let a = cl.index_offset + tri * 3u;
    return vec3<u32>(vgd_byte(cluster_slot, a), vgd_byte(cluster_slot, a + 1u), vgd_byte(cluster_slot, a + 2u));
}

