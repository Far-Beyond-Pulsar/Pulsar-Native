// GPU Morton LBVH over virtual-geometry instances. Prepend `vg_bvh_nodes.wgsl`.
//
// Pipeline (rebuild):
//   cs_gather_local  per-256-row block scan of "live" flags, rank + block totals,
//                    and the world bounds of the live centroids (atomics)
//   cs_block_scan    prefix over block totals -> live count, indirect args
//   cs_gather_write  deterministic compaction + 30-bit Morton key per live row
//   cs_histogram / cs_scan / cs_scatter  x30   stable 1-bit LSD radix sort
//   cs_chunk_eval    decide which aligned chunks of the sorted order merge into
//                    one batch leaf (all tiny, spatially compact)
//   cs_chunk_scan    prefix over chunk leaf counts -> leaf/node counts, args
//   cs_make_leaves   write leaf nodes (first item + count)
//   cs_karras        Karras 2012 internal-node construction + parent links
// then (rebuild AND refit):
//   cs_refit_leaves  leaf bounds from the live object bounds + bottom-up union
//                    (atomic counter: the second child to arrive does the parent)
//   cs_finalize      copy bounds into the nodes, update the header
//
// Bounds exchanged between workgroups during the bottom-up pass go through
// relaxed atomics (`rb`) rather than plain storage so a consumer never reads a
// stale cache line on GPUs whose plain loads are not coherent across
// workgroups.

const WG: u32 = 256u;
const LWG: u32 = 64u;
const BIG: f32 = 1.0e30;
const INST_ENABLED: u32 = 1u; // helio-vg-format INST_ENABLED

struct BuildCfg {
    /// Rows in the instance buffer (capacity, not live count).
    capacity: u32,
    /// Items per batch-leaf chunk, 1..=64 (1 disables batching).
    chunk_size: u32,
    /// An instance is "tiny" when its bounding box side is <= this.
    tiny_extent: f32,
    /// A chunk merges only if its combined AABB's longest side is <= this.
    cell_extent: f32,
}

struct SortBit {
    bit: u32,
}

struct BvhInstance {
    asset_slot: u32,
    flags: u32,
    lod_error_scale: f32,
    object_row: u32,
}

/// `helio_pass_gbuffer::StaticObjectComponent`, byte for byte (see
/// `object_batch.wgsl`). Only `mesh_generation` and `bounds` are read.
struct StaticObjectRow {
    mesh_slot: u32,
    mesh_generation: u32,
    material_slot: u32,
    material_generation: u32,
    transform: array<array<f32, 4>, 4>,
    prev_transform: array<array<f32, 4>, 4>,
    normal_mat: array<array<f32, 4>, 3>,
    bounds: array<f32, 4>,
    index_count: u32,
    first_index: u32,
    vertex_offset: i32,
    material_class: u32,
    graph_hash_lo: u32,
    graph_hash_hi: u32,
    flags: u32,
    skin_vertex_offset: u32,
    bone_palette_offset: u32,
    bone_palette_count: u32,
    _skin_pad: u32,
}

/// A row is in the tree when its instance is enabled, its object row exists,
/// is alive, and its world bounding sphere (centre xyz, radius w) is finite.
fn bounds_valid(g: u32, b: array<f32, 4>) -> bool {
    return g != 0u
        && abs(b[0]) < BIG && abs(b[1]) < BIG && abs(b[2]) < BIG
        && b[3] >= 0.0 && b[3] < BIG;
}

// Order-preserving float <-> u32 mapping so atomicMax works on floats.
fn f2u(f: f32) -> u32 {
    let b = bitcast<u32>(f);
    return select(b ^ 0x80000000u, ~b, (b & 0x80000000u) != 0u);
}
fn u2f(u: u32) -> f32 {
    return bitcast<f32>(select(~u, u ^ 0x80000000u, (u & 0x80000000u) != 0u));
}

fn expand_bits(v: u32) -> u32 {
    var x = v & 0x3ffu;
    x = (x | (x << 16u)) & 0x030000ffu;
    x = (x | (x << 8u)) & 0x0300f00fu;
    x = (x | (x << 4u)) & 0x030c30c3u;
    x = (x | (x << 2u)) & 0x09249249u;
    return x;
}

