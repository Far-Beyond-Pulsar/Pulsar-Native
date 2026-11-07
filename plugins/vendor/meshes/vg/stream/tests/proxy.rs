//! CPU proxy-LOD pipeline: consistent cuts from a real `.vgeo`, decoded
//! positions identical to the reader's, background cache and budget.

#[path = "../../build/tests/common/mod.rs"]
mod common;

use helio_vg_build::{build_vgeo, BuildOptions, VgeoReader};
use helio_vg_format::{ATTR_STRIDE, PAGE_BYTES, VIS_TRI_BITS, VIS_TRI_MASK};
use helio_vg_stream::*;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn asset(subdiv: u32) -> (Vec<u8>, usize) {
    let mesh = common::icosphere(subdiv);
    let bytes = build_vgeo(&mesh.input(), &BuildOptions::default()).expect("build");
    (bytes, mesh.tri_count())
}

/// Counts of how many triangles use each undirected edge, by lattice vertex id.
fn edge_uses(lod: &ProxyLodCpu) -> HashMap<(u32, u32), u32> {
    let mut m = HashMap::new();
    for t in lod.indices.chunks_exact(3) {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            *m.entry((a.min(b), a.max(b))).or_insert(0) += 1;
        }
    }
    m
}

#[test]
fn proxy_levels_are_consistent_cuts() {
    let (bytes, source_tris) = asset(5); // 20480 triangles
    let source = VgeoBytes::new(bytes).unwrap();
    let proxy = build_proxy(&source, &ProxyOptions::default(), None).expect("proxy");
    assert!(proxy.lods.len() >= 3, "only {} levels", proxy.lods.len());
    // Finest level is the original geometry, coarser ones strictly fewer
    // triangles and strictly larger error.
    assert_eq!(proxy.lods[0].triangle_count(), source_tris);
    assert_eq!(proxy.source_triangles as usize, source_tris);
    for w in proxy.lods.windows(2) {
        assert!(w[1].triangle_count() < w[0].triangle_count());
        assert!(w[1].error > w[0].error);
    }
    for (l, lod) in proxy.lods.iter().enumerate() {
        // A consistent cut of a closed manifold is itself closed: no edge has
        // more than two triangles and (almost) none has one. Cracks between
        // clusters of different depth would show up as open edges.
        let uses = edge_uses(lod);
        assert!(
            uses.values().all(|&n| n <= 2),
            "level {l}: non-manifold proxy edge"
        );
        let open = uses.values().filter(|&&n| n == 1).count();
        assert_eq!(
            open,
            0,
            "level {l}: {open} open edges (cracks) of {}",
            uses.len()
        );
        assert_eq!(lod.prim_map.len(), lod.triangle_count());
        assert_eq!(lod.clusters.len(), lod.cluster_pages.len());
        for &m in &lod.prim_map {
            assert!(((m >> VIS_TRI_BITS) as usize) < lod.clusters.len());
        }
        assert!(lod
            .indices
            .iter()
            .all(|&i| (i as usize) < lod.positions.len()));
        // Memory: welding keeps vertices near V = T/2 for a closed mesh.
        assert!(
            lod.positions.len() * 10 < lod.triangle_count() * 8,
            "level {l} is not welded"
        );
    }
    // The bounding sphere encloses the unit icosphere.
    assert!(
        proxy.bounds[3] >= 0.99 && proxy.bounds[3] < 1.5,
        "{:?}",
        proxy.bounds
    );
}

#[test]
fn proxy_positions_and_triangles_match_the_reader() {
    let (bytes, _) = asset(4);
    let reader = VgeoReader::parse(&bytes).unwrap();
    let source = VgeoBytes::new(bytes.clone()).unwrap();
    let proxy = build_proxy(&source, &ProxyOptions::default(), None).unwrap();
    for lod in &proxy.lods {
        // Every proxy triangle equals the reader's decode of its source
        // cluster triangle, bit for bit (what vis_resolve will decode).
        let mut checked = 0;
        for (p, &m) in lod.prim_map.iter().enumerate().step_by(7) {
            let ord = (m >> VIS_TRI_BITS) as usize;
            let tri = (m & VIS_TRI_MASK) as usize;
            let page = reader.page(lod.cluster_pages[ord]).unwrap();
            let d = page.decode_cluster(lod.clusters[ord]).unwrap();
            for k in 0..3 {
                let want = d.positions[d.tris[tri][k] as usize];
                let got = lod.positions[lod.indices[p * 3 + k] as usize];
                assert_eq!(want, got, "proxy triangle {p} vertex {k}");
            }
            checked += 1;
        }
        assert!(checked > 0);
    }
}

