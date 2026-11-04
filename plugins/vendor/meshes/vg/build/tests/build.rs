mod common;

use std::collections::HashSet;

use common::*;
use helio_vg_build::*;
use helio_vg_format::*;

fn opts() -> BuildOptions {
    BuildOptions {
        content_id: [0x1234, 0x5678],
        resident_levels: 2,
    }
}

#[test]
fn cook_revision_invalidates_old_derived_data() {
    let mesh = icosphere(1);
    let bytes = build_vgeo(&mesh.input(), &opts()).unwrap();
    let reader = VgeoReader::parse(&bytes).unwrap();
    assert!(is_current_cook(&reader.header));
    let mut old = reader.header;
    old.asset.flags = 0;
    assert!(!is_current_cook(&old));
    old.asset.flags = (COOK_REVISION + 1) << 16;
    assert!(!is_current_cook(&old));
}

#[test]
fn non_finite_positions_are_rejected_before_native_simplification() {
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut mesh = icosphere(1);
        mesh.positions[0][0] = value;
        assert_eq!(
            build_vgeo(&mesh.input(), &opts()).unwrap_err(),
            BuildError::NonFinitePosition
        );
    }
}

#[test]
fn distant_sphere_has_bounded_density_and_sampled_surface_error() {
    let mesh = icosphere(5);
    let bytes = build_vgeo(&mesh.input(), &opts()).unwrap();
    let reader = VgeoReader::parse(&bytes).unwrap();
    let focal = 720.0 / (2.0 * (45.0f32.to_radians() * 0.5).tan());
    for axis in 0..3 {
        let mut counts = Vec::new();
        for distance in [3.0, 6.0, 12.0, 24.0, 48.0, 96.0] {
            let mut eye = [0.0; 3];
            eye[axis] = distance;
            let cut = reader
                .select_clusters(1.0, &Projection { eye, scale: focal })
                .unwrap();
            let mut triangles = 0;
            let mut deviation = 0.0f32;
            for selected in cut {
                let decoded = reader
                    .page(selected.page_id)
                    .unwrap()
                    .decode_cluster(selected.cluster_index)
                    .unwrap();
                triangles += decoded.tris.len();
                for tri in decoded.tris {
                    let [a, b, c] = tri.map(|v| decoded.positions[v as usize]);
                    // Dense deterministic barycentric samples against the analytic
                    // unit sphere. This measures geometry independently of QEM.
                    for i in 0..=8 {
                        for j in 0..=8 - i {
                            let (u, v) = (i as f32 / 8.0, j as f32 / 8.0);
                            let p: [f32; 3] =
                                std::array::from_fn(|k| a[k] * (1.0 - u - v) + b[k] * u + c[k] * v);
                            let radius = p.iter().map(|x| x * x).sum::<f32>().sqrt();
                            let ideal: [f32; 3] = p.map(|x| x / radius);
                            let delta: [f32; 2] = std::array::from_fn(|k| {
                                let k = (axis + k + 1) % 3;
                                focal
                                    * (p[k] / (distance - p[axis])
                                        - ideal[k] / (distance - ideal[axis]))
                            });
                            deviation =
                                deviation.max((delta[0] * delta[0] + delta[1] * delta[1]).sqrt());
                        }
                    }
                }
            }
            let pixels = deviation;
            eprintln!("Sphere axis={axis} {distance}m: {triangles} triangles, sampled error {pixels:.3}px");
            assert!(
                pixels <= 1.0,
                "{distance}m: actual surface deviation exceeds 1px: {pixels}"
            );
            counts.push(triangles);
        }
        assert!(counts.windows(2).all(|w| w[1] <= w[0]), "{counts:?}");
        assert!(
            counts[5] <= 1024,
            "96m sphere must not stay dense: {counts:?}"
        );
    }
}