// ── Stage 1: cs_gather_local ────────────────────────────────────────────────
// Scene-bounds words: [0..3) = ~enc(min.xyz) (so atomicMax yields the min),
// [3..6) = enc(max.xyz). Zero-initialised before the stage.

@group(0) @binding(0) var<uniform> gl_cfg: BuildCfg;
@group(0) @binding(1) var<storage, read> gl_instances: array<BvhInstance>;
@group(0) @binding(2) var<storage, read> gl_objects: array<StaticObjectRow>;
@group(0) @binding(3) var<storage, read_write> gl_scene: array<atomic<u32>>;
@group(0) @binding(4) var<storage, read_write> gl_rank: array<u32>;
@group(0) @binding(5) var<storage, read_write> gl_totals: array<u32>;

var<workgroup> g_scan: array<u32, 256>;

@compute @workgroup_size(256)
fn cs_gather_local(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
    @builtin(workgroup_id) wgid: vec3<u32>,
) {
    let i = gid.x;
    var live = false;
    var c = vec3<f32>(0.0);
    if i < gl_cfg.capacity {
        let inst = gl_instances[i];
        if (inst.flags & INST_ENABLED) != 0u && inst.object_row < arrayLength(&gl_objects) {
            let g = gl_objects[inst.object_row].mesh_generation;
            let b = gl_objects[inst.object_row].bounds;
            live = bounds_valid(g, b);
            c = vec3<f32>(b[0], b[1], b[2]);
        }
    }
    g_scan[lid.x] = select(0u, 1u, live);
    workgroupBarrier();
    var offset = 1u;
    loop {
        if offset >= WG {
            break;
        }
        var v = 0u;
        if lid.x >= offset {
            v = g_scan[lid.x - offset];
        }
        workgroupBarrier();
        g_scan[lid.x] += v;
        workgroupBarrier();
        offset = offset * 2u;
    }
    let inclusive = g_scan[lid.x];
    if live {
        gl_rank[i] = inclusive - 1u;
        atomicMax(&gl_scene[0], 0xffffffffu - f2u(c.x));
        atomicMax(&gl_scene[1], 0xffffffffu - f2u(c.y));
        atomicMax(&gl_scene[2], 0xffffffffu - f2u(c.z));
        atomicMax(&gl_scene[3], f2u(c.x));
        atomicMax(&gl_scene[4], f2u(c.y));
        atomicMax(&gl_scene[5], f2u(c.z));
    }
    if lid.x == WG - 1u {
        gl_totals[wgid.x] = inclusive;
    }
}

// ── Stage 2: cs_block_scan ──────────────────────────────────────────────────
// Dispatch-args slots (3 u32 each): 0 = items/256, 1 = chunks, 2 = leaves/64,
// 3 = internal nodes/64, 4 = nodes/64 (>= 1).

@group(0) @binding(0) var<uniform> bs_cfg: BuildCfg;
@group(0) @binding(1) var<storage, read_write> bs_header: VgBvhHeader;
@group(0) @binding(2) var<storage, read_write> bs_totals: array<u32>;
@group(0) @binding(3) var<storage, read_write> bs_args: array<u32>;

@compute @workgroup_size(1)
fn cs_block_scan() {
    let nb = (bs_cfg.capacity + WG - 1u) / WG;
    var run = 0u;
    for (var b = 0u; b < nb; b++) {
        let t = bs_totals[b];
        bs_totals[b] = run;
        run += t;
    }
    bs_header.instance_count = run;
    bs_header.build_generation += 1u;
    let cs = max(bs_cfg.chunk_size, 1u);
    bs_args[0] = (run + WG - 1u) / WG;
    bs_args[1] = 1u;
    bs_args[2] = 1u;
    bs_args[3] = (run + cs - 1u) / cs;
    bs_args[4] = 1u;
    bs_args[5] = 1u;
}

// ── Stage 3: cs_gather_write ────────────────────────────────────────────────

@group(0) @binding(0) var<uniform> gw_cfg: BuildCfg;
@group(0) @binding(1) var<storage, read> gw_instances: array<BvhInstance>;
@group(0) @binding(2) var<storage, read> gw_objects: array<StaticObjectRow>;
@group(0) @binding(3) var<storage, read_write> gw_scene: array<atomic<u32>>;
@group(0) @binding(4) var<storage, read_write> gw_rank: array<u32>;
@group(0) @binding(5) var<storage, read_write> gw_base: array<u32>;
@group(0) @binding(6) var<storage, read_write> gw_keys: array<u32>;
@group(0) @binding(7) var<storage, read_write> gw_idx: array<u32>;

