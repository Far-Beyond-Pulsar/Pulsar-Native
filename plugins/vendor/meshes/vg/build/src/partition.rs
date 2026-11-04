//! Partition one DAG level's clusters into groups of 4..=8 adjacent clusters.
//!
//! Adjacency is the shared-edge graph (edge weight = number of shared edges).
//! A group's cost is its boundary length -- those vertices are locked during
//! simplification, so minimizing it maximizes how far a group can reduce. We
//! use an in-house greedy region-growing seed pass (Morton-ordered seeds) and
//! a few Fiduccia-Mattheyses-style boundary refinement sweeps instead of a
//! metis dependency (plan decision D4).

use helio_vg_format::{GROUP_MAX_CLUSTERS, GROUP_MIN_CLUSTERS};

use crate::cluster::csr_from_pairs;
use crate::dag::ClusterNode;
use crate::math::*;
use crate::mesh::VertexStore;

const MAX_S: usize = GROUP_MAX_CLUSTERS as usize;
const MIN_S: usize = GROUP_MIN_CLUSTERS as usize;
const NONE: u32 = u32::MAX;
/// Clusters on each side of a group's seed, in Morton order, searched for the
/// nearest free cluster when adjacency runs out.
const SPATIAL_WINDOW: usize = 96;

/// Returns groups of ids taken from `cur` (each id indexes `clusters`).
/// `cap` is the largest group the page budget allows (>= 8). While the level
/// is small enough (`n <= cap`) it becomes ONE group, so no interior
/// boundary is locked; a level of up to `4 * cap` clusters uses large groups
/// (up to `cap`); finer levels use the usual 4..=8.
pub fn partition(
    vs: &VertexStore,
    cur: &[u32],
    clusters: &[ClusterNode],
    cap: usize,
) -> Vec<Vec<u32>> {
    let n = cur.len();
    if n <= cap.max(MAX_S) {
        return vec![cur.to_vec()];
    }
    let (max_s, min_s, ideal) = if n <= 4 * cap {
        (cap, (cap / 3).max(MIN_S), (cap * 3 / 4) as f32)
    } else {
        (MAX_S, MIN_S, 6.0)
    };

    // ── weighted cluster adjacency ─────────────────────────────────────────
    let mut ek: Vec<(u64, u32)> = Vec::new();
    for (i, &c) in cur.iter().enumerate() {
        for t in &clusters[c as usize].tris {
            for e in 0..3 {
                let a = vs.pid[t[e] as usize];
                let b = vs.pid[t[(e + 1) % 3] as usize];
                ek.push((edge_key(a, b), i as u32));
            }
        }
    }
    ek.sort_unstable();
    ek.dedup(); // an edge interior to one cluster appears twice with the same id
    let mut pairs: Vec<(u32, u32)> = Vec::new();
    let mut i = 0;
    while i < ek.len() {
        let mut j = i + 1;
        while j < ek.len() && ek[j].0 == ek[i].0 {
            j += 1;
        }
        let m = (j - i).min(6);
        for x in i..i + m {
            for y in x + 1..i + m {
                pairs.push((ek[x].1, ek[y].1));
                pairs.push((ek[y].1, ek[x].1));
            }
        }
        i = j;
    }
    pairs.sort_unstable();
    // Collapse duplicate pairs into weights.
    let mut wpairs: Vec<(u32, u32)> = Vec::new();
    let mut weights: Vec<u32> = Vec::new();
    for p in pairs {
        if wpairs.last() == Some(&p) {
            *weights.last_mut().unwrap() += 1;
        } else {
            wpairs.push(p);
            weights.push(1);
        }
    }
    let adj = csr_from_pairs(n, &wpairs);
    let (adj_r, weights_r) = (&adj, &weights);
    let nbrs = |i: usize| {
        let (lo, hi) = (adj_r.start[i] as usize, adj_r.start[i + 1] as usize);
        (lo..hi).map(move |k| (adj_r.nbr[k], weights_r[k]))
    };

    // ── group count / sizes ────────────────────────────────────────────────
    let lo_g = (n + max_s - 1) / max_s;
    let hi_g = (n / min_s).max(lo_g);
    let g_count = ((n as f32 / ideal).round() as usize).clamp(lo_g, hi_g);
    let base = n / g_count;
    let rem = n % g_count;

    // ── greedy region growing from Morton-ordered seeds ───────────────────
    let centers: Vec<V3> = cur
        .iter()
        .map(|&c| {
            // The inherited error sphere covers all siblings/ancestors and
            // is deliberately loose. Partition by the actual geometry so
            // higher levels do not lose spatial locality.
            let s = bounding_sphere(
                clusters[c as usize]
                    .tris
                    .iter()
                    .flat_map(|t| t.iter().map(|&v| vs.pos[v as usize])),
            );
            [s[0], s[1], s[2]]
        })
        .collect();
    let frame = morton_frame(centers.iter().copied());
    let mut order: Vec<(u64, u32)> = centers
        .iter()
        .enumerate()
        .map(|(i, &c)| (morton3(c, frame), i as u32))
        .collect();
    order.sort_unstable();
    let order: Vec<u32> = order.into_iter().map(|o| o.1).collect();
    let mut rank = vec![0usize; n];
    for (r, &c) in order.iter().enumerate() {
        rank[c as usize] = r;
    }

    let mut group_of = vec![NONE; n];
    let mut sizes = vec![0usize; g_count];
    let mut cursor = 0;
    for g in 0..g_count {
        let target = base + (g < rem) as usize;
        while cursor < n && group_of[order[cursor] as usize] != NONE {
            cursor += 1;
        }
        let seed = order[cursor];
        let mut members = vec![seed];
        group_of[seed as usize] = g as u32;
        while members.len() < target {
            let mut cand: Vec<(u32, u32)> = Vec::new(); // (cluster, weight to group)
            for &m in &members {
                for (nb, w) in nbrs(m as usize) {
                    if group_of[nb as usize] == NONE {
                        match cand.iter_mut().find(|c| c.0 == nb) {
                            Some(c) => c.1 += w,
                            None => cand.push((nb, w)),
                        }
                    }
                }
            }
            let pick = cand
                .iter()
                .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
                .map(|c| c.0)
                .unwrap_or_else(|| {
                    // Component exhausted (disconnected pieces such as foliage
                    // cards share no edges, so adjacency says nothing): take the
                    // free cluster nearest the group among a window of the
                    // Morton order around its seed. The window absorbs the
                    // curve's jumps; picking the next free cluster in curve
                    // order instead strings together far-apart clusters.
                    let centre = {
                        let mut s = [0.0f32; 3];
                        for &m in &members {
                            for k in 0..3 {
                                s[k] += centers[m as usize][k];
                            }
                        }
                        scale(s, 1.0 / members.len() as f32)
                    };
                    let at = rank[seed as usize];
                    let (lo, hi) = (
                        at.saturating_sub(SPATIAL_WINDOW),
                        (at + SPATIAL_WINDOW).min(n - 1),
                    );
                    let near = (lo..=hi)
                        .map(|r| order[r])
                        .filter(|&c| group_of[c as usize] == NONE)
                        .min_by(|&a, &b| {
                            dist(centers[a as usize], centre)
                                .total_cmp(&dist(centers[b as usize], centre))
                                .then(a.cmp(&b))
                        });
                    near.unwrap_or_else(|| {
                        while group_of[order[cursor] as usize] != NONE {
                            cursor += 1;
                        }
                        order[cursor]
                    })
                });
            group_of[pick as usize] = g as u32;
            members.push(pick);
        }
        sizes[g] = members.len();
    }

    // ── boundary refinement ────────────────────────────────────────────────
    for _ in 0..4 {
        let mut moved = false;
        for i in 0..n {
            let own = group_of[i] as usize;
            if sizes[own] <= min_s {
                continue;
            }
            let mut w: Vec<(u32, u32)> = Vec::new();
            for (nb, wt) in nbrs(i) {
                let g = group_of[nb as usize];
                match w.iter_mut().find(|e| e.0 == g) {
                    Some(e) => e.1 += wt,
                    None => w.push((g, wt)),
                }
            }
            let w_own = w.iter().find(|e| e.0 as usize == own).map_or(0, |e| e.1);
            let best = w
                .iter()
                .filter(|e| e.0 as usize != own && sizes[e.0 as usize] < max_s)
                .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)));
            if let Some(&(g, wg)) = best {
                if wg > w_own {
                    sizes[own] -= 1;
                    sizes[g as usize] += 1;
                    group_of[i] = g;
                    moved = true;
                }
            }
        }
        if !moved {
            break;
        }
    }

    let mut groups: Vec<Vec<u32>> = vec![Vec::new(); g_count];
    for i in 0..n {
        groups[group_of[i] as usize].push(cur[i]);
    }
    groups.retain(|g| !g.is_empty());
    groups
}