/// Position-id edge key set of triangles used exactly once ("boundary").
fn boundary_edges(vs: &VertexStore, tris: &[[u32; 3]]) -> HashSet<(u32, u32)> {
    let mut count = std::collections::HashMap::new();
    for t in tris {
        for e in 0..3 {
            let (a, b) = (vs.pid[t[e] as usize], vs.pid[t[(e + 1) % 3] as usize]);
            *count.entry((a.min(b), a.max(b))).or_insert(0u32) += 1;
        }
    }
    count
        .into_iter()
        .filter(|&(_, c)| c != 2)
        .map(|(k, _)| k)
        .collect()
}

fn check_dag(mesh: &TestMesh) {
    let dag = build_dag(&mesh.input()).unwrap();
    let vs = &dag.vs;

    // Cluster limits.
    for c in &dag.clusters {
        assert!(!c.tris.is_empty() && c.tris.len() <= CLUSTER_MAX_TRIS as usize);
        let mut v: Vec<u32> = c.tris.iter().flatten().copied().collect();
        v.sort_unstable();
        v.dedup();
        assert!(v.len() <= CLUSTER_MAX_VERTS as usize, "{} verts", v.len());
    }

    // Every source triangle appears exactly once at the leaf level.
    let key = |v: u32| pos_bits(vs.pos[v as usize]);
    let leaves: Vec<_> = dag
        .clusters
        .iter()
        .filter(|c| c.level == 0)
        .flat_map(|c| {
            c.tris
                .iter()
                .map(|t| canon([key(t[0]), key(t[1]), key(t[2])]))
        })
        .collect();
    let src: Vec<_> = mesh
        .indices
        .chunks(3)
        .map(|t| canon(tri3(t).map(|i| pos_bits(mesh.positions[i as usize]))))
        .collect();
    assert_eq!(multiset(leaves), multiset(src));

    // Error / bounds monotonicity and consumer bookkeeping.
    let mut consumed = vec![0u32; dag.clusters.len()];
    let mut max_level = 0;
    for (gi, g) in dag.groups.iter().enumerate() {
        for &c in &g.clusters {
            consumed[c as usize] += 1;
            let cn = &dag.clusters[c as usize];
            assert_eq!(cn.consumer, gi as u32);
            assert_eq!(cn.parent_error, g.error);
            if g.is_root {
                assert!(g.error.is_infinite());
            } else {
                assert!(
                    cn.parent_error > cn.self_error,
                    "parent must exceed child error"
                );
                assert!(sphere_contains(cn.parent_sphere, cn.sphere, 1e-4));
                assert!(g.clusters.len() <= GROUP_MAX_CLUSTERS_LARGE as usize);
            }
        }
        if g.is_root {
            continue;
        }
        let child_max = g
            .clusters
            .iter()
            .map(|&c| dag.clusters[c as usize].self_error)
            .fold(0.0, f32::max);
        for &p in &g.produced {
            let pn = &dag.clusters[p as usize];
            assert_eq!(pn.producer, gi as u32);
            assert_eq!(pn.self_error, g.error);
            assert!(pn.self_error > child_max);
            assert_eq!(pn.level, g.level + 1);
            for &c in &g.clusters {
                assert!(sphere_contains(
                    pn.sphere,
                    dag.clusters[c as usize].sphere,
                    1e-4
                ));
            }
        }
        max_level = max_level.max(g.level + 1);

        // Group boundary is preserved exactly by simplification (this is what
        // makes neighbouring groups crack-free).
        let input: Vec<[u32; 3]> = g
            .clusters
            .iter()
            .flat_map(|&c| dag.clusters[c as usize].tris.clone())
            .collect();
        let output: Vec<[u32; 3]> = g
            .produced
            .iter()
            .flat_map(|&c| dag.clusters[c as usize].tris.clone())
            .collect();
        assert_eq!(
            boundary_edges(vs, &input),
            boundary_edges(vs, &output),
            "group {gi} boundary changed"
        );
    }
    assert!(
        consumed.iter().all(|&c| c == 1),
        "every cluster consumed exactly once"
    );
    let roots: usize = dag
        .groups
        .iter()
        .filter(|g| g.is_root)
        .map(|g| g.clusters.len())
        .sum();
    println!(
        "{} tris: {} clusters, {} groups, {} levels, {} root clusters",
        dag.source_tris,
        dag.clusters.len(),
        dag.groups.len(),
        max_level + 1,
        roots
    );
    assert!(
        roots <= 12, // material-band borders stay locked: a few roots per band remain
        "DAG should converge to a tiny root set, got {roots}"
    );
}

