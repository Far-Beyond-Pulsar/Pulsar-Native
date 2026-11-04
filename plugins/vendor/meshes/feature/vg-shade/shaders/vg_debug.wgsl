// vg_debug: the virtual-geometry debug views (`helio_vg_format::debug_view`).
//
// A small compute pass that runs only while one of the views is selected. It
// re-reads the unified vis payload the resolve wrote, decodes whatever the
// view needs (cluster, triangle, errors, tile counters, residency) and
// overwrites the G-buffer albedo; the deferred lighting pass shows albedo
// unlit for these debug modes. Composed after `helio_vg_format::WGSL`, the
// stream bindings (group 0, bindings 7..=13) and helpers, the generated
// `VGDBG_*` constants, `vg_common.wgsl` and the shared decode.

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

struct DebugParams {
    width: u32,
    height: u32,
    mode: u32,
    bin_capacity: u32,
    tiles_x: u32,
    tiles_y: u32,
    tau_px: f32,
    dense_resolved: u32,
}

@group(0) @binding(0) var<storage, read> cameras: array<Camera, 2>;
@group(0) @binding(1) var<uniform> dp: DebugParams;
@group(0) @binding(2) var<storage, read> static_objects: array<u32>;
@group(0) @binding(3) var<storage, read> coordinate_spaces: array<mat4x4<f32>>;
@group(0) @binding(4) var<storage, read> visible: array<VgVisibleCluster>;
@group(0) @binding(5) var<storage, read> rt_visible: array<VgVisibleCluster>;
@group(0) @binding(6) var<storage, read> vg_instances: array<VgInstance>;
// 7..=13: vg_page_pool, vg_residency, vg_nodes, vg_group_refs, vg_asset_headers,
// vg_asset_bases, vg_asset_quant (`helio_vg_stream::bindings_wgsl`).
@group(0) @binding(14) var<storage, read> materials: array<u32>;
@group(0) @binding(15) var<storage, read> cull_state: array<u32>;
// Per tile: [software triangles overlapping it, software pixel hits].
@group(0) @binding(16) var<storage, read> tile_counters: array<u32>;
@group(0) @binding(17) var<storage, read> dense_mask: array<u32>;
// Tiles Tier 3 resolved (union mask); only valid when `dp.dense_resolved != 0`.
@group(0) @binding(18) var<storage, read> resolved_mask: array<u32>;

@group(1) @binding(0) var payload_tex: texture_2d<u32>;
@group(1) @binding(1) var out_albedo: texture_storage_2d<rgba8unorm, write>;
// What produced each pixel (`VGS_SRC_*`), recorded by `vis_resolve` from the
// candidate that won it.
@group(1) @binding(2) var source_tex: texture_2d<u32>;

// "Ray Trace vs Raster" (renderer mode 33) palette. Warm = rasterized, cool =
// ray traced. Classic pixels and voxel pixels are coloured by the deferred
// lighting pass (orange / cyan); the shade pass paints only virtual geometry.
const DBG_RVR_HW: vec3<f32> = vec3<f32>(1.0, 0.36, 0.08);    // hardware raster
const DBG_RVR_SW: vec3<f32> = vec3<f32>(1.0, 0.85, 0.10);    // compute micro-raster
const DBG_RVR_DENSE: vec3<f32> = vec3<f32>(0.90, 0.10, 0.10); // dense-tile depth peel
const DBG_RVR_RT: vec3<f32> = vec3<f32>(0.30, 0.40, 1.00);    // ray query

const DBG_TILE: u32 = 16u;
const DBG_MATERIAL_WORDS: u32 = 24u;
const DBG_MATERIAL_CLASS_WORD: u32 = 19u;
const DBG_BIN_COUNT: u32 = 16u;
const DBG_CLUSTER_FLAG_LEAF: u32 = 2u;
const DBG_INF_ERR: f32 = 1.0e30;
const DBG_HUGE_PX: f32 = 3.0e38;

