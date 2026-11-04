//! CPU reference of the Tier 2 micro-polygon rasterizer. A line-for-line
//! mirror of `shaders/vg_raster_common.wgsl` (setup + coverage), plus a driver
//! that reproduces the 64-bit-key race and the tile counters of
//! `vg_sw_raster.wgsl`. The GPU kernels are tested against it, and it is the
//! executable specification of the fallback kernel's results.

use crate::abi::TILE_SIZE;

const SNAP: f32 = 256.0;
const INT_EXTENT: f32 = 64.0;
const GUARD: f32 = 1_048_576.0;

/// Screen-space vertex: x, y in pixels (y down), z = NDC depth (reversed-Z),
/// w = clip w; `w <= 0` marks an invalid (clipped) vertex.
pub type ScreenVertex = [f32; 4];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Empty,
    Int,
    Float,
    Clipped,
}

#[derive(Clone, Copy, Debug)]
pub struct TriSetup {
    pub kind: Kind,
    pub x: [i32; 3],
    pub y: [i32; 3],
    pub a2: i32,
    pub fx: [f32; 3],
    pub fy: [f32; 3],
    pub fa2: f32,
    pub z: [f32; 3],
    pub px0: i32,
    pub py0: i32,
    pub px1: i32,
    pub py1: i32,
}

fn empty(kind: Kind) -> TriSetup {
    TriSetup {
        kind,
        x: [0; 3],
        y: [0; 3],
        a2: 0,
        fx: [0.0; 3],
        fy: [0.0; 3],
        fa2: 0.0,
        z: [0.0; 3],
        px0: 0,
        py0: 0,
        px1: 0,
        py1: 0,
    }
}

fn edge_i(u: (i32, i32), v: (i32, i32), p: (i32, i32)) -> i32 {
    (v.0 - u.0) * (p.1 - u.1) - (v.1 - u.1) * (p.0 - u.0)
}
fn edge_f(u: (f32, f32), v: (f32, f32), p: (f32, f32)) -> f32 {
    (v.0 - u.0) * (p.1 - u.1) - (v.1 - u.1) * (p.0 - u.0)
}

/// Project a clip-space position to a [`ScreenVertex`] (mirror of `vg_to_screen`).
pub fn to_screen(clip: [f32; 4], size: [f32; 2]) -> ScreenVertex {
    if clip[3] <= 0.0 {
        return [0.0, 0.0, 0.0, -1.0];
    }
    let ndc = [clip[0] / clip[3], clip[1] / clip[3], clip[2] / clip[3]];
    let v = [
        (ndc[0] * 0.5 + 0.5) * size[0],
        (0.5 - ndc[1] * 0.5) * size[1],
        ndc[2],
        clip[3],
    ];
    if !(v[3] > 0.0 && v[2] > 0.0 && v[2] <= 1.000001) {
        return [0.0, 0.0, 0.0, -1.0];
    }
    v
}

