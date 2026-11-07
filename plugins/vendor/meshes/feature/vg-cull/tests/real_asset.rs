//! Cross-crate contract tests on a REAL asset: `helio-vg-build` writes a
//! `.vgeo`, `helio-vg-stream` parses and streams it, and the cull's CPU
//! reference runs the cut over what the stream made resident.
//!
//! * cone convention end to end (builder cone -> cull cone test),
//! * `group_id` / child-page semantics: exactly one cluster per leaf-to-root
//!   path at many tau / distances, child-page requests only target
//!   non-resident pages,
//! * the scheduler keeps the resident page set upward closed under churn.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use helio_pass_vg_cull::reference::{
    cluster_cone_culled, select_clusters, select_clusters_ex, RefAsset, RefInstance, RefView,
};
use helio_vg_build::{build_vgeo, BuildOptions, MeshInput, VgeoReader};
use helio_vg_format::*;
use helio_vg_stream::core::{StoreCore, VgStreamConfig};
use helio_vg_stream::{Feedback, VgeoBytes, VgeoSource, PAGE_NOT_RESIDENT};

fn torus(nu: u32, nv: u32) -> (Vec<[f32; 3]>, Vec<u32>) {
    let mut p = Vec::new();
    for j in 0..nv {
        for i in 0..nu {
            let a = std::f32::consts::TAU * i as f32 / nu as f32;
            let b = std::f32::consts::TAU * j as f32 / nv as f32;
            let r = 1.0 + 0.4 * b.cos();
            p.push([r * a.cos(), 0.4 * b.sin(), r * a.sin()]);
        }
    }
    let at = |i: u32, j: u32| (j % nv) * nu + (i % nu);
    let mut idx = Vec::new();
    for j in 0..nv {
        for i in 0..nu {
            let (a, b, c, d) = (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
            idx.extend([a, b, c, a, c, d]);
        }
    }
    (p, idx)
}

fn build_torus(nu: u32, nv: u32) -> Vec<u8> {
    let (pos, idx) = torus(nu, nv);
    let attrs = vec![0u8; pos.len() * ATTR_STRIDE as usize];
    let mats = vec![0u32; idx.len() / 3];
    build_vgeo(
        &MeshInput {
            positions: &pos,
            attributes: &attrs,
            attribute_stride: ATTR_STRIDE as usize,
            indices: &idx,
            tri_material: &mats,
        },
        &BuildOptions {
            content_id: [7, 9],
            resident_levels: 2,
        },
    )
    .expect("build")
}

#[test]
fn built_mesh_distance_lod_reduces_triangles_without_holes() {
    let bytes = build_torus(260, 130);
    let r = VgeoReader::parse(&bytes).unwrap();
    let parents = parents_map(&r);
    let slots: Vec<_> = (0..r.header.asset.page_count).map(Some).collect();
    let lookup = |p: u32, local: u32| r.page(p).unwrap().cluster(local).unwrap();
    let asset = RefAsset {
        root_node: r.header.asset.root_node,
        nodes: &r.nodes,
        group_refs: &r.group_refs,
        page_slot: &slots,
        cluster: &lookup,
    };
    let mut counts = Vec::new();
    for distance in [2.5, 6.0, 12.0, 24.0, 48.0, 96.0] {
        let selected = select_clusters(&asset, &view(distance), 1.0, &instance(1.0, 0));
        assert!(selected.requested_pages.is_empty());
        let drawn: HashSet<_> = selected
            .drawn
            .iter()
            .map(|&s| (cluster_slot_page(s), cluster_slot_local(s)))
            .collect();
        check_one_per_path(&r, &parents, &drawn, &format!("distance={distance}"));
        let triangles: u32 = drawn
            .iter()
            .map(|&(p, l)| lookup(p, l).counts_material & 0xff)
            .sum();
        assert!(triangles > 0);
        counts.push(triangles);
    }
    eprintln!("Built torus, 1px error, distances 2.5..96m: {counts:?}");
    assert!(counts.windows(2).all(|w| w[1] <= w[0]), "{counts:?}");
    assert!(
        counts.last().unwrap() * 4 < counts[0],
        "far mesh must simplify: {counts:?}"
    );
}

fn view(dist: f32) -> RefView {
    RefView::looking_down_neg_z([0.0, 0.0, dist], 60f32.to_radians(), 0.1, 1080.0)
}

fn instance(scale: f32, flags: u32) -> RefInstance {
    RefInstance::translated_scaled(
        [0.0; 3],
        scale,
        GpuVgInstance {
            asset_slot: 0,
            flags: INST_ENABLED | flags,
            lod_error_scale: 1.0,
            object_row: 0,
        },
    )
}

fn lcg(s: &mut u64) -> u64 {
    *s = s
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *s >> 33
}

/// parents[cluster] over the whole file, keyed by (page, local).
fn parents_map(r: &VgeoReader) -> HashMap<(u32, u32), Vec<(u32, u32)>> {
    let mut m: HashMap<(u32, u32), Vec<(u32, u32)>> = HashMap::new();
    for p in 0..r.header.asset.page_count {
        for (i, c) in r.page(p).unwrap().clusters().unwrap().iter().enumerate() {
            if c.flags & GpuCluster::FLAG_LEAF != 0 {
                continue;
            }
            let g = r.group_refs[c.group_id as usize];
            for k in g.first_cluster()..g.first_cluster() + g.cluster_count() {
                m.entry((g.page_id, k)).or_default().push((p, i as u32));
            }
        }
    }
    m
}

/// Every leaf-to-root path must contain exactly `1` drawn cluster.
fn check_one_per_path(
    r: &VgeoReader,
    parents: &HashMap<(u32, u32), Vec<(u32, u32)>>,
    drawn: &HashSet<(u32, u32)>,
    what: &str,
) {
    // (min, max) drawn count over all paths from a cluster up to a root.
    let mut memo: HashMap<(u32, u32), (u32, u32)> = HashMap::new();
    fn go(
        key: (u32, u32),
        parents: &HashMap<(u32, u32), Vec<(u32, u32)>>,
        drawn: &HashSet<(u32, u32)>,
        memo: &mut HashMap<(u32, u32), (u32, u32)>,
    ) -> (u32, u32) {
        if let Some(&v) = memo.get(&key) {
            return v;
        }
        let own = drawn.contains(&key) as u32;
        let v = match parents.get(&key) {
            None => (own, own),
            Some(ps) => {
                let mut lo = u32::MAX;
                let mut hi = 0;
                for &p in ps {
                    let (a, b) = go(p, parents, drawn, memo);
                    lo = lo.min(a);
                    hi = hi.max(b);
                }
                (own + lo, own + hi)
            }
        };
        memo.insert(key, v);
        v
    }
    for p in 0..r.header.asset.page_count {
        for (i, c) in r.page(p).unwrap().clusters().unwrap().iter().enumerate() {
            if c.flags & GpuCluster::FLAG_LEAF != 0 {
                assert_eq!(
                    go((p, i as u32), parents, drawn, &mut memo),
                    (1, 1),
                    "{what}: leaf ({p},{i}) path count"
                );
            }
        }
    }
}

struct Pool<'a> {
    r: &'a VgeoReader<'a>,
    /// pool slot -> page
    slot_page: HashMap<u32, u32>,
}

