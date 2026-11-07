// Tier-3-NonRT stage 2: per dense tile, bucket-partitioned front-to-back depth
// peel. One workgroup (TILE x TILE threads, one pixel each) per dense tile.
// CPU reference with identical arithmetic: `src/peel_ref.rs`.
//
// Algorithm
//   H  stream the tile's cluster list once: transform each cluster's vertices
//      into shared memory, set up its triangles, and histogram them by the
//      BUCKET_BITS-bit quantized reversed-Z depth of their nearest vertex
//      (bucket 0 = nearest). The histogram (u16 counts packed in u32s) is the
//      key table of the bucket partition.
//   P  peel layers: a layer takes the nearest not-yet-peeled buckets up to
//      SLAB_TRIS triangles, re-streams the clusters, and every pixel tests only
//      the triangles of that slab (BATCHed through shared memory). A pixel is
//      final once its best depth is nearer than anything any later bucket can
//      contain; when every pixel is final the tile stops, so far buckets are
//      never touched in tiles that resolve early. Layers past NOMINAL_LAYERS
//      are "extra peel iterations"; at MAX_LAYERS the tile clamps (keeps its
//      current nearest fragments) and bumps STAT_CLAMPED_*.
//
// Shared memory: hist + sv + tsetup + tbox, see `DenseConfig::shared_memory_bytes`.
//
// ONE source, two pipelines. Lines starting `//@O ` exist only in the opaque
// pipeline, lines starting `//@M ` only in the alpha-masked one (see
// `config::peel_source` / `peel_masked_source`): the opaque kernel carries no
// texture bindings, no uv staging and no per-fragment branch. Cluster-list
// entries are tagged by `bin.wgsl` with LIST_MASKED_BIT when they came from a
// masked cull bin; each pipeline skips the other class. The masked pipeline
// runs after the opaque one on the same dense tiles and merges into
// `dense_key` by depth, evaluating the alpha test per candidate fragment
// (nearest-so-far only), so a discarded fragment lets the layer behind show.
@group(0) @binding(0) var<uniform> params: DenseParams;
@group(0) @binding(7) var<storage, read_write> slot_lists: array<atomic<u32>>;
@group(0) @binding(8) var<storage, read_write> state: array<atomic<u32>>;
// Output: (depth f32 bits, vis id) per pixel, row-major. depth bits == 0 means
// no fragment. Written for every pixel of every dense tile, nothing else.
@group(0) @binding(9) var<storage, read_write> dense_key: array<vec2<u32>>;

var<workgroup> hist: array<atomic<u32>, HIST_WORDS>;
var<workgroup> sv: array<vec4<f32>, 128>;
var<workgroup> tsetup: array<vec4<f32>, TRI_BATCH_X3>;
var<workgroup> tbox: array<u32, TRI_BATCH>;
var<workgroup> sh_tile: u32;
var<workgroup> sh_start: u32;
var<workgroup> sh_base: u32;
var<workgroup> sh_cnt: u32;
var<workgroup> sh_vis: u32;
var<workgroup> sh_tris: u32;
var<workgroup> sh_verts: u32;
var<workgroup> sh_total: atomic<u32>;
var<workgroup> sh_b1: u32;
var<workgroup> sh_rem: u32;
var<workgroup> sh_consumed: u32;
var<workgroup> sh_unres: atomic<u32>;
var<workgroup> sh_total_p: u32;
var<workgroup> sh_unres_p: u32;
//@M var<workgroup> sv_uv: array<vec2<f32>, 128>;
//@M var<workgroup> sv_iw: array<f32, 128>;
//@M var<workgroup> tuv: array<vec4<f32>, TRI_BATCH_X3>;
//@M fn dense_material(v: VgVisibleCluster, c: VgCluster) -> u32 {
//@M     return vg_objects[vg_instances[v.instance].object_row + vgd_material(c)].head[2];
//@M }
//@M fn dense_mirrored(v: VgVisibleCluster) -> bool {
//@M     let m = dense_xform(v);
//@M     return dot(cross(m[0].xyz, m[1].xyz), m[2].xyz) < 0.0;
//@M }

