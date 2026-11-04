//! Disconnected / open foliage geometry (needle cards, leaf cards, branch
//! fragments) must still build a real DAG with a coarse root.
mod common;

use common::*;
use helio_vg_build::*;

struct Rng(u64);
impl Rng {
    fn f(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 40) as f32) / (1u64 << 24) as f32
    }
}

fn attrs(p: &[[f32; 3]]) -> Vec<u8> {
    p.iter()
        .flat_map(|v| v.iter().flat_map(|f| f.to_le_bytes()))
        .collect()
}

/// `cards` disjoint 2-triangle quads of size `size` scattered in a ball of
/// radius `radius` (a canopy), plus a thin branch tube.
pub fn canopy(cards: usize, radius: f32, size: f32, seed: u64) -> TestMesh {
    let mut r = Rng(seed);
    let (mut p, mut idx) = (Vec::new(), Vec::new());
    for _ in 0..cards {
        let c = loop {
            let c = [r.f() * 2.0 - 1.0, r.f() * 2.0 - 1.0, r.f() * 2.0 - 1.0];
            if c[0] * c[0] + c[1] * c[1] + c[2] * c[2] <= 1.0 {
                break c.map(|x| x * radius);
            }
        };
        let u = [r.f() - 0.5, r.f() - 0.5, r.f() - 0.5];
        let w = [r.f() - 0.5, r.f() - 0.5, r.f() - 0.5];
        let b = p.len() as u32;
        for (s, t) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            p.push([
                c[0] + size * (s * u[0] + t * w[0]),
                c[1] + size * (s * u[1] + t * w[1]),
                c[2] + size * (s * u[2] + t * w[2]),
            ]);
        }
        idx.extend([b, b + 1, b + 2, b, b + 2, b + 3]);
    }
    // branch tube along y
    let (seg, ring) = (200u32, 8u32);
    let b0 = p.len() as u32;
    for i in 0..=seg {
        for k in 0..ring {
            let a = std::f32::consts::TAU * k as f32 / ring as f32;
            p.push([
                0.02 * radius * a.cos(),
                -radius + 2.0 * radius * i as f32 / seg as f32,
                0.02 * radius * a.sin(),
            ]);
        }
    }
    for i in 0..seg {
        for k in 0..ring {
            let (a, b) = (b0 + i * ring + k, b0 + i * ring + (k + 1) % ring);
            idx.extend([a, b, a + ring, b, b + ring, a + ring]);
        }
    }
    let n = idx.len() / 3;
    let attributes = attrs(&p);
    TestMesh {
        positions: p,
        attributes,
        stride: 12,
        indices: idx,
        mats: vec![0; n],
    }
}

/// Triangles selected at a distance for an object of radius `rad`.
pub fn selected(r: &VgeoReader, d: f32, rad: f32) -> usize {
    let scale = 1080.0 / (2.0 * (60f32.to_radians() * 0.5).tan());
    let proj = Projection {
        eye: [rad + d, 0.0, 0.0],
        scale,
    };
    r.select_clusters(1.0, &proj)
        .unwrap()
        .iter()
        .map(|s| s.cluster.tri_count() as usize)
        .sum()
}

#[test]
fn disjoint_canopy_builds_coarse_root() {
    let mesh = canopy(30_000, 3.0, 0.05, 7);
    let dag = build_dag(&mesh.input()).unwrap();
    let leaf: usize = dag
        .clusters
        .iter()
        .filter(|c| c.level == 0)
        .map(|c| c.tris.len())
        .sum();
    let roots: usize = dag
        .groups
        .iter()
        .filter(|g| g.is_root)
        .flat_map(|g| g.clusters.iter())
        .map(|&c| dag.clusters[c as usize].tris.len())
        .sum();
    println!("leaf tris {leaf} root tris {roots}");
    assert!(roots * 20 < leaf, "root {roots} of {leaf}");
}

#[test]
fn disjoint_canopy_selection_tracks_screen_size() {
    let mesh = canopy(30_000, 3.0, 0.05, 7);
    let bytes = build_vgeo(&mesh.input(), &BuildOptions::default()).unwrap();
    let r = VgeoReader::parse(&bytes).unwrap();
    let s: Vec<usize> = [30.0, 240.0, 960.0, 3840.0, 15360.0]
        .iter()
        .map(|&d| selected(&r, d, 3.0))
        .collect();
    println!("selected {s:?} of {}", mesh.tri_count());
    for w in s.windows(2) {
        assert!(w[1] <= w[0], "{s:?}");
    }
    assert!(
        s[4] * 20 < s[0],
        "selection must collapse with distance: {s:?}"
    );
    // Sparse foliage must already coarsen at the second sample. Applying
    // point-to-surface distance as its coverage error retained 62,090 of
    // 63,200 triangles here; the coverage metric selects about 15,763.
    // Keep margin for partition changes while rejecting that regression.
    assert!(
        s[1] < mesh.tri_count() / 2,
        "mid-distance foliage failed to coarsen: {s:?}"
    );
    assert!(s[2] < mesh.tri_count() / 2, "{s:?}");
}

