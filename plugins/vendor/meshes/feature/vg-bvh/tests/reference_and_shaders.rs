//! CPU-only checks: the WGSL validates and mirrors the Rust layouts, and the
//! reference LBVH (the algorithm the GPU implements) builds valid trees.

mod common;

use common::*;
use helio_pass_vg_bvh::reference::{build, header_of, refit, validate};
use helio_pass_vg_bvh::{
    BvhConfig, GpuBvhHeader, GpuBvhNode, BUILD_WGSL, NODES_WGSL, NODE_INVALID,
};

fn naga_module(src: &str) -> naga::Module {
    let module =
        naga::front::wgsl::parse_str(src).unwrap_or_else(|e| panic!("{}", e.emit_to_string(src)));
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap_or_else(|e| panic!("{e:?}"));
    module
}

#[test]
fn build_shader_validates() {
    let src = format!("{NODES_WGSL}\n{BUILD_WGSL}");
    let module = naga_module(&src);
    let entries: Vec<&str> = module
        .entry_points
        .iter()
        .map(|e| e.name.as_str())
        .collect();
    for expected in [
        "cs_gather_local",
        "cs_block_scan",
        "cs_gather_write",
        "cs_histogram",
        "cs_scan",
        "cs_scatter",
        "cs_chunk_eval",
        "cs_chunk_scan",
        "cs_make_leaves",
        "cs_karras",
        "cs_refit_leaves",
        "cs_finalize",
    ] {
        assert!(
            entries.contains(&expected),
            "missing entry point {expected}"
        );
    }
}

#[test]
fn nodes_include_validates_standalone_and_matches_rust() {
    // What helio-pass-vg-cull does: include the file, add its own entry point.
    let src = format!(
        "{NODES_WGSL}\n\
         @group(0) @binding(0) var<storage, read> nodes: array<VgBvhNode>;\n\
         @group(0) @binding(1) var<storage, read> header: VgBvhHeader;\n\
         @group(0) @binding(2) var<storage, read_write> out: array<u32>;\n\
         @compute @workgroup_size(1) fn main() {{\n\
             let n = nodes[header.root_node];\n\
             if vg_bvh_is_leaf(n) {{ out[0] = vg_bvh_leaf_first(n) + vg_bvh_leaf_count(n); }}\n\
             else {{ out[0] = vg_bvh_child(n, 0u) + vg_bvh_child(n, 1u); }}\n\
         }}"
    );
    let module = naga_module(&src);
    let mut layouter = naga::proc::Layouter::default();
    layouter.update(module.to_ctx()).expect("layout");
    let size = |name: &str| {
        let (h, _) = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("missing {name}"));
        layouter[h].size as usize
    };
    assert_eq!(size("VgBvhNode"), std::mem::size_of::<GpuBvhNode>());
    assert_eq!(size("VgBvhHeader"), std::mem::size_of::<GpuBvhHeader>());
}

fn check(
    instances: &[helio_vg_format::GpuVgInstance],
    objects: &[helio_mesh_api::StaticObjectRow],
    cfg: &BvhConfig,
) -> helio_pass_vg_bvh::reference::RefTree {
    let tree = build(instances, objects, cfg);
    let live = tree.items.len() as u32;
    let header = header_of(&tree, live);
    validate(&tree.nodes, &tree.items, &header, instances, objects).expect("valid tree");
    tree
}

#[test]
fn trees_are_valid_for_many_sizes() {
    for n in [0usize, 1, 2, 3, 4, 5, 17, 100, 1000, 5000] {
        let (instances, objects) = random_scene(n as u64 + 1, n, 100.0, 0.1, 2.0);
        check(&instances, &objects, &BvhConfig::default());
        check(
            &instances,
            &objects,
            &BvhConfig {
                batch_max_instances: 1,
                ..Default::default()
            },
        );
    }
}

#[test]
fn empty_scene_gives_empty_tree() {
    let tree = build(&[], &[], &BvhConfig::default());
    assert!(tree.nodes.is_empty());
    let h = header_of(&tree, 0);
    assert_eq!(h.root_node, NODE_INVALID);
}