const BOX_INVALID: u32 = 0xffffffffu;

fn hist_get(b: u32) -> u32 {
    return (atomicLoad(&hist[b >> 1u]) >> ((b & 1u) * 16u)) & 0xffffu;
}

fn bucket_of(zmax: f32) -> u32 {
    let q = (1.0 - clamp(zmax, 0.0, 1.0)) * f32(NB);
    return min(u32(q), NB - 1u);
}

struct TriSetup {
    valid: bool,
    e0: vec4<f32>,
    e1: vec4<f32>,
    e2: vec4<f32>,
    bbox: u32,
    bucket: u32,
}

// v*.w is 1 for a vertex that survived the near-plane test, else 0.
fn setup_tri(v0: vec4<f32>, v1: vec4<f32>, v2: vec4<f32>, origin: vec2<f32>) -> TriSetup {
    var s: TriSetup;
    s.valid = false;
    if v0.w <= 0.0 || v1.w <= 0.0 || v2.w <= 0.0 { return s; }
    let zmax = max(v0.z, max(v1.z, v2.z));
    if zmax <= 0.0 { return s; }
    let a = v0.xy - origin;
    let b = v1.xy - origin;
    let c = v2.xy - origin;
    let area2 = (b.x - a.x) * (c.y - a.y) - (c.x - a.x) * (b.y - a.y);
    if !(abs(area2) > 1.0e-12) { return s; }
    let mn = min(a, min(b, c));
    let mx = max(a, max(b, c));
    let lo = max(ceil(mn - vec2<f32>(0.5)), vec2<f32>(0.0));
    let hi = min(floor(mx - vec2<f32>(0.5)), vec2<f32>(f32(TILE - 1u)));
    if lo.x > hi.x || lo.y > hi.y { return s; }
    let inv = 1.0 / area2;
    let a0 = -(c.y - b.y) * inv;
    let b0 = (c.x - b.x) * inv;
    let c0 = ((c.y - b.y) * b.x - (c.x - b.x) * b.y) * inv;
    let a1 = -(a.y - c.y) * inv;
    let b1 = (a.x - c.x) * inv;
    let c1 = ((a.y - c.y) * c.x - (a.x - c.x) * c.y) * inv;
    let a2 = -(b.y - a.y) * inv;
    let b2 = (b.x - a.x) * inv;
    let c2 = ((b.y - a.y) * a.x - (b.x - a.x) * a.y) * inv;
    s.e0 = vec4<f32>(a0, b0, c0, a0 * v0.z + a1 * v1.z + a2 * v2.z);
    s.e1 = vec4<f32>(a1, b1, c1, b0 * v0.z + b1 * v1.z + b2 * v2.z);
    s.e2 = vec4<f32>(a2, b2, c2, c0 * v0.z + c1 * v1.z + c2 * v2.z);
    s.bbox = u32(lo.x) | (u32(lo.y) << 8u) | (u32(hi.x) << 16u) | (u32(hi.y) << 24u);
    s.bucket = bucket_of(zmax);
    s.valid = true;
    return s;
}

