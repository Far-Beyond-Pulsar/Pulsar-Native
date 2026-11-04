//! CPU simulation of the cut rule on a hand-built DAG: every leaf-to-root path
//! must draw exactly one cluster (no holes, no overlaps) across thresholds,
//! distances, instance scales, LOD scales and upward-closed residency sets,
//! and the hierarchy traversal must find exactly the clusters the
//! per-cluster rule selects.

mod common;

use common::{build, SynthDag};
use helio_pass_vg_cull::reference::{
    choose_bin, cut_draws, project_error, select_clusters, RefInstance, RefView,
};
use helio_pass_vg_cull::{BIN_SW, INF_ERROR};
use helio_vg_format::{GpuVgInstance, TierLock, VgMode, INST_ENABLED, INST_FORCE_ROOT_ONLY};

fn view(dist: f32) -> RefView {
    RefView::looking_down_neg_z([31.0, 0.0, dist], 60f32.to_radians(), 0.1, 1080.0)
}

fn page_of_slot(dag: &SynthDag) -> Vec<usize> {
    let mut p = vec![0; dag.clusters.len()];
    for (g, r) in dag.group_refs.iter().enumerate() {
        let first = dag.page_first_slot[g] + r.first_cluster();
        for i in 0..r.cluster_count() {
            p[(first + i) as usize] = g;
        }
    }
    p
}

