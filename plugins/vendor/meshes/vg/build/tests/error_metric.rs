//! Error-metric validation: stored DAG errors vs measured geometric deviation,
//! and screen-space triangle budget of the CPU cut.

mod measure;

use helio_vg_build::*;
use measure::*;

const FOCAL: f32 = 720.0 / (2.0 * 0.414_213_57); // 45 degree vertical fov, 720p

fn opts() -> BuildOptions {
    BuildOptions {
        content_id: [7, 9],
        resident_levels: 2,
    }
}

fn input<'a>(p: &'a [V3], attrs: &'a [u8], idx: &'a [u32]) -> MeshInput<'a> {
    MeshInput {
        positions: p,
        attributes: attrs,
        attribute_stride: 12,
        indices: idx,
        tri_material: &[],
    }
}

fn pos_attrs(p: &[V3]) -> Vec<u8> {
    p.iter()
        .flat_map(|v| v.iter().flat_map(|f| f.to_le_bytes()))
        .collect()
}

struct LevelReport {
    level: u32,
    tris: usize,
    stored: f32,
    dev_max: f32,
    dev_p99: f32,
    /// max over clusters of dev_c / error_c (must be <= 1 for a conservative metric).
    worst_violation: f32,
    /// max over clusters of error_c / dev_c (looseness).
    worst_loose: f32,
    /// mean log-ratio style tightness: geometric mean of error_c / dev_c.
    geo_mean: f32,
}

fn measure_dag(dag: &Dag, max_points: usize) -> Vec<LevelReport> {
    let tri = |t: &[u32; 3]| [0, 1, 2].map(|k| dag.vs.pos[t[k] as usize]);
    let top = dag.clusters.iter().map(|c| c.level).max().unwrap();
    let orig: Vec<_> = dag
        .clusters
        .iter()
        .filter(|c| c.level == 0)
        .flat_map(|c| c.tris.iter().map(tri))
        .collect();
    let original = Soup::new(orig);
    let mut out = Vec::new();
    for level in 1..=top {
        let cl: Vec<_> = dag.clusters.iter().filter(|c| c.level == level).collect();
        let tris: Vec<Vec<[V3; 3]>> = cl
            .iter()
            .map(|c| c.tris.iter().map(tri).collect())
            .collect();
        let errs: Vec<f32> = cl.iter().map(|c| c.self_error).collect();
        let dev = measure_level(&original, &tris, &errs, max_points);
        let (mut _viol, mut loose, mut logsum, mut n) = (0.0f32, 0.0f32, 0.0f64, 0usize);
        for (c, d) in cl.iter().zip(&dev.per_cluster) {
            if *d > 1e-6 {
                loose = loose.max(c.self_error / d);
                logsum += ((c.self_error / d) as f64).ln();
                n += 1;
            }
        }
        out.push(LevelReport {
            level,
            tris: cl.iter().map(|c| c.tris.len()).sum(),
            stored: cl.iter().map(|c| c.self_error).fold(0.0, f32::max),
            dev_max: dev.max,
            dev_p99: dev.p99,
            worst_violation: dev.worst_ratio,
            worst_loose: loose,
            geo_mean: if n > 0 {
                (logsum / n as f64).exp() as f32
            } else {
                0.0
            },
        });
    }
    out
}

fn print_reports(name: &str, reps: &[LevelReport]) {
    println!("== {name}: stored error vs measured deviation (object space)");
    println!("lvl      tris   stored     dev_max   dev_p99  stored/max  viol(dev/err)  loose(max)  geomean");
    for r in reps {
        println!(
            "{:>3} {:>9} {:>9.5} {:>10.5} {:>9.5} {:>10.2} {:>14.3} {:>11.2} {:>8.2}",
            r.level,
            r.tris,
            r.stored,
            r.dev_max,
            r.dev_p99,
            r.stored / r.dev_max.max(1e-12),
            r.worst_violation,
            r.worst_loose,
            r.geo_mean
        );
    }
}