// Blue -> cyan -> green -> yellow -> red, the legend of the lighting pass's
// heat views.
fn dbg_heat(t: f32) -> vec3<f32> {
    let x = clamp(t, 0.0, 1.0) * 4.0;
    if x < 1.0 { return mix(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 1.0, 1.0), x); }
    if x < 2.0 { return mix(vec3<f32>(0.0, 1.0, 1.0), vec3<f32>(0.0, 1.0, 0.0), x - 1.0); }
    if x < 3.0 { return mix(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 1.0, 0.0), x - 2.0); }
    return mix(vec3<f32>(1.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), x - 3.0);
}

fn dbg_hash(id: u32) -> u32 {
    var h = (id ^ (id >> 16u)) * 2246822519u;
    h = (h ^ (h >> 13u)) * 3266489917u;
    return h ^ (h >> 16u);
}

fn dbg_hash_colour(id: u32) -> vec3<f32> {
    let h = dbg_hash(id);
    return vec3<f32>(0.15) + 0.85 * vec3<f32>(f32(h & 255u), f32((h >> 8u) & 255u), f32((h >> 16u) & 255u)) / 255.0;
}

fn dbg_view_depth(c: vec3<f32>) -> f32 {
    return -(cameras[0].view * vec4<f32>(c, 1.0)).z;
}

// Mirrors the cull's `proj_err`: projected pixels of an object-space error.
fn dbg_proj_err(err: f32, c: vec3<f32>, r: f32) -> f32 {
    if err >= DBG_INF_ERR { return DBG_HUGE_PX; }
    let d = max(dbg_view_depth(c) - r, cameras[0].position_near.w);
    return err * cameras[0].proj[1][1] * 0.5 * f32(dp.height) / d;
}

fn dbg_frustum_margin(c: vec3<f32>) -> f32 {
    // Smallest signed distance to the five side planes (no far plane).
    let m = cameras[0].view_proj;
    let r0 = vec4<f32>(m[0][0], m[1][0], m[2][0], m[3][0]);
    let r1 = vec4<f32>(m[0][1], m[1][1], m[2][1], m[3][1]);
    let r2 = vec4<f32>(m[0][2], m[1][2], m[2][2], m[3][2]);
    let r3 = vec4<f32>(m[0][3], m[1][3], m[2][3], m[3][3]);
    var planes = array<vec4<f32>, 5>(r3 + r0, r3 - r0, r3 + r1, r3 - r1, r3 - r2);
    var best = 3.0e38;
    for (var i = 0u; i < 5u; i++) {
        let p = planes[i] / length(planes[i].xyz);
        best = min(best, dot(p.xyz, c) + p.w);
    }
    return best;
}

fn dbg_tile_colour(px: vec2<u32>) -> vec3<f32> {
    let tile = (px.y / DBG_TILE) * dp.tiles_x + (px.x / DBG_TILE);
    if tile * 2u + 1u >= arrayLength(&tile_counters) { return vec3<f32>(0.0); }
    let tris = f32(tile_counters[tile * 2u]);
    let hits = f32(tile_counters[tile * 2u + 1u]);
    if dp.mode == VGDBG_SW_OVERDRAW {
        if tris == 0.0 { return vec3<f32>(0.03); }
        // Average depth complexity, 8 hits per pixel is the hot end.
        return dbg_heat(hits / f32(DBG_TILE * DBG_TILE) / 8.0);
    }
    if dp.mode == VGDBG_DENSE_OVERLAP {
        if tris == 0.0 { return vec3<f32>(0.03); }
        return dbg_heat(tris / 64.0);
    }
    // Dense mask.
    if tile < arrayLength(&dense_mask) && dense_mask[tile] != 0u { return vec3<f32>(1.0, 0.5, 0.05); }
    if tris > 0.0 { return vec3<f32>(0.25); }
    return vec3<f32>(0.04);
}