/// Mirror of `vg_tri_setup`.
pub fn tri_setup(
    va: ScreenVertex,
    vb: ScreenVertex,
    vc: ScreenVertex,
    size: [i32; 2],
    cull_back: bool,
    mirrored: bool,
) -> TriSetup {
    if va[3] <= 0.0 || vb[3] <= 0.0 || vc[3] <= 0.0 {
        return empty(Kind::Clipped);
    }
    let (b, c) = if mirrored { (vc, vb) } else { (vb, vc) };
    let a = va;
    let minx = a[0].min(b[0]).min(c[0]);
    let maxx = a[0].max(b[0]).max(c[0]);
    let miny = a[1].min(b[1]).min(c[1]);
    let maxy = a[1].max(b[1]).max(c[1]);
    let guard = minx.abs().max(maxx.abs()).max(miny.abs().max(maxy.abs()));
    if !(guard < GUARD) {
        return empty(Kind::Clipped);
    }
    if maxx < 0.0 || maxy < 0.0 || minx > size[0] as f32 || miny > size[1] as f32 {
        return empty(Kind::Empty);
    }
    let mut s = empty(Kind::Empty);
    s.z = [a[2], b[2], c[2]];
    let extent = (maxx - minx).max(maxy - miny);
    if extent <= INT_EXTENT {
        let sn = |v: f32| (v * SNAP).round_ties_even() as i32;
        let (ax, ay, bx, by, cx, cy) = (sn(a[0]), sn(a[1]), sn(b[0]), sn(b[1]), sn(c[0]), sn(c[1]));
        let a2 = edge_i((ax, ay), (bx, by), (cx, cy));
        if a2 == 0 || (cull_back && a2 > 0) {
            return s;
        }
        let (mnx, mxx) = (ax.min(bx).min(cx), ax.max(bx).max(cx));
        let (mny, mxy) = (ay.min(by).min(cy), ay.max(by).max(cy));
        s.px0 = ((mnx + 127) >> 8).max(0);
        s.py0 = ((mny + 127) >> 8).max(0);
        s.px1 = ((mxx - 128) >> 8).min(size[0] - 1);
        s.py1 = ((mxy - 128) >> 8).min(size[1] - 1);
        if s.px0 > s.px1 || s.py0 > s.py1 {
            return s;
        }
        s.kind = Kind::Int;
        s.x = [ax, bx, cx];
        s.y = [ay, by, cy];
        s.a2 = a2;
        return s;
    }
    let fa2 = edge_f((a[0], a[1]), (b[0], b[1]), (c[0], c[1]));
    if fa2 == 0.0 || (cull_back && fa2 > 0.0) {
        return s;
    }
    s.px0 = ((minx - 0.5).ceil() as i32).max(0);
    s.py0 = ((miny - 0.5).ceil() as i32).max(0);
    s.px1 = ((maxx - 0.5).floor() as i32).min(size[0] - 1);
    s.py1 = ((maxy - 0.5).floor() as i32).min(size[1] - 1);
    if s.px0 > s.px1 || s.py0 > s.py1 {
        return s;
    }
    s.kind = Kind::Float;
    s.fx = [a[0], b[0], c[0]];
    s.fy = [a[1], b[1], c[1]];
    s.fa2 = fa2;
    s
}

/// Mirror of `vg_tri_depth_at`: depth at the pixel center or `None`.
pub fn depth_at(s: &TriSetup, px: i32, py: i32) -> Option<f32> {
    if matches!(s.kind, Kind::Empty | Kind::Clipped) {
        return None;
    }
    if px < s.px0 || px > s.px1 || py < s.py0 || py > s.py1 {
        return None;
    }
    let (l0, l1, l2);
    if s.kind == Kind::Int {
        let p = (px * 256 + 128, py * 256 + 128);
        let v = |i: usize| (s.x[i], s.y[i]);
        let w0 = edge_i(v(1), v(2), p);
        let w1 = edge_i(v(2), v(0), p);
        let w2 = edge_i(v(0), v(1), p);
        let sg = if s.a2 < 0 { -1 } else { 1 };
        if w0 * sg < 0 || w1 * sg < 0 || w2 * sg < 0 {
            return None;
        }
        let inv = 1.0 / s.a2 as f32;
        l0 = w0 as f32 * inv;
        l1 = w1 as f32 * inv;
        l2 = w2 as f32 * inv;
    } else {
        let p = (px as f32 + 0.5, py as f32 + 0.5);
        let v = |i: usize| (s.fx[i], s.fy[i]);
        let w0 = edge_f(v(1), v(2), p);
        let w1 = edge_f(v(2), v(0), p);
        let w2 = edge_f(v(0), v(1), p);
        let sg = if s.fa2 < 0.0 { -1.0 } else { 1.0 };
        if w0 * sg < 0.0 || w1 * sg < 0.0 || w2 * sg < 0.0 {
            return None;
        }
        let inv = 1.0 / s.fa2;
        l0 = w0 * inv;
        l1 = w1 * inv;
        l2 = w2 * inv;
    }
    Some((l0 * s.z[0] + l1 * s.z[1] + l2 * s.z[2]).max(1e-30))
}