@compute @workgroup_size(256)
fn cs_gather_write(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(workgroup_id) wgid: vec3<u32>,
) {
    let i = gid.x;
    if i >= gw_cfg.capacity {
        return;
    }
    let inst = gw_instances[i];
    if (inst.flags & INST_ENABLED) == 0u || inst.object_row >= arrayLength(&gw_objects) {
        return;
    }
    let g = gw_objects[inst.object_row].mesh_generation;
    let b = gw_objects[inst.object_row].bounds;
    if !bounds_valid(g, b) {
        return;
    }
    let c = vec3<f32>(b[0], b[1], b[2]);
    let bmin = vec3<f32>(
        u2f(0xffffffffu - atomicLoad(&gw_scene[0])),
        u2f(0xffffffffu - atomicLoad(&gw_scene[1])),
        u2f(0xffffffffu - atomicLoad(&gw_scene[2])),
    );
    let bmax = vec3<f32>(
        u2f(atomicLoad(&gw_scene[3])),
        u2f(atomicLoad(&gw_scene[4])),
        u2f(atomicLoad(&gw_scene[5])),
    );
    let ext = max(bmax - bmin, vec3<f32>(1.0e-20));
    let n = clamp((c - bmin) / ext, vec3<f32>(0.0), vec3<f32>(1.0));
    let q = min(vec3<u32>(n * 1024.0), vec3<u32>(1023u));
    let key = (expand_bits(q.x) << 2u) | (expand_bits(q.y) << 1u) | expand_bits(q.z);
    let slot = gw_base[wgid.x] + gw_rank[i];
    gw_keys[slot] = key;
    gw_idx[slot] = i;
}

// ── Stage 4: radix sort (stable, 1 bit per pass) ────────────────────────────
// Same scheme as `helio-pass-object-batch`'s `cs_histogram`/`cs_scan`/
// `cs_scatter`, with the count read from the header. 30 passes (Morton keys are
// 30 bits), so the sorted result lands back in buffer A.

@group(0) @binding(0) var<uniform> h_bit: SortBit;
@group(0) @binding(1) var<storage, read> h_header: VgBvhHeader;
@group(0) @binding(2) var<storage, read> h_keys: array<u32>;
@group(0) @binding(3) var<storage, read_write> h_hist: array<u32>;

var<workgroup> hist_ones: atomic<u32>;
var<workgroup> hist_total: atomic<u32>;

@compute @workgroup_size(256)
fn cs_histogram(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
    @builtin(workgroup_id) wgid: vec3<u32>,
) {
    if lid.x == 0u {
        atomicStore(&hist_ones, 0u);
        atomicStore(&hist_total, 0u);
    }
    workgroupBarrier();
    if gid.x < h_header.instance_count {
        atomicAdd(&hist_total, 1u);
        if ((h_keys[gid.x] >> h_bit.bit) & 1u) == 1u {
            atomicAdd(&hist_ones, 1u);
        }
    }
    workgroupBarrier();
    if lid.x == 0u {
        let ones = atomicLoad(&hist_ones);
        let total = atomicLoad(&hist_total);
        h_hist[wgid.x * 2u + 0u] = total - ones;
        h_hist[wgid.x * 2u + 1u] = ones;
    }
}

@group(0) @binding(0) var<storage, read> s_header: VgBvhHeader;
@group(0) @binding(1) var<storage, read_write> s_hist: array<u32>;

@compute @workgroup_size(1)
fn cs_scan() {
    let nb = max((s_header.instance_count + WG - 1u) / WG, 1u);
    var total_zeros = 0u;
    for (var blk = 0u; blk < nb; blk++) {
        total_zeros += s_hist[blk * 2u + 0u];
    }
    var run_zero = 0u;
    var run_one = total_zeros;
    for (var blk = 0u; blk < nb; blk++) {
        let cz = s_hist[blk * 2u + 0u];
        let co = s_hist[blk * 2u + 1u];
        s_hist[blk * 2u + 0u] = run_zero;
        s_hist[blk * 2u + 1u] = run_one;
        run_zero += cz;
        run_one += co;
    }
}