#[test]
fn identical_positions_still_build_a_valid_tree() {
    // All Morton keys equal: the index tie-break must give a valid tree.
    let objects: Vec<_> = (0..300).map(|_| object([1.0, 2.0, 3.0], 1.0)).collect();
    let instances: Vec<_> = (0..300).map(instance).collect();
    let tree = check(&instances, &objects, &BvhConfig::default());
    assert_eq!(tree.leaf_count, 300, "large objects are not batched");
}

#[test]
fn tiny_dense_instances_merge_into_batch_leaves() {
    let (instances, objects) = city_scene(9, 16, 400);
    let tree = check(&instances, &objects, &BvhConfig::default());
    assert!(tree.batch_leaf_count > 0, "expected batch leaves");
    assert!(
        (tree.leaf_count as usize) < instances.len() / 2,
        "dense scene should need far fewer leaves than instances: {} vs {}",
        tree.leaf_count,
        instances.len()
    );
    for n in &tree.nodes[tree.leaf_count as usize - 1..] {
        assert!(n.leaf_count() <= 64);
    }
    // Batching off: one leaf per instance.
    let flat = check(
        &instances,
        &objects,
        &BvhConfig {
            batch_max_instances: 1,
            ..Default::default()
        },
    );
    assert_eq!(flat.leaf_count as usize, flat.items.len());
    assert_eq!(flat.batch_leaf_count, 0);
}

#[test]
fn batch_threshold_is_configurable() {
    let (instances, objects) = city_scene(3, 8, 300);
    let wide = check(&instances, &objects, &BvhConfig::default());
    let small = check(
        &instances,
        &objects,
        &BvhConfig {
            batch_max_instances: 8,
            ..Default::default()
        },
    );
    let m = small.leaf_count as usize;
    assert!(
        small.nodes[m - 1..].iter().all(|n| n.leaf_count() <= 8),
        "batch size cap respected"
    );
    assert!(small.batch_leaf_count > 0);
    assert!(wide.nodes[wide.leaf_count as usize - 1..]
        .iter()
        .any(|n| n.leaf_count() > 8));
    let strict = check(
        &instances,
        &objects,
        &BvhConfig {
            tiny_extent: 0.001,
            ..Default::default()
        },
    );
    assert_eq!(strict.batch_leaf_count, 0);
}

#[test]
fn refit_keeps_topology_and_stays_valid_after_motion() {
    let (instances, mut objects) = random_scene(77, 2000, 100.0, 0.1, 1.0);
    let cfg = BvhConfig::default();
    let mut tree = check(&instances, &objects, &cfg);
    let topology: Vec<(u32, u32)> = tree.nodes.iter().map(|n| (n.a, n.b)).collect();
    let mut rng = Rng(5);
    for o in objects.iter_mut() {
        if rng.next_u32() % 3 == 0 {
            for k in 0..3 {
                o.bounds[k] += rng.unit() * 20.0 - 10.0;
            }
        }
    }
    refit(&mut tree, &instances, &objects);
    let header = header_of(&tree, tree.items.len() as u32);
    validate(&tree.nodes, &tree.items, &header, &instances, &objects).expect("valid after refit");
    let after: Vec<(u32, u32)> = tree.nodes.iter().map(|n| (n.a, n.b)).collect();
    assert_eq!(topology, after);
    // A refit tree has the same bounds a fresh build over the new scene has at the root.
    let fresh = build(&instances, &objects, &cfg);
    assert_eq!(tree.nodes[0].min, fresh.nodes[0].min);
    assert_eq!(tree.nodes[0].max, fresh.nodes[0].max);
}

#[test]
fn disabled_and_dead_rows_are_excluded() {
    let mut objects = vec![object([0.0; 3], 1.0), object([5.0; 3], 1.0), dead_object()];
    objects.push(object([9.0; 3], f32::NAN));
    let mut instances: Vec<_> = (0..4).map(instance).collect();
    instances[1].flags = 0;
    instances.push(instance(99)); // out-of-range object row
    let tree = check(&instances, &objects, &BvhConfig::default());
    assert_eq!(tree.items, vec![0]);
}