@compute @workgroup_size(8, 8, 1)
fn vg_debug(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= dp.width || gid.y >= dp.height { return; }
    let px = gid.xy;
    let mode = dp.mode;

    // Ray Trace vs Raster: the recorded source of the pixel, nothing inferred.
    if mode == VGDBG_RAY_VS_RASTER {
        let source = textureLoad(source_tex, vec2<i32>(px), 0).x;
        var rvr = vec3<f32>(0.0);
        switch source {
            case VGS_SRC_HW: { rvr = DBG_RVR_HW; }
            case VGS_SRC_SW: { rvr = DBG_RVR_SW; }
            case VGS_SRC_DENSE: { rvr = DBG_RVR_DENSE; }
            case VGS_SRC_RT: { rvr = DBG_RVR_RT; }
            default: { return; } // not virtual geometry: the lighting pass colours it
        }
        // Alpha 0 is the "painted by the shade pass" marker the lighting pass looks
        // for (no real surface has albedo alpha 0: opaque writes 1, cut-outs below
        // the cutoff are discarded). The lightmap tag cannot say this: (-1, -1) is
        // also what every classic mesh without a lightmap writes.
        textureStore(out_albedo, px, vec4<f32>(rvr, 0.0));
        return;
    }

    // Tile views describe the software raster's tiles, VG pixel or not.
    if mode == VGDBG_SW_OVERDRAW || mode == VGDBG_DENSE_OVERLAP || mode == VGDBG_DENSE_MASK {
        textureStore(out_albedo, px, vec4<f32>(dbg_tile_colour(px), 1.0));
        return;
    }

    let payload = textureLoad(payload_tex, vec2<i32>(px), 0).xy;
    if payload.x == VGS_INVALID {
        // Classic pixel: untouched (shown by its albedo) except in the mask view.
        if mode == VGDBG_MASK { textureStore(out_albedo, px, vec4<f32>(vec3<f32>(0.18), 1.0)); }
        return;
    }
    if mode == VGDBG_MASK {
        textureStore(out_albedo, px, vec4<f32>(0.15, 0.85, 0.35, 1.0));
        return;
    }

    let vis_index = vg_vis_cluster(payload.x);
    let is_rt = vis_index >= VGS_RT_VISIBLE_BASE;
    var vis: VgVisibleCluster;
    if is_rt {
        let i = vis_index - VGS_RT_VISIBLE_BASE;
        if i >= arrayLength(&rt_visible) { return; }
        vis = rt_visible[i];
    } else {
        if vis_index >= arrayLength(&visible) { return; }
        vis = visible[vis_index];
    }
    let cs = vis.cluster_slot;
    let cluster = vgd_load_cluster(cs);
    let inst = vg_instances[vis.instance];
    let tri_id = vg_vis_tri(payload.x);
    let row_word = (inst.object_row + (cluster.counts_material >> 16u)) * VGS_OBJ_WORDS;
    let model = vgs_obj_mat4(row_word + VGS_OBJ_TRANSFORM);
    let scale = max(length(model[0].xyz), max(length(model[1].xyz), length(model[2].xyz)));
    let err_k = max(inst.lod_error_scale, 1.0e-4) * scale;
    let cw = (model * vec4<f32>(cluster.sphere.xyz, 1.0)).xyz;
    let rw = cluster.sphere.w * scale;
    let s_err = dbg_proj_err(cluster.self_error * err_k, cw, rw);
    let tau = max(dp.tau_px, 1.0e-4);
    let uv_b = unpack2x16unorm(payload.y);
    let b = vec3<f32>(1.0 - uv_b.x - uv_b.y, uv_b.x, uv_b.y);
    let is_leaf = (cluster.flags & DBG_CLUSTER_FLAG_LEAF) != 0u;
    let bin = select(vis_index / max(dp.bin_capacity, 1u), 99u, is_rt);

    var col = vec3<f32>(0.5);
    switch mode {
        case VGDBG_TRIANGLE: {
            col = dbg_hash_colour((cs * 1664525u + inst.asset_slot * 1013904223u) ^ (tri_id * 2246822519u));
        }
        case VGDBG_CLUSTER: {
            col = dbg_hash_colour(cs * 1664525u + inst.asset_slot * 1013904223u);
        }
        case VGDBG_LOD: {
            // Error relative to the cluster's own size grows by a roughly
            // constant factor per DAG level: log2 of it is a depth proxy
            // (leaves, which carry no error, are level 0).
            var t = 0.0;
            if !is_leaf && cluster.self_error > 0.0 {
                let rel = cluster.self_error / max(cluster.sphere.w, 1.0e-9);
                t = clamp((log2(max(rel, 1.0e-9)) + 16.0) / 16.0, 0.0, 1.0);
            }
            col = dbg_heat(t);
        }
        case VGDBG_DETAIL: {
            // Selected error over the asset's lattice extent (1 = as coarse as
            // the whole asset).
            let extent = vg_asset_quant[inst.asset_slot].w * 262143.0;
            var t = 0.0;
            if !is_leaf && cluster.self_error > 0.0 {
                t = clamp(1.0 + log2(max(cluster.self_error / max(extent, 1.0e-12), 1.0e-9)) / 20.0, 0.0, 1.0);
            }
            col = dbg_heat(t);
        }
        case VGDBG_TRI_SIZE: {
            let blk = vgd_pos_block(cs, cluster, inst.asset_slot);
            let tri = vgd_tri(cs, cluster, tri_id);
            let cam = cameras[0];
            let c0 = cam.proj * (cam.view * vec4<f32>(vgs_world_pos(row_word, vgd_pos(blk, tri.x)), 1.0));
            let c1 = cam.proj * (cam.view * vec4<f32>(vgs_world_pos(row_word, vgd_pos(blk, tri.y)), 1.0));
            let c2 = cam.proj * (cam.view * vec4<f32>(vgs_world_pos(row_word, vgd_pos(blk, tri.z)), 1.0));
            var area = 0.0;
            if c0.w > 0.0 && c1.w > 0.0 && c2.w > 0.0 {
                let size = vec2<f32>(f32(dp.width), f32(dp.height));
                let s0 = c0.xy / c0.w * 0.5 * size;
                let s1 = c1.xy / c1.w * 0.5 * size;
                let s2 = c2.xy / c2.w * 0.5 * size;
                let e0 = s1 - s0;
                let e1 = s2 - s0;
                area = 0.5 * abs(e0.x * e1.y - e0.y * e1.x);
            }
            // 1/8 px^2 and below is red, 64 px^2 and above blue.
            let t = clamp((log2(max(area, 1.0e-3)) + 3.0) / 9.0, 0.0, 1.0);
            col = dbg_heat(1.0 - t);
        }
        case VGDBG_TIER: {
            // The tier is what `vis_resolve` recorded for the pixel (the candidate
            // that won it), not inferred from the cull bin of its cluster or from
            // a tile mask: a dense tile can still show Tier 1/2 pixels (nearest
            // wins) and a dense-bin cluster can be won by the hardware raster.
            let tier_source = textureLoad(source_tex, vec2<i32>(px), 0).x;
            if tier_source == VGS_SRC_RT {
                col = vec3<f32>(1.0, 0.1, 0.9);
            } else if tier_source == VGS_SRC_DENSE {
                col = vec3<f32>(1.0, 0.55, 0.05);
            } else if tier_source == VGS_SRC_SW {
                col = vec3<f32>(0.1, 0.9, 0.2);
            } else if bin == 0u {
                col = vec3<f32>(0.3, 0.55, 1.0);
            } else if bin == 1u {
                col = vec3<f32>(0.15, 0.4, 0.95);
            } else if bin == 2u {
                col = vec3<f32>(0.1, 0.25, 0.8);
            } else {
                col = vec3<f32>(0.05, 0.12, 0.6);
            }
        }
        case VGDBG_RESIDENCY: {
            col = vec3<f32>(0.1, 0.7, 0.7);
            if !is_leaf && cluster.group_id != VG_NODE_INVALID {
                let gref = vg_group_refs[vg_group_ref_base(inst.asset_slot) + cluster.group_id];
                let child_page = vg_global_page(inst.asset_slot, gref);
                if child_page < arrayLength(&vg_residency) && !vg_page_resident(child_page) {
                    // Fallback parent: brighter the further above tau it is.
                    let pressure = clamp(log2(max(s_err / tau, 1.0)) / 5.0, 0.0, 1.0);
                    col = vec3<f32>(0.45 + 0.55 * pressure, 0.04, 0.04);
                } else {
                    col = vec3<f32>(0.2, 0.85, 0.2);
                }
            }
        }
        case VGDBG_CULL: {
            var phase1 = false;
            if !is_rt && bin < 8u {
                let k = vis_index - bin * max(dp.bin_capacity, 1u);
                phase1 = k >= cull_state[VGDBG_ST_PHASE0 + bin];
            }
            col = select(vec3<f32>(0.1, 0.7, 0.2), vec3<f32>(0.95, 0.8, 0.1), phase1);
            if dbg_frustum_margin(cw) < rw {
                col = mix(col, vec3<f32>(0.0, 1.0, 1.0), 0.55);
            }
            let cone = unpack4x8snorm(cluster.cone);
            if cone.w < 0.9999 {
                let axis = normalize((model * vec4<f32>(cone.xyz, 0.0)).xyz);
                let dv = cw - cameras[0].position_near.xyz;
                let len = max(length(dv), 1.0e-6);
                let sin_a = sqrt(max(1.0 - cone.w * cone.w, 0.0));
                // Distance (in view-length units) to the back-face cull test.
                if (sin_a * len + rw - dot(dv, axis)) / len < 0.15 {
                    col = mix(col, vec3<f32>(1.0, 0.0, 1.0), 0.55);
                }
            }
            if s_err / tau > 0.8 {
                col = mix(col, vec3<f32>(1.0, 0.1, 0.1), 0.55);
            }
        }
        case VGDBG_ERROR: {
            let r = s_err / tau;
            col = select(dbg_heat(r), vec3<f32>(1.0, 0.0, 1.0), r > 1.0);
        }
        case VGDBG_INSTANCE: {
            col = dbg_hash_colour(vis.instance * 7919u + 17u);
        }
        case VGDBG_BARY: {
            col = b;
        }
        case VGDBG_NORMAL: {
            let blk = vgd_pos_block(cs, cluster, inst.asset_slot);
            let tri = vgd_tri(cs, cluster, tri_id);
            let a0 = vgd_attr(cs, blk, tri.x);
            let a1 = vgd_attr(cs, blk, tri.y);
            let a2 = vgd_attr(cs, blk, tri.z);
            let space = coordinate_spaces[vgs_space_id(row_word)];
            let space_rot = mat3x3<f32>(space[0].xyz, space[1].xyz, space[2].xyz);
            let nm0 = vgs_obj_vec4(row_word + VGS_OBJ_NORMAL_MAT);
            let nm1 = vgs_obj_vec4(row_word + VGS_OBJ_NORMAL_MAT + 4u);
            let nm2 = vgs_obj_vec4(row_word + VGS_OBJ_NORMAL_MAT + 8u);
            let normal_mat = space_rot * mat3x3<f32>(nm0.xyz, nm1.xyz, nm2.xyz);
            col = normalize(normal_mat * (a0.normal * b.x + a1.normal * b.y + a2.normal * b.z)) * 0.5 + 0.5;
        }
        case VGDBG_MATERIAL: {
            let material_row = static_objects[row_word + VGS_OBJ_MATERIAL_SLOT];
            let mclass = materials[material_row * DBG_MATERIAL_WORDS + DBG_MATERIAL_CLASS_WORD];
            col = dbg_hash_colour(min(mclass, DBG_BIN_COUNT - 1u) * 977u + 3u);
        }
        default: {
            return;
        }
    }
    textureStore(out_albedo, px, vec4<f32>(col, 1.0));
}
