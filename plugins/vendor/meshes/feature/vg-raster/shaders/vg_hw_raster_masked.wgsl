// Tier 1 for alpha-tested (masked) clusters: the vertex-pulling raster of
// `vg_hw_raster.wgsl` plus the programmable part. The vertex stage also
// decodes uv0 and the cluster's material row; the fragment stage samples the
// base-color alpha with the interpolated uv and `discard`s below the cutoff,
// otherwise it writes the same vis id. Back faces are discarded in the
// fragment stage for single-sided materials (the pipeline culls nothing, so
// two-sided foliage keeps both faces).
//
// One `draw_indirect` of `3 * 128` vertices per cluster (bin 6 of the cull).
// Separate pipeline: the opaque pipeline has no texture bindings and no
// branches.
//
// Requires vg_format.wgsl, vg_bindings.wgsl (+ decode bindings), vg_decode.wgsl,
// the vg_alpha bindings and vg_alpha.wgsl + vg_alpha_uv.wgsl.

struct MVsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) id: u32,
    @location(1) uv: vec2<f32>,
    @location(2) @interpolate(flat) material: u32,
}

@vertex
fn vs_masked(
    @builtin(vertex_index) vi: u32,
    @builtin(instance_index) ii: u32,
) -> MVsOut {
    var out: MVsOut;
    out.pos = vec4<f32>(0.0, 0.0, 2.0, 1.0);
    out.id = 0u;
    out.uv = vec2<f32>(0.0);
    out.material = 0u;

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
    out.uv = vga_uv0(cs, blk, idx[corner]);
    // Section `material_slot` of the instance is the object row's material.
    out.material = vg_objects[vg_instances[vis.instance].object_row + vgd_material(cl)].head[2];
    return out;
}

@fragment
fn fs_masked(in: MVsOut, @builtin(front_facing) front: bool) -> @location(0) u32 {
    // Derivatives first, in uniform control flow.
    let dx = dpdx(in.uv);
    let dy = dpdy(in.uv);
    let a = vga_load(in.material);
    if (!front && !vga_double_sided(a)) {
        discard;
    }
    let alpha = vga_alpha_grad(a, in.uv, dx, dy);
    if (vga_rejects(a, alpha)) {
        discard;
    }
    return in.id;
}