/// A triangle submitted to the reference rasterizer.
#[derive(Clone, Copy, Debug)]
pub struct RefTri {
    pub v: [ScreenVertex; 3],
    /// The vis id written for this triangle.
    pub id: u32,
    pub mirrored: bool,
}

/// Result of the reference raster.
pub struct RefRaster {
    pub width: u32,
    pub height: u32,
    /// `depth_bits << 32 | id` per pixel, 0 = empty. Same as the GPU key buffer.
    pub keys: Vec<u64>,
    /// `[tile*2]` = triangles overlapping the tile, `[tile*2+1]` = depth-test-passing hits.
    pub tile_counters: Vec<u32>,
    pub float_tris: u32,
    pub clipped_tris: u32,
}

/// Rasterize `tris` the way `vg_sw_raster.wgsl` does: depth test against
/// `hw_depth` (row-major, reversed-Z, or `None` = empty), then keep the max
/// 64-bit key per pixel.
pub fn raster(
    tris: &[RefTri],
    width: u32,
    height: u32,
    hw_depth: Option<&[f32]>,
    cull_back: bool,
) -> RefRaster {
    let tiles_x = width.div_ceil(TILE_SIZE);
    let tiles_y = height.div_ceil(TILE_SIZE);
    let mut out = RefRaster {
        width,
        height,
        keys: vec![0; (width * height) as usize],
        tile_counters: vec![0; (tiles_x * tiles_y * 2) as usize],
        float_tris: 0,
        clipped_tris: 0,
    };
    for t in tris {
        let s = tri_setup(
            t.v[0],
            t.v[1],
            t.v[2],
            [width as i32, height as i32],
            cull_back,
            t.mirrored,
        );
        match s.kind {
            Kind::Clipped => {
                out.clipped_tris += 1;
                continue;
            }
            Kind::Empty => continue,
            Kind::Float => out.float_tris += 1,
            Kind::Int => {}
        }
        let (tx0, ty0) = (s.px0 as u32 >> 4, s.py0 as u32 >> 4);
        let (tx1, ty1) = (
            (s.px1 as u32 >> 4).min(tx0 + 7),
            (s.py1 as u32 >> 4).min(ty0 + 7),
        );
        for ty in ty0..=ty1 {
            for tx in tx0..=tx1 {
                out.tile_counters[((ty * tiles_x + tx) * 2) as usize] += 1;
            }
        }
        for y in s.py0..=s.py1 {
            for x in s.px0..=s.px1 {
                let Some(d) = depth_at(&s, x, y) else {
                    continue;
                };
                let i = (y as u32 * width + x as u32) as usize;
                if let Some(h) = hw_depth {
                    if d < h[i] {
                        continue;
                    }
                }
                let key = ((d.to_bits() as u64) << 32) | t.id as u64;
                if key > out.keys[i] {
                    out.keys[i] = key;
                }
                let tile = (y as u32 >> 4) * tiles_x + (x as u32 >> 4);
                out.tile_counters[(tile * 2 + 1) as usize] += 1;
            }
        }
    }
    out
}