impl Pool<'_> {
    fn cluster(&self, slot: u32, local: u32) -> GpuCluster {
        self.r
            .page(self.slot_page[&slot])
            .unwrap()
            .cluster(local)
            .unwrap()
    }
}

fn run_cuts(
    r: &VgeoReader,
    parents: &HashMap<(u32, u32), Vec<(u32, u32)>>,
    page_slot: &[Option<u32>],
    what: &str,
    rng: &mut u64,
) -> (usize, usize) {
    let pool = Pool {
        r,
        slot_page: page_slot
            .iter()
            .enumerate()
            .filter_map(|(p, s)| s.map(|s| (s, p as u32)))
            .collect(),
    };
    let lookup = |s: u32, l: u32| pool.cluster(s, l);
    let asset = RefAsset {
        root_node: r.header.asset.root_node,
        nodes: &r.nodes,
        group_refs: &r.group_refs,
        page_slot,
        cluster: &lookup,
    };
    let (mut checked, mut requested) = (0, 0);
    for dist in [2.5f32, 6.0, 20.0, 80.0, 400.0] {
        let v = view(dist);
        let tau = [0.25f32, 1.0, 4.0, 16.0][(lcg(rng) % 4) as usize];
        let scale = [0.5f32, 1.0, 2.0][(lcg(rng) % 3) as usize];
        let sel = select_clusters(&asset, &v, tau, &instance(scale, 0));
        let drawn: HashSet<(u32, u32)> = sel
            .drawn
            .iter()
            .map(|&cs| {
                (
                    pool.slot_page[&cluster_slot_page(cs)],
                    cluster_slot_local(cs),
                )
            })
            .collect();
        let what = format!("{what} dist={dist} tau={tau} scale={scale}");
        check_one_per_path(r, parents, &drawn, &what);
        for &p in &sel.requested_pages {
            assert!(
                page_slot[p as usize].is_none(),
                "{what}: requested resident page {p}"
            );
        }
        checked += 1;
        requested += sel.requested_pages.len();
    }
    (checked, requested)
}

