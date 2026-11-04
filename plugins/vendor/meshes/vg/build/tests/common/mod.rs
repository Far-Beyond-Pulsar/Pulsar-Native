//! Test mesh generators and cut-inspection helpers.
#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap, HashSet};

use helio_vg_build::*;

pub struct TestMesh {
    pub positions: Vec<[f32; 3]>,
    pub attributes: Vec<u8>,
    pub stride: usize,
    pub indices: Vec<u32>,
    pub mats: Vec<u32>,
}

impl TestMesh {
    pub fn input(&self) -> MeshInput<'_> {
        MeshInput {
            positions: &self.positions,
            attributes: &self.attributes,
            attribute_stride: self.stride,
            indices: &self.indices,
            tri_material: &self.mats,
        }
    }
    pub fn tri_count(&self) -> usize {
        self.indices.len() / 3
    }
}

fn pos_attrs(p: &[[f32; 3]]) -> Vec<u8> {
    p.iter()
        .flat_map(|v| v.iter().flat_map(|f| f.to_le_bytes()))
        .collect()
}

/// Closed manifold sphere with `20 * 4^subdiv` triangles; attribute = position.
pub fn icosphere(subdiv: u32) -> TestMesh {
    let t = (1.0 + 5.0f32.sqrt()) / 2.0;
    let mut p: Vec<[f32; 3]> = vec![
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
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
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
        let mut mid = |a: u32, b: u32, p: &mut Vec<[f32; 3]>| -> u32 {
            let k = (a.min(b), a.max(b));
            *cache.entry(k).or_insert_with(|| {
                let (pa, pb) = (p[a as usize], p[b as usize]);
                let m = [
                    (pa[0] + pb[0]) / 2.0,
                    (pa[1] + pb[1]) / 2.0,
                    (pa[2] + pb[2]) / 2.0,
                ];
                let l = (m[0] * m[0] + m[1] * m[1] + m[2] * m[2]).sqrt();
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
    let attributes = pos_attrs(&p);
    let tris = idx.len();
    TestMesh {
        positions: p,
        attributes,
        stride: 12,
        indices: idx.into_iter().flatten().collect(),
        mats: vec![0; tris],
    }
}

/// Torus with duplicated seam vertices (u/v wrap) carrying distinct UVs, and
/// three material bands. Closed in position space.
pub fn torus(nu: u32, nv: u32) -> TestMesh {
    let mut positions = Vec::new();
    let mut attrs: Vec<u8> = Vec::new();
    for j in 0..=nv {
        for i in 0..=nu {
            let (iu, jv) = (i % nu, j % nv);
            let a = std::f32::consts::TAU * iu as f32 / nu as f32;
            let b = std::f32::consts::TAU * jv as f32 / nv as f32;
            let r = 1.0 + 0.4 * b.cos();
            positions.push([r * a.cos(), 0.4 * b.sin(), r * a.sin()]);
            attrs.extend((i as f32 / nu as f32).to_le_bytes());
            attrs.extend((j as f32 / nv as f32).to_le_bytes());
        }
    }
    let at = |i: u32, j: u32| j * (nu + 1) + i;
    let (mut indices, mut mats) = (Vec::new(), Vec::new());
    for j in 0..nv {
        for i in 0..nu {
            let (a, b, c, d) = (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
            indices.extend([a, b, c, a, c, d]);
            let m = i * 3 / nu;
            mats.extend([m, m]);
        }
    }
    TestMesh {
        positions,
        attributes: attrs,
        stride: 8,
        indices,
        mats,
    }
}

/// Open `n x n` quad grid (heightfield), attribute = position.
pub fn grid(n: u32) -> TestMesh {
    let mut p = Vec::new();
    for j in 0..=n {
        for i in 0..=n {
            let (x, y) = (i as f32 / n as f32, j as f32 / n as f32);
            p.push([x, (x * 9.0).sin() * 0.05 + (y * 7.0).cos() * 0.05, y]);
        }
    }
    let at = |i: u32, j: u32| j * (n + 1) + i;
    let mut indices = Vec::new();
    for j in 0..n {
        for i in 0..n {
            let (a, b, c, d) = (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
            indices.extend([a, c, b, a, d, c]);
        }
    }
    let attributes = pos_attrs(&p);
    let tris = indices.len() / 3;
    TestMesh {
        positions: p,
        attributes,
        stride: 12,
        indices,
        mats: vec![0; tris],
    }
}

/// Quantize like the writer does (must stay in sync with `Lattice::quantize`).
pub fn quantize(p: [f32; 3], meta: &VgeoMeta) -> [u32; 3] {
    let mut q = [0u32; 3];
    for i in 0..3 {
        let v = ((p[i] as f64 - meta.origin[i] as f64) / meta.step as f64).round();
        q[i] = v.clamp(0.0, LATTICE_MAX as f64) as u32;
    }
    q
}

pub type Lat = [u32; 3];

/// Directed edges with no reverse partner (degenerate edges ignored).
pub fn open_edges(tris: &[[Lat; 3]]) -> HashSet<(Lat, Lat)> {
    let mut dir: HashMap<(Lat, Lat), u32> = HashMap::new();
    for t in tris {
        for e in 0..3 {
            let (a, b) = (t[e], t[(e + 1) % 3]);
            if a != b {
                *dir.entry((a, b)).or_default() += 1;
            }
        }
    }
    dir.keys()
        .filter(|&&(a, b)| !dir.contains_key(&(b, a)))
        .copied()
        .collect()
}

pub fn cut_tris(r: &VgeoReader, tau: f32, proj: &Projection) -> (usize, Vec<[Lat; 3]>) {
    let sel = r.select_clusters(tau, proj).unwrap();
    let mut out = Vec::new();
    for s in &sel {
        let d = r.decode(s).unwrap();
        for t in &d.tris {
            out.push([
                d.lattice[t[0] as usize],
                d.lattice[t[1] as usize],
                d.lattice[t[2] as usize],
            ]);
        }
    }
    (sel.len(), out)
}

/// Canonical (rotation-minimal) form of a triangle of keys.
pub fn canon<T: Ord + Copy>(t: [T; 3]) -> [T; 3] {
    let r = [t, [t[1], t[2], t[0]], [t[2], t[0], t[1]]];
    *r.iter().min().unwrap()
}

pub fn multiset<T: Ord>(v: Vec<T>) -> BTreeMap<T, u32> {
    let mut m = BTreeMap::new();
    for x in v {
        *m.entry(x).or_insert(0) += 1;
    }
    m
}
