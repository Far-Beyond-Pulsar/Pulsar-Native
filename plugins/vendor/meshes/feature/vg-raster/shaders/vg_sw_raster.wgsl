// Tier 2: compute micro-polygon rasterizer with 64-bit atomicMax.
//
// One workgroup (128 threads) per software-bin cluster. Phase 1: thread t < vert_count
// decodes and projects vertex t into shared memory. Phase 2: thread t < tri_count
// rasterizes triangle t: pixel-center coverage over its (small) bbox, depth
// tested against the Tier 1 depth, then `atomicMax` on
// `(depth_bits << 32) | vis_id` into `sw_keys` (reversed-Z: larger = nearer, so
// the max is the nearest fragment, ties broken by the larger id).
//
// Requires SHADER_INT64 | SHADER_INT64_ATOMIC_MIN_MAX. Without them use
// `vg_sw_bin.wgsl` + `vg_tile_raster.wgsl` (same results, no 64-bit atomics).
//
// Requires vg_format.wgsl, vg_bindings.wgsl, vg_decode.wgsl, vg_raster_common.wgsl.

@group(1) @binding(0) var<storage, read_write> sw_keys: array<atomic<u64>>;
@group(1) @binding(1) var hw_depth: texture_depth_2d;
// Per tile: [2t] = triangles overlapping the tile, [2t+1] = pixel hits that
// passed the depth test (the overlap/contention signal for `tile_classify`).
@group(1) @binding(2) var<storage, read_write> tile_counters: array<atomic<u32>>;
@group(1) @binding(3) var<storage, read_write> stats: array<atomic<u32>>;
// [off] = index of the first SW cluster in `vg_visible`, [off + 1] = SW cluster count
// (`off` = vg_params.sw_meta_word: a standalone `SwMeta` or the cull's `state` ranges).
@group(1) @binding(4) var<storage, read> sw_meta: array<u32>;

var<workgroup> sh_pos: array<vec4<f32>, 128>;

@compute @workgroup_size(128)
fn sw_raster_main(
    @builtin(workgroup_id) wid: vec3<u32>,
    @builtin(local_invocation_index) lid: u32,
) {
    // 2D dispatch {min(n, 32768), ceil(n / 32768), 1}: cluster = first + y * 32768 + x.
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

    if (lid < n_verts) {
        sh_pos[lid] = vg_screen_vertex(m, h, lid, size_f);
    }
    workgroupBarrier();

    if (lid < n_tris) {
        let ix = vgd_tri(cs, cl, lid);
        let s = vg_tri_setup(
            sh_pos[ix.x], sh_pos[ix.y], sh_pos[ix.z],
            size_i,
            (vg_params.flags & VG_FLAG_CULL_BACK) != 0u,
            vg_is_mirrored(m),
        );
        if (s.kind == VG_TRI_CLIPPED) {
            atomicAdd(&stats[VG_STAT_CLIPPED_TRIS], 1u);
        } else if (s.kind != VG_TRI_EMPTY) {
            if (s.kind == VG_TRI_FLOAT) {
                atomicAdd(&stats[VG_STAT_FLOAT_TRIS], 1u);
            }
            // The visible-cluster index is the draw's instance index in Tier 1;
            // here it is the position in the visible list.
            let vis_index = first + idx;
            let id = vg_vis_pack(vis_index, lid);

            // Tile overlap counters (bounded to 8x8 tiles for huge triangles).
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
                    let d = vg_tri_depth_at(s, x, y);
                    if (d < 0.0) { continue; }
                    if (d < textureLoad(hw_depth, vec2<i32>(x, y), 0)) { continue; }
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
