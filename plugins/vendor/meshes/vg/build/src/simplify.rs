//! Group simplification using meshoptimizer.
//!
//! Collapse ordering comes from meshoptimizer; the error of a group is not
//! taken from it but measured by `deviation`.
//!
//! A group's triangles fall into two classes:
//! * **anchored** geometry (connected surfaces that touch other groups, or big
//!   open/non-manifold components): boundary, non-manifold edges and attribute
//!   seams stay locked, so neighbouring groups never crack.
//! * **floating** pieces (small connected components that lie entirely inside
//!   the group, e.g. needle/leaf cards, branch fragments): nothing outside the
//!   group references them, so nothing is locked. They are reduced by edge
//!   collapse with island pruning and, when that stalls (disconnected pieces
//!   cannot collapse into each other), by proximity merging (topology-free
//!   vertex clustering), so canopies coarsen into fewer/larger cards.
//!
//! Material sections are always simplified independently. Output references
//! original vertices, preserving their attribute bytes and exact positions.

use crate::deviation::point_deviations;
use crate::math::{add, cross, dist, dot, edge_key, length, normalize, scale, sub, V3};
use crate::mesh::{Tri, VertexStore};
use std::collections::BTreeMap;

/// A connected component with at most this many triangles that touches no
/// other group is "floating".
pub const FLOATING_MAX_TRIS: usize = 1536;
/// Appearance error of a floating-dominated group (see
/// [`SimplifyResult::coverage_tris`]): `COVERAGE_FACTOR` x the spacing of its
/// output triangles, `r * cbrt(4pi/3 / n)` for `n` triangles in a group sphere
/// of radius `r`. Below that scale the extra triangles of a canopy add detail
/// the viewer cannot resolve (a node smaller than ~1/COVERAGE_FACTOR spacings
/// per pixel).
pub const COVERAGE_FACTOR: f32 = 0.05;
/// ... and never below this many linear footprints (sqrt of mean triangle area) of
/// the output: triangles smaller than a pixel add nothing.
pub const COVERAGE_FOOTPRINT: f32 = 2.0;
/// Edge collapse that ends above this ratio of the target hands over to
/// proximity merging.
const SLOPPY_TRIGGER: (usize, usize) = (115, 100);

/// A floating piece with at most this many triangles is a "card" (needle, leaf
/// quad, small polygon): it is merged with its neighbours into area-preserving
/// quads instead of being collapsed or pruned (see [`merge_cards`]).
const CARD_MAX_TRIS: usize = 6;
/// A merge cell whose members spread over more than this multiple of the median
/// cell radius is split further.
const CELL_RADIUS_SPREAD: f32 = 2.0;

pub struct SimplifyResult {
    pub tris: Vec<([u32; 3], u32)>,
    /// Vertices created by this simplification (position, source vertex whose
    /// attributes they use); output corners reference them as `vs.len() +
    /// index`. Only card merging creates them, and all of them are floating.
    pub new_verts: Vec<(V3, u32)>,
    /// Diagnostics: locked / total vertices of the input group.
    pub locked: usize,
    pub verts: usize,
    /// Diagnostics: input triangles in floating pieces.
    pub floating: usize,
    /// Floating output triangles when the group is (>= 90%) floating pieces
    /// (needle/leaf cards, branch fragments); `None` for all other groups.
    /// For those the DAG builder replaces the Hausdorff distance (a vanished
    /// card leaves a hole its own size while its neighbours are further away
    /// than that, so the distance says little about a canopy) by the spacing
    /// error above.
    pub coverage_tris: Option<(usize, f32)>,
}

/// Edges of one DAG level whose global use count is not 2 (open, non-manifold),
/// used to tell a group-boundary edge (shared with another group) from a
/// genuinely open one.
pub struct GlobalEdges {
    special: Vec<(u64, u32)>,
}