fn tri3(t: &[u32]) -> [u32; 3] {
    [t[0], t[1], t[2]]
}

fn pos_bits(p: [f32; 3]) -> [u32; 3] {
    [
        (p[0] + 0.0).to_bits(),
        (p[1] + 0.0).to_bits(),
        (p[2] + 0.0).to_bits(),
    ]
}

#[test]
fn dag_invariants_sphere() {
    check_dag(&icosphere(4));
}

#[test]
fn dag_invariants_torus_seams_and_materials() {
    check_dag(&torus(64, 32));
}

#[test]
fn dag_invariants_open_grid() {
    check_dag(&grid(48));
}

/// The cut over a closed mesh must not open any edge the source did not have,
/// for any tau and any viewpoint (mixed-LOD cuts are the interesting ones).
fn crack_test(mesh: &TestMesh, eyes: &[[f32; 3]]) {
    let bytes = build_vgeo(&mesh.input(), &opts()).unwrap();
    let r = VgeoReader::parse(&bytes).unwrap();
    let src_tris: Vec<[Lat; 3]> = mesh
        .indices
        .chunks(3)
        .map(|t| tri3(t).map(|i| quantize(mesh.positions[i as usize], &r.meta)))
        .collect();
    let src_open = open_edges(&src_tris);

    for &eye in eyes {
        let proj = Projection { eye, scale: 1000.0 };
        let mut last_tris = usize::MAX;
        let mut distinct = HashSet::new();
        for tau in [0.0, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0, 64.0, 256.0, 1e30] {
            let (n, tris) = cut_tris(&r, tau, &proj);
            // Triangles, not clusters: a coarser level may split into more
            // (smaller, per-material) clusters while holding fewer triangles.
            assert!(
                n > 0 && tris.len() <= last_tris,
                "cut must shrink monotonically ({} tris after {last_tris})",
                tris.len()
            );
            last_tris = tris.len();
            distinct.insert(n);
            let open = open_edges(&tris);
            assert_eq!(
                open,
                src_open,
                "eye {eye:?} tau {tau}: cut has {} open edges, source has {}",
                open.len(),
                src_open.len()
            );
            if tau == 0.0 {
                assert_eq!(
                    tris.len(),
                    mesh.tri_count(),
                    "tau 0 selects the full-resolution leaves"
                );
            }
        }
        assert!(
            distinct.len() >= 2, // eyes inside coarse bounding spheres legitimately stay at fine levels
            "eye {eye:?}: cuts should actually change with tau: {distinct:?}"
        );
    }
}

#[test]
fn crack_sphere() {
    crack_test(
        &icosphere(5),
        &[
            [3.0, 0.0, 0.0],
            [1.02, 0.0, 0.0],
            [0.3, 0.9, 0.2],
            [0.0, 12.0, 0.0],
        ],
    );
}

#[test]
fn crack_torus_with_uv_seams() {
    crack_test(
        &torus(96, 48),
        &[[0.0, 0.0, 0.0], [1.4, 0.3, 0.0], [5.0, 5.0, 5.0]],
    );
}

#[test]
fn crack_open_grid() {
    crack_test(
        &grid(64),
        &[[0.5, 0.5, 0.5], [0.0, 0.1, 0.0], [2.0, 3.0, 2.0]],
    );
}