/// CPU cut triangle counts at `px` on-screen height for an object of `diameter`.
fn cut_counts(reader: &VgeoReader, diameter: f32, center: V3, pixels: &[f32]) -> Vec<(f32, usize)> {
    pixels
        .iter()
        .map(|&px| {
            let d = FOCAL * diameter / px;
            let eye = [center[0], center[1], center[2] + d];
            let sel = reader
                .select_clusters(1.0, &Projection { eye, scale: FOCAL })
                .unwrap();
            let tris = sel
                .iter()
                .map(|s| {
                    reader
                        .page(s.page_id)
                        .unwrap()
                        .decode_cluster(s.cluster_index)
                        .unwrap()
                        .tris
                        .len()
                })
                .sum::<usize>();
            (px, tris)
        })
        .collect()
}

#[test]
#[ignore = "measurement harness: cargo test --release --test error_metric -- --ignored --nocapture"]
fn harness_noisy_sphere_million() {
    let (p, idx) = noisy_sphere(
        std::env::var("VG_SUBDIV")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(8),
    );
    let attrs = pos_attrs(&p);
    let t = std::time::Instant::now();
    let dag = build_dag(&input(&p, &attrs, &idx)).unwrap();
    println!("{} tris, dag {:?}", idx.len() / 3, t.elapsed());
    print_reports("noisy sphere", &measure_dag(&dag, 150_000));
    let bytes = write_vgeo(&dag, &attrs, 12, &opts()).unwrap();
    let r = VgeoReader::parse(&bytes).unwrap();
    for (px, n) in cut_counts(&r, 2.2, [0.0; 3], &[18.0, 100.0, 1000.0]) {
        println!("cut @ {px:>5} px: {n} triangles");
    }
}

fn small_sphere() -> (Vec<V3>, Vec<u8>, Vec<u32>) {
    let (p, idx) = noisy_sphere(6); // 81,920 triangles
    let attrs = pos_attrs(&p);
    (p, attrs, idx)
}

#[test]
fn stored_error_bounds_measured_deviation() {
    let (p, attrs, idx) = small_sphere();
    let dag = build_dag(&input(&p, &attrs, &idx)).unwrap();
    let reps = measure_dag(&dag, 60_000);
    print_reports("noisy sphere 82k", &reps);
    for r in &reps {
        // conservative with a small numeric slack for sampling vs. quantised metrics
        assert!(
            r.worst_violation <= 1.0,
            "level {}: deviation {} exceeds stored error",
            r.level,
            r.worst_violation
        );
        // honest: the level-wide stored error must be within a small factor of reality
        assert!(
            r.stored <= 2.0 * r.dev_max.max(1e-6),
            "level {}: stored error {} is more than 2x measured {}",
            r.level,
            r.stored,
            r.dev_max
        );
    }
}

#[test]
fn cut_triangles_track_screen_pixels() {
    let (p, idx) = noisy_sphere(7); // 327,680 triangles
    let attrs = pos_attrs(&p);
    let bytes = build_vgeo(&input(&p, &attrs, &idx), &opts()).unwrap();
    let r = VgeoReader::parse(&bytes).unwrap();
    let total = idx.len() / 3;
    let counts = cut_counts(&r, 2.2, [0.0; 3], &[18.0, 100.0, 1000.0]);
    println!("cut counts: {counts:?}");
    for (px, n) in &counts {
        // Both hemispheres are selected, so the surface has ~2 * (pi/4 px^2) px of area.
        let px_area = 2.0 * std::f32::consts::FRAC_PI_4 * px * px;
        let budget = ((px_area / 2.0) * 2.0).max(300.0) as usize;
        assert!(
            *n <= budget.min(total),
            "{px}px selected {n}, budget {budget}"
        );
    }
    assert!(
        counts[0].1 < counts[1].1 && counts[1].1 < counts[2].1,
        "{counts:?}"
    );
    assert!(
        counts[0].1 <= 2000,
        "18px object must be a few thousand triangles at most: {counts:?}"
    );
}

