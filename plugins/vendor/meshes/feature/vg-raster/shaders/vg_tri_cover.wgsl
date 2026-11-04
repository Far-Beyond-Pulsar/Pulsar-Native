
// Like `vg_tri_depth_at`, but also returns the screen-space barycentrics:
// (l0, l1, l2, depth), with depth < 0 when the pixel center is not covered.
// Used by the alpha-masked software kernel, which interpolates uv from them.
// Kept out of `vg_raster_common.wgsl` so the opaque kernels do not carry it.
fn vg_tri_cover_at(s: VgTriSetup, px: i32, py: i32) -> vec4<f32> {
    let miss = vec4<f32>(0.0, 0.0, 0.0, -1.0);
    if (s.kind == VG_TRI_EMPTY || s.kind == VG_TRI_CLIPPED) { return miss; }
    if (px < s.px0 || px > s.px1 || py < s.py0 || py > s.py1) { return miss; }
    var l0: f32;
    var l1: f32;
    var l2: f32;
    if (s.kind == VG_TRI_INT) {
        let cx = px * 256 + 128;
        let cy = py * 256 + 128;
        let w0 = vg_edge_i(s.x1, s.y1, s.x2, s.y2, cx, cy);
        let w1 = vg_edge_i(s.x2, s.y2, s.x0, s.y0, cx, cy);
        let w2 = vg_edge_i(s.x0, s.y0, s.x1, s.y1, cx, cy);
        let sg = select(1, -1, s.a2 < 0);
        if (w0 * sg < 0 || w1 * sg < 0 || w2 * sg < 0) { return miss; }
        let inv = 1.0 / f32(s.a2);
        l0 = f32(w0) * inv; l1 = f32(w1) * inv; l2 = f32(w2) * inv;
    } else {
        let x0 = bitcast<f32>(s.x0); let y0 = bitcast<f32>(s.y0);
        let x1 = bitcast<f32>(s.x1); let y1 = bitcast<f32>(s.y1);
        let x2 = bitcast<f32>(s.x2); let y2 = bitcast<f32>(s.y2);
        let a2 = bitcast<f32>(s.a2);
        let cx = f32(px) + 0.5;
        let cy = f32(py) + 0.5;
        let w0 = vg_edge_f(x1, y1, x2, y2, cx, cy);
        let w1 = vg_edge_f(x2, y2, x0, y0, cx, cy);
        let w2 = vg_edge_f(x0, y0, x1, y1, cx, cy);
        let sg = select(1.0, -1.0, a2 < 0.0);
        if (w0 * sg < 0.0 || w1 * sg < 0.0 || w2 * sg < 0.0) { return miss; }
        let inv = 1.0 / a2;
        l0 = w0 * inv; l1 = w1 * inv; l2 = w2 * inv;
    }
    return vec4<f32>(l0, l1, l2, max(l0 * s.z0 + l1 * s.z1 + l2 * s.z2, 1e-30));
}