impl GlobalEdges {
    pub fn build<'a>(vs: &VertexStore, tris: impl Iterator<Item = &'a [u32; 3]>) -> Self {
        use rayon::prelude::*;
        let mut keys: Vec<u64> = Vec::new();
        for t in tris {
            for e in 0..3 {
                keys.push(edge_key(
                    vs.pid[t[e] as usize],
                    vs.pid[t[(e + 1) % 3] as usize],
                ));
            }
        }
        keys.par_sort_unstable();
        let mut special = Vec::new();
        let mut i = 0;
        while i < keys.len() {
            let mut j = i + 1;
            while j < keys.len() && keys[j] == keys[i] {
                j += 1;
            }
            if j - i != 2 {
                special.push((keys[i], (j - i) as u32));
            }
            i = j;
        }
        Self { special }
    }

    fn count(&self, key: u64) -> u32 {
        match self.special.binary_search_by_key(&key, |e| e.0) {
            Ok(i) => self.special[i].1,
            Err(_) => 2,
        }
    }
}

// Include non-manifold multiplicities: locking vertices alone does not stop
// a simplifier from removing a duplicate face incident to a locked edge.
fn boundary_signature(vs: &VertexStore, tris: &[[u32; 3]], strict: bool) -> BTreeMap<u64, usize> {
    let mut edges = BTreeMap::new();
    for t in tris {
        for e in 0..3 {
            *edges
                .entry(edge_key(
                    vs.pid[t[e] as usize],
                    vs.pid[t[(e + 1) % 3] as usize],
                ))
                .or_insert(0) += 1;
        }
    }
    edges.retain(|_, n| *n == 1 || (strict && *n > 2));
    edges
}

fn find(p: &mut [u32], mut x: u32) -> u32 {
    while p[x as usize] != x {
        p[x as usize] = p[p[x as usize] as usize];
        x = p[x as usize];
    }
    x
}

/// Per triangle: does it belong to a floating piece? `edges` = sorted edge
/// keys of the group (one entry per use).
fn floating_flags(
    vs: &VertexStore,
    tris: &[Tri],
    edges: &[u64],
    global: Option<&GlobalEdges>,
) -> Vec<bool> {
    let cross = |key: u64| -> bool {
        let Some(global) = global else { return false };
        let lo = edges.partition_point(|&e| e < key);
        let hi = edges.partition_point(|&e| e <= key);
        (hi - lo) as u32 != global.count(key)
    };
    let mut pids: Vec<u32> = tris
        .iter()
        .filter(|t| vs.floating[t.v[0] as usize])
        .flat_map(|t| t.v.map(|v| vs.pid[v as usize]))
        .collect();
    pids.sort_unstable();
    pids.dedup();
    let local = |v: u32| pids.binary_search(&vs.pid[v as usize]).unwrap() as u32;
    let mut parent: Vec<u32> = (0..pids.len() as u32).collect();
    for t in tris.iter().filter(|t| vs.floating[t.v[0] as usize]) {
        for k in 1..3 {
            let a = find(&mut parent, local(t.v[0]));
            let b = find(&mut parent, local(t.v[k]));
            if a != b {
                parent[b as usize] = a;
            }
        }
    }
    let mut touches = vec![false; pids.len()];
    let roots: Vec<Option<u32>> = tris
        .iter()
        .map(|t| vs.floating[t.v[0] as usize].then(|| find(&mut parent, local(t.v[0]))))
        .collect();
    for (t, r) in tris.iter().zip(&roots) {
        let Some(r) = r else { continue };
        for e in 0..3 {
            let key = edge_key(vs.pid[t.v[e] as usize], vs.pid[t.v[(e + 1) % 3] as usize]);
            if cross(key) {
                touches[*r as usize] = true;
            }
        }
    }
    roots
        .iter()
        .map(|r| r.is_some_and(|r| !touches[r as usize]))
        .collect()
}

