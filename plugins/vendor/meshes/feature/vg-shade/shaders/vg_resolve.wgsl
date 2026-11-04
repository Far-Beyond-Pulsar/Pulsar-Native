// vis_resolve: merge the hardware, software, dense and ray-traced visibility
// results into the unified RG32Uint payload (R = vis id, G = bary_pack(u,v))
// and the resolved depth, depth-testing against the classic G-buffer depth.
//
// Reversed-Z: larger depth is nearer; 0 is "nothing".

struct Camera {
    view:           mat4x4<f32>,
    proj:           mat4x4<f32>,
    view_proj:      mat4x4<f32>,
    view_proj_inv:  mat4x4<f32>,
    position_near:  vec4<f32>,
    forward_far:    vec4<f32>,
    jitter_frame:   vec4<f32>,
    prev_view_proj: mat4x4<f32>,
}

struct Params {
    width: u32,
    height: u32,
    flags: u32,
    bin_count: u32,
    pixel_count: u32,
    _p0: u32,
    _p1: u32,
    _p2: u32,
}

const FLAG_SW: u32 = 1u;
const FLAG_DENSE: u32 = 2u;
const FLAG_RT: u32 = 4u;
// `dense_mask` is bound: pixels of a dense tile take ONLY the dense result (it
// replaces Tier 1/2 there, so the tiers never mix); pixels outside ignore
// `dense_keys` (stale or unrelated).
const FLAG_DENSE_MASK: u32 = 8u;
// Tier 3 holds only what the cull diverted to it: inside dense tiles take the
// nearest of it and the Tier 1/2 pixel instead of only the dense result.
const FLAG_DENSE_MERGE: u32 = 16u;

@group(0) @binding(0) var<storage, read> cameras: array<Camera, 2>;
@group(0) @binding(1) var<uniform> params: Params;
@group(0) @binding(2) var<storage, read> static_objects: array<u32>;
@group(0) @binding(3) var<storage, read> coordinate_spaces: array<mat4x4<f32>>;
@group(0) @binding(4) var<storage, read> visible: array<VgVisibleCluster>;
// Shared decode inputs (helio_vg_format::DECODE_WGSL).
@group(0) @binding(5) var<storage, read> vg_asset_quant: array<vec4<f32>>;
@group(0) @binding(6) var<storage, read> vg_instances: array<VgInstance>;
@group(0) @binding(7) var<storage, read> vg_page_pool: array<u32>;
// 64-bit raster keys as word pairs (little endian): [2i] = id, [2i+1] = depth bits.
@group(0) @binding(8) var<storage, read> sw_keys: array<u32>;
@group(0) @binding(9) var<storage, read> dense_keys: array<u32>;
@group(0) @binding(10) var<storage, read_write> pixel_material: array<u32>;
@group(0) @binding(11) var<storage, read> rt_visible: array<VgVisibleCluster>;
@group(0) @binding(12) var<storage, read> dense_mask: array<u32>;

@group(1) @binding(0) var hw_vis: texture_2d<u32>;
@group(1) @binding(1) var hw_depth: texture_depth_2d;
@group(1) @binding(2) var scene_depth: texture_depth_2d;
@group(1) @binding(3) var rt_payload: texture_2d<u32>;
@group(1) @binding(4) var rt_depth: texture_2d<f32>;
@group(1) @binding(5) var out_payload: texture_storage_2d<rg32uint, write>;
@group(1) @binding(6) var out_depth: texture_storage_2d<r32float, write>;
// Per-pixel source of the winning surface (`VGS_SRC_*`): the debug views read it,
// so they report what produced the pixel, not what the other passes think.
@group(1) @binding(7) var out_source: texture_storage_2d<r32uint, write>;

fn write_none(px: vec2<u32>, pidx: u32) {
    textureStore(out_source, px, vec4<u32>(VGS_SRC_NONE, 0u, 0u, 0u));
    textureStore(out_payload, px, vec4<u32>(VGS_INVALID, 0u, 0u, 0u));
    textureStore(out_depth, px, vec4<f32>(0.0));
    pixel_material[pidx] = VGS_INVALID;
}

