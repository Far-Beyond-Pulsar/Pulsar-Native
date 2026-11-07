//! True geometric-deviation measurement for DAG levels (test support).
//!
//! Points sampled on one triangle soup are measured against another with a
//! multi-resolution uniform grid, giving a Hausdorff-style deviation in both
//! directions. Used to validate that stored cluster errors bound the real
//! object-space deviation from the original mesh and to report how tight the
//! bound is.
#![allow(dead_code)]

use std::collections::HashMap;

use rayon::prelude::*;

pub type V3 = [f32; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn madd(a: V3, b: V3, s: f32) -> V3 {
    [a[0] + b[0] * s, a[1] + b[1] * s, a[2] + b[2] * s]
}

/// Squared distance from `p` to triangle `abc` (Ericson, Real-Time Collision Detection).
pub fn point_tri_dist2(p: V3, a: V3, b: V3, c: V3) -> f32 {
    let (ab, ac, ap) = (sub(b, a), sub(c, a), sub(p, a));
    let (d1, d2) = (dot(ab, ap), dot(ac, ap));
    let q = |q: V3| {
        let d = sub(p, q);
        dot(d, d)
    };
    if d1 <= 0.0 && d2 <= 0.0 {
        return q(a);
    }
    let bp = sub(p, b);
    let (d3, d4) = (dot(ab, bp), dot(ac, bp));
    if d3 >= 0.0 && d4 <= d3 {
        return q(b);
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return q(madd(a, ab, v));
    }
    let cp = sub(p, c);
    let (d5, d6) = (dot(ab, cp), dot(ac, cp));
    if d6 >= 0.0 && d5 <= d6 {
        return q(c);
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return q(madd(a, ac, w));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return q(madd(b, sub(c, b), w));
    }
    let denom = 1.0 / (va + vb + vc);
    let (v, w) = (vb * denom, vc * denom);
    q(madd(madd(a, ab, v), ac, w))
}

struct Grid {
    cell: f32,
    origin: V3,
    map: HashMap<[i32; 3], Vec<u32>>,
}

impl Grid {
    fn key(&self, p: V3) -> [i32; 3] {
        std::array::from_fn(|k| ((p[k] - self.origin[k]) / self.cell).floor() as i32)
    }
}

/// A triangle soup with cached geometry and a 3-level grid for nearest queries.
pub struct Soup {
    pub tris: Vec<[V3; 3]>,
    grids: Vec<Grid>,
}

impl Soup {
    pub fn new(tris: Vec<[V3; 3]>) -> Self {
        let mut lo = [f32::INFINITY; 3];
        for t in &tris {
            for v in t {
                for k in 0..3 {
                    lo[k] = lo[k].min(v[k]);
                }
            }
        }
        let area: f64 = tris
            .iter()
            .map(|t| {
                let (e1, e2) = (sub(t[1], t[0]), sub(t[2], t[0]));
                let n = [
                    e1[1] * e2[2] - e1[2] * e2[1],
                    e1[2] * e2[0] - e1[0] * e2[2],
                    e1[0] * e2[1] - e1[1] * e2[0],
                ];
                0.5 * (dot(n, n) as f64).sqrt()
            })
            .sum();
        let base = ((8.0 * area / tris.len().max(1) as f64).sqrt() as f32).max(1e-6);
        let grids = [1.0f32, 8.0, 64.0]
            .iter()
            .map(|m| {
                let mut g = Grid {
                    cell: base * m,
                    origin: lo,
                    map: HashMap::new(),
                };
                for (i, t) in tris.iter().enumerate() {
                    let mut a = [f32::INFINITY; 3];
                    let mut b = [f32::NEG_INFINITY; 3];
                    for v in t {
                        for k in 0..3 {
                            a[k] = a[k].min(v[k]);
                            b[k] = b[k].max(v[k]);
                        }
                    }
                    let (ka, kb) = (g.key(a), g.key(b));
                    for x in ka[0]..=kb[0] {
                        for y in ka[1]..=kb[1] {
                            for z in ka[2]..=kb[2] {
                                g.map.entry([x, y, z]).or_default().push(i as u32);
                            }
                        }
                    }
                }
                g
            })
            .collect();
        Self { tris, grids }
    }

    /// Smallest `distance / error(owner)` over triangles within `rad` of `p`:
    /// <= 1 means some cluster's stored error actually covers this point.
    pub fn covered_ratio(&self, p: V3, owner: &[u32], errs: &[f32], rad: f32) -> f32 {
        let g = self
            .grids
            .iter()
            .find(|g| g.cell * 3.0 >= rad)
            .unwrap_or_else(|| self.grids.last().unwrap());
        let reach = (rad / g.cell).ceil() as i32 + 1;
        let c = g.key(p);
        let mut best = f32::INFINITY;
        for x in -reach..=reach {
            for y in -reach..=reach {
                for z in -reach..=reach {
                    if let Some(list) = g.map.get(&[c[0] + x, c[1] + y, c[2] + z]) {
                        for &i in list {
                            let t = &self.tris[i as usize];
                            let d = point_tri_dist2(p, t[0], t[1], t[2]).sqrt();
                            best = best.min(d / errs[owner[i as usize] as usize].max(1e-12));
                        }
                    }
                }
            }
        }
        best
    }

    /// (distance, triangle index) of the nearest triangle.
    pub fn nearest(&self, p: V3) -> (f32, u32) {
        let mut best = (f32::INFINITY, u32::MAX);
        for (gi, g) in self.grids.iter().enumerate() {
            let last = gi + 1 == self.grids.len();
            let max_r: i32 = if last { 1 << 20 } else { 4 };
            let c = g.key(p);
            for r in 0..=max_r {
                if r > 0 && best.0 <= (r - 1) as f32 * g.cell {
                    return best;
                }
                for x in -r..=r {
                    for y in -r..=r {
                        for z in -r..=r {
                            if x.abs().max(y.abs()).max(z.abs()) != r {
                                continue;
                            }
                            if let Some(list) = g.map.get(&[c[0] + x, c[1] + y, c[2] + z]) {
                                for &i in list {
                                    let t = &self.tris[i as usize];
                                    let d = point_tri_dist2(p, t[0], t[1], t[2]).sqrt();
                                    if d < best.0 {
                                        best = (d, i);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if best.0.is_finite() && best.0 <= max_r as f32 * g.cell {
                return best;
            }
        }
        best
    }
}

/// Vertices, edge midpoints and centroid of a triangle (7 samples).
pub fn tri_samples(t: &[V3; 3]) -> [V3; 7] {
    let mid = |a: V3, b: V3| {
        [
            (a[0] + b[0]) * 0.5,
            (a[1] + b[1]) * 0.5,
            (a[2] + b[2]) * 0.5,
        ]
    };
    [
        t[0],
        t[1],
        t[2],
        mid(t[0], t[1]),
        mid(t[1], t[2]),
        mid(t[2], t[0]),
        [
            (t[0][0] + t[1][0] + t[2][0]) / 3.0,
            (t[0][1] + t[1][1] + t[2][1]) / 3.0,
            (t[0][2] + t[1][2] + t[2][2]) / 3.0,
        ],
    ]
}

pub struct LevelDeviation {
    /// Per cluster (index into the level's cluster list): max deviation
    /// attributed to it (forward samples of its own triangles, plus original
    /// samples whose nearest simplified triangle belongs to it).
    pub per_cluster: Vec<f32>,
    /// All sampled deviations (both directions), unsorted.
    pub samples: Vec<f32>,
    pub max: f32,
    pub p99: f32,
    /// max over samples of the covering ratio (<= 1 when conservative).
    pub worst_ratio: f32,
}

/// Measure deviation between `original` and the simplified surface made of
/// `clusters` (each a list of triangles). At most `max_points` samples are
/// used from the original.
pub fn measure_level(
    original: &Soup,
    clusters: &[Vec<[V3; 3]>],
    errs: &[f32],
    max_points: usize,
) -> LevelDeviation {
    let mut level_tris = Vec::new();
    let mut owner = Vec::new();
    for (ci, c) in clusters.iter().enumerate() {
        for t in c {
            level_tris.push(*t);
            owner.push(ci as u32);
        }
    }
    let level = Soup::new(level_tris);
    // simplified -> original
    let fwd: Vec<(u32, f32)> = level
        .tris
        .par_iter()
        .enumerate()
        .map(|(i, t)| {
            let d = tri_samples(t)
                .iter()
                .map(|&p| original.nearest(p).0)
                .fold(0.0f32, f32::max);
            (owner[i], d)
        })
        .collect();
    // original -> simplified
    let rad = errs.iter().cloned().fold(0.0f32, f32::max) * 1.5;
    let stride = (original.tris.len() * 4 / max_points.max(1)).max(1);
    let bwd: Vec<(u32, f32, f32)> = original
        .tris
        .par_iter()
        .enumerate()
        .filter(|(i, _)| i % stride == 0)
        .map(|(_, t)| {
            let mut worst = (0u32, 0.0f32, 0.0f32);
            for p in tri_samples(t).iter() {
                let (d, ti) = level.nearest(*p);
                let cov = level.covered_ratio(*p, &owner, errs, rad);
                if d >= worst.1 {
                    worst = (owner[ti as usize], d, worst.2);
                }
                worst.2 = worst.2.max(cov);
            }
            worst
        })
        .collect();
    let mut per = vec![0.0f32; clusters.len()];
    let mut worst_ratio = 0.0f32;
    for &(c, d) in &fwd {
        worst_ratio = worst_ratio.max(d / errs[c as usize].max(1e-12));
    }
    let mut samples = Vec::with_capacity(fwd.len() + bwd.len());
    for (c, d) in fwd.into_iter().chain(bwd.into_iter().map(|(c, d, r)| {
        worst_ratio = worst_ratio.max(r);
        (c, d)
    })) {
        per[c as usize] = per[c as usize].max(d);
        samples.push(d);
    }
    let mut sorted = samples.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let max = sorted.last().copied().unwrap_or(0.0);
    let p99 = sorted[((sorted.len() as f32 * 0.99) as usize).min(sorted.len().saturating_sub(1))];
    LevelDeviation {
        per_cluster: per,
        samples,
        max,
        p99,
        worst_ratio,
    }
}

/// Deterministic noisy sphere (bumpy "scan-like" detail) with
/// `20 * 4^subdiv` triangles and radius ~1.
pub fn noisy_sphere(subdiv: u32) -> (Vec<V3>, Vec<u32>) {
    let t = (1.0 + 5.0f32.sqrt()) / 2.0;
    let mut p: Vec<V3> = vec![
        [-1.0, t, 0.0],
        [1.0, t, 0.0],
        [-1.0, -t, 0.0],
        [1.0, -t, 0.0],
        [0.0, -1.0, t],
        [0.0, 1.0, t],
        [0.0, -1.0, -t],
        [0.0, 1.0, -t],
        [t, 0.0, -1.0],
        [t, 0.0, 1.0],
        [-t, 0.0, -1.0],
        [-t, 0.0, 1.0],
    ];
    for v in p.iter_mut() {
        let l = dot(*v, *v).sqrt();
        *v = [v[0] / l, v[1] / l, v[2] / l];
    }
    let mut idx: Vec<[u32; 3]> = vec![
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    for _ in 0..subdiv {
        let mut cache: HashMap<(u32, u32), u32> = HashMap::new();
        let mut mid = |a: u32, b: u32, p: &mut Vec<V3>| -> u32 {
            *cache.entry((a.min(b), a.max(b))).or_insert_with(|| {
                let (pa, pb) = (p[a as usize], p[b as usize]);
                let m = [
                    (pa[0] + pb[0]) * 0.5,
                    (pa[1] + pb[1]) * 0.5,
                    (pa[2] + pb[2]) * 0.5,
                ];
                let l = dot(m, m).sqrt();
                p.push([m[0] / l, m[1] / l, m[2] / l]);
                (p.len() - 1) as u32
            })
        };
        let mut next = Vec::new();
        for t in &idx {
            let (a, b, c) = (
                mid(t[0], t[1], &mut p),
                mid(t[1], t[2], &mut p),
                mid(t[2], t[0], &mut p),
            );
            next.extend([[t[0], a, c], [t[1], b, a], [t[2], c, b], [a, b, c]]);
        }
        idx = next;
    }
    // Scan-like displacement: smooth lobes at several scales + fine hash noise.
    for v in p.iter_mut() {
        let d = *v;
        let mut h = 0.0f32;
        for (k, amp) in [
            (2.0f32, 0.06f32),
            (7.0, 0.02),
            (23.0, 0.006),
            (71.0, 0.0015),
        ] {
            h += amp
                * ((d[0] * k + 0.3).sin()
                    * (d[1] * k * 1.3 + 1.1).sin()
                    * (d[2] * k * 0.9 + 2.0).cos());
        }
        let bits = (d[0].to_bits().wrapping_mul(0x9E37_79B1)
            ^ d[1].to_bits().wrapping_mul(0x85EB_CA6B)
            ^ d[2].to_bits().wrapping_mul(0xC2B2_AE35))
        .wrapping_mul(0x27D4_EB2F);
        h += 0.0004 * ((bits >> 8) as f32 / (1 << 24) as f32 - 0.5);
        *v = [d[0] * (1.0 + h), d[1] * (1.0 + h), d[2] * (1.0 + h)];
    }
    (p, idx.into_iter().flatten().collect())
}

/// Minimal GLB reader: positions + triangle indices of every primitive of
/// every mesh (read-only; used for local Stanford-scan measurements).
pub fn load_glb(path: &str) -> (Vec<V3>, Vec<u32>) {
    let bytes = std::fs::read(path).unwrap();
    let u32_at = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap()) as usize;
    assert_eq!(&bytes[0..4], b"glTF");
    let json_len = u32_at(12);
    let json: serde_json::Value = serde_json::from_slice(&bytes[20..20 + json_len]).unwrap();
    let bin_start = 20 + json_len + 8;
    let bin = &bytes[bin_start..bin_start + u32_at(20 + json_len)];
    let accessor = |i: usize| -> (usize, usize, usize) {
        let a = &json["accessors"][i];
        let v = &json["bufferViews"][a["bufferView"].as_u64().unwrap() as usize];
        let off = v["byteOffset"].as_u64().unwrap_or(0) as usize
            + a["byteOffset"].as_u64().unwrap_or(0) as usize;
        (
            off,
            a["count"].as_u64().unwrap() as usize,
            a["componentType"].as_u64().unwrap() as usize,
        )
    };
    let (mut pos, mut idx) = (Vec::new(), Vec::new());
    for mesh in json["meshes"].as_array().unwrap() {
        for prim in mesh["primitives"].as_array().unwrap() {
            let base = pos.len() as u32;
            let (off, count, _) =
                accessor(prim["attributes"]["POSITION"].as_u64().unwrap() as usize);
            for i in 0..count {
                let f = |k: usize| {
                    f32::from_le_bytes(
                        bin[off + i * 12 + k * 4..off + i * 12 + k * 4 + 4]
                            .try_into()
                            .unwrap(),
                    )
                };
                pos.push([f(0), f(1), f(2)]);
            }
            let (off, count, ty) = accessor(prim["indices"].as_u64().unwrap() as usize);
            for i in 0..count {
                idx.push(
                    base + if ty == 5125 {
                        u32::from_le_bytes(bin[off + i * 4..off + i * 4 + 4].try_into().unwrap())
                    } else {
                        u16::from_le_bytes(bin[off + i * 2..off + i * 2 + 2].try_into().unwrap())
                            as u32
                    },
                );
            }
        }
    }
    (pos, idx)
}