/// Simplify a group to about `target` triangles with its boundary locked.
///
/// `budget` is an object-space deviation allowance: input vertices that the
/// result leaves further than `budget` from the simplified surface (typically
/// the tips of thin spikes, whose quadric cost is deceptively low) are locked
/// and the group is re-simplified, so one hard feature cannot make the whole
/// group's error (and that of all its ancestors) explode.
///
/// `whole` marks a single group covering every remaining cluster: there is no
/// neighbour to crack against, so only open (single-use) edges are protected
/// and non-manifold edges may simplify like any other.
///
/// `global` gives the level's open/non-manifold edge counts (needed to tell
/// floating pieces from group-boundary geometry); `None` when the group is
/// the whole level.
pub fn simplify_group(
    vs: &VertexStore,
    tris: &[Tri],
    target: usize,
    budget: f32,
    whole: bool,
    global: Option<&GlobalEdges>,
) -> SimplifyResult {
    let mut edges = Vec::with_capacity(tris.len() * 3);
    for t in tris {
        for e in 0..3 {
            edges.push(edge_key(
                vs.pid[t.v[e] as usize],
                vs.pid[t.v[(e + 1) % 3] as usize],
            ));
        }
    }
    edges.sort_unstable();
    let mut boundary = Vec::new();
    let mut i = 0;
    while i < edges.len() {
        let mut end = i + 1;
        while end < edges.len() && edges[end] == edges[i] {
            end += 1;
        }
        if end - i == 1 || (!whole && end - i > 2) {
            boundary.push((edges[i] >> 32) as u32);
            boundary.push(edges[i] as u32);
        }
        i = end;
    }
    boundary.sort_unstable();
    boundary.dedup();

    // Without level-wide edge counts nothing can be proven isolated.
    let floating = if global.is_some() || whole {
        floating_flags(vs, tris, &edges, global)
    } else {
        vec![false; tris.len()]
    };
    let n_floating = floating.iter().filter(|&&f| f).count();

    // (material, floating) sections.
    let mut sections: BTreeMap<(u32, bool), Vec<[u32; 3]>> = BTreeMap::new();
    for (t, &f) in tris.iter().zip(&floating) {
        sections.entry((t.mat, f)).or_default().push(t.v);
    }
    let mut output = Vec::new();
    let mut new_verts: Vec<(V3, u32)> = Vec::new();
    let mut merged_cards = false;
    let (mut n_locked, mut n_verts) = (0usize, 0usize);
    for ((material, is_floating), section) in sections {
        let want = (target * section.len() / tris.len()).max(1);
        if is_floating {
            // Vertices on an edge shared with another material section stay put
            // (the sections are simplified independently and must still meet).
            let mut sec_edges: Vec<u64> = section
                .iter()
                .flat_map(|t| {
                    (0..3).map(move |e| {
                        edge_key(vs.pid[t[e] as usize], vs.pid[t[(e + 1) % 3] as usize])
                    })
                })
                .collect();
            sec_edges.sort_unstable();
            let mut pinned: Vec<u32> = Vec::new();
            let mut k = 0;
            while k < sec_edges.len() {
                let mut j = k + 1;
                while j < sec_edges.len() && sec_edges[j] == sec_edges[k] {
                    j += 1;
                }
                let all = edges.partition_point(|&e| e <= sec_edges[k])
                    - edges.partition_point(|&e| e < sec_edges[k]);
                if all != j - k {
                    pinned.push((sec_edges[k] >> 32) as u32);
                    pinned.push(sec_edges[k] as u32);
                }
                k = j;
            }
            pinned.sort_unstable();
            pinned.dedup();
            let (reduced, nv, merged) =
                simplify_floating(vs, &section, want, &pinned, &mut new_verts);
            merged_cards |= merged;
            n_verts += nv;
            output.extend(reduced.into_iter().map(|t| (t, material)));
            continue;
        }
        let mut ids: Vec<u32> = section.iter().flatten().copied().collect();
        ids.sort_unstable();
        ids.dedup();
        let positions: Vec<_> = ids.iter().map(|&g| vs.pos[g as usize]).collect();
        let mut locks: Vec<_> = ids
            .iter()
            .map(|&g| vs.seam[g as usize] || boundary.binary_search(&vs.pid[g as usize]).is_ok())
            .collect();
        n_locked += locks.iter().filter(|&&l| l).count();
        n_verts += locks.len();
        let indices: Vec<_> = section
            .iter()
            .flatten()
            .map(|g| ids.binary_search(g).unwrap() as u32)
            .collect();
        let vertices = meshopt::VertexDataAdapter::new(bytemuck::cast_slice(&positions), 12, 0)
            .expect("packed float3 positions");
        let mut section_error = 0.0f32;
        let expected = boundary_signature(vs, &section, !whole);
        let nm_in = if whole { nonmanifold(vs, &section) } else { 0 };
        let mut reduced = section.clone();
        let mut budget_rounds = 0;
        for _ in 0..16 {
            let result = meshopt::simplify_with_locks(
                &indices,
                &vertices,
                &locks,
                want * 3,
                f32::MAX,
                meshopt::SimplifyOptions::LockBorder | meshopt::SimplifyOptions::ErrorAbsolute,
                Some(&mut section_error),
            );
            reduced = result
                .chunks_exact(3)
                .map(|t| [ids[t[0] as usize], ids[t[1] as usize], ids[t[2] as usize]])
                .collect();
            let actual = boundary_signature(vs, &reduced, !whole);
            if expected == actual
                && !reduced.is_empty()
                && (!whole || nonmanifold(vs, &reduced) <= nm_in)
            {
                budget_rounds += 1;
                if budget.is_infinite() || budget_rounds > 3 {
                    break;
                }
                let out: Vec<[V3; 3]> = reduced
                    .iter()
                    .map(|t| t.map(|v| vs.pos[v as usize]))
                    .collect();
                let dev = point_deviations(&positions, &out);
                let mut any = false;
                for (l, d) in locks.iter_mut().zip(dev) {
                    if d > budget && !*l {
                        *l = true;
                        any = true;
                    }
                }
                if !any {
                    break;
                }
                continue;
            }
            // Freeze the one-ring around changed topology, then retry the
            // original section. Preserve as much safe reduction as possible.
            let mut affected = std::collections::BTreeSet::new();
            for (&edge, count) in expected.iter().chain(actual.iter()) {
                if expected.get(&edge) != actual.get(&edge) {
                    let _ = count;
                    affected.insert((edge >> 32) as u32);
                    affected.insert(edge as u32);
                }
            }
            for t in &section {
                if t.iter().any(|&v| affected.contains(&vs.pid[v as usize])) {
                    for v in t {
                        locks[ids.binary_search(v).unwrap()] = true;
                    }
                }
            }
        }
        if !reduced.is_empty()
            && expected == boundary_signature(vs, &reduced, !whole)
            && (!whole || nonmanifold(vs, &reduced) <= nm_in)
        {
            // The simplifier's own error estimate is ignored: the DAG builder
            // measures the real deviation against the original surface
            // (`deviation.rs`).
            output.extend(reduced.into_iter().map(|t| (t, material)));
        } else {
            // Malformed/non-manifold input must retain its geometry instead
            // of opening a crack between independently selected groups.
            output.extend(section.into_iter().map(|t| (t, material)));
        }
    }
    // Dominance is by source pieces (small components of the input mesh), also when
    // some of them currently straddle a group border and stay anchored.
    let src_floating = tris.iter().filter(|t| vs.floating[t.v[0] as usize]).count();
    // Merged cards keep the measured deviation (their error is a real distance,
    // not the spacing proxy used for pieces that could only be thinned).
    let coverage = (!merged_cards && src_floating * 10 >= tris.len() * 9).then(|| {
        let float_out: Vec<[u32; 3]> = output
            .iter()
            .filter(|(t, _)| vs.floating[t[0] as usize])
            .map(|(t, _)| *t)
            .collect();
        (!float_out.is_empty()).then(|| (float_out.len(), mean_footprint(vs, &float_out)))
    });
    SimplifyResult {
        tris: output,
        new_verts,
        locked: n_locked,
        verts: n_verts,
        floating: n_floating,
        coverage_tris: coverage.flatten(),
    }
}