// Stage list entry `ci` of `slot`: its vertices into `sv`. Uniform control
// flow only. Returns the cluster's triangle count (workgroup-uniform).
fn stage_cluster(slot: u32, ci: u32, lid: u32) -> u32 {
    if lid == 0u {
        let entry = atomicLoad(&slot_lists[LIST_POOL_BASE + sh_start + ci]);
        let vi = entry & LIST_INDEX_MASK;
        sh_vis = vi;
        let c = vgd_load_cluster(visible[vi].cluster_slot);
        sh_tris = c.counts_material & 0xffu;
        sh_verts = (c.counts_material >> 8u) & 0xffu;
    }
    let tris = workgroupUniformLoad(&sh_tris);
    if tris == 0u { return 0u; }
    let vis = visible[sh_vis];
    let c = vgd_load_cluster(vis.cluster_slot);
    let blk = vgd_pos_block(vis.cluster_slot, c, dense_asset(vis));
    let m = dense_xform(vis);
    let fw = f32(params.viewport.x);
    let fh = f32(params.viewport.y);
    for (var i = lid; i < sh_verts; i += WG_THREADS) {
        let wp = m * vec4<f32>(vgd_pos(blk, i), 1.0);
        let clip = params.view_proj * wp;
        if clip.w > 1.0e-6 {
            let ndc = clip.xyz / clip.w;
            sv[i] = vec4<f32>((ndc.x * 0.5 + 0.5) * fw, (0.5 - ndc.y * 0.5) * fh, ndc.z, 1.0);
//@M             sv_uv[i] = vga_uv0(vis.cluster_slot, blk, i);
//@M             sv_iw[i] = 1.0 / clip.w;
        } else {
            sv[i] = vec4<f32>(0.0);
//@M             sv_iw[i] = 0.0;
        }
    }
    workgroupBarrier();
    return tris;
}

fn tri_setup_from_sv(vis: VgVisibleCluster, c: VgCluster, t: u32, origin: vec2<f32>) -> TriSetup {
    let ix = vgd_tri(vis.cluster_slot, c, t);
    return setup_tri(sv[ix.x], sv[ix.y], sv[ix.z], origin);
}

