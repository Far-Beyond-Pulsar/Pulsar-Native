//! Cluster DAG construction (plan section 2.1).
//!
//! Level 0 = clusters of the full-resolution mesh (per material section).
//! Each round partitions the current level into groups, simplifies each group
//! to 60% of its triangles
//! with its boundary locked, and splits the result into new clusters; this
//! repeats until a single (or, if simplification stalls, a handful of)
//! cluster(s) remain.
//!
//! Terminology used across the crate:
//! * *producer group*: the group whose simplification created a cluster.
//! * *consumer group* ("hgroup"): the group that consumed a cluster as input.
//!   All clusters sharing a consumer share `parent_error`/`parent_sphere`;
//!   they are exactly the clusters the hierarchy leaves enumerate.

use std::collections::BTreeMap;

use rayon::prelude::*;

use helio_vg_format::PAGE_BYTES;

use crate::cluster::cluster_tris;
use crate::deviation::{measure, Measured};
use crate::math::*;
use crate::mesh::{mark_floating, weld, BuildError, MeshInput, Tri, VertexStore};
use crate::partition::partition;
use crate::simplify::{
    simplify_group, GlobalEdges, SimplifyResult, COVERAGE_FACTOR, COVERAGE_FOOTPRINT,
};

pub const NONE: u32 = u32::MAX;
/// Safety factor on the measured deviation (covers the interior of original
/// triangles, which reference points sample only at their vertices).
const ERROR_MARGIN: f32 = 1.4;
/// Selected triangles per screen pixel the hierarchy is built to stay under when
/// a whole asset is drawn at one level. Below the 2 per pixel the project aims
/// for, because cuts mix neighbouring levels and the silhouette is estimated.
const DENSITY_TARGET: f32 = 1.5;
/// A group whose children are over that density (an asset-wide level of `n`
/// triangles over a silhouette of area `a` has more than `DENSITY_TARGET`
/// triangles per pixel once a pixel is wider than `sqrt(target * a / n)`, and
/// likewise once it is wider than the children's own triangles) may be selected
/// before its measured error reaches the tolerance: its stored error is relaxed
/// to that pixel width, but never below `measured / DENSITY_TOLERANCE_GAIN`.
/// This bounds triangles per pixel on assets whose finest detail is far below a
/// pixel at a distance (scans, foliage) without dropping geometry: the coarser
/// level still stands in for all of it, with at most this many times the
/// tolerance of deviation.
const DENSITY_TOLERANCE_GAIN: f32 = 12.0;
/// Per-group deviation budget handed to the simplifier: at least this multiple
/// of the group's children's error ...
const ERR_GROWTH: f32 = 2.0;
/// ... or this multiple of the level's median child error ...
const ERR_GROWTH_TYPICAL: f32 = 3.0;
/// ... or this fraction of the group's mean edge length (level 1, where
/// children have no error yet).
const BUDGET_EDGE_FRACTION: f32 = 0.5;
/// Fraction of triangles kept per level (a gentler ladder than halving keeps
/// locked-boundary distortion, and so error growth, down).
/// A level is abandoned only when it keeps more than this percentage of the
/// triangles (it makes essentially no progress).
const STALL_PERCENT: usize = 97;
const LEVEL_KEEP: (usize, usize) = (3, 5);
/// Roots are chunked into hierarchy leaves of at most this many clusters.
const ROOT_CHUNK: usize = 8;
/// Upper bound on clusters per simplification group (also bounded by the page).
const MAX_GROUP_CLUSTERS: usize = helio_vg_format::GROUP_MAX_CLUSTERS_LARGE as usize;

pub struct ClusterNode {
    /// Triangles as global unified vertex ids.
    pub tris: Vec<[u32; 3]>,
    pub mat: u32,
    /// Culling + self-error sphere. For non-leaf clusters this is the sphere
    /// of the producing group, shared by all siblings, so the per-cluster
    /// screen-error test agrees across the whole group.
    pub sphere: [f32; 4],
    pub parent_sphere: [f32; 4],
    pub self_error: f32,
    pub parent_error: f32,
    pub level: u32,
    /// Group that produced this cluster (`NONE` for leaves).
    pub producer: u32,
    /// Group that consumed this cluster (root groups for the final level).
    pub consumer: u32,
    /// Build-time only: original-surface reference points this cluster
    /// stands in for (empty at level 0, where the triangles are the surface,
    /// and again once the consuming group has measured them).
    pub(crate) refs: Vec<V3>,
}

