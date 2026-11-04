// Tier 1: hardware visibility-buffer raster by vertex pulling.
//
// One `draw_indirect` per triangle-count bin (32/64/96/128): vertex_count =
// 3 * bin cap, instance_count = clusters in the bin, first_instance = index of
// the bin's first cluster in `vg_visible`. `instance_index` therefore *is* the
// visible-cluster index, and `vertex_index / 3` the triangle. Triangles past
// the cluster's real count collapse to a clipped point. No material
// evaluation: the fragment stage writes the id word only.
//
// Requires vg_format.wgsl, vg_bindings.wgsl (+ decode bindings), vg_decode.wgsl.

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) id: u32,
}

@vertex
fn vs_main(
    @builtin(vertex_index) vi: u32,
    @builtin(instance_index) ii: u32,
) -> VsOut {
    var out: VsOut;
    // Outside the clip volume and degenerate: discarded by clipping.
    out.pos = vec4<f32>(0.0, 0.0, 2.0, 1.0);
    out.id = 0u;

    let vis = vg_visible[ii];
    let cs = vis.cluster_slot;
    let cl = vgd_load_cluster(cs);
    let tri = vi / 3u;
    if (tri >= vgd_tri_count(cl)) {
        return out;
    }
    let m = vg_object_transform(vis);
    var corner = vi % 3u;
    if (corner != 0u && vg_is_mirrored(m)) {
        corner = 3u - corner;
    }
    let idx = vgd_tri(cs, cl, tri);
    let blk = vgd_pos_block(cs, cl, vg_asset_of(vis));
    let p = vgd_pos(blk, idx[corner]);
    out.pos = vg_to_clip(m, p);
    out.id = vg_vis_pack(ii, tri);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) u32 {
    return in.id;
}
