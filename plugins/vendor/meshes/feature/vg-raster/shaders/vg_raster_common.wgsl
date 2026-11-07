// Shared micro-polygon triangle setup and coverage test, used by the 64-bit
// atomic kernel (`vg_sw_raster.wgsl`), the tile binner (`vg_sw_bin.wgsl`) and
// the tile raster kernel (`vg_tile_raster.wgsl`). The CPU mirror is
// `helio_pass_vg_raster::reference`.
//
// Requires vg_format.wgsl, vg_bindings.wgsl, vg_decode.wgsl.
//
// Screen vertices are vec4(x_px, y_px, z_ndc, w_clip), y pointing down.
//
// Exactness: when the triangle bbox is <= VG_INT_EXTENT px the vertices are
// snapped to 1/256 px and edge functions are evaluated in i32 (differences
// are small, so no overflow): coverage is then exact and watertight between
// neighbouring triangles. Larger triangles (which should have been routed to
// Tier 1) use f32 edge functions: coverage is still correct but not
// bit-watertight across shared edges. Both use an inclusive (>= 0)
// pixel-center test, so shared edges may be hit twice (harmless for a depth
// race) but never missed on the integer path.

const VG_SNAP: f32 = 256.0;
const VG_INT_EXTENT: f32 = 64.0;
const VG_GUARD: f32 = 1048576.0;

const VG_TRI_EMPTY: u32 = 0u;   // culled / no pixel center covered
const VG_TRI_INT: u32 = 1u;
const VG_TRI_FLOAT: u32 = 2u;
const VG_TRI_CLIPPED: u32 = 3u; // crosses a clip plane: not handled by SW

struct VgTriSetup {
    kind: u32,
    // snapped ints (VG_TRI_INT) or bitcast<i32>(f32) screen coords (VG_TRI_FLOAT)
    x0: i32, y0: i32, x1: i32, y1: i32, x2: i32, y2: i32,
    // signed doubled area: i32 (INT) or bitcast<i32>(f32) (FLOAT)
    a2: i32,
    z0: f32, z1: f32, z2: f32,
    // inclusive pixel bbox clamped to the viewport
    px0: i32, py0: i32, px1: i32, py1: i32,
}

fn vg_edge_i(ux: i32, uy: i32, vx: i32, vy: i32, px: i32, py: i32) -> i32 {
    return (vx - ux) * (py - uy) - (vy - uy) * (px - ux);
}
fn vg_edge_f(ux: f32, uy: f32, vx: f32, vy: f32, px: f32, py: f32) -> f32 {
    return (vx - ux) * (py - uy) - (vy - uy) * (px - ux);
}

fn vg_vertex_valid(v: vec4<f32>) -> bool {
    return v.w > 0.0 && v.z > 0.0 && v.z <= 1.000001;
}

// Project a clip-space vertex to the screen vec4 described above.
// Invalid (behind the eye / outside the depth range) vertices get w = -1.
fn vg_to_screen(clip: vec4<f32>, size: vec2<f32>) -> vec4<f32> {
    if (clip.w <= 0.0) { return vec4<f32>(0.0, 0.0, 0.0, -1.0); }
    let ndc = clip.xyz / clip.w;
    let v = vec4<f32>(
        (ndc.x * 0.5 + 0.5) * size.x,
        (0.5 - ndc.y * 0.5) * size.y,
        ndc.z,
        clip.w,
    );
    if (!vg_vertex_valid(v)) { return vec4<f32>(0.0, 0.0, 0.0, -1.0); }
    return v;
}