#[test]
fn cut_is_exact_on_a_real_asset_fully_resident_and_under_streaming_churn() {
    let bytes = build_torus(260, 130);
    let r = VgeoReader::parse(&bytes).unwrap();
    let pages = r.header.asset.page_count as usize;
    assert!(
        pages >= 6,
        "test asset should span several pages, has {pages}"
    );
    let parents = parents_map(&r);

    // T3's own parsing of the same bytes agrees with the builder's reader.
    let src = VgeoBytes::new(bytes.clone()).unwrap();
    assert_eq!(src.hierarchy().unwrap(), r.nodes);
    assert_eq!(src.group_refs().unwrap(), r.group_refs);
    assert_eq!(src.page_deps().unwrap(), r.page_deps);
    assert_eq!(
        src.read_page(1).unwrap(),
        bytes[r.page_dir[1].file_offset as usize..][..PAGE_BYTES as usize]
    );
    let q = src.quant();
    assert_eq!((q.origin, q.step), (r.meta.origin, r.meta.step));

    let mut rng = 0x5eed_u64;
    // Fully resident (slot == page).
    let all: Vec<Option<u32>> = (0..pages as u32).map(Some).collect();
    let (n, req) = run_cuts(&r, &parents, &all, "all-resident", &mut rng);
    assert!(
        n > 0 && req == 0,
        "nothing to request when everything is resident"
    );

    // Streaming churn through the real scheduler with a tight page budget.
    let slots = (r.header.asset.root_page_count + 3).max(5);
    let cfg = VgStreamConfig {
        async_loads: false,
        initial_slots: slots,
        grow_chunk_slots: slots,
        uploads_per_frame: 2,
        ..Default::default()
    };
    let mut core = StoreCore::new(cfg, slots);
    core.cfg.scheduler.eviction_grace_frames = 1;
    let reg = core
        .register(Arc::new(VgeoBytes::new(bytes.clone()).unwrap()))
        .unwrap();
    let mut evictions_seen = false;
    let mut total_requested = 0;
    for step in 0..400 {
        core.update();
        let table = core.table();
        let page_slot: Vec<Option<u32>> = (0..pages as u32)
            .map(|p| Some(table[(reg.page_base + p) as usize]).filter(|&s| s != PAGE_NOT_RESIDENT))
            .collect();
        // Invariant the cull's exactness rests on: resident set is upward closed.
        for (p, s) in page_slot.iter().enumerate() {
            if s.is_some() {
                for &d in &r.page_deps[p] {
                    assert!(
                        page_slot[d as usize].is_some(),
                        "step {step}: page {p} resident without dependency {d}"
                    );
                }
            }
        }
        if core.is_ready(reg.slot) {
            let (_, requested) =
                run_cuts(&r, &parents, &page_slot, &format!("step {step}"), &mut rng);
            total_requested += requested;
            // Feed what the GPU would have asked for: every page the fine
            // cut wants (close camera), plus a random page.
            let v = view(2.5);
            let pool_slot_page: HashMap<u32, u32> = page_slot
                .iter()
                .enumerate()
                .filter_map(|(p, s)| s.map(|s| (s, p as u32)))
                .collect();
            let pool = Pool {
                r: &r,
                slot_page: pool_slot_page,
            };
            let lookup = |s: u32, l: u32| pool.cluster(s, l);
            let asset = RefAsset {
                root_node: r.header.asset.root_node,
                nodes: &r.nodes,
                group_refs: &r.group_refs,
                page_slot: &page_slot,
                cluster: &lookup,
            };
            let mut want = select_clusters(&asset, &v, 0.25, &instance(1.0, 0)).requested_pages;
            want.insert((lcg(&mut rng) % pages as u64) as u32);
            core.ingest(&Feedback {
                requests: want.into_iter().map(|p| reg.page_base + p).collect(),
                used: vec![],
                overflowed: false,
            });
        }
        evictions_seen |= core.scheduler_stats().evictions > 0;
    }
    assert!(core.is_ready(reg.slot));
    assert!(
        total_requested > 0,
        "the partially resident cut must have requested child pages"
    );
    assert!(
        evictions_seen,
        "the budget was tight enough to force evictions"
    );
}