@compute @workgroup_size(8, 8, 1)
fn vis_resolve(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= params.width || gid.y >= params.height {
        return;
    }
    let px = gid.xy;
    let ipx = vec2<i32>(px);
    let pidx = px.y * params.width + px.x;

    // Nearest candidate wins; hardware wins exact ties (checked first, strict >).
    var best_z = 0.0;
    var best_id = VGS_INVALID;
    var best_src = 0u; // 0 none, 1 raster id (HW/SW/dense), 2 ray-traced payload
    var best_kind = VGS_SRC_NONE; // which candidate holds best_z (VGS_SRC_*)
    var rt_bary = 0u;

    var in_dense = false;
    if (params.flags & FLAG_DENSE_MASK) != 0u {
        let tile = (px.y / 16u) * ((params.width + 15u) / 16u) + (px.x / 16u);
        in_dense = tile < arrayLength(&dense_mask) && dense_mask[tile] != 0u;
    }
    let replace = in_dense && (params.flags & FLAG_DENSE_MERGE) == 0u;
    let use_dense = (params.flags & FLAG_DENSE) != 0u && (in_dense || (params.flags & FLAG_DENSE_MASK) == 0u);

    let hv = textureLoad(hw_vis, ipx, 0).x;
    if hv != VGS_INVALID && !replace {
        let z = textureLoad(hw_depth, ipx, 0);
        if z > best_z {
            best_z = z;
            best_id = hv;
            best_src = 1u;
            best_kind = VGS_SRC_HW;
        }
    }
    if (params.flags & FLAG_SW) != 0u && !replace {
        let zbits = sw_keys[pidx * 2u + 1u];
        let z = bitcast<f32>(zbits);
        if zbits != 0u && z > best_z {
            best_z = z;
            best_id = sw_keys[pidx * 2u];
            best_src = 1u;
            best_kind = VGS_SRC_SW;
        }
    }
    if use_dense {
        let zbits = dense_keys[pidx * 2u + 1u];
        let z = bitcast<f32>(zbits);
        if zbits != 0u && z > best_z {
            best_z = z;
            best_id = dense_keys[pidx * 2u];
            best_src = 1u;
            best_kind = VGS_SRC_DENSE;
        }
    }
    if (params.flags & FLAG_RT) != 0u {
        let rp = textureLoad(rt_payload, ipx, 0);
        if rp.x != VGS_INVALID {
            let z = textureLoad(rt_depth, ipx, 0).x;
            if z > best_z {
                best_z = z;
                best_id = rp.x;
                best_src = 2u;
                best_kind = VGS_SRC_RT;
                rt_bary = rp.y;
            }
        }
    }

    // Classic geometry nearer (or no VG at all): leave the pixel to it.
    // When the HW raster shares the classic depth buffer the HW depth equals
    // the scene depth bit for bit, which the >= keeps.
    if best_src == 0u || best_z <= 0.0 || best_z < textureLoad(scene_depth, ipx, 0) {
        write_none(px, pidx);
        return;
    }

    let vis_index = vg_vis_cluster(best_id);
    var vis: VgVisibleCluster;
    if vis_index >= VGS_RT_VISIBLE_BASE {
        let rt_index = vis_index - VGS_RT_VISIBLE_BASE;
        if rt_index >= arrayLength(&rt_visible) {
            write_none(px, pidx);
            return;
        }
        vis = rt_visible[rt_index];
    } else {
        if vis_index >= arrayLength(&visible) {
            write_none(px, pidx);
            return;
        }
        vis = visible[vis_index];
    }
    let cs = vis.cluster_slot;
    let cluster = vgd_load_cluster(cs);
    let inst = vg_instances[vis.instance];
    let row_word = (inst.object_row + (cluster.counts_material >> 16u)) * VGS_OBJ_WORDS;

    var bary_word = rt_bary;
    if best_src == 1u {
        let blk = vgd_pos_block(cs, cluster, inst.asset_slot);
        let tri = vgd_tri(cs, cluster, vg_vis_tri(best_id));
        let cam = cameras[0];
        let size = vec2<f32>(f32(params.width), f32(params.height));
        let ndc = vgs_pixel_ndc(vec2<f32>(px) + vec2<f32>(0.5), size);
        let w0 = vgs_world_pos(row_word, vgd_pos(blk, tri.x));
        let w1 = vgs_world_pos(row_word, vgd_pos(blk, tri.y));
        let w2 = vgs_world_pos(row_word, vgd_pos(blk, tri.z));
        let c0 = cam.proj * (cam.view * vec4<f32>(w0, 1.0));
        let c1 = cam.proj * (cam.view * vec4<f32>(w1, 1.0));
        let c2 = cam.proj * (cam.view * vec4<f32>(w2, 1.0));
        let b = vgs_bary(c0, c1, c2, ndc);
        bary_word = pack2x16unorm(clamp(vec2<f32>(b.y, b.z), vec2<f32>(0.0), vec2<f32>(1.0)));
    }

    // An id in the ray-traced range is a ray-query hit whichever buffer carried it
    // (the ray-traced backend can also fill the dense keys).
    var source = best_kind;
    if vis_index >= VGS_RT_VISIBLE_BASE {
        source = VGS_SRC_RT;
    }
    textureStore(out_source, px, vec4<u32>(source, 0u, 0u, 0u));
    textureStore(out_payload, px, vec4<u32>(best_id, bary_word, 0u, 0u));
    textureStore(out_depth, px, vec4<f32>(best_z, 0.0, 0.0, 0.0));
    pixel_material[pidx] = static_objects[row_word + VGS_OBJ_MATERIAL_SLOT];
}
