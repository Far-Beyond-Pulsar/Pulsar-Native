//! CPU reference for the 3-NonRT kernels (`shaders/peel.wgsl`, `bin.wgsl`).
//!
//! The arithmetic mirrors the shader operation for operation so the GPU test
//! can compare against it, and so unit tests can check the algorithm (bucket
//! partition, slab peel, early exit, clamp) against a brute-force z-sort.

/// Largest `1 - b1/NB` slack used by the "pixel is final" test.
pub const BOUND_EPS: f32 = 1.0e-6;
/// Edge-function slack (see `EDGE_EPS` in `common.wgsl`).
pub const EDGE_EPS: f32 = 1.0e-7;

/// A screen-space vertex: pixel coordinates (y down), reversed-Z NDC depth.
/// `valid == false` marks a vertex dropped by the near-plane test.
#[derive(Clone, Copy, Debug)]
pub struct RefVertex {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub valid: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct RefTri {
    pub v: [RefVertex; 3],
    /// Vis id this triangle writes (`vis_pack(visible_cluster, tri)`).
    pub id: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct PeelParams {
    pub tile: u32,
    pub bucket_bits: u32,
    pub slab_tris: u32,
    pub nominal_layers: u32,
    pub max_layers: u32,
}

impl From<&crate::DenseConfig> for PeelParams {
    fn from(c: &crate::DenseConfig) -> Self {
        Self {
            tile: c.tile,
            bucket_bits: c.bucket_bits,
            slab_tris: c.slab_tris,
            nominal_layers: c.nominal_layers,
            max_layers: c.max_layers,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TileResult {
    /// `tile * tile` depths, row-major; 0.0 = no fragment.
    pub z: Vec<f32>,
    pub id: Vec<u32>,
    pub layers: u32,
    pub clamped: bool,
    pub clamped_tris: u32,
}

/// Bucket of a triangle's nearest depth. Bucket 0 is the nearest.
pub fn bucket_of(zmax: f32, bucket_bits: u32) -> u32 {
    let nb = 1u32 << bucket_bits;
    let q = (1.0 - zmax.clamp(0.0, 1.0)) * nb as f32;
    (q as u32).min(nb - 1)
}

#[derive(Clone, Copy, Debug)]
pub struct Setup {
    /// Edge rows (A, B, C, depth-plane coefficient).
    pub e: [[f32; 4]; 3],
    /// Inclusive tile-local pixel box x0, y0, x1, y1.
    pub bbox: [u32; 4],
    pub bucket: u32,
}

/// Triangle setup relative to the tile origin; `None` when culled.
pub fn setup_tri(t: &RefTri, origin: [f32; 2], p: &PeelParams) -> Option<Setup> {
    let [v0, v1, v2] = t.v;
    if !(v0.valid && v1.valid && v2.valid) {
        return None;
    }
    let zmax = v0.z.max(v1.z.max(v2.z));
    if zmax <= 0.0 {
        return None;
    }
    let (ax, ay) = (v0.x - origin[0], v0.y - origin[1]);
    let (bx, by) = (v1.x - origin[0], v1.y - origin[1]);
    let (cx, cy) = (v2.x - origin[0], v2.y - origin[1]);
    let area2 = (bx - ax) * (cy - ay) - (cx - ax) * (by - ay);
    if !(area2.abs() > 1.0e-12) {
        return None;
    }
    let mnx = ax.min(bx.min(cx));
    let mny = ay.min(by.min(cy));
    let mxx = ax.max(bx.max(cx));
    let mxy = ay.max(by.max(cy));
    let lox = (mnx - 0.5).ceil().max(0.0);
    let loy = (mny - 0.5).ceil().max(0.0);
    let hix = (mxx - 0.5).floor().min((p.tile - 1) as f32);
    let hiy = (mxy - 0.5).floor().min((p.tile - 1) as f32);
    if lox > hix || loy > hiy {
        return None;
    }
    let inv = 1.0 / area2;
    let a0 = -(cy - by) * inv;
    let b0 = (cx - bx) * inv;
    let c0 = ((cy - by) * bx - (cx - bx) * by) * inv;
    let a1 = -(ay - cy) * inv;
    let b1 = (ax - cx) * inv;
    let c1 = ((ay - cy) * cx - (ax - cx) * cy) * inv;
    let a2 = -(by - ay) * inv;
    let b2 = (bx - ax) * inv;
    let c2 = ((by - ay) * ax - (bx - ax) * ay) * inv;
    Some(Setup {
        e: [
            [a0, b0, c0, a0 * v0.z + a1 * v1.z + a2 * v2.z],
            [a1, b1, c1, b0 * v0.z + b1 * v1.z + b2 * v2.z],
            [a2, b2, c2, c0 * v0.z + c1 * v1.z + c2 * v2.z],
        ],
        bbox: [lox as u32, loy as u32, hix as u32, hiy as u32],
        bucket: bucket_of(zmax, p.bucket_bits),
    })
}

/// Fragment depth if pixel `(x, y)` (tile-local) is covered by `s`.
fn fragment(s: &Setup, x: u32, y: u32) -> Option<f32> {
    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
    let w0 = s.e[0][0] * px + s.e[0][1] * py + s.e[0][2];
    let w1 = s.e[1][0] * px + s.e[1][1] * py + s.e[1][2];
    let w2 = s.e[2][0] * px + s.e[2][1] * py + s.e[2][2];
    if w0 >= -EDGE_EPS && w1 >= -EDGE_EPS && w2 >= -EDGE_EPS {
        let z = s.e[0][3] * px + s.e[1][3] * py + s.e[2][3];
        if z > 0.0 {
            return Some(z);
        }
    }
    None
}

fn better(z: f32, id: u32, best_z: f32, best_id: u32) -> bool {
    z > best_z || (z == best_z && id > best_id)
}

/// The kernel's algorithm: histogram, slab peel with per-pixel finality and
/// tile early-out, clamp at `max_layers`.
pub fn peel_tile(tris: &[RefTri], origin: [f32; 2], p: &PeelParams) -> TileResult {
    let n = (p.tile * p.tile) as usize;
    let nb = 1usize << p.bucket_bits;
    let setups: Vec<(Setup, u32)> = tris
        .iter()
        .filter_map(|t| setup_tri(t, origin, p).map(|s| (s, t.id)))
        .collect();
    let mut hist = vec![0u32; nb];
    for (s, _) in &setups {
        hist[s.bucket as usize] += 1;
    }
    let total = setups.len() as u32;
    let mut z = vec![0.0f32; n];
    let mut id = vec![0u32; n];
    let (mut layers, mut clamped, mut clamped_tris) = (0, false, 0);
    let (mut b0, mut consumed) = (0usize, 0u32);
    while total != 0 {
        let (mut acc, mut b) = (0u32, b0);
        while b < nb {
            acc += hist[b];
            b += 1;
            if acc >= p.slab_tris {
                break;
            }
        }
        let b1 = b;
        consumed += acc;
        let remaining = total - consumed;
        for (s, tid) in &setups {
            if (s.bucket as usize) < b0 || (s.bucket as usize) >= b1 {
                continue;
            }
            for y in s.bbox[1]..=s.bbox[3] {
                for x in s.bbox[0]..=s.bbox[2] {
                    if let Some(f) = fragment(s, x, y) {
                        let i = (y * p.tile + x) as usize;
                        if better(f, *tid, z[i], id[i]) {
                            z[i] = f;
                            id[i] = *tid;
                        }
                    }
                }
            }
        }
        layers += 1;
        let bound = 1.0 - b1 as f32 / nb as f32;
        let unresolved = z
            .iter()
            .filter(|&&zz| !(remaining == 0 || zz > bound + BOUND_EPS))
            .count();
        b0 = b1;
        if remaining == 0 || unresolved == 0 {
            break;
        }
        if layers >= p.max_layers {
            clamped = true;
            clamped_tris = remaining;
            break;
        }
    }
    TileResult {
        z,
        id,
        layers,
        clamped,
        clamped_tris,
    }
}

/// Plain z-sort-equivalent: every triangle against every pixel, nearest wins
/// (ties to the larger id). Same coverage/depth arithmetic as the peel, so a
/// non-clamped peel must equal it bit for bit.
pub fn brute_force_tile(tris: &[RefTri], origin: [f32; 2], p: &PeelParams) -> TileResult {
    let n = (p.tile * p.tile) as usize;
    let mut z = vec![0.0f32; n];
    let mut id = vec![0u32; n];
    for t in tris {
        let Some(s) = setup_tri(t, origin, p) else {
            continue;
        };
        for y in s.bbox[1]..=s.bbox[3] {
            for x in s.bbox[0]..=s.bbox[2] {
                if let Some(f) = fragment(&s, x, y) {
                    let i = (y * p.tile + x) as usize;
                    if better(f, t.id, z[i], id[i]) {
                        z[i] = f;
                        id[i] = t.id;
                    }
                }
            }
        }
    }
    TileResult {
        z,
        id,
        layers: 0,
        clamped: false,
        clamped_tris: 0,
    }
}

/// Independent f64 evaluation (textbook barycentrics, no setup shortcuts) used
/// to sanity-check the f32 pipeline away from edges.
pub fn reference_f64(tris: &[RefTri], origin: [f32; 2], tile: u32) -> (Vec<f64>, Vec<u32>) {
    let n = (tile * tile) as usize;
    let mut z = vec![0.0f64; n];
    let mut id = vec![0u32; n];
    for t in tris {
        if !t.v.iter().all(|v| v.valid) {
            continue;
        }
        let p = |i: usize| {
            (
                t.v[i].x as f64 - origin[0] as f64,
                t.v[i].y as f64 - origin[1] as f64,
                t.v[i].z as f64,
            )
        };
        let (a, b, c) = (p(0), p(1), p(2));
        let area = (b.0 - a.0) * (c.1 - a.1) - (c.0 - a.0) * (b.1 - a.1);
        if area.abs() < 1e-12 {
            continue;
        }
        for y in 0..tile {
            for x in 0..tile {
                let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                let w0 = ((b.0 - px) * (c.1 - py) - (c.0 - px) * (b.1 - py)) / area;
                let w1 = ((c.0 - px) * (a.1 - py) - (a.0 - px) * (c.1 - py)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0 {
                    let zz = w0 * a.2 + w1 * b.2 + w2 * c.2;
                    let i = (y * tile + x) as usize;
                    if zz > 0.0 && (zz > z[i] || (zz == z[i] && t.id > id[i])) {
                        z[i] = zz;
                        id[i] = t.id;
                    }
                }
            }
        }
    }
    (z, id)
}

/// CPU mirror of `bin.wgsl`'s conservative sphere-to-tile bounds. `m` is the
/// column-major view-projection (`m[col][row]`), `center` is a world position.
/// Returns inclusive `(tx0, ty0, tx1, ty1)` or `None` when culled.
pub fn sphere_tile_bounds(
    m: &[[f32; 4]; 4],
    center: [f32; 3],
    radius: f32,
    viewport: [u32; 2],
    tile: u32,
) -> Option<[u32; 4]> {
    let row = |r: usize| m[0][r] * center[0] + m[1][r] * center[1] + m[2][r] * center[2] + m[3][r];
    let len = |r: usize| (m[0][r] * m[0][r] + m[1][r] * m[1][r] + m[2][r] * m[2][r]).sqrt();
    let (cx, cy, cw) = (row(0), row(1), row(3));
    let (lx, ly, lw) = (len(0), len(1), len(3));
    let wmin = cw - radius * lw;
    let wmax = cw + radius * lw;
    if wmax <= 0.0 {
        return None;
    }
    let tiles_x = viewport[0].div_ceil(tile);
    let tiles_y = viewport[1].div_ceil(tile);
    if wmin <= 1.0e-4 {
        return Some([0, 0, tiles_x - 1, tiles_y - 1]);
    }
    let (xlo, xhi) = (cx - radius * lx, cx + radius * lx);
    let (ylo, yhi) = (cy - radius * ly, cy + radius * ly);
    let nx0 = (xlo / wmin).min(xlo / wmax);
    let nx1 = (xhi / wmin).max(xhi / wmax);
    let ny0 = (ylo / wmin).min(ylo / wmax);
    let ny1 = (yhi / wmin).max(yhi / wmax);
    let (fw, fh) = (viewport[0] as f32, viewport[1] as f32);
    let px0 = (nx0 * 0.5 + 0.5) * fw;
    let px1 = (nx1 * 0.5 + 0.5) * fw;
    let py0 = (0.5 - ny1 * 0.5) * fh;
    let py1 = (0.5 - ny0 * 0.5) * fh;
    if px1 < 0.0 || px0 >= fw || py1 < 0.0 || py0 >= fh {
        return None;
    }
    let ft = tile as f32;
    let t = |v: f32, hi: u32| (v / ft).floor().clamp(0.0, (hi - 1) as f32) as u32;
    Some([
        t(px0, tiles_x),
        t(py0, tiles_y),
        t(px1, tiles_x),
        t(py1, tiles_y),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 32) as u32
        }
        fn f(&mut self, lo: f32, hi: f32) -> f32 {
            lo + (self.next() as f32 / u32::MAX as f32) * (hi - lo)
        }
    }

    fn random_tris(
        rng: &mut Rng,
        n: usize,
        tile: f32,
        z_lo: f32,
        z_hi: f32,
        size: f32,
    ) -> Vec<RefTri> {
        (0..n)
            .map(|i| {
                let (cx, cy) = (rng.f(-2.0, tile + 2.0), rng.f(-2.0, tile + 2.0));
                let mut v = [RefVertex {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                    valid: true,
                }; 3];
                for vv in &mut v {
                    vv.x = cx + rng.f(-size, size);
                    vv.y = cy + rng.f(-size, size);
                    vv.z = rng.f(z_lo, z_hi);
                }
                RefTri {
                    v,
                    id: i as u32 + 1,
                }
            })
            .collect()
    }

    fn cfg(tile: u32, bits: u32, slab: u32, nominal: u32, max: u32) -> PeelParams {
        PeelParams {
            tile,
            bucket_bits: bits,
            slab_tris: slab,
            nominal_layers: nominal,
            max_layers: max,
        }
    }

    /// Without a layer cap the peel equals the brute-force z-sort exactly, for
    /// every slab size and bucket resolution, including heavy bucket
    /// collisions (narrow z range) and a 1-triangle slab.
    #[test]
    fn peel_equals_brute_force_z_sort() {
        let mut rng = Rng(0x9E3779B97F4A7C15);
        for &(bits, slab, z_lo, z_hi) in &[
            (12, 1024, 0.05, 0.95),
            (12, 8, 0.05, 0.95),
            (12, 1, 0.05, 0.95),
            (6, 16, 0.0, 1.0),
            (12, 8, 0.50, 0.5005), // everything lands in a couple of buckets
            (2, 4, 0.01, 0.99),
        ] {
            for &tile in &[16u32, 8] {
                for trial in 0..8 {
                    let n = [1, 5, 40, 200, 600][trial % 5];
                    let tris = random_tris(&mut rng, n, tile as f32, z_lo, z_hi, 9.0);
                    let p = cfg(tile, bits, slab, 4, u32::MAX);
                    let peel = peel_tile(&tris, [0.0, 0.0], &p);
                    let brute = brute_force_tile(&tris, [0.0, 0.0], &p);
                    assert!(!peel.clamped);
                    assert_eq!(
                        peel.id, brute.id,
                        "bits={bits} slab={slab} tile={tile} n={n}"
                    );
                    assert_eq!(peel.z, brute.z);
                }
            }
        }
    }

    /// Slanted triangles whose far corners are farther than a later bucket's
    /// nearest point must not be finalized early (the finality bound).
    #[test]
    fn finality_bound_handles_slanted_overlap() {
        let big = RefTri {
            // Nearest vertex z=0.9 (bucket near), far vertices z=0.1; covers the tile.
            v: [
                RefVertex {
                    x: -50.0,
                    y: -50.0,
                    z: 0.9,
                    valid: true,
                },
                RefVertex {
                    x: 90.0,
                    y: -50.0,
                    z: 0.1,
                    valid: true,
                },
                RefVertex {
                    x: -50.0,
                    y: 90.0,
                    z: 0.1,
                    valid: true,
                },
            ],
            id: 1,
        };
        let small = RefTri {
            // Flat at z=0.5: in a later bucket than `big`, but in front of it away from the near corner.
            v: [
                RefVertex {
                    x: 4.0,
                    y: 4.0,
                    z: 0.5,
                    valid: true,
                },
                RefVertex {
                    x: 12.0,
                    y: 4.0,
                    z: 0.5,
                    valid: true,
                },
                RefVertex {
                    x: 4.0,
                    y: 12.0,
                    z: 0.5,
                    valid: true,
                },
            ],
            id: 2,
        };
        let p = cfg(16, 12, 1, 4, 64);
        let peel = peel_tile(&[big, small], [0.0, 0.0], &p);
        let brute = brute_force_tile(&[big, small], [0.0, 0.0], &p);
        assert_eq!(peel.id, brute.id);
        assert!(peel.layers >= 2, "must peel past the first slab");
    }

    /// Tiles whose nearest layer already covers every pixel stop after one
    /// layer and never touch the far buckets.
    #[test]
    fn early_out_when_all_pixels_final() {
        let mut tris = Vec::new();
        // Opaque near cover (two triangles), then lots of far clutter.
        let q = |x0: f32, y0: f32, x1: f32, y1: f32, x2: f32, y2: f32, z: f32, id| RefTri {
            v: [
                RefVertex {
                    x: x0,
                    y: y0,
                    z,
                    valid: true,
                },
                RefVertex {
                    x: x1,
                    y: y1,
                    z,
                    valid: true,
                },
                RefVertex {
                    x: x2,
                    y: y2,
                    z,
                    valid: true,
                },
            ],
            id,
        };
        tris.push(q(-1.0, -1.0, 30.0, -1.0, -1.0, 30.0, 0.9, 1000));
        tris.push(q(30.0, -1.0, 30.0, 30.0, -1.0, 30.0, 0.9, 1001));
        let mut rng = Rng(7);
        tris.extend(random_tris(&mut rng, 300, 16.0, 0.05, 0.4, 6.0));
        let p = cfg(16, 12, 2, 4, 64);
        let peel = peel_tile(&tris, [0.0, 0.0], &p);
        assert_eq!(peel.layers, 1);
        assert!(peel.id.iter().all(|&i| i == 1000 || i == 1001));
    }

    /// Layer cap: results are the best of the processed slabs (never farther
    /// than brute force's nearest would be... never nearer), and the clamp is
    /// reported. Uncapped it is exact.
    #[test]
    fn clamp_is_reported_and_partial() {
        let mut rng = Rng(42);
        // Slanted overlapping layers so many pixels stay unresolved.
        let tris = random_tris(&mut rng, 400, 16.0, 0.02, 0.98, 14.0);
        let capped = peel_tile(&tris, [0.0, 0.0], &cfg(16, 12, 2, 2, 3));
        let exact = peel_tile(&tris, [0.0, 0.0], &cfg(16, 12, 2, 2, u32::MAX));
        assert!(capped.clamped && capped.clamped_tris > 0 && capped.layers == 3);
        assert!(!exact.clamped && exact.layers > 3);
        for (c, e) in capped.z.iter().zip(&exact.z) {
            assert!(c <= e, "a clamped peel can only miss nearer fragments");
        }
    }

    /// f32 pipeline vs independent f64 barycentrics: only edge-grazing pixels
    /// may disagree.
    #[test]
    fn matches_independent_f64_reference() {
        let mut rng = Rng(0xDEADBEEF);
        let (mut mismatch, mut total) = (0usize, 0usize);
        for _ in 0..20 {
            let tris = random_tris(&mut rng, 80, 16.0, 0.05, 0.95, 8.0);
            let p = cfg(16, 12, 16, 4, u32::MAX);
            let peel = peel_tile(&tris, [0.0, 0.0], &p);
            let (rz, rid) = reference_f64(&tris, [0.0, 0.0], 16);
            for i in 0..256 {
                total += 1;
                if peel.id[i] != rid[i] && (peel.z[i] as f64 - rz[i]).abs() > 1e-4 {
                    mismatch += 1;
                }
            }
        }
        assert!(
            mismatch * 100 <= total,
            "{mismatch}/{total} pixels disagree"
        );
    }

    #[test]
    fn near_plane_dropped_vertices_cull_the_triangle() {
        let mut t = random_tris(&mut Rng(3), 1, 16.0, 0.2, 0.8, 20.0)[0];
        t.v[1].valid = false;
        let p = cfg(16, 12, 8, 4, 8);
        assert!(peel_tile(&[t], [0.0, 0.0], &p).id.iter().all(|&i| i == 0));
    }

    /// Every point of the sphere that lands on screen is inside the returned
    /// tile range, including spheres that straddle the near plane.
    #[test]
    fn sphere_bounds_are_conservative() {
        // Perspective reversed-Z, column-major, looking down -Z from the origin.
        let (f, n, aspect) = (1.0 / (0.5f32).tan(), 0.1f32, 16.0 / 9.0);
        let m = [
            [f / aspect, 0.0, 0.0, 0.0],
            [0.0, f, 0.0, 0.0],
            [0.0, 0.0, 0.0, -1.0],
            [0.0, 0.0, n, 0.0],
        ];
        let vp = [1920u32, 1080];
        let tile = 16;
        let mut rng = Rng(99);
        for _ in 0..300 {
            let c = [rng.f(-8.0, 8.0), rng.f(-5.0, 5.0), rng.f(-30.0, 1.0)];
            let r = rng.f(0.05, 4.0);
            let b = sphere_tile_bounds(&m, c, r, vp, tile);
            for _ in 0..40 {
                let d = [rng.f(-1.0, 1.0), rng.f(-1.0, 1.0), rng.f(-1.0, 1.0)];
                let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-3);
                let k = r * rng.f(0.0, 1.0) / l;
                let p = [c[0] + d[0] * k, c[1] + d[1] * k, c[2] + d[2] * k];
                let clip =
                    |row: usize| m[0][row] * p[0] + m[1][row] * p[1] + m[2][row] * p[2] + m[3][row];
                let w = clip(3);
                if w <= 1e-4 {
                    continue;
                }
                let (sx, sy) = (
                    (clip(0) / w * 0.5 + 0.5) * 1920.0,
                    (0.5 - clip(1) / w * 0.5) * 1080.0,
                );
                if !(0.0..1920.0).contains(&sx) || !(0.0..1080.0).contains(&sy) {
                    continue;
                }
                let b = b.expect("an on-screen sphere point implies bounds");
                let (tx, ty) = ((sx / 16.0) as u32, (sy / 16.0) as u32);
                assert!(
                    tx >= b[0] && tx <= b[2] && ty >= b[1] && ty <= b[3],
                    "{tx},{ty} outside {b:?}"
                );
            }
        }
    }
}