/// Floating pieces of one material section, reduced to about `want` triangles.
/// Card-like pieces (needles, leaf quads) are merged into area-preserving
/// quads; larger pieces are thinned by [`collapse_floating`]. Returns
/// (triangles, vertex count, whether cards were merged).
fn simplify_floating(
    vs: &VertexStore,
    section: &[[u32; 3]],
    want: usize,
    pinned: &[u32],
    new_verts: &mut Vec<(V3, u32)>,
) -> (Vec<[u32; 3]>, usize, bool) {
    // Connected pieces (by canonical position id).
    let mut pids: Vec<u32> = section
        .iter()
        .flatten()
        .map(|&v| vs.pid[v as usize])
        .collect();
    pids.sort_unstable();
    pids.dedup();
    let local = |v: u32| pids.binary_search(&vs.pid[v as usize]).unwrap() as u32;
    let mut parent: Vec<u32> = (0..pids.len() as u32).collect();
    for t in section {
        for k in 1..3 {
            let (a, b) = (
                find(&mut parent, local(t[0])),
                find(&mut parent, local(t[k])),
            );
            if a != b {
                parent[a.max(b) as usize] = a.min(b);
            }
        }
    }
    let mut pieces: BTreeMap<u32, Vec<[u32; 3]>> = BTreeMap::new();
    for t in section {
        pieces
            .entry(find(&mut parent, local(t[0])))
            .or_default()
            .push(*t);
    }
    let mut cards: Vec<Vec<[u32; 3]>> = Vec::new();
    let mut rest: Vec<[u32; 3]> = Vec::new();
    for (_, p) in pieces {
        let pinned_piece = p
            .iter()
            .flatten()
            .any(|&v| pinned.binary_search(&vs.pid[v as usize]).is_ok());
        if p.len() <= CARD_MAX_TRIS && !pinned_piece {
            cards.push(p);
        } else {
            rest.extend(p);
        }
    }
    let card_tris: usize = cards.iter().map(Vec::len).sum();
    let want_cards = (want * card_tris / section.len().max(1)).max(2) / 2;
    if cards.len() <= want_cards.max(1) {
        // Nothing to merge: the section is thinned as a whole.
        return {
            let (t, n) = collapse_floating(vs, section, want, pinned);
            (t, n, false)
        };
    }
    let mut out = Vec::new();
    let mut verts = 0;
    if !rest.is_empty() {
        let want_rest = (want * rest.len() / section.len()).max(1);
        let (t, n) = collapse_floating(vs, &rest, want_rest, pinned);
        verts += n;
        out.extend(t);
    }
    let base0 = vs.len() as u32;
    let merged = merge_cards(vs, &cards, want_cards, base0, new_verts);
    verts += merged.len() * 2;
    out.extend(merged);
    (out, verts, true)
}

