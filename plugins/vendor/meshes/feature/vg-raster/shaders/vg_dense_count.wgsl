// Overlap bookkeeping for clusters the cull diverted to the dense bin.
//
// A cluster routed to the dense bin is NOT rasterised by Tier 1/2, so it adds
// nothing to `tile_counters`. If `tile_classify` looked at those counters
// alone, routing a tile's clusters away would starve the very counters that
// keep it dense, and the mask would collapse the next frame, release the
// clusters back to Tier 2, and come back one frame later: ON/OFF forever.
//
// This kernel closes the loop. One workgroup per dense-bin cluster (indirect,
// 2D split like the software dispatch); its 64 threads walk the tile
// rectangle of the cluster's bounding sphere and add an ESTIMATE of the
// triangle count per tile to `dense_est[tile]`:
//
//     est = ceil(EST_SCALE * triangles / rect_tiles)
//
// The sphere rectangle over-covers the surface patch, so the plain average
// under-counts what Tier 2 would have measured; `EST_SCALE` compensates.
// `tile_classify` adds `dense_est` to the Tier 2 triangle counter ONLY for
// tiles that were dense last frame (it can keep a tile dense, never create
// one), so the estimate cannot dilate the mask.
//
// Requires vg_format.wgsl, vg_bindings.wgsl, vg_decode.wgsl.

const EST_SCALE: u32 = 2u;

@group(1) @binding(0) var<storage, read_write> dense_est: array<atomic<u32>>;
// [off] = first dense-bin cluster (unified index), [off + 1] = count.
@group(1) @binding(1) var<storage, read> dense_meta: array<u32>;

@compute @workgroup_size(64)
fn dense_count_main(
    @builtin(workgroup_id) wid: vec3<u32>,
    @builtin(local_invocation_index) lid: u32,
) {
    let idx = wid.y * VG_SW_ROW + wid.x;
    let first = dense_meta[vg_params.sw_meta_word];
    let count = dense_meta[vg_params.sw_meta_word + 1u];
    if idx >= count { return; }
    let vis = vg_visible[first + idx];
    let c = vgd_load_cluster(vis.cluster_slot);
    let m = vg_object_transform(vis);
    let wc = (m * vec4<f32>(c.sphere.xyz, 1.0)).xyz;
    let sc = max(length(m[0].xyz), max(length(m[1].xyz), length(m[2].xyz)));
    let r = c.sphere.w * sc;
    let vp = vg_cameras[vg_params.view_index].view_proj;
    let cc = vp * vec4<f32>(wc, 1.0);
    let lx = length(vec3<f32>(vp[0][0], vp[1][0], vp[2][0]));
    let ly = length(vec3<f32>(vp[0][1], vp[1][1], vp[2][1]));
    let lw = length(vec3<f32>(vp[0][3], vp[1][3], vp[2][3]));
    let wmin = cc.w - r * lw;
    let wmax = cc.w + r * lw;
    if wmax <= 0.0 { return; }
    let tiles_x = vg_params.tiles_x;
    let tiles_y = vg_params.tiles_y;
    var tx0 = 0u;
    var ty0 = 0u;
    var tx1 = tiles_x - 1u;
    var ty1 = tiles_y - 1u;
    if wmin > 1.0e-4 {
        let xlo = cc.x - r * lx;
        let xhi = cc.x + r * lx;
        let ylo = cc.y - r * ly;
        let yhi = cc.y + r * ly;
        let nx0 = min(xlo / wmin, xlo / wmax);
        let nx1 = max(xhi / wmin, xhi / wmax);
        let ny0 = min(ylo / wmin, ylo / wmax);
        let ny1 = max(yhi / wmin, yhi / wmax);
        let fw = f32(vg_params.width);
        let fh = f32(vg_params.height);
        let px0 = (nx0 * 0.5 + 0.5) * fw;
        let px1 = (nx1 * 0.5 + 0.5) * fw;
        let py0 = (0.5 - ny1 * 0.5) * fh;
        let py1 = (0.5 - ny0 * 0.5) * fh;
        if px1 < 0.0 || px0 >= fw || py1 < 0.0 || py0 >= fh { return; }
        let ft = 16.0;
        tx0 = u32(clamp(floor(px0 / ft), 0.0, f32(tiles_x - 1u)));
        tx1 = u32(clamp(floor(px1 / ft), 0.0, f32(tiles_x - 1u)));
        ty0 = u32(clamp(floor(py0 / ft), 0.0, f32(tiles_y - 1u)));
        ty1 = u32(clamp(floor(py1 / ft), 0.0, f32(tiles_y - 1u)));
    }
    let w = tx1 - tx0 + 1u;
    let n = w * (ty1 - ty0 + 1u);
    let est = (vgd_tri_count(c) * EST_SCALE + n - 1u) / n;
    for (var i = lid; i < n; i += 64u) {
        let t = (ty0 + i / w) * tiles_x + tx0 + i % w;
        atomicAdd(&dense_est[t], est);
    }
}
