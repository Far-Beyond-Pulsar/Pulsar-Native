//! Triangle clustering: split a triangle set into clusters of at most 128
//! triangles / 128 vertices.
//!
//! Greedy growth in the style of meshopt's `buildMeshlets`: seed at the next
//! unassigned triangle in Morton order, then repeatedly absorb the adjacent
//! triangle that reuses the most cluster vertices, agrees best with the
//! cluster's average normal (so backface-cone culling stays tight) and stays
//! closest to the cluster centroid (so the bounding sphere stays small). A
//! cleanup pass merges undersized leftovers so grouping later sees clusters of
//! useful size.

use helio_vg_format::{CLUSTER_MAX_TRIS, CLUSTER_MAX_VERTS, CLUSTER_MIN_TRIS};

use crate::math::*;
use crate::mesh::VertexStore;

const MAX_T: usize = CLUSTER_MAX_TRIS as usize;
const MAX_V: usize = CLUSTER_MAX_VERTS as usize;
const MIN_T: usize = CLUSTER_MIN_TRIS as usize;

/// CSR adjacency (neighbours across shared edges, welded by position id).
pub(crate) struct Adj {
    pub(crate) start: Vec<u32>,
    pub(crate) nbr: Vec<u32>,
}
impl Adj {
    fn of(&self, t: usize) -> &[u32] {
        &self.nbr[self.start[t] as usize..self.start[t + 1] as usize]
    }
}

pub(crate) fn csr_from_pairs(n: usize, pairs: &[(u32, u32)]) -> Adj {
    let mut start = vec![0u32; n + 1];
    for &(a, _) in pairs {
        start[a as usize + 1] += 1;
    }
    for i in 0..n {
        start[i + 1] += start[i];
    }
    Adj {
        start,
        nbr: pairs.iter().map(|p| p.1).collect(),
    }
}

fn tri_adjacency(vs: &VertexStore, tris: &[[u32; 3]]) -> Adj {
    // Edges are bucketed by key hash and processed one bucket at a time, so
    // the (16 B per directed edge) sort buffer is a fraction of the mesh.
    let buckets = (tris.len() / (2 << 20)).clamp(1, 16) as u64;
    let mut pairs: Vec<(u32, u32)> = Vec::with_capacity(tris.len() * 3);
    for bucket in 0..buckets {
        let mut ek: Vec<(u64, u32)> = Vec::with_capacity(tris.len() * 3 / buckets as usize + 16);
        for (t, tri) in tris.iter().enumerate() {
            for e in 0..3 {
                let a = vs.pid[tri[e] as usize];
                let b = vs.pid[tri[(e + 1) % 3] as usize];
                let k = edge_key(a, b);
                if (k.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 40) % buckets == bucket {
                    ek.push((k, t as u32));
                }
            }
        }
        ek.sort_unstable();
        let mut i = 0;
        while i < ek.len() {
            let mut j = i + 1;
            while j < ek.len() && ek[j].0 == ek[i].0 {
                j += 1;
            }
            // Non-manifold edges (>2 triangles) link all pairs, capped.
            let m = (j - i).min(6);
            for x in i..i + m {
                for y in x + 1..i + m {
                    let (a, b) = (ek[x].1, ek[y].1);
                    if a != b {
                        pairs.push((a, b));
                        pairs.push((b, a));
                    }
                }
            }
            i = j;
        }
    }
    pairs.sort_unstable();
    pairs.dedup();
    csr_from_pairs(tris.len(), &pairs)
}