/// Symmetric 3x3 eigen decomposition (cyclic Jacobi, fixed sweeps, f64 so the
/// result does not depend on the order of anything). Returns eigenvalues
/// descending with matching unit eigenvectors.
fn eigen_sym3(m: [[f64; 3]; 3]) -> ([f64; 3], [[f64; 3]; 3]) {
    let mut a = m;
    let mut v = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for _ in 0..24 {
        let off = a[0][1].abs() + a[0][2].abs() + a[1][2].abs();
        if off < 1e-18 {
            break;
        }
        for (p, q) in [(0, 1), (0, 2), (1, 2)] {
            if a[p][q].abs() < 1e-30 {
                continue;
            }
            let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
            let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
            let (c, s) = (1.0 / (t * t + 1.0).sqrt(), t / (t * t + 1.0).sqrt());
            for k in 0..3 {
                let (akp, akq) = (a[k][p], a[k][q]);
                a[k][p] = c * akp - s * akq;
                a[k][q] = s * akp + c * akq;
            }
            for k in 0..3 {
                let (apk, aqk) = (a[p][k], a[q][k]);
                a[p][k] = c * apk - s * aqk;
                a[q][k] = s * apk + c * aqk;
            }
            for k in 0..3 {
                let (vkp, vkq) = (v[k][p], v[k][q]);
                v[k][p] = c * vkp - s * vkq;
                v[k][q] = s * vkp + c * vkq;
            }
        }
    }
    let mut order = [0usize, 1, 2];
    order.sort_by(|&i, &j| a[j][j].total_cmp(&a[i][i]));
    let vals = order.map(|i| a[i][i]);
    let vecs = order.map(|i| [v[0][i], v[1][i], v[2][i]]);
    (vals, vecs)
}

fn to_f32(v: [f64; 3]) -> V3 {
    [v[0] as f32, v[1] as f32, v[2] as f32]
}