#[test]
fn round_trip() {
    let mesh = torus(64, 32);
    let bytes = build_vgeo(&mesh.input(), &opts()).unwrap();
    let r = VgeoReader::parse(&bytes).unwrap();
    let a = r.header.asset;
    assert_eq!(r.header.content_id, [0x1234, 0x5678]);
    assert_eq!(a.triangle_count as usize, mesh.tri_count());
    assert_eq!(r.nodes.len(), a.node_count as usize);
    assert_eq!(r.group_refs.len(), a.group_count as usize);
    assert!(a.root_page_count >= 1 && a.root_page_count <= a.page_count);
    assert_eq!(
        (bytes.len() - r.header.pages_offset as usize) % PAGE_BYTES as usize,
        0
    );
    assert_eq!(r.header.pages_offset % 4096, 0);

    // Root level comes first: page 0 holds only the coarsest levels.
    let first = r.page_dir[0];
    let last = *r.page_dir.last().unwrap();
    assert!(first.min_level >= last.max_level || r.page_dir.len() == 1);

    let mut seen_clusters = 0;
    let mut leaf_tris = Vec::new();
    let mut src_attr_ok = true;
    for p in 0..a.page_count {
        let page = r.page(p).unwrap();
        for (i, c) in page.clusters().unwrap().into_iter().enumerate() {
            seen_clusters += 1;
            assert!(c.tri_count() > 0 && c.tri_count() <= 128 && c.vert_count() <= 128);
            assert!(c.parent_error >= c.self_error);
            let d = page.decode_cluster(i as u32).unwrap();
            assert_eq!(d.tris.len(), c.tri_count() as usize);
            assert_eq!(d.lattice.len(), c.vert_count() as usize);
            assert!(d
                .tris
                .iter()
                .flatten()
                .all(|&v| (v as usize) < d.lattice.len()));
            // Quantization error is at most half a lattice step per axis.
            if c.flags & GpuCluster::FLAG_LEAF != 0 {
                for t in &d.tris {
                    leaf_tris.push(canon(t.map(|v| d.lattice[v as usize])));
                }
                // Attribute stream carries the source (u, v) of each vertex.
                src_attr_ok &= d.attributes.len() == d.lattice.len() * 8;
            }
            // `group_id` is the group that PRODUCED the cluster: its members are
            // the finer clusters this one replaces (same error as our self_error).
            if c.flags & GpuCluster::FLAG_LEAF != 0 {
                assert_eq!(c.group_id, NODE_INVALID);
            } else {
                let kids = r.group_clusters(c.group_id).unwrap();
                assert!(!kids.is_empty());
                assert!(
                    kids.iter().all(|k| k.parent_error == c.self_error),
                    "children of the producer share our self_error"
                );
            }
        }
    }
    assert!(src_attr_ok);
    assert_eq!(seen_clusters, a.cluster_count);

    let src: Vec<_> = mesh
        .indices
        .chunks(3)
        .map(|t| canon(tri3(t).map(|i| quantize(mesh.positions[i as usize], &r.meta))))
        .collect();
    assert_eq!(multiset(leaf_tris), multiset(src));

    // Every hierarchy leaf is a valid group ref and every group is reachable.
    let mut reach = HashSet::new();
    let mut stack = vec![0u32];
    while let Some(n) = stack.pop() {
        for i in 0..NODE_FANOUT {
            let c = r.nodes[n as usize].children[i];
            if c == NODE_INVALID {
                continue;
            }
            if c & NODE_LEAF_BIT != 0 {
                reach.insert(c & !NODE_LEAF_BIT);
            } else {
                stack.push(c);
            }
        }
    }
    assert_eq!(reach.len(), r.group_refs.len());

    // Parse rejects garbage.
    assert!(VgeoReader::parse(&bytes[..100]).is_err());
    let mut bad = bytes.clone();
    bad[0] = b'X';
    assert_eq!(VgeoReader::parse(&bad).err(), Some(ParseError::BadMagic));
}