@compute @workgroup_size(TILE, TILE)
fn peel(
    @builtin(local_invocation_index) lid: u32,
    @builtin(local_invocation_id) l3: vec3<u32>,
    @builtin(workgroup_id) wid: vec3<u32>,
) {
    let slot = wid.x;
    if lid == 0u {
        sh_tile = atomicLoad(&state[STATE_HEADER + slot]);
        // Each pipeline streams only its own class: opaque entries first, then masked.
//@O         sh_cnt = atomicLoad(&slot_lists[slot * LIST_HDR]);
//@O         sh_base = atomicLoad(&slot_lists[slot * LIST_HDR + 2u]);
//@M         sh_cnt = atomicLoad(&slot_lists[slot * LIST_HDR + 1u]);
//@M         sh_base = atomicLoad(&slot_lists[slot * LIST_HDR + 2u]) + atomicLoad(&slot_lists[slot * LIST_HDR]);
        sh_start = sh_base;
    }
    let tile = workgroupUniformLoad(&sh_tile);
    let list_len = workgroupUniformLoad(&sh_cnt);
    let tiles_x = params.viewport.z;
    let tile_xy = vec2<u32>(tile % tiles_x, tile / tiles_x);
    let origin = vec2<f32>(tile_xy * TILE);
//@M     if list_len == 0u { return; }

    var best_z = 0.0;
    var best_id = 0u;
    let px = f32(l3.x) + 0.5;
    let py = f32(l3.y) + 0.5;
    var layers = 0u;
    var tris_acc = 0u;
    // The list is peeled in chunks of at most CLUSTER_CAP clusters (the u16
    // bucket histogram bounds a chunk); the nearest fragment is carried across
    // chunks, so a long list loses nothing.
    for (var chunk = 0u; chunk < list_len; chunk += CLUSTER_CAP) {
    let cnt = min(CLUSTER_CAP, list_len - chunk);
    // Entry `ci` of this chunk is list entry `chunk + ci`: stage_cluster reads
    // `sh_start + ci`, so shift the start for the chunk.
    workgroupBarrier();
    if lid == 0u {
        sh_start = sh_base + chunk;
        atomicStore(&sh_total, 0u);
        sh_consumed = 0u;
    }
    for (var i = lid; i < HIST_WORDS; i += WG_THREADS) {
        atomicStore(&hist[i], 0u);
    }
    workgroupBarrier();

    // H: histogram.
    for (var ci = 0u; ci < cnt; ci++) {
        let tris = stage_cluster(slot, ci, lid);
        let vis = visible[sh_vis];
        let c = vgd_load_cluster(vis.cluster_slot);
        for (var t = lid; t < tris; t += WG_THREADS) {
            let s = tri_setup_from_sv(vis, c, t, origin);
            if s.valid {
                atomicAdd(&hist[s.bucket >> 1u], 1u << ((s.bucket & 1u) * 16u));
                atomicAdd(&sh_total, 1u);
            }
        }
        workgroupBarrier();
    }
    workgroupBarrier();
    if lid == 0u { sh_total_p = atomicLoad(&sh_total); }
    let total = workgroupUniformLoad(&sh_total_p);
    tris_acc += total;

    var chunk_layers = 0u;
    var b0 = 0u;
    loop {
        if total == 0u { break; }
        // Choose the slab [b0, b1).
        if lid == 0u {
            var acc = 0u;
            var b = b0;
            loop {
                if b >= NB { break; }
                acc += hist_get(b);
                b += 1u;
                if acc >= SLAB_TRIS { break; }
            }
            sh_b1 = b;
            sh_consumed += acc;
            sh_rem = total - sh_consumed;
            atomicStore(&sh_unres, 0u);
        }
        let b1 = workgroupUniformLoad(&sh_b1);
        let remaining = workgroupUniformLoad(&sh_rem);

        // P: stream clusters, test only slab triangles.
        for (var ci = 0u; ci < cnt; ci++) {
            let tris = stage_cluster(slot, ci, lid);
            let vis = visible[sh_vis];
            let c = vgd_load_cluster(vis.cluster_slot);
//@M             let am = vga_load(dense_material(vis, c));
//@M             let mirrored = dense_mirrored(vis);
            for (var base = 0u; base < tris; base += TRI_BATCH) {
                if lid < TRI_BATCH {
                    tbox[lid] = BOX_INVALID;
                    let t = base + lid;
                    if t < tris {
                        let s = tri_setup_from_sv(vis, c, t, origin);
                        if s.valid && s.bucket >= b0 && s.bucket < b1 {
                            tsetup[lid * 3u] = s.e0;
                            tsetup[lid * 3u + 1u] = s.e1;
                            tsetup[lid * 3u + 2u] = s.e2;
                            tbox[lid] = s.bbox;
//@M                             let ix = vgd_tri(vis.cluster_slot, c, t);
//@M                             let p0 = sv[ix.x].xy;
//@M                             let p1 = sv[ix.y].xy;
//@M                             let p2 = sv[ix.z].xy;
//@M                             let area2 = (p1.x - p0.x) * (p2.y - p0.y) - (p2.x - p0.x) * (p1.y - p0.y);
//@M                             if !(vga_double_sided(am) || ((area2 < 0.0) != mirrored)) {
//@M                                 tbox[lid] = BOX_INVALID;
//@M                             }
//@M                             let uv0 = sv_uv[ix.x];
//@M                             let uv1 = sv_uv[ix.y];
//@M                             let uv2 = sv_uv[ix.z];
//@M                             let e1 = uv1 - uv0;
//@M                             let e2 = uv2 - uv0;
//@M                             let lod = vga_lod_for_area(am, 0.5 * abs(e1.x * e2.y - e1.y * e2.x), 0.5 * abs(area2));
//@M                             tuv[lid * 3u] = vec4<f32>(uv0 * sv_iw[ix.x], sv_iw[ix.x], lod);
//@M                             tuv[lid * 3u + 1u] = vec4<f32>(uv1 * sv_iw[ix.y], sv_iw[ix.y], lod);
//@M                             tuv[lid * 3u + 2u] = vec4<f32>(uv2 * sv_iw[ix.z], sv_iw[ix.z], lod);
                        }
                    }
                }
                workgroupBarrier();
                for (var i = 0u; i < TRI_BATCH; i++) {
                    let bb = tbox[i];
                    if bb == BOX_INVALID { continue; }
                    let bx0 = bb & 0xffu;
                    let by0 = (bb >> 8u) & 0xffu;
                    let bx1 = (bb >> 16u) & 0xffu;
                    let by1 = bb >> 24u;
                    if l3.x < bx0 || l3.x > bx1 || l3.y < by0 || l3.y > by1 { continue; }
                    let e0 = tsetup[i * 3u];
                    let e1 = tsetup[i * 3u + 1u];
                    let e2 = tsetup[i * 3u + 2u];
                    let w0 = e0.x * px + e0.y * py + e0.z;
                    let w1 = e1.x * px + e1.y * py + e1.z;
                    let w2 = e2.x * px + e2.y * py + e2.z;
                    if w0 >= -EDGE_EPS && w1 >= -EDGE_EPS && w2 >= -EDGE_EPS {
                        let z = e0.w * px + e1.w * py + e2.w;
                        if z > 0.0 {
                            let id = vg_vis_pack(sh_vis, base + i);
                            if z > best_z || (z == best_z && id > best_id) {
//@M                                 let q = tuv[i * 3u] * w0 + tuv[i * 3u + 1u] * w1 + tuv[i * 3u + 2u] * w2;
//@M                                 let uv = q.xy / max(q.z, 1.0e-30);
//@M                                 if vga_rejects(am, vga_alpha_level(am, uv, tuv[i * 3u].w)) { continue; }
                                best_z = z;
                                best_id = id;
                            }
                        }
                    }
                }
                workgroupBarrier();
            }
        }

        layers += 1u;
        chunk_layers += 1u;
        let bound = 1.0 - f32(b1) / f32(NB);
        if !(remaining == 0u || best_z > bound + 1.0e-6) {
            atomicAdd(&sh_unres, 1u);
        }
        workgroupBarrier();
        if lid == 0u { sh_unres_p = atomicLoad(&sh_unres); }
        let unres = workgroupUniformLoad(&sh_unres_p);
        b0 = b1;
        if remaining == 0u || unres == 0u { break; }
        if chunk_layers >= MAX_LAYERS {
            if lid == 0u {
                atomicAdd(&state[STAT_CLAMPED_TILES], 1u);
                atomicAdd(&state[STAT_CLAMPED_TRIS], remaining);
            }
            break;
        }
    }
    }

//@O     if lid == 0u {
//@O         atomicAdd(&state[STAT_TRIS_TOTAL], tris_acc);
//@O         atomicMax(&state[STAT_TRIS_MAX], tris_acc);
//@O         atomicAdd(&state[STAT_TILES], 1u);
//@O         atomicAdd(&state[STAT_LAYERS], layers);
//@O         if layers > NOMINAL_LAYERS {
//@O             atomicAdd(&state[STAT_EXTRA_LAYERS], layers - NOMINAL_LAYERS);
//@O         }
//@O     }
//@M     if lid == 0u {
//@M         atomicAdd(&state[STAT_MASKED_TILES], 1u);
//@M         atomicAdd(&state[STAT_MASKED_TRIS], tris_acc);
//@M         atomicMax(&state[STAT_MASKED_TRIS_MAX], tris_acc);
//@M     }
    let pix = tile_xy * TILE + l3.xy;
    if pix.x < params.viewport.x && pix.y < params.viewport.y {
//@O         dense_key[pix.y * params.viewport.x + pix.x] = vg_raster_key(best_z, best_id);
//@M         let at = pix.y * params.viewport.x + pix.x;
//@M         let cur = dense_key[at];
//@M         let cur_z = bitcast<f32>(cur.x);
//@M         if best_z > 0.0 && (best_z > cur_z || (best_z == cur_z && best_id > cur.y)) {
//@M             dense_key[at] = vg_raster_key(best_z, best_id);
//@M         }
    }
}