/// Cut counts for existing `.vgeo` caches (read-only). Set `VG_CACHE_DIR`.
#[test]
#[ignore = "measurement harness over local Stanford caches"]
fn harness_caches() {
    let dir = std::env::var("VG_CACHE_DIR")
        .unwrap_or_else(|_| "../examples/assets/vg_stress/cache".into());
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().map_or(false, |e| e == "vgeo"))
        .collect();
    files.sort();
    for f in files {
        let bytes = std::fs::read(&f).unwrap();
        report_vgeo(&f.file_name().unwrap().to_string_lossy(), &bytes);
    }
}

fn report_vgeo(name: &str, bytes: &[u8]) {
    let r = VgeoReader::parse(bytes).unwrap();
    let roots = r
        .select_clusters(
            1e30,
            &Projection {
                eye: [0.0; 3],
                scale: 1.0,
            },
        )
        .unwrap();
    let mut sphere = roots[0].cluster.sphere;
    let mut root_err = 0.0f32;
    let mut root_tris = 0;
    for s in &roots {
        let c = s.cluster;
        sphere = [sphere[0], sphere[1], sphere[2], sphere[3]];
        root_err = root_err.max(c.self_error);
        root_tris += r.decode(s).unwrap().tris.len();
    }
    // Root group sphere (containing every root cluster).
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for s in &roots {
        let c = s.cluster.sphere;
        for k in 0..3 {
            lo[k] = lo[k].min(c[k] - c[3]);
            hi[k] = hi[k].max(c[k] + c[3]);
        }
    }
    let center = [0, 1, 2].map(|k| (lo[k] + hi[k]) * 0.5);
    let diameter = (0..3).map(|k| hi[k] - lo[k]).fold(0.0f32, f32::max);
    let _ = &mut sphere;
    println!(
        "{name}: cook rev {} tris {} roots {} root_tris {} max root error {:.5} (x{:.3} for 2m tall)",
        bytes_flags(r.header.asset.flags),
        r.header.asset.triangle_count,
        roots.len(),
        root_tris,
        root_err,
        2.0 / diameter * root_err
    );
    for (px, n) in cut_counts(&r, diameter, center, &[18.0, 100.0, 1000.0]) {
        println!("    @{px:>5} px: {n} triangles");
    }
}

fn bytes_flags(f: u32) -> u32 {
    f >> 16
}

/// Reconstruct the full-resolution mesh of a cache (leaf clusters), positions
/// only (lattice-quantised), rebuild it with the current builder and report.
fn leaves_of(r: &VgeoReader) -> (Vec<V3>, Vec<u32>) {
    let roots = r
        .select_clusters(
            1e30,
            &Projection {
                eye: [0.0; 3],
                scale: 1.0,
            },
        )
        .unwrap();
    let c = roots[0].cluster.sphere;
    let leaves = r
        .select_clusters(
            1e-12,
            &Projection {
                eye: [c[0], c[1], c[2]],
                scale: 1.0,
            },
        )
        .unwrap();
    let mut pos = Vec::new();
    let mut idx = Vec::new();
    for s in &leaves {
        let d = r.decode(s).unwrap();
        let base = pos.len() as u32;
        pos.extend(d.positions.iter().copied());
        for t in &d.tris {
            idx.extend(t.iter().map(|&v| base + v as u32));
        }
    }
    (pos, idx)
}

#[test]
#[ignore = "measurement harness over a local cache: VG_CACHE_FILE=<name>.vgeo"]
fn harness_rebuild_cache_mesh() {
    let dir = std::env::var("VG_CACHE_DIR")
        .unwrap_or_else(|_| "../examples/assets/vg_stress/cache".into());
    let file = std::env::var("VG_CACHE_FILE")
        .unwrap_or_else(|_| "ded1daa6960e2e1a4fa36e415e4d74ee.vgeo".into());
    let bytes = std::fs::read(format!("{dir}/{file}")).unwrap();
    let (pos, idx) = {
        let r = VgeoReader::parse(&bytes).unwrap();
        leaves_of(&r)
    };
    drop(bytes);
    println!("{file}: {} verts, {} tris", pos.len(), idx.len() / 3);
    let attrs = pos_attrs(&pos);
    let t = std::time::Instant::now();
    let dag = build_dag(&input(&pos, &attrs, &idx)).unwrap();
    println!("dag build {:?}", t.elapsed());
    if std::env::var("VG_NO_MEASURE").is_err() {
        print_reports(&file, &measure_dag(&dag, 150_000));
    }
    let bytes = write_vgeo(&dag, &attrs, 12, &opts()).unwrap();
    println!(
        "total build+write {:?}, {} MiB",
        t.elapsed(),
        bytes.len() >> 20
    );
    report_vgeo(&file, &bytes);
}