fn vg_tri_setup(va: vec4<f32>, vb: vec4<f32>, vc: vec4<f32>, size: vec2<i32>, cull_back: bool, mirrored: bool) -> VgTriSetup {
    var s = VgTriSetup();
    if (va.w <= 0.0 || vb.w <= 0.0 || vc.w <= 0.0) {
        s.kind = VG_TRI_CLIPPED;
        return s;
    }
    var b = vb;
    var c = vc;
    if (mirrored) { b = vc; c = vb; }
    let a = va;

    let minx = min(a.x, min(b.x, c.x));
    let maxx = max(a.x, max(b.x, c.x));
    let miny = min(a.y, min(b.y, c.y));
    let maxy = max(a.y, max(b.y, c.y));
    let guard = max(max(abs(minx), abs(maxx)), max(abs(miny), abs(maxy)));
    if (!(guard < VG_GUARD)) {            // also catches NaN
        s.kind = VG_TRI_CLIPPED;
        return s;
    }
    if (maxx < 0.0 || maxy < 0.0 || minx > f32(size.x) || miny > f32(size.y)) {
        return s;
    }
    s.z0 = a.z; s.z1 = b.z; s.z2 = c.z;
    let extent = max(maxx - minx, maxy - miny);

    if (extent <= VG_INT_EXTENT) {
        let ax = i32(round(a.x * VG_SNAP)); let ay = i32(round(a.y * VG_SNAP));
        let bx = i32(round(b.x * VG_SNAP)); let by = i32(round(b.y * VG_SNAP));
        let cx = i32(round(c.x * VG_SNAP)); let cy = i32(round(c.y * VG_SNAP));
        let a2 = vg_edge_i(ax, ay, bx, by, cx, cy);
        if (a2 == 0) { return s; }
        // Screen y points down, so a counter-clockwise (front) triangle has a2 < 0.
        if (cull_back && a2 > 0) { return s; }
        let mnx = min(ax, min(bx, cx)); let mxx = max(ax, max(bx, cx));
        let mny = min(ay, min(by, cy)); let mxy = max(ay, max(by, cy));
        // Pixel centers sit at (p*256 + 128): ceil / floor of the bbox in pixels.
        s.px0 = max((mnx + 127) >> 8u, 0);
        s.py0 = max((mny + 127) >> 8u, 0);
        s.px1 = min((mxx - 128) >> 8u, size.x - 1);
        s.py1 = min((mxy - 128) >> 8u, size.y - 1);
        if (s.px0 > s.px1 || s.py0 > s.py1) { return s; }
        s.kind = VG_TRI_INT;
        s.x0 = ax; s.y0 = ay; s.x1 = bx; s.y1 = by; s.x2 = cx; s.y2 = cy;
        s.a2 = a2;
        return s;
    }

    let a2 = vg_edge_f(a.x, a.y, b.x, b.y, c.x, c.y);
    if (a2 == 0.0) { return s; }
    if (cull_back && a2 > 0.0) { return s; }
    s.px0 = max(i32(ceil(minx - 0.5)), 0);
    s.py0 = max(i32(ceil(miny - 0.5)), 0);
    s.px1 = min(i32(floor(maxx - 0.5)), size.x - 1);
    s.py1 = min(i32(floor(maxy - 0.5)), size.y - 1);
    if (s.px0 > s.px1 || s.py0 > s.py1) { return s; }
    s.kind = VG_TRI_FLOAT;
    s.x0 = bitcast<i32>(a.x); s.y0 = bitcast<i32>(a.y);
    s.x1 = bitcast<i32>(b.x); s.y1 = bitcast<i32>(b.y);
    s.x2 = bitcast<i32>(c.x); s.y2 = bitcast<i32>(c.y);
    s.a2 = bitcast<i32>(a2);
    return s;
}

// Depth (reversed-Z, > 0) of the triangle at the center of pixel (px, py),
// or -1.0 if the center is not covered.
fn vg_tri_depth_at(s: VgTriSetup, px: i32, py: i32) -> f32 {
    if (s.kind == VG_TRI_EMPTY || s.kind == VG_TRI_CLIPPED) { return -1.0; }
    if (px < s.px0 || px > s.px1 || py < s.py0 || py > s.py1) { return -1.0; }
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
        if (w0 * sg < 0 || w1 * sg < 0 || w2 * sg < 0) { return -1.0; }
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
        if (w0 * sg < 0.0 || w1 * sg < 0.0 || w2 * sg < 0.0) { return -1.0; }
        let inv = 1.0 / a2;
        l0 = w0 * inv; l1 = w1 * inv; l2 = w2 * inv;
    }
    return max(l0 * s.z0 + l1 * s.z1 + l2 * s.z2, 1e-30);
}

// Transform one cluster vertex to screen space.
fn vg_screen_vertex(m: mat4x4<f32>, h: VgdPos, i: u32, size: vec2<f32>) -> vec4<f32> {
    return vg_to_screen(vg_to_clip(m, vgd_pos(h, i)), size);
}

// Statistic slots in the `stats` buffer.
const VG_STAT_FLOAT_TRIS: u32 = 0u;
const VG_STAT_CLIPPED_TRIS: u32 = 1u;
const VG_STAT_BIN_OVERFLOW: u32 = 2u;