@group(0) @binding(0) var<uniform> c_bit: SortBit;
@group(0) @binding(1) var<storage, read> c_header: VgBvhHeader;
@group(0) @binding(2) var<storage, read> c_src_keys: array<u32>;
@group(0) @binding(3) var<storage, read> c_src_idx: array<u32>;
@group(0) @binding(4) var<storage, read_write> c_dst_keys: array<u32>;
@group(0) @binding(5) var<storage, read_write> c_dst_idx: array<u32>;
@group(0) @binding(6) var<storage, read> c_offsets: array<u32>;

var<workgroup> scan_buf: array<u32, 256>;

@compute @workgroup_size(256)
fn cs_scatter(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
    @builtin(workgroup_id) wgid: vec3<u32>,
) {
    let has_elem = gid.x < c_header.instance_count;
    let key = select(0u, c_src_keys[gid.x], has_elem);
    let bit = select(0u, (key >> c_bit.bit) & 1u, has_elem);
    scan_buf[lid.x] = bit;
    workgroupBarrier();
    var offset = 1u;
    loop {
        if offset >= WG {
            break;
        }
        var v = 0u;
        if lid.x >= offset {
            v = scan_buf[lid.x - offset];
        }
        workgroupBarrier();
        scan_buf[lid.x] += v;
        workgroupBarrier();
        offset = offset * 2u;
    }
    let inclusive_ones = scan_buf[lid.x];
    if !has_elem {
        return;
    }
    var local_pos: u32;
    if bit == 1u {
        local_pos = inclusive_ones - 1u;
    } else {
        local_pos = lid.x - inclusive_ones;
    }
    let dst = c_offsets[wgid.x * 2u + bit] + local_pos;
    c_dst_keys[dst] = key;
    c_dst_idx[dst] = c_src_idx[gid.x];
}

// ── Stage 5: cs_chunk_eval ──────────────────────────────────────────────────
// One workgroup per aligned chunk of `chunk_size` sorted items.

@group(0) @binding(0) var<uniform> ce_cfg: BuildCfg;
@group(0) @binding(1) var<storage, read> ce_header: VgBvhHeader;
@group(0) @binding(2) var<storage, read> ce_instances: array<BvhInstance>;
@group(0) @binding(3) var<storage, read> ce_objects: array<StaticObjectRow>;
@group(0) @binding(4) var<storage, read> ce_idx: array<u32>;
@group(0) @binding(5) var<storage, read_write> ce_leafcount: array<u32>;
@group(0) @binding(6) var<storage, read_write> ce_merge: array<u32>;

var<workgroup> ce_min: array<vec3<f32>, 64>;
var<workgroup> ce_max: array<vec3<f32>, 64>;
var<workgroup> ce_tiny: array<u32, 64>;
var<workgroup> ce_valid: array<u32, 64>;

@compute @workgroup_size(64)
fn cs_chunk_eval(
    @builtin(local_invocation_id) lid: vec3<u32>,
    @builtin(workgroup_id) wgid: vec3<u32>,
) {
    let cs = clamp(ce_cfg.chunk_size, 1u, 64u);
    let count = ce_header.instance_count;
    let c = wgid.x;
    let p = c * cs + lid.x;
    var mn = vec3<f32>(BIG);
    var mx = vec3<f32>(-BIG);
    var tiny = 1u;
    var valid = 0u;
    if lid.x < cs && p < count {
        let row = ce_idx[p];
        let inst = ce_instances[row];
        let b = ce_objects[inst.object_row].bounds;
        let ctr = vec3<f32>(b[0], b[1], b[2]);
        mn = ctr - b[3];
        mx = ctr + b[3];
        tiny = select(0u, 1u, 2.0 * b[3] <= ce_cfg.tiny_extent);
        valid = 1u;
    }
    ce_min[lid.x] = mn;
    ce_max[lid.x] = mx;
    ce_tiny[lid.x] = tiny;
    ce_valid[lid.x] = valid;
    workgroupBarrier();
    var s = 32u;
    loop {
        if s == 0u {
            break;
        }
        if lid.x < s {
            ce_min[lid.x] = min(ce_min[lid.x], ce_min[lid.x + s]);
            ce_max[lid.x] = max(ce_max[lid.x], ce_max[lid.x + s]);
            ce_tiny[lid.x] = min(ce_tiny[lid.x], ce_tiny[lid.x + s]);
            ce_valid[lid.x] = ce_valid[lid.x] + ce_valid[lid.x + s];
        }
        workgroupBarrier();
        s = s / 2u;
    }
    if lid.x == 0u {
        let n = ce_valid[0];
        let d = ce_max[0] - ce_min[0];
        let longest = max(d.x, max(d.y, d.z));
        let merge = cs >= 2u && n >= 2u && ce_tiny[0] == 1u && longest <= ce_cfg.cell_extent;
        ce_leafcount[c] = select(n, 1u, merge);
        ce_merge[c] = select(0u, 1u, merge);
    }
}