// ── real assets (cached .vgeo leaves re-fed to the builder) ──────────────────

fn cache_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/assets/vg_stress/cache")
}

/// Rebuild the full-resolution mesh from the leaf clusters of a cooked asset.
fn leaf_mesh(bytes: &[u8]) -> TestMesh {
    let r = VgeoReader::parse(bytes).unwrap();
    let stride = r.meta.attribute_stride as usize;
    let (mut positions, mut attributes, mut indices, mut mats) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for pid in 0..r.page_dir.len() as u32 {
        let page = r.page(pid).unwrap();
        for i in 0..page.header.cluster_count {
            let cl = page.cluster(i).unwrap();
            if cl.group_id != helio_vg_format::NODE_INVALID {
                continue;
            }
            let d = page.decode_cluster(i).unwrap();
            let base = positions.len() as u32;
            positions.extend_from_slice(&d.positions);
            attributes.extend_from_slice(d.attributes);
            for t in &d.tris {
                indices.extend(t.map(|x| base + x as u32));
                mats.push(cl.counts_material >> 16);
            }
        }
    }
    TestMesh {
        positions,
        attributes,
        stride,
        indices,
        mats,
    }
}

#[test]
#[ignore = "reads the cached vg_stress assets; run with --release -- --ignored --nocapture"]
fn real_assets_leaf_stats() {
    let Ok(rd) = std::fs::read_dir(cache_dir()) else {
        return;
    };
    let only = std::env::var("VG_ONLY").ok();
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|s| s.to_str()) != Some("vgeo") {
            continue;
        }
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        if only.as_deref().is_some_and(|o| !name.starts_with(o)) {
            continue;
        }
        let bytes = std::fs::read(&p).unwrap();
        let Ok(r) = VgeoReader::parse(&bytes) else {
            continue;
        };
        let tris = r.header.asset.triangle_count;
        if tris < 100_000 {
            continue;
        }
        let m = leaf_mesh(&bytes);
        // connected components (by position) and open-edge share
        let mut parent: Vec<u32> = (0..m.positions.len() as u32).collect();
        fn find(p: &mut Vec<u32>, mut x: u32) -> u32 {
            while p[x as usize] != x {
                p[x as usize] = p[p[x as usize] as usize];
                x = p[x as usize];
            }
            x
        }
        let mut pm = std::collections::HashMap::new();
        let canon_v: Vec<u32> = m
            .positions
            .iter()
            .enumerate()
            .map(|(i, q)| *pm.entry(q.map(f32::to_bits)).or_insert(i as u32))
            .collect();
        for t in m.indices.chunks(3) {
            let a = find(&mut parent, canon_v[t[0] as usize]);
            for k in 1..3 {
                let b = find(&mut parent, canon_v[t[k] as usize]);
                if a != b {
                    parent[b as usize] = a;
                }
            }
        }
        let mut comp = std::collections::HashMap::new();
        for t in m.indices.chunks(3) {
            *comp
                .entry(find(&mut parent, canon_v[t[0] as usize]))
                .or_insert(0usize) += 1;
        }
        let mut sizes: Vec<usize> = comp.values().copied().collect();
        sizes.sort_unstable();
        let mut edges = std::collections::HashMap::new();
        for t in m.indices.chunks(3) {
            for k in 0..3 {
                let (a, b) = (canon_v[t[k] as usize], canon_v[t[(k + 1) % 3] as usize]);
                *edges.entry((a.min(b), a.max(b))).or_insert(0u32) += 1;
            }
        }
        let uniq = canon_v
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len();
        println!("  unique positions {uniq} of {} verts", m.positions.len());
        let open = edges.values().filter(|&&c| c == 1).count();
        let nm = edges.values().filter(|&&c| c > 2).count();
        println!(
            "{name}: tris {tris} verts {} comps {} median {} p90 {} max {} open-edges {:.1}% nonmanifold {} mats {}",
            m.positions.len(),
            sizes.len(),
            sizes[sizes.len() / 2],
            sizes[sizes.len() * 9 / 10],
            sizes.last().unwrap(),
            100.0 * open as f64 / edges.len() as f64,
            nm,
            m.mats.iter().collect::<std::collections::BTreeSet<_>>().len()
        );
    }
}