/// Cluster `tris` (corners are global unified vertex ids). Returns, per
/// cluster, the indices into `tris` it owns (in growth order, which is a good
/// vertex-cache order too).
pub fn cluster_tris(vs: &VertexStore, tris: &[[u32; 3]]) -> Vec<Vec<u32>> {
    let n = tris.len();
    if n == 0 {
        return Vec::new();
    }
    // Compact local vertex ids so the per-vertex stamp array is sized by this
    // call, not by the whole mesh (this runs once per group per level).
    let mut ids: Vec<u32> = tris.iter().flat_map(|t| t.iter().copied()).collect();
    ids.sort_unstable();
    ids.dedup();
    let local: Vec<[u32; 3]> = tris
        .iter()
        .map(|t| {
            [
                ids.binary_search(&t[0]).unwrap() as u32,
                ids.binary_search(&t[1]).unwrap() as u32,
                ids.binary_search(&t[2]).unwrap() as u32,
            ]
        })
        .collect();

    let mut cent = Vec::with_capacity(n);
    let mut nrm = Vec::with_capacity(n);
    let mut area_sum = 0.0f64;
    for t in tris {
        let (a, b, c) = (
            vs.pos[t[0] as usize],
            vs.pos[t[1] as usize],
            vs.pos[t[2] as usize],
        );
        let cr = cross(sub(b, a), sub(c, a));
        area_sum += 0.5 * length(cr) as f64;
        nrm.push(normalize(cr));
        cent.push(scale(add(add(a, b), c), 1.0 / 3.0));
    }
    let avg_area = (area_sum / n as f64) as f32;
    // Radius of a disc made of a full cluster of average triangles; used to
    // normalize the distance term of the score.
    let r_est = ((avg_area * MAX_T as f32) / std::f32::consts::PI)
        .sqrt()
        .max(1e-20);

    let adj = tri_adjacency(vs, tris);

    let frame = morton_frame(cent.iter().copied());
    let mut order: Vec<(u64, u32)> = cent
        .iter()
        .enumerate()
        .map(|(i, &c)| (morton3(c, frame), i as u32))
        .collect();
    order.sort_unstable();
    let order: Vec<u32> = order.into_iter().map(|o| o.1).collect();

    let mut assigned = vec![false; n];
    let mut fstamp = vec![u32::MAX; n]; // frontier membership stamp (cluster id)
    let mut vstamp = vec![u32::MAX; ids.len()]; // vertex-in-cluster stamp
    let mut clusters: Vec<Vec<u32>> = Vec::new();
    let mut cursor = 0;

    loop {
        while cursor < n && assigned[order[cursor] as usize] {
            cursor += 1;
        }
        if cursor == n {
            break;
        }
        let cid = clusters.len() as u32;
        let mut cl: Vec<u32> = Vec::with_capacity(MAX_T);
        let mut nverts = 0usize;
        let mut sum_n = [0.0f32; 3];
        let mut sum_c = [0.0f32; 3];
        let mut frontier: Vec<u32> = Vec::new();

        let add_tri = |t: u32,
                       cl: &mut Vec<u32>,
                       nverts: &mut usize,
                       sum_n: &mut V3,
                       sum_c: &mut V3,
                       frontier: &mut Vec<u32>,
                       assigned: &mut Vec<bool>,
                       vstamp: &mut Vec<u32>,
                       fstamp: &mut Vec<u32>| {
            assigned[t as usize] = true;
            cl.push(t);
            for &v in &local[t as usize] {
                if vstamp[v as usize] != cid {
                    vstamp[v as usize] = cid;
                    *nverts += 1;
                }
            }
            *sum_n = add(*sum_n, nrm[t as usize]);
            *sum_c = add(*sum_c, cent[t as usize]);
            for &nb in adj.of(t as usize) {
                if !assigned[nb as usize] && fstamp[nb as usize] != cid {
                    fstamp[nb as usize] = cid;
                    frontier.push(nb);
                }
            }
        };

        add_tri(
            order[cursor],
            &mut cl,
            &mut nverts,
            &mut sum_n,
            &mut sum_c,
            &mut frontier,
            &mut assigned,
            &mut vstamp,
            &mut fstamp,
        );
        while cl.len() < MAX_T {
            frontier.retain(|&t| !assigned[t as usize]);
            let axis = normalize(sum_n);
            let centroid = scale(sum_c, 1.0 / cl.len() as f32);
            let mut best: Option<(f32, u32)> = None;
            for &t in &frontier {
                let newv = local[t as usize]
                    .iter()
                    .filter(|&&v| vstamp[v as usize] != cid)
                    .count();
                if nverts + newv > MAX_V {
                    continue;
                }
                let d = dist(cent[t as usize], centroid) / r_est;
                let score = (3 - newv) as f32 + 0.5 * dot(nrm[t as usize], axis) - 0.5 * d;
                match best {
                    Some((s, bt)) if score < s || (score == s && t > bt) => {}
                    _ => best = Some((score, t)),
                }
            }
            match best {
                Some((_, t)) => add_tri(
                    t,
                    &mut cl,
                    &mut nverts,
                    &mut sum_n,
                    &mut sum_c,
                    &mut frontier,
                    &mut assigned,
                    &mut vstamp,
                    &mut fstamp,
                ),
                None => break,
            }
        }
        clusters.push(cl);
    }

    merge_small(&mut clusters, &local, &adj, &order, n);
    clusters.retain(|c| !c.is_empty());
    clusters
}