// ── Stage 6: cs_chunk_scan ──────────────────────────────────────────────────

@group(0) @binding(0) var<uniform> cc_cfg: BuildCfg;
@group(0) @binding(1) var<storage, read_write> cc_header: VgBvhHeader;
@group(0) @binding(2) var<storage, read> cc_leafcount: array<u32>;
@group(0) @binding(3) var<storage, read> cc_merge: array<u32>;
@group(0) @binding(4) var<storage, read_write> cc_base: array<u32>;
@group(0) @binding(5) var<storage, read_write> cc_args: array<u32>;

@compute @workgroup_size(1)
fn cs_chunk_scan() {
    let cs = clamp(cc_cfg.chunk_size, 1u, 64u);
    let count = cc_header.instance_count;
    let nc = (count + cs - 1u) / cs;
    var run = 0u;
    var merged = 0u;
    for (var c = 0u; c < nc; c++) {
        cc_base[c] = run;
        run += cc_leafcount[c];
        merged += cc_merge[c];
    }
    let m = run;
    let internal = select(0u, m - 1u, m > 0u);
    let nodes = select(0u, 2u * m - 1u, m > 0u);
    cc_header.leaf_count = m;
    cc_header.internal_count = internal;
    cc_header.node_count = nodes;
    cc_header.root_node = select(VG_BVH_INVALID, 0u, m > 0u);
    cc_header.batch_leaf_count = merged;
    cc_header.dispatch_leaves = array<u32, 3>((m + LWG - 1u) / LWG, 1u, 1u);
    cc_header.dispatch_instances = array<u32, 3>((count + LWG - 1u) / LWG, 1u, 1u);
    cc_header.dispatch_nodes = array<u32, 3>((nodes + LWG - 1u) / LWG, 1u, 1u);
    cc_args[6] = (m + LWG - 1u) / LWG;
    cc_args[7] = 1u;
    cc_args[8] = 1u;
    cc_args[9] = (internal + LWG - 1u) / LWG;
    cc_args[10] = 1u;
    cc_args[11] = 1u;
    cc_args[12] = max((nodes + LWG - 1u) / LWG, 1u);
    cc_args[13] = 1u;
    cc_args[14] = 1u;
}

// ── Stage 7: cs_make_leaves ─────────────────────────────────────────────────

@group(0) @binding(0) var<uniform> ml_cfg: BuildCfg;
@group(0) @binding(1) var<storage, read> ml_header: VgBvhHeader;
@group(0) @binding(2) var<storage, read> ml_merge: array<u32>;
@group(0) @binding(3) var<storage, read> ml_base: array<u32>;
@group(0) @binding(4) var<storage, read_write> ml_nodes: array<VgBvhNode>;

@compute @workgroup_size(64)
fn cs_make_leaves(
    @builtin(local_invocation_id) lid: vec3<u32>,
    @builtin(workgroup_id) wgid: vec3<u32>,
) {
    let cs = clamp(ml_cfg.chunk_size, 1u, 64u);
    let count = ml_header.instance_count;
    let c = wgid.x;
    let t = lid.x;
    let n = min(cs, count - c * cs);
    if t >= n {
        return;
    }
    let m = ml_header.leaf_count;
    let base = ml_base[c];
    if ml_merge[c] == 1u {
        if t == 0u {
            let node = (m - 1u) + base;
            ml_nodes[node].a = VG_BVH_LEAF_BIT | n;
            ml_nodes[node].b = c * cs;
        }
    } else {
        let node = (m - 1u) + base + t;
        ml_nodes[node].a = VG_BVH_LEAF_BIT | 1u;
        ml_nodes[node].b = c * cs + t;
    }
}