#[test]
fn builder_cone_never_culls_front_faces_and_culls_back_faces_end_to_end() {
    let bytes = build_torus(120, 60);
    let r = VgeoReader::parse(&bytes).unwrap();
    let mut rng = 0xc0ffee_u64;
    let (mut culled, mut backfacing_clusters, mut culled_back) = (0usize, 0usize, 0usize);
    for _ in 0..40 {
        let eye = [
            (lcg(&mut rng) % 2000) as f32 / 100.0 - 10.0,
            (lcg(&mut rng) % 2000) as f32 / 100.0 - 10.0,
            (lcg(&mut rng) % 2000) as f32 / 100.0 - 10.0,
        ];
        if eye.iter().map(|x| x * x).sum::<f32>() < 4.0 {
            continue;
        }
        let mut v = view(1.0);
        // Put the camera at `eye` (only `eye()` is used by the cone test).
        v.view[12] = -eye[0];
        v.view[13] = -eye[1];
        v.view[14] = -eye[2];
        let inst = instance(1.0, INST_CONE_CULL);
        for p in 0..r.header.asset.page_count {
            let page = r.page(p).unwrap();
            for (i, c) in page.clusters().unwrap().iter().enumerate() {
                let d = page.decode_cluster(i as u32).unwrap();
                // Facing of every triangle toward the eye (positive = front).
                let (mut any_front, mut all_back) = (false, true);
                for t in &d.tris {
                    let [a, b, cc] = t.map(|k| d.positions[k as usize]);
                    let n = [
                        (b[1] - a[1]) * (cc[2] - a[2]) - (b[2] - a[2]) * (cc[1] - a[1]),
                        (b[2] - a[2]) * (cc[0] - a[0]) - (b[0] - a[0]) * (cc[2] - a[2]),
                        (b[0] - a[0]) * (cc[1] - a[1]) - (b[1] - a[1]) * (cc[0] - a[0]),
                    ];
                    let to = [eye[0] - a[0], eye[1] - a[1], eye[2] - a[2]];
                    let f = n[0] * to[0] + n[1] * to[1] + n[2] * to[2];
                    any_front |= f > 1e-9;
                    all_back &= f < 0.0;
                }
                let is_culled = cluster_cone_culled(&v, &inst, c);
                if is_culled {
                    culled += 1;
                    assert!(
                        !any_front,
                        "cone cull dropped a cluster with a front-facing triangle"
                    );
                }
                if all_back {
                    backfacing_clusters += 1;
                    culled_back += is_culled as usize;
                }
            }
        }
    }
    assert!(culled > 0 && backfacing_clusters > 0);
    assert!(
        culled_back * 10 >= backfacing_clusters,
        "the cone should catch a good share of back-facing clusters: {culled_back}/{backfacing_clusters}"
    );
    // The same decision through the selection API (instance flag honoured).
    let all: Vec<Option<u32>> = (0..r.header.asset.page_count).map(Some).collect();
    let lookup = |s: u32, l: u32| r.page(s).unwrap().cluster(l).unwrap();
    let asset = RefAsset {
        root_node: r.header.asset.root_node,
        nodes: &r.nodes,
        group_refs: &r.group_refs,
        page_slot: &all,
        cluster: &lookup,
    };
    let v = view(6.0);
    let with = select_clusters_ex(&asset, &v, 0.5, &instance(1.0, INST_CONE_CULL), true)
        .drawn
        .len();
    let without = select_clusters(&asset, &v, 0.5, &instance(1.0, INST_CONE_CULL))
        .drawn
        .len();
    assert!(
        with < without,
        "cone culling removes some of the {without} clusters ({with} left)"
    );
}
