// Tier 2 for alpha-tested clusters: `vg_sw_raster.wgsl` with the alpha test
// evaluated per covered pixel, before the 64-bit `atomicMax` race, so a
// discarded pixel never competes. uv0 is decoded with the vertices (shared
// memory), interpolated perspective-correctly from the screen barycentrics and
// the clip w of each vertex, and the alpha texture is sampled with
// `textureSampleLevel` at the mip matching the triangle's uv-to-pixel density
// (compute shaders have no implicit derivatives; a software triangle covers
// at most a few pixels, so one level per triangle is the footprint).
//
// Back faces are culled per material: two-sided materials keep them.
//
// One workgroup (128 threads) per entry of the masked software bin.
//
// Requires vg_format.wgsl, vg_bindings.wgsl, vg_decode.wgsl, vg_raster_common.wgsl,
// the vg_alpha bindings, vg_alpha.wgsl and vg_alpha_uv.wgsl.

@group(1) @binding(0) var<storage, read_write> sw_keys: array<atomic<u64>>;
@group(1) @binding(1) var hw_depth: texture_depth_2d;
@group(1) @binding(2) var<storage, read_write> tile_counters: array<atomic<u32>>;
@group(1) @binding(3) var<storage, read_write> stats: array<atomic<u32>>;
@group(1) @binding(4) var<storage, read> sw_meta: array<u32>;

var<workgroup> sh_pos: array<vec4<f32>, 128>;
var<workgroup> sh_uv: array<vec2<f32>, 128>;

@compute @workgroup_size(128)
fn sw_raster_masked_main(
    @builtin(workgroup_id) wid: vec3<u32>,
    @builtin(local_invocation_index) lid: u32,
) {
    let idx = wid.y * VG_SW_ROW + wid.x;
    let first = sw_meta[vg_params.sw_meta_word];
    let count = sw_meta[vg_params.sw_meta_word + 1u];
    let live = idx < count;

    let size_f = vec2<f32>(f32(vg_params.width), f32(vg_params.height));
    let size_i = vec2<i32>(i32(vg_params.width), i32(vg_params.height));
    let vis = vg_visible[first + min(idx, max(count, 1u) - 1u)];
    let cs = vis.cluster_slot;
    let cl = vgd_load_cluster(cs);
    let n_tris = select(0u, vgd_tri_count(cl), live);
    let n_verts = select(0u, vgd_vert_count(cl), live);
    let m = vg_object_transform(vis);
    let h = vgd_pos_block(cs, cl, vg_asset_of(vis));
    let material = vg_objects[vg_instances[vis.instance].object_row + vgd_material(cl)].head[2];
    let alpha_mat = vga_load(material);

    if (lid < n_verts) {
        sh_pos[lid] = vg_screen_vertex(m, h, lid, size_f);
        sh_uv[lid] = vga_uv0(cs, h, lid);
    }
    workgroupBarrier();

    if (lid < n_tris) {
        let ix = vgd_tri(cs, cl, lid);
        let mirrored = vg_is_mirrored(m);
        let va = sh_pos[ix.x];
        var vb = sh_pos[ix.y];
        var vc = sh_pos[ix.z];
        let uva = sh_uv[ix.x];
        var uvb = sh_uv[ix.y];
        var uvc = sh_uv[ix.z];
        let cull_back = (vg_params.flags & VG_FLAG_CULL_BACK) != 0u && !vga_double_sided(alpha_mat);
        // `vg_tri_setup` swaps b and c itself for a mirrored transform; the uv and
        // clip w of the swapped corners follow.
        let s = vg_tri_setup(va, vb, vc, size_i, cull_back, mirrored);
        if (mirrored) {
            let tv = vb;
            vb = vc;
            vc = tv;
            let tu = uvb;
            uvb = uvc;
            uvc = tu;
        }
        if (s.kind == VG_TRI_CLIPPED) {
            atomicAdd(&stats[VG_STAT_CLIPPED_TRIS], 1u);
        } else if (s.kind != VG_TRI_EMPTY) {
            if (s.kind == VG_TRI_FLOAT) {
                atomicAdd(&stats[VG_STAT_FLOAT_TRIS], 1u);
            }
            let vis_index = first + idx;
            let id = vg_vis_pack(vis_index, lid);

            // Perspective-correct uv inputs and the footprint's mip level.
            let iw = vec3<f32>(1.0 / va.w, 1.0 / vb.w, 1.0 / vc.w);
            let e1 = uvb - uva;
            let e2 = uvc - uva;
            let uv_area = 0.5 * abs(e1.x * e2.y - e1.y * e2.x);
            var px_area: f32;
            if (s.kind == VG_TRI_INT) {
                px_area = 0.5 * abs(f32(s.a2)) / (VG_SNAP * VG_SNAP);
            } else {
                px_area = 0.5 * abs(bitcast<f32>(s.a2));
            }
            let lod = vga_lod_for_area(alpha_mat, uv_area, px_area);

            let tx0 = u32(s.px0) >> 4u;
            let ty0 = u32(s.py0) >> 4u;
            let tx1 = min(u32(s.px1) >> 4u, tx0 + 7u);
            let ty1 = min(u32(s.py1) >> 4u, ty0 + 7u);
            for (var ty = ty0; ty <= ty1; ty++) {
                for (var tx = tx0; tx <= tx1; tx++) {
                    atomicAdd(&tile_counters[(ty * vg_params.tiles_x + tx) * 2u], 1u);
                }
            }

            var run_tile = 0xffffffffu;
            var run_hits = 0u;
            for (var y = s.py0; y <= s.py1; y++) {
                for (var x = s.px0; x <= s.px1; x++) {
                    let c = vg_tri_cover_at(s, x, y);
                    let d = c.w;
                    if (d < 0.0) { continue; }
                    if (d < textureLoad(hw_depth, vec2<i32>(x, y), 0)) { continue; }
                    // Alpha test before the race.
                    let wgt = c.xyz * iw;
                    let uv = (uva * wgt.x + uvb * wgt.y + uvc * wgt.z) / max(wgt.x + wgt.y + wgt.z, 1.0e-30);
                    if (vga_rejects(alpha_mat, vga_alpha_level(alpha_mat, uv, lod))) { continue; }
                    let key = (u64(bitcast<u32>(d)) << 32u) | u64(id);
                    atomicMax(&sw_keys[u32(y) * vg_params.width + u32(x)], key);
                    let tile = (u32(y) >> 4u) * vg_params.tiles_x + (u32(x) >> 4u);
                    if (tile != run_tile) {
                        if (run_hits != 0u) {
                            atomicAdd(&tile_counters[run_tile * 2u + 1u], run_hits);
                        }
                        run_tile = tile;
                        run_hits = 0u;
                    }
                    run_hits++;
                }
                if (run_hits != 0u) {
                    atomicAdd(&tile_counters[run_tile * 2u + 1u], run_hits);
                    run_hits = 0u;
                }
                run_tile = 0xffffffffu;
            }
        }
    }
}