/// Triangles selected at 20/50/100 m (tau 1 px, 1080p, 60 deg) for a built asset.
fn cut_sizes(r: &VgeoReader) -> [usize; 3] {
    let root = &r.nodes[r.header.asset.root_node as usize];
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for i in 0..4 {
        if root.children[i] == helio_vg_format::NODE_INVALID {
            continue;
        }
        let s = root.spheres[i];
        for a in 0..3 {
            lo[a] = lo[a].min(s[a] - s[3]);
            hi[a] = hi[a].max(s[a] + s[3]);
        }
    }
    let c = [0, 1, 2].map(|a| (lo[a] + hi[a]) / 2.0);
    let rad = ((0..3).map(|a| (hi[a] - lo[a]).powi(2)).sum::<f32>()).sqrt() / 2.0;
    let scale = 1080.0 / (2.0 * (60f32.to_radians() * 0.5).tan());
    [20.0f32, 50.0, 100.0].map(|d| {
        let proj = Projection {
            eye: [c[0] + rad + d, c[1], c[2]],
            scale,
        };
        r.select_clusters(1.0, &proj)
            .unwrap()
            .iter()
            .map(|s| s.cluster.tri_count() as usize)
            .sum()
    })
}

#[test]
#[ignore = "rebuilds cached vg_stress assets; VG_ONLY=<hash prefix> to pick one; run with --release -- --ignored --nocapture"]
fn real_assets_rebuild_and_reduce() {
    let Ok(rd) = std::fs::read_dir(cache_dir()) else {
        return;
    };
    let only = std::env::var("VG_ONLY").ok();
    let mut files: Vec<_> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("vgeo"))
        .collect();
    files.sort();
    for p in files {
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        if only.as_deref().is_some_and(|o| !name.starts_with(o)) {
            continue;
        }
        let bytes = std::fs::read(&p).unwrap();
        let Ok(r) = VgeoReader::parse(&bytes) else {
            continue;
        };
        let tris = r.header.asset.triangle_count;
        if tris < 300_000 || (only.is_none() && tris > 8_000_000) {
            continue;
        }
        let before = cut_sizes(&r);
        let m = leaf_mesh(&bytes);
        let t0 = std::time::Instant::now();
        let out = build_vgeo(&m.input(), &BuildOptions::default()).unwrap();
        let secs = t0.elapsed().as_secs_f32();
        let r2 = VgeoReader::parse(&out).unwrap();
        let after = cut_sizes(&r2);
        println!(
            "{name}: {tris} tris, built {secs:.1}s, 20/50/100 m before {before:?} after {after:?}"
        );
        if only.is_none() {
            assert!(
                after[2] * 10 <= after[0].max(1) * 10 && after[2] < tris as usize / 4,
                "{name} did not reduce: {after:?}"
            );
        }
    }
}

#[test]
#[ignore = "diagnostic: per-level sphere/error stats of a rebuilt cached asset (VG_ONLY=prefix)"]
fn real_asset_level_stats() {
    let Ok(rd) = std::fs::read_dir(cache_dir()) else {
        return;
    };
    let only = std::env::var("VG_ONLY").unwrap_or_default();
    for p in rd.flatten().map(|e| e.path()) {
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        if !name.ends_with(".vgeo") || !name.starts_with(&only) {
            continue;
        }
        let m = leaf_mesh(&std::fs::read(&p).unwrap());
        let dag = build_dag(&m.input()).unwrap();
        let top = dag.clusters.iter().map(|c| c.level).max().unwrap();
        for lv in 0..=top {
            let cs: Vec<_> = dag.clusters.iter().filter(|c| c.level == lv).collect();
            let n = cs.len() as f32;
            let tris: usize = cs.iter().map(|c| c.tris.len()).sum();
            let r: f32 = cs.iter().map(|c| c.sphere[3]).sum::<f32>() / n;
            let e: f32 = cs.iter().map(|c| c.self_error).sum::<f32>() / n;
            let pr: f32 = cs.iter().map(|c| c.parent_sphere[3]).sum::<f32>() / n;
            println!("{name} L{lv}: clusters {} tris {tris} mean r {r:.3} parent r {pr:.3} mean err {e:.4}", cs.len());
        }
        break;
    }
}

#[test]
fn root_set_is_page_bounded() {
    let mesh = canopy(30_000, 3.0, 0.05, 7);
    let bytes = build_vgeo(
        &mesh.input(),
        &BuildOptions {
            content_id: [1, 2],
            resident_levels: 64,
        },
    )
    .unwrap();
    let r = VgeoReader::parse(&bytes).unwrap();
    assert!(
        r.header.asset.root_page_count <= MAX_ROOT_PAGES,
        "{}",
        r.header.asset.root_page_count
    );
    assert!(
        r.header.asset.root_page_count < r.header.asset.page_count
            || r.header.asset.page_count <= MAX_ROOT_PAGES
    );
}