/// Build a local GLB (e.g. a Stanford scan) with the current builder; report
/// build time, deviation table (unless `VG_NO_MEASURE`) and cut counts.
/// `VG_GLB=<path>`.
#[test]
#[ignore = "measurement harness over a local GLB"]
fn harness_glb() {
    let path = std::env::var("VG_GLB").expect("VG_GLB=<path to .glb>");
    let (pos, idx) = load_glb(&path);
    println!("{path}: {} verts, {} tris", pos.len(), idx.len() / 3);
    let attrs = pos_attrs(&pos);
    let t = std::time::Instant::now();
    let dag = build_dag(&input(&pos, &attrs, &idx)).unwrap();
    println!(
        "dag build {:?} ({} clusters)",
        t.elapsed(),
        dag.clusters.len()
    );
    print_ladder(&dag);
    if std::env::var("VG_NO_MEASURE").is_err() {
        print_reports(&path, &measure_dag(&dag, 150_000));
    }
    let bytes = write_vgeo(&dag, &attrs, 12, &opts()).unwrap();
    println!(
        "total build+write {:?}, {} MiB",
        t.elapsed(),
        bytes.len() >> 20
    );
    report_vgeo(&path, &bytes);
}

/// Per-level triangle count and stored-error distribution (no geometry measurement).
fn print_ladder(dag: &Dag) {
    let top = dag.clusters.iter().map(|c| c.level).max().unwrap();
    println!("lvl clusters      tris   err_p50    err_p90    err_max");
    for level in 1..=top {
        let cl: Vec<_> = dag.clusters.iter().filter(|c| c.level == level).collect();
        let mut e: Vec<f32> = cl.iter().map(|c| c.self_error).collect();
        e.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!(
            "{:>3} {:>8} {:>9} {:>9.5} {:>10.5} {:>10.5}",
            level,
            cl.len(),
            cl.iter().map(|c| c.tris.len()).sum::<usize>(),
            e[e.len() / 2],
            e[e.len() * 9 / 10],
            e[e.len() - 1]
        );
    }
}

/// Monolithic (single global) simplification of a GLB: what a DAG-free
/// simplifier achieves at the same triangle count. `VG_GLB`.
#[test]
#[ignore = "measurement harness"]
fn harness_monolithic_glb() {
    let (p, idx) = load_glb(&std::env::var("VG_GLB").unwrap());
    let tri_of = |ids: &[u32]| -> Vec<[V3; 3]> {
        ids.chunks_exact(3)
            .map(|t| [p[t[0] as usize], p[t[1] as usize], p[t[2] as usize]])
            .collect()
    };
    let original = Soup::new(tri_of(&idx));
    let bytes: &[u8] = unsafe { std::slice::from_raw_parts(p.as_ptr() as *const u8, p.len() * 12) };
    let adapter = meshopt::VertexDataAdapter::new(bytes, 12, 0).unwrap();
    for target in [20000usize, 6000, 2500, 1200, 600] {
        let out = meshopt::simplify(
            &idx,
            &adapter,
            target * 3,
            f32::MAX,
            meshopt::SimplifyOptions::ErrorAbsolute,
            None,
        );
        let dev = measure_level(&original, &[tri_of(&out)], &[1.0], 100_000);
        println!(
            "monolithic {target}: {} tris, max {:.4} p99 {:.4}",
            out.len() / 3,
            dev.max,
            dev.p99
        );
    }
}