/// One card-like piece reduced to the quad that stands in for it.
struct Card {
    centre: V3,
    axis: V3,
    normal: V3,
    area: f32,
    length: f32,
    verts: Vec<u32>,
}

fn describe_card(vs: &VertexStore, tris: &[[u32; 3]]) -> Option<Card> {
    let mut verts: Vec<u32> = tris.iter().flatten().copied().collect();
    verts.sort_unstable_by_key(|&v| (vs.pid[v as usize], v));
    verts.dedup_by_key(|v| vs.pid[*v as usize]);
    let mut area = 0.0f64;
    let mut centre = [0.0f64; 3];
    let mut n_sum = [0.0f64; 3];
    for t in tris {
        let p = t.map(|v| vs.pos[v as usize]);
        let n = cross(sub(p[1], p[0]), sub(p[2], p[0]));
        let a = 0.5 * length(n) as f64;
        area += a;
        for k in 0..3 {
            centre[k] += a * ((p[0][k] + p[1][k] + p[2][k]) / 3.0) as f64;
            n_sum[k] += n[k] as f64;
        }
    }
    if area <= 0.0 {
        return None;
    }
    for c in &mut centre {
        *c /= area;
    }
    let mut cov = [[0.0f64; 3]; 3];
    for &v in &verts {
        let d = [0, 1, 2].map(|k| vs.pos[v as usize][k] as f64 - centre[k]);
        for i in 0..3 {
            for j in 0..3 {
                cov[i][j] += d[i] * d[j];
            }
        }
    }
    let (_, e) = eigen_sym3(cov);
    let axis = to_f32(e[0]);
    let mut normal = to_f32(e[2]);
    if dot(normal, to_f32(n_sum)) < 0.0 {
        normal = scale(normal, -1.0);
    }
    let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
    for &v in &verts {
        let d = dot(sub(vs.pos[v as usize], to_f32(centre)), axis);
        lo = lo.min(d);
        hi = hi.max(d);
    }
    Some(Card {
        centre: to_f32(centre),
        axis,
        normal,
        area: area as f32,
        length: (hi - lo).max(1e-9),
        verts,
    })
}