#[test]
fn attributes_round_trip_by_value() {
    // Attribute = source position; decoded attribute must match the decoded
    // position to within quantization (proves vertex <-> attribute pairing).
    let mesh = icosphere(3);
    let bytes = build_vgeo(&mesh.input(), &opts()).unwrap();
    let r = VgeoReader::parse(&bytes).unwrap();
    for p in 0..r.header.asset.page_count {
        let page = r.page(p).unwrap();
        for (i, c) in page.clusters().unwrap().iter().enumerate() {
            if c.flags & GpuCluster::FLAG_LEAF == 0 {
                continue;
            }
            let d = page.decode_cluster(i as u32).unwrap();
            for (v, pos) in d.positions.iter().enumerate() {
                for a in 0..3 {
                    let f = f32::from_le_bytes(
                        d.attributes[v * 12 + a * 4..v * 12 + a * 4 + 4]
                            .try_into()
                            .unwrap(),
                    );
                    assert!((f - pos[a]).abs() <= r.meta.step, "attr/position mismatch");
                }
            }
        }
    }
}

#[test]
fn cone_is_conservative() {
    // For leaf clusters of a sphere: any camera that the cone culls must see
    // only back-facing triangles.
    let mesh = icosphere(4);
    let bytes = build_vgeo(&mesh.input(), &opts()).unwrap();
    let r = VgeoReader::parse(&bytes).unwrap();
    let eyes = [
        [4.0f32, 0.3, 0.1],
        [0.0, 3.0, 1.0],
        [-2.0, -2.0, 2.0],
        [0.0, 0.0, -8.0],
    ];
    let mut culled_any = false;
    for p in 0..r.header.asset.page_count {
        let page = r.page(p).unwrap();
        for (i, c) in page.clusters().unwrap().iter().enumerate() {
            let (axis, cutoff) = unpack_cone(c.cone);
            if cutoff >= 1.0 {
                continue;
            }
            let d = page.decode_cluster(i as u32).unwrap();
            for eye in eyes {
                let ctr = [c.sphere[0], c.sphere[1], c.sphere[2]];
                let v = [ctr[0] - eye[0], ctr[1] - eye[1], ctr[2] - eye[2]];
                let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                let dp = v[0] * axis[0] + v[1] * axis[1] + v[2] * axis[2];
                if helio_vg_format::cone_culled(eye, ctr, c.sphere[3], axis, cutoff) {
                    culled_any = true;
                    for t in &d.tris {
                        let [a, b, cc] = t.map(|k| d.positions[k as usize]);
                        let n = [
                            (b[1] - a[1]) * (cc[2] - a[2]) - (b[2] - a[2]) * (cc[1] - a[1]),
                            (b[2] - a[2]) * (cc[0] - a[0]) - (b[0] - a[0]) * (cc[2] - a[2]),
                            (b[0] - a[0]) * (cc[1] - a[1]) - (b[1] - a[1]) * (cc[0] - a[0]),
                        ];
                        let to = [a[0] - eye[0], a[1] - eye[1], a[2] - eye[2]];
                        assert!(
                            n[0] * to[0] + n[1] * to[1] + n[2] * to[2] >= -1e-6,
                            "culled front face"
                        );
                    }
                }
            }
        }
    }
    assert!(culled_any, "cone culling should trigger for a sphere");
}

#[test]
fn deterministic_across_runs_and_thread_counts() {
    let mesh = torus(80, 40);
    let a = build_vgeo(&mesh.input(), &opts()).unwrap();
    let b = build_vgeo(&mesh.input(), &opts()).unwrap();
    assert!(a == b, "two runs differ");
    let one = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let c = one.install(|| build_vgeo(&mesh.input(), &opts()).unwrap());
    assert!(a == c, "1-thread build differs");
    let many = rayon::ThreadPoolBuilder::new()
        .num_threads(7)
        .build()
        .unwrap();
    let d = many.install(|| build_vgeo(&mesh.input(), &opts()).unwrap());
    assert!(a == d, "7-thread build differs");
}