/// Per-cluster rule only (no hierarchy), restricted to resident pages.
fn independent(
    dag: &SynthDag,
    resident: &[bool],
    v: &RefView,
    tau: f32,
    inst: &RefInstance,
) -> Vec<u32> {
    let slots = dag.slots(resident);
    let lookup = dag.lookup();
    let asset = dag.asset(&slots, &lookup);
    let pages = page_of_slot(dag);
    (0..dag.clusters.len() as u32)
        .filter(|&s| resident[pages[s as usize]])
        .filter(|&s| {
            let c = &dag.clusters[s as usize];
            cut_draws(v, tau, inst, c, asset.children_resident(c))
        })
        .map(|s| dag.slot_of(s as usize))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn check_paths(dag: &SynthDag, drawn: &[u32], what: &str) {
    let mut is_drawn = vec![false; dag.clusters.len()];
    for &s in drawn {
        is_drawn[dag.index_of(s)] = true;
    }
    fn walk(dag: &SynthDag, is_drawn: &[bool], c: u32, count: u32, what: &str, leaf: u32) {
        let count = count + is_drawn[c as usize] as u32;
        match dag.consumer_group[c as usize] {
            None => assert_eq!(
                count, 1,
                "{what}: path from leaf {leaf} to root {c} draws {count} clusters"
            ),
            Some(h) => {
                for &p in &dag.produced_by[h as usize] {
                    walk(dag, is_drawn, p, count, what, leaf);
                }
            }
        }
    }
    for s in 0..dag.clusters.len() as u32 {
        if dag.level[dag.index_of(s)] == 0 {
            walk(dag, &is_drawn, s, 0, what, s);
        }
    }
}

fn instance(scale: f32, lod: f32, flags: u32) -> RefInstance {
    RefInstance::translated_scaled(
        [0.0, 0.0, 0.0],
        scale,
        GpuVgInstance {
            asset_slot: 0,
            flags: INST_ENABLED | flags,
            lod_error_scale: lod,
            object_row: 0,
        },
    )
}

fn lcg(seed: &mut u64) -> u64 {
    *seed = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *seed >> 33
}

#[test]
fn every_leaf_to_root_path_draws_exactly_one_cluster() {
    let mut seed = 0x1234_5678u64;
    let mut checked = 0;
    for interleave in [false, true] {
        let dag = build(64, interleave);
        let all = vec![true; dag.page_count()];
        let root_only = dag.close_upward(
            &(0..dag.page_count())
                .map(|g| g == dag.root_group())
                .collect::<Vec<_>>(),
        );
        let mut residencies = vec![all, root_only];
        for _ in 0..6 {
            let seedv: Vec<bool> = (0..dag.page_count())
                .map(|g| g == dag.root_group() || lcg(&mut seed) % 3 == 0)
                .collect();
            residencies.push(dag.close_upward(&seedv));
        }
        for resident in &residencies {
            for dist in [3.0, 30.0, 150.0, 800.0, 6000.0] {
                let v = view(dist);
                for tau in [0.05, 0.5, 1.0, 3.0, 12.0, 100.0, 5.0e5] {
                    for (scale, lod) in [(1.0, 1.0), (0.5, 1.0), (3.0, 0.25), (1.0, 8.0)] {
                        let inst = instance(scale, lod, 0);
                        let (slots, lookup) = (dag.slots(resident), dag.lookup());
                        let asset = dag.asset(&slots, &lookup);

                        let sel = select_clusters(&asset, &v, tau, &inst);
                        let what = format!(
                            "interleave={interleave} dist={dist} tau={tau} scale={scale} lod={lod}"
                        );
                        check_paths(&dag, &sel.drawn, &what);
                        assert_eq!(
                            sel.drawn,
                            independent(&dag, resident, &v, tau, &inst),
                            "{what}: hierarchy traversal differs from the per-cluster rule"
                        );
                        for p in &sel.requested_pages {
                            assert!(
                                !resident[*p as usize],
                                "{what}: requested resident page {p}"
                            );
                        }
                        if resident.iter().all(|&r| r) {
                            assert!(sel.requested_pages.is_empty(), "{what}");
                        }
                        checked += 1;
                    }
                }
            }
        }
    }
    assert!(checked > 1000);
}

#[test]
fn threshold_extremes_pick_roots_and_leaves() {
    let dag = build(32, true);
    let all = vec![true; dag.page_count()];
    let (slots, lookup) = (dag.slots(&all), dag.lookup());
    let asset = dag.asset(&slots, &lookup);
    let top = dag.level.iter().copied().max().unwrap();
    // Huge tau: only the (infinite-parent) roots.
    let sel = select_clusters(&asset, &view(100.0), 1.0e12, &instance(1.0, 1.0, 0));
    assert!(!sel.drawn.is_empty());
    assert!(sel.drawn.iter().all(|&s| dag.level[dag.index_of(s)] == top));
    // Tiny tau up close: only leaves.
    let sel = select_clusters(&asset, &view(2.0), 1.0e-4, &instance(1.0, 1.0, 0));
    assert_eq!(sel.drawn.len(), dag.leaf_count);
    assert!(sel.drawn.iter().all(|&s| dag.level[dag.index_of(s)] == 0));
}

#[test]
fn force_root_only_draws_only_roots_whatever_the_distance() {
    let dag = build(32, false);
    let all = vec![true; dag.page_count()];
    let (slots, lookup) = (dag.slots(&all), dag.lookup());
    let asset = dag.asset(&slots, &lookup);
    let top = dag.level.iter().copied().max().unwrap();
    for dist in [1.0, 50.0, 5000.0] {
        let sel = select_clusters(
            &asset,
            &view(dist),
            0.5,
            &instance(1.0, 1.0, INST_FORCE_ROOT_ONLY),
        );
        assert!(!sel.drawn.is_empty());
        for s in &sel.drawn {
            assert_eq!(dag.level[dag.index_of(*s)], top);
        }
    }
}

#[test]
fn missing_child_page_is_replaced_by_the_resident_parent_and_requested() {
    let dag = build(32, false);
    // Only the root group's page is resident: the roots stand in for all.
    let resident: Vec<bool> = (0..dag.page_count())
        .map(|g| g == dag.root_group())
        .collect();
    let (slots, lookup) = (dag.slots(&resident), dag.lookup());
    let asset = dag.asset(&slots, &lookup);
    let sel = select_clusters(&asset, &view(2.0), 1.0e-4, &instance(1.0, 1.0, 0));
    let top = dag.level.iter().copied().max().unwrap();
    assert!(sel.drawn.iter().all(|&s| dag.level[dag.index_of(s)] == top));
    assert!(
        !sel.requested_pages.is_empty(),
        "the finer pages must be requested"
    );
    check_paths(&dag, &sel.drawn, "root only");
}

#[test]
fn projection_is_monotone_in_the_containing_sphere() {
    let v = view(100.0);
    let inner = project_error(&v, 3.0, [10.0, 0.0, 0.0], 1.0);
    // A sphere containing the inner one projects at least as large.
    for (c, r) in [
        ([10.0, 0.0, 0.0], 1.0),
        ([12.0, 0.0, 5.0], 6.5),
        ([10.0, 3.0, 40.0], 41.0),
    ] {
        assert!(project_error(&v, 3.0, c, r) >= inner);
    }
    assert_eq!(project_error(&v, INF_ERROR, [0.0; 3], 1.0), 3.0e38);
    // Inside the sphere the near plane bounds the distance.
    assert!(project_error(&v, 1.0, [0.0, 0.0, 100.0], 5.0).is_finite());
}

#[test]
fn tier_binning_rules() {
    let dag = build(32, false);
    let v = view(100.0);
    let c = dag.clusters[0]; // 32-128 tris, radius 1
    let auto = instance(1.0, 1.0, 0);
    // Far away the average triangle is sub-pixel: software.
    assert_eq!(
        choose_bin(&view(5000.0), &auto, &c, VgMode::Auto, 2.0, false),
        BIN_SW
    );
    // Dense mask promotes SW clusters only.
    assert_eq!(
        choose_bin(&view(5000.0), &auto, &c, VgMode::Auto, 2.0, true),
        helio_pass_vg_cull::BIN_DENSE
    );
    // Close up: hardware, binned by triangle count.
    assert_eq!(
        choose_bin(&view(3.0), &auto, &c, VgMode::Auto, 2.0, true),
        helio_pass_vg_cull::hw_bin_for_tris(c.tri_count())
    );
    let big = dag.clusters.iter().find(|c| c.tri_count() == 128).unwrap();
    assert_eq!(
        choose_bin(&view(3.0), &auto, big, VgMode::Auto, 2.0, false),
        3
    );
    // Mode and lock overrides.
    assert_eq!(
        choose_bin(&view(5000.0), &auto, &c, VgMode::ForceHwRaster, 2.0, true),
        helio_pass_vg_cull::hw_bin_for_tris(c.tri_count())
    );
    let locked = RefInstance {
        inst: GpuVgInstance {
            asset_slot: 0,
            flags: INST_ENABLED,
            lod_error_scale: 1.0,
            object_row: 0,
        }
        .with_tier_lock(TierLock::Dense),
        ..auto
    };
    assert_eq!(
        choose_bin(&v, &locked, &c, VgMode::Auto, 2.0, false),
        helio_pass_vg_cull::BIN_DENSE
    );
    let sw = RefInstance {
        inst: GpuVgInstance {
            asset_slot: 0,
            flags: INST_ENABLED,
            lod_error_scale: 1.0,
            object_row: 0,
        }
        .with_tier_lock(TierLock::SwRaster),
        ..auto
    };
    assert_eq!(
        choose_bin(&view(3.0), &sw, &c, VgMode::Auto, 2.0, false),
        BIN_SW
    );
}