#[test]
fn material_sections_are_never_merged() {
    let mut mesh = canopy(6_000, 3.0, 0.05, 11);
    // alternate material per card (2 triangles per card; the tube is material 2)
    let cards = 6_000 * 2;
    for (i, m) in mesh.mats.iter_mut().enumerate() {
        *m = if i < cards { (i / 2 % 2) as u32 } else { 2 };
    }
    let dag = build_dag(&mesh.input()).unwrap();
    assert!(dag.clusters.iter().all(|c| c.tris.len() <= 128));
    let mats: std::collections::BTreeSet<u32> = dag.clusters.iter().map(|c| c.mat).collect();
    assert_eq!(mats.into_iter().collect::<Vec<_>>(), vec![0, 1, 2]);
    // every non-leaf cluster's triangles come from vertices of its own material
    let mut vmat = std::collections::HashMap::new();
    for c in dag.clusters.iter().filter(|c| c.level == 0) {
        for t in &c.tris {
            for &v in t {
                assert_eq!(*vmat.entry(v).or_insert(c.mat), c.mat);
            }
        }
    }
    for c in dag.clusters.iter().filter(|c| c.level > 0) {
        for t in &c.tris {
            for &v in t {
                if let Some(&m) = vmat.get(&v) {
                    assert_eq!(m, c.mat, "vertex {v} crossed material sections");
                }
            }
        }
    }
}

#[test]
fn connected_open_surface_keeps_its_border() {
    // A big open grid is not a floating piece: its open border stays fixed.
    let mesh = grid(60);
    let bytes = build_vgeo(&mesh.input(), &BuildOptions::default()).unwrap();
    let r = VgeoReader::parse(&bytes).unwrap();
    let src: Vec<[Lat; 3]> = mesh
        .indices
        .chunks(3)
        .map(|t| [0, 1, 2].map(|k| quantize(mesh.positions[t[k] as usize], &r.meta)))
        .collect();
    let (_, cut) = cut_tris(
        &r,
        1e30,
        &Projection {
            eye: [0.5, 5.0, 0.5],
            scale: 1000.0,
        },
    );
    assert_eq!(open_edges(&cut), open_edges(&src));
}

#[test]
#[ignore = "recooks cached vg_stress assets whose root is the whole mesh, in place, same content id (VG_RECOOK=1); run with --release"]
fn recook_non_reducing_caches() {
    if std::env::var("VG_RECOOK").is_err() {
        return;
    }
    let Ok(rd) = std::fs::read_dir(cache_dir()) else {
        return;
    };
    let mut files: Vec<_> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("vgeo"))
        .collect();
    files.sort();
    for p in files {
        let bytes = std::fs::read(&p).unwrap();
        let Ok(r) = VgeoReader::parse(&bytes) else {
            continue;
        };
        let a = r.header.asset;
        let rev = r.header.asset.flags >> 16;
        let healthy =
            a.root_page_count < a.page_count.min(64) && a.root_page_count <= MAX_ROOT_PAGES;
        // Older healthy cooks are left alone (the loader refreshes them on demand);
        // unbounded roots and caches of an earlier build of this revision are redone.
        if rev > COOK_REVISION
            || (healthy && rev != COOK_REVISION)
            || (healthy && std::env::var("VG_RECOOK_ONLY_BAD").is_ok())
        {
            continue;
        }
        let m = leaf_mesh(&bytes);
        let t0 = std::time::Instant::now();
        let out = build_vgeo(
            &m.input(),
            &BuildOptions {
                content_id: r.header.content_id,
                resident_levels: 2,
            },
        );
        let secs = t0.elapsed().as_secs_f32();
        match out {
            Ok(out) => {
                let r2 = VgeoReader::parse(&out).unwrap();
                let after = cut_sizes(&r2);
                let before = cut_sizes(&r);
                println!(
                    "{}: {} tris, root pages {}/{} -> {}/{}, 20/50/100 m {:?} -> {:?}, {secs:.1}s",
                    p.file_name().unwrap().to_string_lossy(),
                    a.triangle_count,
                    a.root_page_count,
                    a.page_count,
                    r2.header.asset.root_page_count,
                    r2.header.asset.page_count,
                    before,
                    after
                );
                let tmp = p.with_extension("vgeo.tmp");
                std::fs::write(&tmp, &out).unwrap();
                std::fs::rename(&tmp, &p).unwrap();
            }
            Err(e) => println!("{}: build refused: {e}", p.display()),
        }
    }
}