#[test]
fn errors_are_reported() {
    let mesh = icosphere(0);
    let mut bad = mesh.indices.clone();
    bad.pop();
    let m = MeshInput {
        indices: &bad,
        ..mesh.input()
    };
    assert_eq!(
        build_vgeo(&m, &opts()).err(),
        Some(BuildError::BadIndexCount)
    );
    let empty = MeshInput {
        indices: &[],
        tri_material: &[],
        ..mesh.input()
    };
    assert_eq!(
        build_vgeo(&empty, &opts()).err(),
        Some(BuildError::EmptyMesh)
    );
}

#[test]
fn tiny_mesh_single_cluster() {
    let mesh = icosphere(0);
    let bytes = build_vgeo(&mesh.input(), &opts()).unwrap();
    let r = VgeoReader::parse(&bytes).unwrap();
    assert_eq!(r.header.asset.cluster_count, 1);
    let sel = r
        .select_clusters(
            0.0,
            &Projection {
                eye: [5.0, 0.0, 0.0],
                scale: 1000.0,
            },
        )
        .unwrap();
    assert_eq!(sel.len(), 1);
    assert_eq!(
        sel[0].cluster.flags,
        GpuCluster::FLAG_LEAF | GpuCluster::FLAG_ROOT
    );
}

#[test]
#[ignore = "perf smoke: ~1M triangles; run with --release -- --ignored --nocapture"]
fn perf_one_million_triangles() {
    let mesh = grid(708); // 708^2 * 2 = 1,002,528 triangles
    let t = std::time::Instant::now();
    let dag = build_dag(&mesh.input()).unwrap();
    let t_dag = t.elapsed();
    let bytes = write_vgeo(&dag, &mesh.attributes, mesh.stride, &opts()).unwrap();
    println!(
        "{} tris -> {} clusters, dag {:?}, total {:?}, {} MiB",
        mesh.tri_count(),
        dag.clusters.len(),
        t_dag,
        t.elapsed(),
        bytes.len() >> 20
    );
    let r = VgeoReader::parse(&bytes).unwrap();
    let (n, tris) = cut_tris(
        &r,
        1.0,
        &Projection {
            eye: [0.5, 0.5, 0.5],
            scale: 1000.0,
        },
    );
    println!("cut at 1px: {n} clusters, {} tris", tris.len());
    assert!(r.header.asset.triangle_count as usize == mesh.tri_count());
}

#[test]
fn page_deps_cover_every_parent_page() {
    for mesh in [icosphere(4), torus(64, 32)] {
        let bytes = build_vgeo(&mesh.input(), &opts()).unwrap();
        let r = VgeoReader::parse(&bytes).unwrap();
        let mut nonempty = false;
        for q in 0..r.header.asset.page_count {
            for c in r.page(q).unwrap().clusters().unwrap() {
                if c.flags & GpuCluster::FLAG_LEAF != 0 {
                    continue;
                }
                // `c` replaces the members of its producer group, which live on
                // `child_page`: that page depends on the page holding `c`.
                let child_page = r.group_refs[c.group_id as usize].page_id;
                if child_page != q {
                    assert!(
                        r.page_deps[child_page as usize].contains(&q),
                        "page {child_page} must depend on {q}"
                    );
                    nonempty = true;
                }
            }
        }
        assert!(nonempty || r.header.asset.page_count == 1);
        // Dependencies point at coarser (lower-numbered) pages: roots stay a closed prefix.
        for (p, d) in r.page_deps.iter().enumerate() {
            assert!(d.iter().all(|&x| (x as usize) < p));
        }
    }
}
