use pulsar_terrain::{
    CellWord, ContentHash, EditMode, EditOp, EditShape, FixedSphereGenerator, NodeState, PageKey,
    PlanetDefinition, PlanetId, PlanetPosition, PlanetView, SparseBrickTree, TerrainCore,
    TerrainStreamingConfig, TerrainStreamingPlanner,
};
use std::time::Instant;

fn main() {
    const TOUCHES: i64 = 10_000;
    let started = Instant::now();
    let mut sparse = SparseBrickTree::centered(24, NodeState::Air).unwrap();
    for index in 0..TOUCHES {
        sparse
            .set(
                PageKey::new(0, [index - TOUCHES / 2, index % 97, -(index % 193)]),
                NodeState::Page(ContentHash::of(&index.to_le_bytes())),
            )
            .unwrap();
    }
    let sparse_time = started.elapsed();

    const DENSE_EDGE: usize = 128;
    let dense_started = Instant::now();
    let dense = vec![CellWord::AIR; DENSE_EDGE * DENSE_EDGE * DENSE_EDGE];
    std::hint::black_box(&dense);
    let dense_time = dense_started.elapsed();

    let mut core = TerrainCore::new(
        PlanetId([1; 16]),
        24,
        FixedSphereGenerator {
            center_cell: [0; 3],
            radius_cells: 63_710_000,
            material: 1,
        },
    )
    .unwrap();
    core.append_edit(EditOp {
        sequence: 1,
        stable_id: [1; 16],
        shape: EditShape::Sphere {
            center_cell: [0; 3],
            radius_cells: 10,
        },
        mode: EditMode::Subtract,
        material: 0,
    })
    .unwrap();
    let edit_started = Instant::now();
    let compacted = core.compact_page(PageKey::new(0, [0; 3])).unwrap();
    let edit_time = edit_started.elapsed();

    let mut coarse_core = TerrainCore::new(
        PlanetId([3; 16]),
        24,
        FixedSphereGenerator {
            center_cell: [0; 3],
            radius_cells: 63_710_000,
            material: 1,
        },
    )
    .unwrap();
    coarse_core
        .append_edit(EditOp {
            sequence: 1,
            stable_id: [2; 16],
            shape: EditShape::Sphere {
                center_cell: [0; 3],
                radius_cells: 10,
            },
            mode: EditMode::Subtract,
            material: 0,
        })
        .unwrap();
    let coarse_started = Instant::now();
    coarse_core.compact_page(PageKey::new(12, [0; 3])).unwrap();
    let coarse_time = coarse_started.elapsed();
    let coarse_work = coarse_core.work_counters();

    let delete_started = Instant::now();
    core.set_root(NodeState::Air).unwrap();
    let delete_time = delete_started.elapsed();

    let edit_amplification = [1_u32, 10, 100, 1_000].map(|radius_cells| {
        EditShape::Sphere {
            center_cell: [0; 3],
            radius_cells,
        }
        .affected_lod0_page_count()
    });
    let memory = core.memory_counters();
    let work = core.work_counters();

    let planet = PlanetDefinition {
        planet_id: PlanetId([2; 16]),
        center_cell: [0; 3],
        radius_cells: 63_710_000,
        material: 1,
        root_lod: 22,
        max_resident_pages: 2_048,
    };
    let view = PlanetView::new(
        PlanetPosition::from_lod0_cell([103_710_000, 0, 0]),
        [-1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        60_f64.to_radians(),
        [2560, 1440],
        0.1,
        20_000_000.0,
        [0.0; 3],
    )
    .unwrap();
    let planner = TerrainStreamingPlanner::new(TerrainStreamingConfig {
        max_pages: 2_048,
        max_traversal_nodes: 131_072,
        ..TerrainStreamingConfig::default()
    })
    .unwrap();
    let mut plan_times = Vec::with_capacity(20);
    let mut latest_plan = None;
    for _ in 0..20 {
        let started = Instant::now();
        let plan = planner.plan_fixed_sphere(&planet, view).unwrap();
        plan_times.push(started.elapsed());
        latest_plan = Some(plan);
    }
    plan_times.sort_unstable();
    let plan_p95 = plan_times[18];
    let latest_plan = latest_plan.unwrap();

    let ground_planet = PlanetDefinition {
        max_resident_pages: 8_192,
        ..planet.clone()
    };
    let ground_view = PlanetView::new(
        PlanetPosition::from_lod0_cell([63_710_000, 0, 0]),
        [-1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        60_f64.to_radians(),
        [2560, 1440],
        0.1,
        20_000.0,
        [0.0; 3],
    )
    .unwrap();
    let ground_planner = TerrainStreamingPlanner::new(TerrainStreamingConfig::default()).unwrap();
    let mut ground_times = Vec::with_capacity(10);
    let mut latest_ground_plan = None;
    for _ in 0..10 {
        let started = Instant::now();
        let plan = ground_planner
            .plan_fixed_sphere(&ground_planet, ground_view)
            .unwrap();
        ground_times.push(started.elapsed());
        latest_ground_plan = Some(plan);
    }
    ground_times.sort_unstable();
    let ground_p95 = ground_times[8];
    let latest_ground_plan = latest_ground_plan.unwrap();

    // A billion logical cells are represented by the root without allocation.
    let logical_dense_bytes = 1_000_000_000_u64 * 4;
    println!(
        "terrain_core sparse_touches={TOUCHES} nodes={} sparse_ms={:.3} dense_sample_cells={} dense_sample_bytes={} dense_fill_ms={:.3} billion_dense_equivalent_bytes={logical_dense_bytes} edited_page_bytes={} resident_dense_bytes={} generated_cells={} edit_attachment_regions={} edit_attachment_refs={} edit_candidates_replayed={} edit_compact_ms={:.3} coarse_lod=12 coarse_generated_cells={} coarse_edit_candidates={} coarse_compact_ms={:.3} edit_radius_cells=[1,10,100,1000] edit_aabb_pages={edit_amplification:?} root_delete_us={:.3} orbit_plan_pages={} orbit_plan_nodes={} orbit_plan_p95_ms={:.3} orbit_plan_limits={:?} ground_plan_pages={} ground_plan_nodes={} ground_plan_p95_ms={:.3} ground_plan_limits={:?}",
        sparse.node_count(),
        sparse_time.as_secs_f64() * 1_000.0,
        dense.len(),
        dense.len() * std::mem::size_of::<CellWord>(),
        dense_time.as_secs_f64() * 1_000.0,
        core.page(compacted.key).unwrap().encode().len(),
        memory.resident_dense_bytes,
        work.cells_generated,
        memory.edit_attachment_regions,
        memory.edit_attachment_references,
        work.edit_candidates_replayed,
        edit_time.as_secs_f64() * 1_000.0,
        coarse_work.cells_generated,
        coarse_work.edit_candidates_replayed,
        coarse_time.as_secs_f64() * 1_000.0,
        delete_time.as_secs_f64() * 1_000_000.0,
        latest_plan.demands().len(),
        latest_plan.counters().traversed_nodes,
        plan_p95.as_secs_f64() * 1_000.0,
        latest_plan.limits(),
        latest_ground_plan.demands().len(),
        latest_ground_plan.counters().traversed_nodes,
        ground_p95.as_secs_f64() * 1_000.0,
        latest_ground_plan.limits(),
    );
}