/// Merge card-like pieces into `want_cards` quads (two triangles each).
///
/// Pieces are split by recursive median bisection along the widest spatial
/// axis into `want_cards` equally populated cells; each cell becomes one
/// quad whose area is the summed area of its members (coverage is preserved)
/// and whose length is the members' mean length (scale is preserved) until
/// the quad would be wider than it is long, after which it grows evenly. The
/// quad's width is the only thing that changes: the deviation that
/// introduces, and the spread of the members inside the cell, are what the
/// DAG builder measures as this group's error, so a level is only selected at
/// distances where that error is below a pixel.
fn merge_cards(
    vs: &VertexStore,
    pieces: &[Vec<[u32; 3]>],
    want_cards: usize,
    base0: u32,
    new_verts: &mut Vec<(V3, u32)>,
) -> Vec<[u32; 3]> {
    let cards: Vec<Card> = pieces.iter().filter_map(|p| describe_card(vs, p)).collect();
    let mut cells: Vec<Vec<usize>> = Vec::new();
    fn split(cards: &[Card], idx: Vec<usize>, k: usize, out: &mut Vec<Vec<usize>>) {
        if k <= 1 || idx.len() <= 1 {
            out.push(idx);
            return;
        }
        let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        for &i in &idx {
            for a in 0..3 {
                lo[a] = lo[a].min(cards[i].centre[a]);
                hi[a] = hi[a].max(cards[i].centre[a]);
            }
        }
        let axis = (0..3)
            .max_by(|&a, &b| (hi[a] - lo[a]).total_cmp(&(hi[b] - lo[b])))
            .unwrap();
        let mut idx = idx;
        idx.sort_by(|&a, &b| {
            cards[a].centre[axis]
                .total_cmp(&cards[b].centre[axis])
                .then(a.cmp(&b))
        });
        let k1 = k / 2;
        let cut = (idx.len() * k1 / k).clamp(1, idx.len() - 1);
        let right = idx.split_off(cut);
        split(cards, idx, k1, out);
        split(cards, right, k - k1, out);
    }
    split(
        &cards,
        (0..cards.len()).collect(),
        want_cards.max(1),
        &mut cells,
    );
    // A cell that straddles a gap (stray pieces left at the edge of a group)
    // would put its card where no piece is: split such cells until compact, at
    // the price of fewer merges there.
    let radius_of = |cell: &[usize]| -> f32 {
        let mut c = [0.0f32; 3];
        for &i in cell {
            for a in 0..3 {
                c[a] += cards[i].centre[a];
            }
        }
        let c = scale(c, 1.0 / cell.len() as f32);
        cell.iter()
            .map(|&i| dist(cards[i].centre, c))
            .fold(0.0f32, f32::max)
    };
    let mut radii: Vec<f32> = cells
        .iter()
        .filter(|c| c.len() > 1)
        .map(|c| radius_of(c))
        .collect();
    if !radii.is_empty() {
        radii.sort_by(f32::total_cmp);
        let limit = CELL_RADIUS_SPREAD * radii[radii.len() / 2];
        fn compact(
            cards: &[Card],
            cell: Vec<usize>,
            limit: f32,
            radius_of: &dyn Fn(&[usize]) -> f32,
            out: &mut Vec<Vec<usize>>,
        ) {
            if cell.len() <= 1 || radius_of(&cell) <= limit {
                out.push(cell);
                return;
            }
            let mut halves = Vec::new();
            split(cards, cell, 2, &mut halves);
            for h in halves {
                compact(cards, h, limit, radius_of, out);
            }
        }
        let mut refined = Vec::with_capacity(cells.len());
        for cell in std::mem::take(&mut cells) {
            compact(&cards, cell, limit, &radius_of, &mut refined);
        }
        cells = refined;
    }

    let mut out = Vec::with_capacity(cells.len() * 2);
    for cell in cells {
        let members: Vec<&Card> = cell.iter().map(|&i| &cards[i]).collect();
        let area: f32 = members.iter().map(|c| c.area).sum();
        let reference = members
            .iter()
            .max_by(|a, b| a.area.total_cmp(&b.area))
            .unwrap();
        // Orientation-agnostic dominant axis and consistent plane normal.
        let mut axis_cov = [[0.0f64; 3]; 3];
        let mut centre = [0.0f64; 3];
        let mut n_sum = [0.0f64; 3];
        let mut mean_len = 0.0f64;
        for c in &members {
            let w = c.area as f64;
            for i in 0..3 {
                centre[i] += w * c.centre[i] as f64;
                for j in 0..3 {
                    axis_cov[i][j] += w * (c.axis[i] * c.axis[j]) as f64;
                }
            }
            let s = if dot(c.normal, reference.normal) < 0.0 {
                -1.0
            } else {
                1.0
            };
            for i in 0..3 {
                n_sum[i] += w * (s * c.normal[i]) as f64;
            }
            mean_len += w * c.length as f64;
        }
        let a_total = area.max(1e-30) as f64;
        let centre = to_f32([
            centre[0] / a_total,
            centre[1] / a_total,
            centre[2] / a_total,
        ]);
        let mean_len = (mean_len / a_total) as f32;
        let normal = {
            let n = normalize(to_f32(n_sum));
            if length(n) > 0.0 {
                n
            } else {
                reference.normal
            }
        };
        let (_, ev) = eigen_sym3(axis_cov);
        let mut axis = sub(to_f32(ev[0]), scale(normal, dot(to_f32(ev[0]), normal)));
        if length(axis) < 1e-6 {
            axis = sub(reference.axis, scale(normal, dot(reference.axis, normal)));
        }
        let axis = normalize(axis);
        let side = normalize(cross(normal, axis));
        let len = mean_len.max(area.sqrt());
        let wid = area / len;
        let (a, b) = (scale(axis, len * 0.5), scale(side, wid * 0.5));
        let corners = [
            sub(sub(centre, a), b),
            sub(add(centre, a), b),
            add(add(centre, a), b),
            add(sub(centre, a), b),
        ];
        let first = base0 + new_verts.len() as u32;
        let signs = [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
        for (p, (sa, sb)) in corners.into_iter().zip(signs) {
            // Attribute bytes (normal, uv, ...) of all four corners come from ONE
            // member, the largest: the vertex of that member that lies furthest
            // toward this corner of the card. Its texture coordinates stay a
            // coherent quad (the member's whole leaf or needle texture is
            // stretched over the card). Taking each corner from whichever member
            // is nearest mixes unrelated atlas islands across the card, which
            // sampled as dark shards.
            let mut best = (f32::NEG_INFINITY, reference.verts[0]);
            for &v in &reference.verts {
                let d = sub(vs.pos[v as usize], reference.centre);
                let score = sa * dot(d, axis) + sb * dot(d, side);
                if score > best.0 {
                    best = (score, v);
                }
            }
            new_verts.push((p, vs.attr[best.1 as usize]));
        }
        out.push([first, first + 1, first + 2]);
        out.push([first, first + 2, first + 3]);
    }
    out
}

/// Reduce floating pieces of one material section to about `want` triangles:
/// unlocked edge collapse with island pruning, then proximity merging if the
/// pieces cannot collapse into each other. Returns (triangles, vertex count).
fn collapse_floating(
    vs: &VertexStore,
    section: &[[u32; 3]],
    want: usize,
    pinned: &[u32],
) -> (Vec<[u32; 3]>, usize) {
    let mut ids: Vec<u32> = section.iter().flatten().copied().collect();
    ids.sort_unstable();
    ids.dedup();
    let positions: Vec<_> = ids.iter().map(|&g| vs.pos[g as usize]).collect();
    let locks: Vec<bool> = ids
        .iter()
        .map(|&g| pinned.binary_search(&vs.pid[g as usize]).is_ok())
        .collect();
    let indices: Vec<u32> = section
        .iter()
        .flatten()
        .map(|g| ids.binary_search(g).unwrap() as u32)
        .collect();
    let vertices = meshopt::VertexDataAdapter::new(bytemuck::cast_slice(&positions), 12, 0)
        .expect("packed float3 positions");
    let mut result = meshopt::simplify_with_locks(
        &indices,
        &vertices,
        &locks,
        want * 3,
        f32::MAX,
        meshopt::SimplifyOptions::ErrorAbsolute | meshopt::SimplifyOptions::Prune,
        None,
    );
    if result.len() / 3 * SLOPPY_TRIGGER.1 > want * SLOPPY_TRIGGER.0 {
        let merged = meshopt::simplify_sloppy_with_locks(
            &result,
            &vertices,
            &locks,
            want * 3,
            f32::MAX,
            None,
        );
        if !merged.is_empty() {
            result = merged;
        }
    }
    let out: Vec<[u32; 3]> = result
        .chunks_exact(3)
        .map(|t| [ids[t[0] as usize], ids[t[1] as usize], ids[t[2] as usize]])
        .filter(|t| {
            let p = t.map(|v| vs.pid[v as usize]);
            p[0] != p[1] && p[1] != p[2] && p[0] != p[2]
        })
        .collect();
    if out.is_empty() {
        // Never drop a whole section: keep it (cannot happen with a sane target).
        return (section.to_vec(), ids.len());
    }
    (out, ids.len())
}

/// Number of edges used by more than two triangles.
fn nonmanifold(vs: &VertexStore, tris: &[[u32; 3]]) -> usize {
    let mut edges = BTreeMap::new();
    for t in tris {
        for e in 0..3 {
            *edges
                .entry(edge_key(
                    vs.pid[t[e] as usize],
                    vs.pid[t[(e + 1) % 3] as usize],
                ))
                .or_insert(0usize) += 1;
        }
    }
    edges.values().filter(|&&n| n > 2).count()
}

/// Square root of the mean triangle area: the linear footprint of one output
/// triangle (thin needles count as thin, not as long).
fn mean_footprint(vs: &VertexStore, tris: &[[u32; 3]]) -> f32 {
    let mut s = 0.0f64;
    for t in tris {
        let p = t.map(|v| vs.pos[v as usize]);
        s += 0.5
            * crate::math::length(crate::math::cross(
                crate::math::sub(p[1], p[0]),
                crate::math::sub(p[2], p[0]),
            )) as f64;
    }
    (s / tris.len().max(1) as f64).sqrt() as f32
}