/// How far can meshopt reduce a whole GLB at all (border/topology floor)? `VG_GLB`.
#[test]
#[ignore = "measurement harness"]
fn harness_floor_glb() {
    let (p, idx) = load_glb(&std::env::var("VG_GLB").unwrap());
    let bytes: &[u8] = unsafe { std::slice::from_raw_parts(p.as_ptr() as *const u8, p.len() * 12) };
    let adapter = meshopt::VertexDataAdapter::new(bytes, 12, 0).unwrap();
    for opts in [
        meshopt::SimplifyOptions::ErrorAbsolute,
        meshopt::SimplifyOptions::ErrorAbsolute | meshopt::SimplifyOptions::Prune,
    ] {
        let out = meshopt::simplify(&idx, &adapter, 300 * 3, f32::MAX, opts, None);
        println!(
            "floor {:?}: {} -> {} tris",
            opts,
            idx.len() / 3,
            out.len() / 3
        );
    }
}

/// Count open (1 use) and non-manifold (>2 uses) edges of a GLB by position. `VG_GLB`.
#[test]
#[ignore = "measurement harness"]
fn harness_topology_glb() {
    use std::collections::HashMap;
    let (p, idx) = load_glb(&std::env::var("VG_GLB").unwrap());
    let key = |v: u32| p[v as usize].map(f32::to_bits);
    let mut e: HashMap<([u32; 3], [u32; 3]), u32> = HashMap::new();
    for t in idx.chunks_exact(3) {
        for k in 0..3 {
            let (a, b) = (key(t[k]), key(t[(k + 1) % 3]));
            *e.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    let open = e.values().filter(|&&n| n == 1).count();
    let nm = e.values().filter(|&&n| n > 2).count();
    println!("edges {} open {open} nonmanifold {nm}", e.len());
}

/// Why does the top of a closed mesh stop reducing? Try simplifier variants on
/// the second-coarsest level. `VG_GLB`.
#[test]
#[ignore = "measurement harness"]
fn harness_top_stall() {
    let (pos, idx) = load_glb(&std::env::var("VG_GLB").unwrap());
    let attrs = pos_attrs(&pos);
    let dag = build_dag(&input(&pos, &attrs, &idx)).unwrap();
    let top = dag.clusters.iter().map(|c| c.level).max().unwrap();
    let lvl = top; // last built level; try to reduce it further
    let tris: Vec<[u32; 3]> = dag
        .clusters
        .iter()
        .filter(|c| c.level == lvl)
        .flat_map(|c| c.tris.clone())
        .collect();
    let mut ids: Vec<u32> = tris.iter().flatten().copied().collect();
    ids.sort_unstable();
    ids.dedup();
    let positions: Vec<V3> = ids.iter().map(|&g| dag.vs.pos[g as usize]).collect();
    let indices: Vec<u32> = tris
        .iter()
        .flatten()
        .map(|g| ids.binary_search(g).unwrap() as u32)
        .collect();
    let bytes: &[u8] = unsafe {
        std::slice::from_raw_parts(positions.as_ptr() as *const u8, positions.len() * 12)
    };
    let adapter = meshopt::VertexDataAdapter::new(bytes, 12, 0).unwrap();
    // topology
    let mut e: std::collections::HashMap<(u32, u32), u32> = Default::default();
    for t in indices.chunks_exact(3) {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            *e.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    println!(
        "level {lvl}: {} tris, {} verts, open edges {}, nonmanifold {}",
        tris.len(),
        ids.len(),
        e.values().filter(|&&n| n == 1).count(),
        e.values().filter(|&&n| n > 2).count()
    );
    use meshopt::SimplifyOptions as O;
    for (name, o) in [
        ("lockborder", O::LockBorder | O::ErrorAbsolute),
        ("none", O::ErrorAbsolute),
        ("prune", O::ErrorAbsolute | O::Prune),
        ("permissive", O::ErrorAbsolute | O::Permissive),
    ] {
        let out = meshopt::simplify(&indices, &adapter, 200 * 3, f32::MAX, o, None);
        println!("  {name}: -> {} tris", out.len() / 3);
    }
    let out = meshopt::simplify_sloppy(&indices, &adapter, 200 * 3, f32::MAX, None);
    println!("  sloppy: -> {} tris", out.len() / 3);
}