// ── Stage 8: cs_karras ──────────────────────────────────────────────────────
// Karras, "Maximizing Parallelism in the Construction of BVHs, Octrees, and
// k-d Trees" (2012). Equal Morton keys are disambiguated by leaf index.

@group(0) @binding(0) var<storage, read> kr_header: VgBvhHeader;
@group(0) @binding(1) var<storage, read> kr_keys: array<u32>;
@group(0) @binding(2) var<storage, read_write> kr_nodes: array<VgBvhNode>;
@group(0) @binding(3) var<storage, read_write> kr_parents: array<u32>;

var<private> kr_m: u32;

fn kr_leaf_key(l: i32) -> u32 {
    return kr_keys[kr_nodes[u32(l) + kr_m - 1u].b];
}

fn kr_delta(i: i32, j: i32) -> i32 {
    if j < 0 || j >= i32(kr_m) {
        return -1;
    }
    let ki = kr_leaf_key(i);
    let kj = kr_leaf_key(j);
    if ki == kj {
        return 32 + i32(countLeadingZeros(u32(i) ^ u32(j)));
    }
    return i32(countLeadingZeros(ki ^ kj));
}

@compute @workgroup_size(64)
fn cs_karras(@builtin(global_invocation_id) gid: vec3<u32>) {
    kr_m = kr_header.leaf_count;
    if gid.x + 1u >= kr_m {
        return;
    }
    let i = i32(gid.x);
    let dp = kr_delta(i, i + 1);
    let dm = kr_delta(i, i - 1);
    let d = select(-1, 1, dp > dm);
    let dmin = kr_delta(i, i - d);
    var lmax = 2;
    loop {
        if kr_delta(i, i + lmax * d) > dmin {
            lmax = lmax * 2;
        } else {
            break;
        }
    }
    var l = 0;
    var t = lmax / 2;
    loop {
        if t < 1 {
            break;
        }
        if kr_delta(i, i + (l + t) * d) > dmin {
            l = l + t;
        }
        t = t / 2;
    }
    let j = i + l * d;
    let dnode = kr_delta(i, j);
    var s = 0;
    var t2 = l;
    loop {
        t2 = (t2 + 1) / 2;
        if kr_delta(i, i + (s + t2) * d) > dnode {
            s = s + t2;
        }
        if t2 <= 1 {
            break;
        }
    }
    let gamma = i + s * d + min(d, 0);
    let lo = min(i, j);
    let hi = max(i, j);
    var left = u32(gamma);
    if lo == gamma {
        left = (kr_m - 1u) + u32(gamma);
    }
    var right = u32(gamma + 1);
    if hi == gamma + 1 {
        right = (kr_m - 1u) + u32(gamma + 1);
    }
    kr_nodes[gid.x].a = left;
    kr_nodes[gid.x].b = right;
    kr_parents[left] = gid.x;
    kr_parents[right] = gid.x;
}

// ── Stage 9: cs_refit_leaves ────────────────────────────────────────────────
// Node bounds live in `rb` (8 words per node: min.xyz, max.xyz as float bits).

@group(0) @binding(0) var<storage, read> rl_header: VgBvhHeader;
@group(0) @binding(1) var<storage, read> rl_instances: array<BvhInstance>;
@group(0) @binding(2) var<storage, read> rl_objects: array<StaticObjectRow>;
@group(0) @binding(3) var<storage, read> rl_idx: array<u32>;
@group(0) @binding(4) var<storage, read> rl_nodes: array<VgBvhNode>;
@group(0) @binding(5) var<storage, read> rl_parents: array<u32>;
@group(0) @binding(6) var<storage, read_write> rl_rb: array<atomic<u32>>;
@group(0) @binding(7) var<storage, read_write> rl_counters: array<atomic<u32>>;

fn rl_load(n: u32) -> array<vec3<f32>, 2> {
    let o = n * 8u;
    return array<vec3<f32>, 2>(
        vec3<f32>(
            bitcast<f32>(atomicLoad(&rl_rb[o + 0u])),
            bitcast<f32>(atomicLoad(&rl_rb[o + 1u])),
            bitcast<f32>(atomicLoad(&rl_rb[o + 2u])),
        ),
        vec3<f32>(
            bitcast<f32>(atomicLoad(&rl_rb[o + 3u])),
            bitcast<f32>(atomicLoad(&rl_rb[o + 4u])),
            bitcast<f32>(atomicLoad(&rl_rb[o + 5u])),
        ),
    );
}

