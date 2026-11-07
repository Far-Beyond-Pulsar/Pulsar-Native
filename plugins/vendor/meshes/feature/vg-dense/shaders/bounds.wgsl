// Conservative tile rectangle of a visible cluster's bounding sphere, shared by
// the 3-NonRT `bin` kernel and the 3-RT candidate collector. Needs `params`
// (DenseParams) and `dense_xform` (inputs.wgsl) declared before it.
//
// The sphere bound is conservative (see `sphere_tile_bounds` in
// `src/peel_ref.rs`, the CPU mirror that tests it). A cluster straddling the
// near plane covers every tile.
struct TileRect {
    ok: bool,
    tx0: u32,
    ty0: u32,
    tx1: u32,
    ty1: u32,
}

fn cluster_tile_rect(v: VgVisibleCluster, c: VgCluster) -> TileRect {
    var out: TileRect;
    out.ok = false;
    let m = dense_xform(v);
    let wc = (m * vec4<f32>(c.sphere.xyz, 1.0)).xyz;
    let sc = max(length(m[0].xyz), max(length(m[1].xyz), length(m[2].xyz)));
    let r = c.sphere.w * sc;
    let vp = params.view_proj;
    let cc = vp * vec4<f32>(wc, 1.0);
    let lx = length(vec3<f32>(vp[0][0], vp[1][0], vp[2][0]));
    let ly = length(vec3<f32>(vp[0][1], vp[1][1], vp[2][1]));
    let lw = length(vec3<f32>(vp[0][3], vp[1][3], vp[2][3]));
    let wmin = cc.w - r * lw;
    let wmax = cc.w + r * lw;
    if wmax <= 0.0 { return out; }
    let tiles_x = params.viewport.z;
    let tiles_y = params.viewport.w;
    out.tx0 = 0u;
    out.ty0 = 0u;
    out.tx1 = tiles_x - 1u;
    out.ty1 = tiles_y - 1u;
    if wmin > 1.0e-4 {
        let xlo = cc.x - r * lx;
        let xhi = cc.x + r * lx;
        let ylo = cc.y - r * ly;
        let yhi = cc.y + r * ly;
        let nx0 = min(xlo / wmin, xlo / wmax);
        let nx1 = max(xhi / wmin, xhi / wmax);
        let ny0 = min(ylo / wmin, ylo / wmax);
        let ny1 = max(yhi / wmin, yhi / wmax);
        let fw = f32(params.viewport.x);
        let fh = f32(params.viewport.y);
        let px0 = (nx0 * 0.5 + 0.5) * fw;
        let px1 = (nx1 * 0.5 + 0.5) * fw;
        let py0 = (0.5 - ny1 * 0.5) * fh;
        let py1 = (0.5 - ny0 * 0.5) * fh;
        if px1 < 0.0 || px0 >= fw || py1 < 0.0 || py0 >= fh { return out; }
        let ft = f32(TILE);
        out.tx0 = u32(clamp(floor(px0 / ft), 0.0, f32(tiles_x - 1u)));
        out.tx1 = u32(clamp(floor(px1 / ft), 0.0, f32(tiles_x - 1u)));
        out.ty0 = u32(clamp(floor(py0 / ft), 0.0, f32(tiles_y - 1u)));
        out.ty1 = u32(clamp(floor(py1 / ft), 0.0, f32(tiles_y - 1u)));
    }
    out.ok = true;
    return out;
}