fn union_verts(local: &[[u32; 3]], a: &[u32], b: &[u32]) -> usize {
    let mut v: Vec<u32> = a
        .iter()
        .chain(b.iter())
        .flat_map(|&t| local[t as usize])
        .collect();
    v.sort_unstable();
    v.dedup();
    v.len()
}

/// Merge clusters below `CLUSTER_MIN_TRIS`: first into the adjacent cluster
/// sharing the most edges, then (for disconnected islands) by concatenating
/// small clusters that are close in Morton order.
fn merge_small(
    clusters: &mut Vec<Vec<u32>>,
    local: &[[u32; 3]],
    adj: &Adj,
    order: &[u32],
    n: usize,
) {
    let mut cl_of = vec![0u32; n];
    for (c, cl) in clusters.iter().enumerate() {
        for &t in cl {
            cl_of[t as usize] = c as u32;
        }
    }
    let mut small: Vec<usize> = (0..clusters.len())
        .filter(|&c| clusters[c].len() < MIN_T)
        .collect();
    small.sort_by_key(|&c| (clusters[c].len(), c));
    for &c in &small {
        if clusters[c].is_empty() || clusters[c].len() >= MIN_T {
            continue;
        }
        let mut counts: Vec<(u32, u32)> = Vec::new(); // (cluster, shared edge count)
        for &t in &clusters[c] {
            for &nb in adj.of(t as usize) {
                let d = cl_of[nb as usize];
                if d as usize != c {
                    match counts.iter_mut().find(|e| e.0 == d) {
                        Some(e) => e.1 += 1,
                        None => counts.push((d, 1)),
                    }
                }
            }
        }
        counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        for (d, _) in counts {
            let d = d as usize;
            if clusters[c].len() + clusters[d].len() <= MAX_T
                && union_verts(local, &clusters[c], &clusters[d]) <= MAX_V
            {
                let moved = std::mem::take(&mut clusters[c]);
                for &t in &moved {
                    cl_of[t as usize] = d as u32;
                }
                clusters[d].extend(moved);
                break;
            }
        }
    }

    // Disconnected leftovers: pack by Morton rank of their first triangle.
    let mut rank = vec![0u32; n];
    for (r, &t) in order.iter().enumerate() {
        rank[t as usize] = r as u32;
    }
    let mut rest: Vec<usize> = (0..clusters.len())
        .filter(|&c| !clusters[c].is_empty() && clusters[c].len() < MIN_T)
        .collect();
    rest.sort_by_key(|&c| (rank[clusters[c][0] as usize], c));
    let mut target: Option<usize> = None;
    for c in rest {
        match target {
            Some(d)
                if clusters[d].len() + clusters[c].len() <= MAX_T
                    && union_verts(local, &clusters[d], &clusters[c]) <= MAX_V =>
            {
                let moved = std::mem::take(&mut clusters[c]);
                clusters[d].extend(moved);
            }
            _ => target = Some(c),
        }
    }
}
