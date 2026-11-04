// Tier 2 fallback, stage 1 (no 64-bit atomics): bin software-cluster
// triangles into per-16x16-tile lists.
//
// Same workgroup shape as `vg_sw_raster.wgsl`. Each surviving triangle appends
// its vis id to the list of every tile its pixel bbox overlaps
// (`tile_bin_entries[tile * cap + slot]`, `slot = atomicAdd(tile_bin_counts[tile])`).
// A tile that overflows its capacity drops the entry and counts it in
// `stats[VG_STAT_BIN_OVERFLOW]` (bounded, surfaced in the debug UI; see crate docs).
// `vg_tile_raster.wgsl` consumes the lists.
//
// Requires vg_format.wgsl, vg_bindings.wgsl, vg_decode.wgsl, vg_raster_common.wgsl.

@group(1) @binding(0) var<storage, read_write> tile_bin_counts: array<atomic<u32>>;
@group(1) @binding(1) var<storage, read_write> tile_bin_entries: array<u32>;
@group(1) @binding(2) var<storage, read_write> stats: array<atomic<u32>>;
@group(1) @binding(3) var<storage, read> sw_meta: array<u32>;

var<workgroup> sh_pos: array<vec4<f32>, 128>;

@compute @workgroup_size(128)
fn sw_bin_main(
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
            let id = vg_vis_pack(first + idx, lid);
            let tx0 = u32(s.px0) >> 4u;
            let ty0 = u32(s.py0) >> 4u;
            let tx1 = u32(s.px1) >> 4u;
            let ty1 = u32(s.py1) >> 4u;
            for (var ty = ty0; ty <= ty1; ty++) {
                for (var tx = tx0; tx <= tx1; tx++) {
                    let tile = ty * vg_params.tiles_x + tx;
                    let slot = atomicAdd(&tile_bin_counts[tile], 1u);
                    if (slot < vg_params.tile_bin_cap) {
                        tile_bin_entries[tile * vg_params.tile_bin_cap + slot] = id;
                    } else {
                        atomicAdd(&stats[VG_STAT_BIN_OVERFLOW], 1u);
                    }
                }
            }
        }
    }
}