/// A set of clusters sharing one parent: either the inputs of one
/// simplification group, or a chunk of root clusters.
pub struct Group {
    pub clusters: Vec<u32>,
    pub level: u32,
    /// Sphere (== `parent_sphere` of its clusters).
    pub sphere: [f32; 4],
    /// Error (== `parent_error` of its clusters); infinite for roots.
    pub error: f32,
    pub is_root: bool,
    /// Clusters this group's simplification produced.
    pub produced: Vec<u32>,
}

pub struct Dag {
    pub vs: VertexStore,
    pub clusters: Vec<ClusterNode>,
    pub groups: Vec<Group>,
    /// Triangles that survived welding/degenerate removal.
    pub source_tris: usize,
}

fn split_into_clusters(vs: &VertexStore, tris: &[([u32; 3], u32)]) -> Vec<(Vec<[u32; 3]>, u32)> {
    let mut by_mat: BTreeMap<u32, Vec<[u32; 3]>> = BTreeMap::new();
    for (t, m) in tris {
        by_mat.entry(*m).or_default().push(*t);
    }
    let mut out = Vec::new();
    for (mat, ts) in by_mat {
        for cl in cluster_tris(vs, &ts) {
            out.push((cl.iter().map(|&i| ts[i as usize]).collect(), mat));
        }
    }
    out
}