fn rl_store(n: u32, mn: vec3<f32>, mx: vec3<f32>) {
    let o = n * 8u;
    atomicStore(&rl_rb[o + 0u], bitcast<u32>(mn.x));
    atomicStore(&rl_rb[o + 1u], bitcast<u32>(mn.y));
    atomicStore(&rl_rb[o + 2u], bitcast<u32>(mn.z));
    atomicStore(&rl_rb[o + 3u], bitcast<u32>(mx.x));
    atomicStore(&rl_rb[o + 4u], bitcast<u32>(mx.y));
    atomicStore(&rl_rb[o + 5u], bitcast<u32>(mx.z));
}

@compute @workgroup_size(64)
fn cs_refit_leaves(@builtin(global_invocation_id) gid: vec3<u32>) {
    let m = rl_header.leaf_count;
    if gid.x >= m {
        return;
    }
    var node = (m - 1u) + gid.x;
    let cnt = rl_nodes[node].a & 0xffu;
    let first = rl_nodes[node].b;
    var mn = vec3<f32>(BIG);
    var mx = vec3<f32>(-BIG);
    for (var k = 0u; k < cnt; k++) {
        let inst = rl_instances[rl_idx[first + k]];
        if (inst.flags & INST_ENABLED) == 0u || inst.object_row >= arrayLength(&rl_objects) {
            continue;
        }
        let g = rl_objects[inst.object_row].mesh_generation;
        let b = rl_objects[inst.object_row].bounds;
        if !bounds_valid(g, b) {
            continue;
        }
        let ctr = vec3<f32>(b[0], b[1], b[2]);
        mn = min(mn, ctr - b[3]);
        mx = max(mx, ctr + b[3]);
    }
    rl_store(node, mn, mx);
    // Bottom-up: the second child to arrive combines both and continues.
    loop {
        if node == 0u {
            break;
        }
        let p = rl_parents[node];
        let prev = atomicAdd(&rl_counters[p], 1u);
        if prev == 0u {
            break;
        }
        let lc = rl_nodes[p].a;
        let rc = rl_nodes[p].b;
        let lb = rl_load(lc);
        let rb = rl_load(rc);
        rl_store(p, min(lb[0], rb[0]), max(lb[1], rb[1]));
        node = p;
    }
}

// ── Stage 10: cs_finalize ───────────────────────────────────────────────────

@group(0) @binding(0) var<storage, read_write> fi_header: VgBvhHeader;
@group(0) @binding(1) var<storage, read_write> fi_nodes: array<VgBvhNode>;
@group(0) @binding(2) var<storage, read_write> fi_rb: array<atomic<u32>>;

fn fi_min(n: u32) -> vec3<f32> {
    let o = n * 8u;
    return vec3<f32>(
        bitcast<f32>(atomicLoad(&fi_rb[o + 0u])),
        bitcast<f32>(atomicLoad(&fi_rb[o + 1u])),
        bitcast<f32>(atomicLoad(&fi_rb[o + 2u])),
    );
}
fn fi_max(n: u32) -> vec3<f32> {
    let o = n * 8u;
    return vec3<f32>(
        bitcast<f32>(atomicLoad(&fi_rb[o + 3u])),
        bitcast<f32>(atomicLoad(&fi_rb[o + 4u])),
        bitcast<f32>(atomicLoad(&fi_rb[o + 5u])),
    );
}

@compute @workgroup_size(64)
fn cs_finalize(@builtin(global_invocation_id) gid: vec3<u32>) {
    let nn = fi_header.node_count;
    if gid.x < nn {
        fi_nodes[gid.x].bmin = fi_min(gid.x);
        fi_nodes[gid.x].bmax = fi_max(gid.x);
    }
    if gid.x == 0u {
        fi_header.refit_generation += 1u;
        if nn > 0u {
            let mn = fi_min(0u);
            let mx = fi_max(0u);
            fi_header.bounds_min = array<f32, 3>(mn.x, mn.y, mn.z);
            fi_header.bounds_max = array<f32, 3>(mx.x, mx.y, mx.z);
        } else {
            fi_header.bounds_min = array<f32, 3>(0.0, 0.0, 0.0);
            fi_header.bounds_max = array<f32, 3>(0.0, 0.0, 0.0);
        }
    }
}