#[test]
fn proxy_triangle_cap_skips_the_fine_levels() {
    let (bytes, _) = asset(5);
    let source = VgeoBytes::new(bytes).unwrap();
    let opts = ProxyOptions {
        max_triangles_per_lod: 3000,
        ..ProxyOptions::default()
    };
    let proxy = build_proxy(&source, &opts, None).unwrap();
    assert!(proxy.skipped_levels > 0);
    assert!(proxy.lods.iter().all(|l| l.triangle_count() <= 3000));
    assert!(
        proxy.lods[0].error > 0.0,
        "the leaf level should have been skipped"
    );
}

/// The trace proxy without UVs: the test assets' attributes are their
/// positions, not the runtime's 16-byte records ([`ATTR_STRIDE`]).
fn untextured_trace() -> ProxyOptions {
    ProxyOptions {
        uvs: false,
        ..ProxyOptions::trace()
    }
}

/// f16 bits of `x` (normal numbers, truncated; zero as zero).
fn f16(x: f32) -> u32 {
    if x == 0.0 {
        return 0;
    }
    let b = x.to_bits();
    ((b >> 16) & 0x8000) | ((((b >> 23) & 0xff) - 112) << 10) | ((b >> 13) & 0x3ff)
}

/// The trace proxy's vertices keep their uv0 (decoded from the runtime's
/// attribute records): a grid whose uv0 is its position's x and z keeps
/// that pairing on every proxy vertex, coarse cut or not, on both sides of
/// a UV seam (the uv jumps along the middle column).
#[test]
fn trace_proxy_vertices_keep_their_uvs() {
    let mesh = common::grid(64);
    let n = 64;
    let mut positions = Vec::new();
    let mut attributes = Vec::new();
    let mut indices = Vec::new();
    // Two halves sharing the middle column's positions but not its UVs:
    // the right half's u starts again at 0.
    for (half, first) in [(0u32, 0u32), (1, n / 2)] {
        let base = (positions.len()) as u32;
        for j in 0..=n {
            for i in first..=first + n / 2 {
                let p = mesh.positions[(j * (n + 1) + i) as usize];
                positions.push(p);
                let u = if half == 0 { p[0] } else { p[0] - 0.5 };
                let mut record = [0u8; ATTR_STRIDE as usize];
                record[8..12].copy_from_slice(&(f16(u) | (f16(p[2]) << 16)).to_le_bytes());
                attributes.extend_from_slice(&record);
            }
        }
        let at = |i: u32, j: u32| base + j * (n / 2 + 1) + i;
        for j in 0..n {
            for i in 0..n / 2 {
                let (a, b, c, d) = (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
                indices.extend([a, c, b, a, d, c]);
            }
        }
    }
    let input = helio_vg_build::MeshInput {
        positions: &positions,
        attributes: &attributes,
        attribute_stride: ATTR_STRIDE as usize,
        indices: &indices,
        tri_material: &[],
    };
    let source = VgeoBytes::new(build_vgeo(&input, &BuildOptions::default()).unwrap()).unwrap();
    for opts in [
        ProxyOptions::trace(),
        ProxyOptions {
            finest_relative_error: 0.08,
            ..ProxyOptions::trace()
        },
    ] {
        let proxy = build_proxy(&source, &opts, None).unwrap();
        let lod = &proxy.lods[0];
        assert_eq!(lod.uvs.len(), lod.positions.len());
        let mut seam = 0;
        for (p, uv) in lod.positions.iter().zip(&lod.uvs) {
            let u = if (uv[0] - p[0]).abs() < 2e-3 {
                p[0]
            } else {
                p[0] - 0.5
            };
            assert!(
                (uv[0] - u).abs() < 2e-3 && (uv[1] - p[2]).abs() < 2e-3,
                "vertex at {p:?} has uv {uv:?}"
            );
            seam += usize::from((p[0] - 0.5).abs() < 1e-4);
        }
        eprintln!(
            "trace proxy at error {}: {} triangles, {} vertices, {seam} on the seam",
            lod.error,
            lod.triangle_count(),
            lod.positions.len()
        );
    }
}

/// The scene TLAS's proxy: one closed cut at 1 % of the bounding radius
/// (the unit sphere: about 0.01), far fewer triangles than the source, its
/// vertices on the surface within its error.
#[test]
fn trace_proxy_is_one_coarse_closed_cut_within_its_error() {
    let (bytes, source_tris) = asset(5);
    let source = VgeoBytes::new(bytes).unwrap();
    let proxy = build_proxy(&source, &untextured_trace(), None).unwrap();
    assert_eq!(proxy.lods.len(), 1);
    let lod = &proxy.lods[0];
    // 1 % of the radius, measured around the root cut's bounding box centre
    // (a few percent over the sphere's own radius).
    let bound = ProxyOptions::TRACE_RELATIVE_ERROR * proxy.bounds[3] * 1.05;
    assert!(
        lod.error > 0.0 && lod.error <= bound,
        "error {} over {bound}",
        lod.error
    );
    assert!(
        lod.triangle_count() * 4 < source_tris,
        "{} of {source_tris} triangles",
        lod.triangle_count()
    );
    assert!(lod.triangle_count() as u32 <= ProxyOptions::TRACE_MAX_TRIANGLES);
    let open = edge_uses(lod).values().filter(|&&n| n != 2).count();
    assert_eq!(open, 0, "{open} open or non-manifold edges");
    let off = lod
        .positions
        .iter()
        .map(|p| ((p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt() - 1.0).abs())
        .fold(0.0, f32::max);
    assert!(
        off <= 2.0 * lod.error + 1e-3,
        "a vertex {off} off the sphere (error {})",
        lod.error
    );
}

/// Over the triangle budget, the trace proxy takes the first coarser cut
/// that fits instead.
#[test]
fn trace_proxy_coarsens_to_fit_its_triangle_budget() {
    let (bytes, _) = asset(5);
    let source = VgeoBytes::new(bytes).unwrap();
    let fine = build_proxy(&source, &untextured_trace(), None).unwrap();
    let budget = fine.lods[0].triangle_count() as u32 / 3;
    let opts = ProxyOptions {
        max_triangles_per_lod: budget,
        ..untextured_trace()
    };
    let proxy = build_proxy(&source, &opts, None).unwrap();
    assert_eq!(proxy.lods.len(), 1);
    assert!(proxy.skipped_levels > 0);
    assert!(proxy.lods[0].triangle_count() as u32 <= budget);
    assert!(proxy.lods[0].error > fine.lods[0].error);
}

#[test]
fn proxy_pages_are_listed_and_bounded() {
    let (bytes, _) = asset(5);
    let source = VgeoBytes::new(bytes.clone()).unwrap();
    let pages = (bytes.len() as u32).div_ceil(PAGE_BYTES);
    let proxy = build_proxy(&source, &ProxyOptions::default(), None).unwrap();
    for lod in &proxy.lods {
        assert!(!lod.pages.is_empty());
        assert!(lod.pages.windows(2).all(|w| w[0] < w[1]));
        assert!(*lod.pages.last().unwrap() < pages);
        assert!(lod
            .cluster_pages
            .iter()
            .all(|p| lod.pages.binary_search(p).is_ok()));
    }
}

fn wait_ready(cache: &mut ProxyCache, want: usize) -> Vec<(u64, Arc<ProxyBuild>)> {
    let start = Instant::now();
    let mut out = Vec::new();
    let mut frame = 1;
    while out.len() < want {
        cache.poll(frame);
        out.extend(cache.take_ready());
        frame += 1;
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "proxy builds never finished"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    out
}

#[test]
fn cache_builds_in_the_background_and_evicts_to_budget() {
    let a: Arc<dyn VgeoSource> = Arc::new(VgeoBytes::new(asset(4).0).unwrap());
    let b: Arc<dyn VgeoSource> = Arc::new(VgeoBytes::new(asset(5).0).unwrap());
    let mut cache = ProxyCache::new(usize::MAX, ProxyOptions::default());
    cache.request(1, a.clone(), 1);
    cache.request(2, b.clone(), 1);
    cache.request(1, a.clone(), 1); // duplicate request is a no-op
                                    // The call returns immediately: nothing is built on this thread.
    assert!(matches!(cache.state(1), Some(ProxyState::Building)));
    assert_eq!(cache.stats().jobs_started, 0);
    let ready = wait_ready(&mut cache, 2);
    assert_eq!(ready.len(), 2);
    let stats = cache.stats();
    assert_eq!(stats.jobs_started, 2);
    assert_eq!(stats.entries, 2);
    let total = stats.bytes;
    assert!(total > 0);

    // Over budget: the least recently used, unprotected entry goes.
    cache.touch(2, 100, 0);
    cache.set_budget(ready.iter().find(|(k, _)| *k == 2).unwrap().1.bytes());
    let evicted = cache.poll(200);
    assert_eq!(evicted, vec![1]);
    assert_eq!(cache.stats().evictions, 1);
    assert!(cache.stats().bytes <= cache.stats().budget_bytes);

    // A protected entry is not evicted even when over budget.
    cache.touch(2, 300, 50);
    cache.set_budget(1);
    assert!(cache.poll(310).is_empty());
    assert_eq!(cache.poll(400), vec![2]);

    // A build bigger than the whole budget is rejected, not cached.
    let mut tiny = ProxyCache::new(64, ProxyOptions::default());
    tiny.request(9, a, 1);
    let start = Instant::now();
    while matches!(tiny.state(9), Some(ProxyState::Building)) {
        tiny.poll(1);
        assert!(start.elapsed() < Duration::from_secs(60));
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(matches!(tiny.state(9), Some(ProxyState::Failed(_))));
    assert_eq!(tiny.stats().rejected_over_budget, 1);
    assert!(tiny.take_ready().is_empty());
}

/// Diagnostic (`--ignored --nocapture`): selected triangles of the real
/// vg_stress assets at 20 / 50 / 100 m from the surface, by the CPU reference
/// cut (tau 1 px, 1080p, 60 degrees), against the asset's triangle count.
#[test]
#[ignore]
fn real_asset_cut_sizes() {
    let dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/assets/vg_stress/cache");
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return;
    };
    let scale = 1080.0 / (2.0 * (60f32.to_radians() * 0.5).tan());
    for e in rd.flatten() {
        let path = e.path();
        if path.extension().and_then(|s| s.to_str()) != Some("vgeo") {
            continue;
        }
        let bytes = std::fs::read(&path).unwrap();
        let Ok(r) = VgeoReader::parse(&bytes) else {
            continue;
        };
        let tris = r.header.asset.triangle_count;
        if tris < 500_000 {
            continue;
        }
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
        let c = [
            (lo[0] + hi[0]) / 2.0,
            (lo[1] + hi[1]) / 2.0,
            (lo[2] + hi[2]) / 2.0,
        ];
        let rad = ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2) + (hi[2] - lo[2]).powi(2))
            .sqrt()
            / 2.0;
        let mut line = format!(
            "{} tris {} extent {:.1}:",
            path.file_name().unwrap().to_string_lossy(),
            tris,
            rad * 2.0
        );
        for d in [20.0f32, 50.0, 100.0] {
            let proj = helio_vg_build::Projection {
                eye: [c[0] + rad + d, c[1], c[2]],
                scale,
            };
            let sel = r.select_clusters(1.0, &proj).unwrap();
            let t: u32 = sel.iter().map(|s| s.cluster.tri_count()).sum();
            line += &format!("  {d} m: {t} tris ({} clusters)", sel.len());
        }
        println!("{line}");
    }
}