/// CPU mirror of `vg_tile_classify.wgsl`: dense flags per tile.
pub fn classify(
    counters: &[u32],
    prev: &[u32],
    tri_thresh: u32,
    hit_thresh: u32,
    hysteresis: f32,
) -> Vec<u32> {
    (0..counters.len() / 2)
        .map(|t| {
            let (tris, hits) = (counters[t * 2] as f32, counters[t * 2 + 1] as f32);
            let k = if prev[t] != 0 { hysteresis } else { 1.0 };
            (tris >= tri_thresh as f32 * k || hits >= hit_thresh as f32 * k) as u32
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Front-facing (a2 < 0 in y-down pixel space) triangle at constant depth.
    fn tri(p: [[f32; 2]; 3], z: f32, id: u32) -> RefTri {
        let v = |q: [f32; 2]| [q[0], q[1], z, 1.0];
        let mut t = RefTri {
            v: [v(p[0]), v(p[1]), v(p[2])],
            id,
            mirrored: false,
        };
        let s = tri_setup(t.v[0], t.v[1], t.v[2], [4096, 4096], false, false);
        if s.kind == Kind::Int && s.a2 > 0 || s.kind == Kind::Float && s.fa2 > 0.0 {
            t.v.swap(1, 2);
        }
        t
    }

    #[test]
    fn subpixel_triangle_hits_only_if_it_covers_a_center() {
        // Tiny triangle around pixel (3,3)'s center (3.5, 3.5).
        let hit = tri([[3.4, 3.4], [3.7, 3.45], [3.5, 3.7]], 0.5, 1);
        let r = raster(&[hit], 8, 8, None, true);
        assert_ne!(r.keys[3 * 8 + 3], 0);
        assert_eq!(r.keys.iter().filter(|&&k| k != 0).count(), 1);
        // Same size, between centers: nothing.
        let miss = tri([[3.6, 3.6], [3.9, 3.65], [3.7, 3.9]], 0.5, 2);
        let r = raster(&[miss], 8, 8, None, true);
        assert!(r.keys.iter().all(|&k| k == 0));
    }

    #[test]
    fn nearer_wins_and_hw_depth_rejects() {
        let far = tri([[0.0, 0.0], [8.0, 0.0], [0.0, 8.0]], 0.2, 7);
        let near = tri([[0.0, 0.0], [8.0, 0.0], [0.0, 8.0]], 0.6, 5);
        let r = raster(&[near, far], 8, 8, None, true);
        let k = r.keys[0];
        assert_eq!(raster_id(k), 5);
        assert!((f32::from_bits((k >> 32) as u32) - 0.6).abs() < 1e-6);
        // HW depth nearer than both: everything rejected.
        let hw = vec![0.9f32; 64];
        let r = raster(&[near, far], 8, 8, Some(&hw), true);
        assert!(r.keys.iter().all(|&k| k == 0));
        // HW depth between them: only `near` survives.
        let hw = vec![0.4f32; 64];
        let r = raster(&[near, far], 8, 8, Some(&hw), true);
        assert_eq!(raster_id(r.keys[0]), 5);
    }

    fn raster_id(k: u64) -> u32 {
        k as u32
    }

    #[test]
    fn equal_depth_tie_goes_to_larger_id() {
        let a = tri([[0.0, 0.0], [8.0, 0.0], [0.0, 8.0]], 0.5, 3);
        let b = tri([[0.0, 0.0], [8.0, 0.0], [0.0, 8.0]], 0.5, 9);
        for order in [[a, b], [b, a]] {
            let r = raster(&order, 8, 8, None, true);
            assert_eq!(raster_id(r.keys[0]), 9);
        }
    }

    #[test]
    fn back_faces_cull_and_mirroring_flips_winding() {
        let front = tri([[0.0, 0.0], [8.0, 0.0], [0.0, 8.0]], 0.5, 1);
        let mut back = front;
        back.v.swap(1, 2);
        assert!(raster(&[back], 8, 8, None, true)
            .keys
            .iter()
            .all(|&k| k == 0));
        assert!(raster(&[back], 8, 8, None, false)
            .keys
            .iter()
            .any(|&k| k != 0));
        back.mirrored = true;
        assert!(raster(&[back], 8, 8, None, true)
            .keys
            .iter()
            .any(|&k| k != 0));
    }

    #[test]
    fn jittered_grid_is_watertight() {
        // 30x30 cells of ~1.7 px, shared jittered vertices, fixed boundary:
        // every pixel center inside the rectangle must be covered at least once.
        let n = 30usize;
        let cell = 1.7f32;
        let mut seed = 99u32;
        let mut rnd = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as f32 / (1 << 24) as f32 - 0.5
        };
        let mut grid = vec![[0.0f32; 2]; (n + 1) * (n + 1)];
        for j in 0..=n {
            for i in 0..=n {
                let boundary = i == 0 || j == 0 || i == n || j == n;
                let (jx, jy) = if boundary {
                    (0.0, 0.0)
                } else {
                    (rnd() * 1.0, rnd() * 1.0)
                };
                grid[j * (n + 1) + i] = [4.0 + i as f32 * cell + jx, 4.0 + j as f32 * cell + jy];
            }
        }
        let mut tris = Vec::new();
        for j in 0..n {
            for i in 0..n {
                let p = |a: usize, b: usize| grid[(j + b) * (n + 1) + i + a];
                tris.push(tri([p(0, 0), p(1, 0), p(0, 1)], 0.5, tris.len() as u32));
                tris.push(tri([p(1, 0), p(1, 1), p(0, 1)], 0.5, tris.len() as u32));
            }
        }
        let r = raster(&tris, 64, 64, None, true);
        assert_eq!(r.float_tris, 0);
        let (lo, hi) = (4.0f32, 4.0 + n as f32 * cell);
        let mut checked = 0;
        for y in 0..64 {
            for x in 0..64 {
                let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
                if cx > lo && cx < hi && cy > lo && cy < hi {
                    checked += 1;
                    assert_ne!(r.keys[y * 64 + x], 0, "hole at ({x},{y})");
                }
            }
        }
        assert!(checked > 1000);
    }

    #[test]
    fn large_triangle_uses_float_path_and_covers_expected_area() {
        let big = tri([[2.0, 2.0], [102.0, 2.0], [2.0, 102.0]], 0.5, 1);
        let r = raster(&[big], 128, 128, None, true);
        assert_eq!(r.float_tris, 1);
        let covered = r.keys.iter().filter(|&&k| k != 0).count() as f32;
        let area = 100.0 * 100.0 / 2.0;
        assert!((covered - area).abs() < 150.0, "{covered} vs {area}");
    }

    #[test]
    fn clipped_vertices_are_skipped_and_counted() {
        let mut t = tri([[1.0, 1.0], [6.0, 1.0], [1.0, 6.0]], 0.5, 1);
        t.v[1] = to_screen([0.0, 0.0, 0.5, -1.0], [8.0, 8.0]);
        let r = raster(&[t], 8, 8, None, true);
        assert_eq!(r.clipped_tris, 1);
        assert!(r.keys.iter().all(|&k| k == 0));
    }

    #[test]
    fn tile_counters_count_overlap_and_hits() {
        // One triangle spanning two tiles horizontally.
        let t = tri([[10.0, 2.0], [22.0, 2.0], [10.0, 14.0]], 0.5, 1);
        let r = raster(&[t], 32, 16, None, true);
        assert_eq!(r.tile_counters[0], 1); // tile 0 tris
        assert_eq!(r.tile_counters[2], 1); // tile 1 tris
        let hits: u32 = r.tile_counters.iter().skip(1).step_by(2).sum();
        assert_eq!(hits as usize, r.keys.iter().filter(|&&k| k != 0).count());
    }

    #[test]
    fn classify_uses_thresholds_and_hysteresis() {
        // tile 0: below, tile 1: above tri thresh, tile 2: above hit thresh,
        // tile 3: below threshold but above hysteresis band.
        let counters = [10, 10, 100, 0, 0, 5000, 70, 0];
        let prev = [0, 0, 0, 1];
        let d = classify(&counters, &prev, 100, 1000, 0.5);
        assert_eq!(d, vec![0, 1, 1, 1]);
        let prev = [0, 0, 0, 0];
        let d = classify(&counters, &prev, 100, 1000, 0.5);
        assert_eq!(d, vec![0, 1, 1, 0]);
    }
}