/// Mean projected area of the mesh over the three axis directions: each
/// vertex and triangle centroid marks a cell of a 128 x 128 grid over the
/// bounds. Used only to estimate how many triangles per pixel a whole level
/// of the hierarchy puts on screen.
fn silhouette_area(vs: &VertexStore, tris: &[crate::mesh::Tri]) -> f32 {
    const GRID: usize = 128;
    let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for t in tris {
        for &v in &t.v {
            let p = vs.pos[v as usize];
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
    }
    let mut total = 0.0f64;
    for axis in 0..3 {
        let (u, w) = ((axis + 1) % 3, (axis + 2) % 3);
        let (du, dw) = ((hi[u] - lo[u]).max(1e-12), (hi[w] - lo[w]).max(1e-12));
        let mut cells = vec![false; GRID * GRID];
        let mut mark = |p: V3| {
            let i = (((p[u] - lo[u]) / du * GRID as f32) as usize).min(GRID - 1);
            let j = (((p[w] - lo[w]) / dw * GRID as f32) as usize).min(GRID - 1);
            cells[j * GRID + i] = true;
        };
        for t in tris {
            let p = t.v.map(|v| vs.pos[v as usize]);
            for q in p {
                mark(q);
            }
            mark(scale(add(add(p[0], p[1]), p[2]), 1.0 / 3.0));
        }
        let filled = cells.iter().filter(|&&c| c).count();
        total += filled as f64 / (GRID * GRID) as f64 * du as f64 * dw as f64;
    }
    (total / 3.0) as f32
}
pub fn build_dag(mesh: &MeshInput) -> Result<Dag, BuildError> {
    mesh.validate()?;
    let (mut vs, tris) = weld(mesh);
    vs.release_lookup();
    mark_floating(&mut vs, &tris, crate::simplify::FLOATING_MAX_TRIS);
    if tris.is_empty() {
        return Err(BuildError::EmptyMesh);
    }
    let mut dag = Dag {
        vs,
        clusters: Vec::new(),
        groups: Vec::new(),
        source_tris: tris.len(),
    };

    // ── level 0 ────────────────────────────────────────────────────────────
    let mut by_mat: BTreeMap<u32, Vec<[u32; 3]>> = BTreeMap::new();
    for t in &tris {
        by_mat.entry(t.mat).or_default().push(t.v);
    }
    let silhouette = silhouette_area(&dag.vs, &tris);
    drop(tris);
    let per_mat: Vec<(u32, Vec<[u32; 3]>, Vec<Vec<u32>>)> = by_mat
        .into_par_iter()
        .map(|(mat, ts)| {
            let parts = cluster_tris(&dag.vs, &ts);
            (mat, ts, parts)
        })
        .collect();
    let mut cur: Vec<u32> = Vec::new();
    for (mat, ts, parts) in per_mat {
        for cl in parts {
            let tris: Vec<[u32; 3]> = cl.iter().map(|&i| ts[i as usize]).collect();
            let sphere = bounding_sphere(
                tris.iter()
                    .flat_map(|t| t.iter().map(|&v| dag.vs.pos[v as usize])),
            );
            cur.push(dag.clusters.len() as u32);
            dag.clusters.push(ClusterNode {
                tris,
                mat,
                sphere,
                parent_sphere: sphere,
                self_error: 0.0,
                parent_error: f32::INFINITY,
                level: 0,
                producer: NONE,
                consumer: NONE,
                refs: Vec::new(),
            });
        }
    }

    // ── levels ─────────────────────────────────────────────────────────────
    // Largest group whose streams are guaranteed to fit one page (worst case:
    // 128 verts/cluster, 12 B position, 3x u16 indices per triangle).
    let group_cap = {
        let per_cluster = 64 + 128 * 12 + 128 * mesh.attribute_stride + 128 * 6;
        ((PAGE_BYTES as usize - 1024) * 9 / 10 / per_cluster).clamp(8, MAX_GROUP_CLUSTERS)
    };
    let mut level = 0u32;
    while cur.len() > 1 {
        let groups = partition(&dag.vs, &cur, &dag.clusters, group_cap);
        // A group that covers everything left has no interior boundary to
        // protect, so it can halve like a monolithic simplification.
        let keep = if groups.len() == 1 {
            (1, 2)
        } else {
            LEVEL_KEEP
        };
        let whole = groups.len() == 1;
        let base0 = dag.vs.len() as u32;
        // Error budget per group: a group that would raise the error far above
        // what its level typically needs keeps more triangles instead (see
        // `simplify_group`), so one hard region cannot pin every ancestor.
        let child_max: Vec<f32> = groups
            .iter()
            .map(|g| {
                g.iter()
                    .map(|&c| dag.clusters[c as usize].self_error)
                    .fold(0.0f32, f32::max)
            })
            .collect();
        let typical = {
            let mut v: Vec<f32> = child_max.iter().copied().filter(|&e| e > 0.0).collect();
            v.sort_by(f32::total_cmp);
            v.get(v.len() / 2).copied().unwrap_or(0.0)
        };
        let global = (groups.len() > 1).then(|| {
            GlobalEdges::build(
                &dag.vs,
                cur.iter()
                    .flat_map(|&c| dag.clusters[c as usize].tris.iter()),
            )
        });
        let results: Vec<SimplifyResult> = groups
            .par_iter()
            .zip(child_max.par_iter())
            .map(|(g, &child)| {
                let tris: Vec<Tri> = g
                    .iter()
                    .flat_map(|&c| {
                        let cn = &dag.clusters[c as usize];
                        cn.tris.iter().map(move |t| Tri { v: *t, mat: cn.mat })
                    })
                    .collect();
                let mean_edge = {
                    let mut sum = 0.0f64;
                    for t in &tris {
                        let p = t.v.map(|v| dag.vs.pos[v as usize]);
                        sum += (dist(p[0], p[1]) + dist(p[1], p[2]) + dist(p[2], p[0])) as f64;
                    }
                    (sum / (3 * tris.len().max(1)) as f64) as f32
                };
                // A single group covers the whole remaining mesh: no neighbour to
                // protect and nothing to compare against, so no budget feedback.
                let budget = if whole {
                    f32::INFINITY
                } else {
                    (child * ERR_GROWTH)
                        .max(typical * ERR_GROWTH_TYPICAL)
                        .max(BUDGET_EDGE_FRACTION * mean_edge)
                };
                simplify_group(
                    &dag.vs,
                    &tris,
                    tris.len() * keep.0 / keep.1,
                    budget,
                    whole,
                    global.as_ref(),
                )
            })
            .collect();

        // Serial: allocate the moved vertices (ids must not depend on thread
        // scheduling) and translate output corners to global ids.
        struct Staged {
            tris: Vec<([u32; 3], u32)>,
            coverage: Option<(usize, f32)>,
        }
        let results_lock = results.iter().fold((0usize, 0usize, 0usize), |a, r| {
            (a.0 + r.locked, a.1 + r.verts, a.2 + r.floating)
        });
        let mut staged: Vec<Staged> = Vec::with_capacity(results.len());
        for res in results {
            let coverage = res.coverage_tris;
            let ids: Vec<u32> = res
                .new_verts
                .iter()
                .map(|&(p, a)| {
                    // Only merged foliage cards create vertices: they are floating
                    // pieces at the next level too.
                    let id = dag.vs.push(p, a);
                    dag.vs.floating[id as usize] = true;
                    id
                })
                .collect();
            let tris = res
                .tris
                .iter()
                .map(|&(c, m)| {
                    let f = |x: u32| {
                        if x >= base0 {
                            ids[(x - base0) as usize]
                        } else {
                            x
                        }
                    };
                    ([f(c[0]), f(c[1]), f(c[2])], m)
                })
                .collect();
            staged.push(Staged { tris, coverage });
        }
        let parts: Vec<Vec<(Vec<[u32; 3]>, u32)>> = staged
            .par_iter()
            .map(|s| split_into_clusters(&dag.vs, &s.tris))
            .collect();
        let new_total: usize = parts.iter().map(|p| p.len()).sum();
        let old_tris: usize = cur
            .iter()
            .map(|&c| dag.clusters[c as usize].tris.len())
            .sum();
        let new_tris: usize = staged.iter().map(|s| s.tris.len()).sum();
        if std::env::var("VG_DBG").is_ok() {
            let (l, v, fl) = results_lock;
            eprintln!("DBG level {level} clusters {} groups {} old {old_tris} new {new_tris} locked {:.1}% floating {:.1}%", cur.len(), groups.len(), 100.0 * l as f64 / v.max(1) as f64, 100.0 * fl as f64 / old_tris.max(1) as f64);
        }
        if new_tris * 100 > old_tris * STALL_PERCENT || new_total == 0 {
            // Stop once a level removes less than 20% of the triangles. Cluster
            // counts can stay unchanged while triangles fall substantially
            // (material boundaries), so triangles, not clusters, decide. A
            // stalled level (locked group boundaries on a tiny remainder) is
            // never worth building: it adds depth and error but no detail
            // reduction, so the previous level becomes the root.
            break;
        }

        // Measure the true deviation of every group from the original surface.
        let measured: Vec<Measured> = groups
            .par_iter()
            .zip(parts.par_iter())
            .map(|(members, pieces)| {
                let mut refs: Vec<V3> = Vec::new();
                for &c in members {
                    let cn = &dag.clusters[c as usize];
                    if cn.level == 0 {
                        let mut ids: Vec<u32> = cn.tris.iter().flatten().copied().collect();
                        ids.sort_unstable();
                        ids.dedup();
                        refs.extend(ids.iter().map(|&v| dag.vs.pos[v as usize]));
                        // Triangle centroids too: vertices alone under-sample the
                        // surface between them (high-frequency scan noise).
                        for t in &cn.tris {
                            let p = t.map(|v| dag.vs.pos[v as usize]);
                            refs.push(scale(add(add(p[0], p[1]), p[2]), 1.0 / 3.0));
                        }
                    } else {
                        refs.extend_from_slice(&cn.refs);
                    }
                }
                let piece_tris: Vec<&[[u32; 3]]> = pieces.iter().map(|p| p.0.as_slice()).collect();
                let input: Vec<[V3; 3]> = members
                    .iter()
                    .flat_map(|&c| dag.clusters[c as usize].tris.iter())
                    .map(|t| t.map(|v| dag.vs.pos[v as usize]))
                    .collect();
                measure(&refs, &input, &piece_tris, &dag.vs.pos)
            })
            .collect();

        // Pixel width above which this whole level would put more than
        // `DENSITY_TARGET` triangles on each pixel of the asset's silhouette.
        let level_pixel = (DENSITY_TARGET * silhouette / old_tris.max(1) as f32).sqrt();
        let mut next: Vec<u32> = Vec::with_capacity(new_total);
        for (((members, st), pieces), meas) in
            groups.into_iter().zip(staged).zip(parts).zip(measured)
        {
            // Group sphere contains every child sphere (so containment is
            // monotone up the DAG) and every vertex the simplifier produced.
            let mut sphere = dag.clusters[members[0] as usize].sphere;
            let mut child_err = 0.0f32;
            for &c in &members {
                sphere = sphere_union(sphere, dag.clusters[c as usize].sphere);
                child_err = child_err.max(dag.clusters[c as usize].self_error);
            }
            for (t, _) in &st.tris {
                for &v in t {
                    let p = dag.vs.pos[v as usize];
                    sphere = sphere_union(sphere, [p[0], p[1], p[2], 0.0]);
                }
            }
            // `meas.error` is the measured distance of original-surface points
            // to this group's simplified surface, so it already includes all
            // deviation accumulated by earlier levels: take the max with the
            // children (monotone), never a sum. `st.error` (the simplifier's
            // own estimate) is deliberately not used.
            let floor = 1e-5 * sphere[3].max(1e-6);
            let mut error = match st.coverage {
                Some((n, foot)) => {
                    let spacing = sphere[3] * (4.18879 / n.max(1) as f32).cbrt();
                    child_err
                        .max(COVERAGE_FOOTPRINT * foot)
                        .max(COVERAGE_FACTOR * spacing)
                        .max(floor)
                }
                None => {
                    // Measured deviation, relaxed by at most `DENSITY_TOLERANCE_GAIN` while
                    // the inputs are over the target density (see the constants).
                    let geo = meas.error.max(meas.forward) * ERROR_MARGIN;
                    let (mut area, mut count) = (0.0f64, 0usize);
                    for &c in &members {
                        for t in &dag.clusters[c as usize].tris {
                            let p = t.map(|v| dag.vs.pos[v as usize]);
                            area += 0.5 * length(cross(sub(p[1], p[0]), sub(p[2], p[0]))) as f64;
                            count += 1;
                        }
                    }
                    let footprint = (area / count.max(1) as f64).sqrt() as f32;
                    let pixel = level_pixel.max(0.5 * footprint);
                    child_err
                        .max(geo.min(pixel.max(geo / DENSITY_TOLERANCE_GAIN)))
                        .max(floor)
                }
            };
            if error <= child_err {
                error = f32::from_bits(child_err.to_bits() + 1);
            }

            let gid = dag.groups.len() as u32;
            for &c in &members {
                let cn = &mut dag.clusters[c as usize];
                cn.parent_error = error;
                cn.parent_sphere = sphere;
                cn.consumer = gid;
                cn.refs = Vec::new();
            }
            let mut produced = Vec::new();
            for ((tris, mat), refs) in pieces.into_iter().zip(meas.piece_refs) {
                produced.push(dag.clusters.len() as u32);
                next.push(dag.clusters.len() as u32);
                dag.clusters.push(ClusterNode {
                    tris,
                    mat,
                    sphere,
                    parent_sphere: sphere,
                    self_error: error,
                    parent_error: f32::INFINITY,
                    level: level + 1,
                    producer: gid,
                    consumer: NONE,
                    refs,
                });
            }
            dag.groups.push(Group {
                clusters: members,
                level,
                sphere,
                error,
                is_root: false,
                produced,
            });
        }
        cur = next;
        level += 1;
    }

    // ── roots ──────────────────────────────────────────────────────────────
    for chunk in cur.chunks(ROOT_CHUNK) {
        let gid = dag.groups.len() as u32;
        let mut sphere = dag.clusters[chunk[0] as usize].sphere;
        for &c in chunk {
            sphere = sphere_union(sphere, dag.clusters[c as usize].sphere);
        }
        for &c in chunk {
            let cn = &mut dag.clusters[c as usize];
            cn.parent_error = f32::INFINITY;
            cn.parent_sphere = cn.sphere;
            cn.consumer = gid;
        }
        dag.groups.push(Group {
            clusters: chunk.to_vec(),
            level,
            sphere,
            error: f32::INFINITY,
            is_root: true,
            produced: Vec::new(),
        });
    }
    Ok(dag)
}
