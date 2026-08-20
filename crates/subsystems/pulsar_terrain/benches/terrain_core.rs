use engine_fs::virtual_fs;
use engine_subsystems::{Subsystem, SubsystemContext};
use pulsar_terrain::{
    CellWord, ContentHash, EditMode, EditOp, EditShape, NodeState, PageKey, PlanetDefinition,
    PlanetId, PlanetPosition, PlanetSdfConfig, PlanetView, SparseBrickTree, TerrainCore,
    TerrainIncrementalResidencySession, TerrainPersistenceEvent, TerrainPersistenceHandle,
    TerrainPlanningConfig, TerrainRefinementConfig, TerrainRefinementFrontier,
    TerrainRenderDeltaConfig, TerrainRenderDeltaPublisher, TerrainRuntimeConfig, TerrainStore,
    TerrainStreamingConfig, TerrainStreamingPlanner, TerrainSubsystem,
};
use std::time::{Duration, Instant};

const SMOOTH_CELL_SIZE_MM: u32 = 1_000;
const EARTH_RADIUS_CELLS: u64 = 6_371_000;

fn benchmark_planet(planet_id: PlanetId) -> PlanetDefinition {
    PlanetDefinition {
        planet_id,
        center_cell: [0; 3],
        radius_cells: EARTH_RADIUS_CELLS,
        material: 1,
        lod0_cell_size_mm: SMOOTH_CELL_SIZE_MM,
        sdf: PlanetSdfConfig::earthlike(0x5eed, SMOOTH_CELL_SIZE_MM).unwrap(),
        root_lod: 22,
        max_resident_pages: 2_048,
    }
}

fn wait_for_persistence_event(handle: &TerrainPersistenceHandle) -> TerrainPersistenceEvent {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        handle.pump(64);
        if let Some(event) = handle.drain_events(1).into_iter().next() {
            return event;
        }
        assert!(
            Instant::now() < deadline,
            "asynchronous terrain persistence timed out: {:?}",
            handle.counters()
        );
        std::thread::yield_now();
    }
}

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

    let core_planet = benchmark_planet(PlanetId([1; 16]));
    let mut core =
        TerrainCore::new(core_planet.planet_id, 24, core_planet.generator().unwrap()).unwrap();
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

    let coarse_planet = benchmark_planet(PlanetId([3; 16]));
    let mut coarse_core = TerrainCore::new(
        coarse_planet.planet_id,
        24,
        coarse_planet.generator().unwrap(),
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

    const DELETE_EDIT_PREFIX: u64 = 10_000;
    let delete_planet = benchmark_planet(PlanetId([4; 16]));
    let mut delete_core = TerrainCore::new(
        delete_planet.planet_id,
        24,
        delete_planet.generator().unwrap(),
    )
    .unwrap();
    for sequence in 1..=DELETE_EDIT_PREFIX {
        let mut stable_id = [0_u8; 16];
        stable_id[..8].copy_from_slice(&sequence.to_le_bytes());
        stable_id[8..].copy_from_slice(b"rootperf");
        delete_core
            .append_edit(EditOp {
                sequence,
                stable_id,
                shape: EditShape::Sphere {
                    center_cell: [0; 3],
                    radius_cells: u32::MAX,
                },
                mode: EditMode::Subtract,
                material: 0,
            })
            .unwrap();
    }
    let delete_started = Instant::now();
    delete_core.set_root(NodeState::Air).unwrap();
    let delete_time = delete_started.elapsed();
    assert!(
        delete_time < Duration::from_millis(10),
        "root delete exceeded the 10 ms acceptance gate: {delete_time:?}"
    );
    let mut long_history_capture_times = Vec::with_capacity(100);
    for _ in 0..100 {
        let started = Instant::now();
        std::hint::black_box(delete_core.planning_snapshot());
        long_history_capture_times.push(started.elapsed());
    }
    long_history_capture_times.sort_unstable();
    let long_history_capture_p95 = long_history_capture_times[94];
    assert!(
        long_history_capture_p95 <= Duration::from_micros(500),
        "10,000-edit planning capture exceeded the 0.5 ms acceptance gate: {long_history_capture_p95:?}"
    );

    let edit_amplification = [1_u32, 10, 100, 1_000].map(|radius_cells| {
        EditShape::Sphere {
            center_cell: [0; 3],
            radius_cells,
        }
        .affected_lod0_page_count()
    });
    let memory = core.memory_counters();
    let work = core.work_counters();

    let planet = benchmark_planet(PlanetId([2; 16]));
    let view = PlanetView::new(
        PlanetPosition::from_lod0_cell([10_371_000, 0, 0], planet.lod0_cell_size_mm).unwrap(),
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
        let plan = planner.plan_planet(&planet, view).unwrap();
        plan_times.push(started.elapsed());
        latest_plan = Some(plan);
    }
    plan_times.sort_unstable();
    let plan_p95 = plan_times[18];
    let latest_plan = latest_plan.unwrap();
    let planning_core = TerrainCore::new(
        planet.planet_id,
        planet.root_lod,
        planet.generator().unwrap(),
    )
    .unwrap();
    let mut authoritative_plan_times = Vec::with_capacity(20);
    let mut latest_authoritative_plan = None;
    for _ in 0..20 {
        let started = Instant::now();
        let plan = planner
            .plan_with_classifier(&planet, view, &planning_core)
            .unwrap();
        authoritative_plan_times.push(started.elapsed());
        latest_authoritative_plan = Some(plan);
    }
    authoritative_plan_times.sort_unstable();
    let authoritative_plan_p95 = authoritative_plan_times[18];
    let latest_authoritative_plan = latest_authoritative_plan.unwrap();
    assert_eq!(latest_authoritative_plan, latest_plan);

    let mut refinement =
        TerrainRefinementFrontier::new(planet.planet_id, TerrainRefinementConfig::default())
            .unwrap();
    refinement.set_target(&latest_authoritative_plan).unwrap();
    let mut stationary_reconcile_times = Vec::with_capacity(1_000);
    for _ in 0..1_000 {
        let started = Instant::now();
        assert!(refinement
            .set_target(&latest_authoritative_plan)
            .unwrap()
            .is_empty());
        stationary_reconcile_times.push(started.elapsed());
    }
    stationary_reconcile_times.sort_unstable();
    let stationary_reconcile_p95 = stationary_reconcile_times[949];
    assert!(
        stationary_reconcile_p95 <= Duration::from_micros(500),
        "stationary refinement orchestration exceeded the 0.5 ms acceptance gate: {stationary_reconcile_p95:?}"
    );
    let mut terrain_subsystem = TerrainSubsystem::new(TerrainRuntimeConfig {
        worker_count: 1,
        ..TerrainRuntimeConfig::default()
    })
    .unwrap();
    terrain_subsystem.init(&SubsystemContext::new()).unwrap();
    let runtime = terrain_subsystem.runtime_handle();
    runtime.upsert_planet(planet.clone()).unwrap();
    let planning = terrain_subsystem.planning_handle();
    let planning_config = TerrainPlanningConfig {
        streaming: planner.config(),
        ..TerrainPlanningConfig::default()
    };
    let first_planning_ticket = planning
        .submit(planet.planet_id, view, planning_config)
        .unwrap();
    let planning_deadline = Instant::now() + Duration::from_secs(10);
    let async_plan = loop {
        if let Some(result) = planning
            .drain_completed(1)
            .into_iter()
            .find(|result| result.ticket() == first_planning_ticket)
        {
            break result.into_plan().unwrap();
        }
        assert!(
            Instant::now() < planning_deadline,
            "asynchronous authoritative plan timed out"
        );
        std::thread::yield_now();
    };
    assert_eq!(async_plan, latest_authoritative_plan);
    let mut stationary_planning_submit_times = Vec::with_capacity(1_000);
    for _ in 0..1_000 {
        let started = Instant::now();
        assert_eq!(
            planning
                .submit(planet.planet_id, view, planning_config)
                .unwrap(),
            first_planning_ticket
        );
        let _ = planning.drain_completed(1);
        stationary_planning_submit_times.push(started.elapsed());
    }
    stationary_planning_submit_times.sort_unstable();
    let stationary_planning_submit_p95 = stationary_planning_submit_times[949];
    assert!(
        stationary_planning_submit_p95 <= Duration::from_micros(500),
        "stationary asynchronous planning submission exceeded the 0.5 ms acceptance gate: {stationary_planning_submit_p95:?}"
    );
    let mut active_planning_submit_times = Vec::with_capacity(1_000);
    for tick in 1..=1_000_i64 {
        let moving_view = PlanetView::new(
            PlanetPosition::from_lod0_cell(
                [10_371_000 + tick * 10, 0, 0],
                planet.lod0_cell_size_mm,
            )
            .unwrap(),
            [-1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            60_f64.to_radians(),
            [2560, 1440],
            0.1,
            20_000_000.0,
            [10.0, 0.0, 0.0],
        )
        .unwrap();
        let started = Instant::now();
        planning
            .submit(planet.planet_id, moving_view, planning_config)
            .unwrap();
        let _ = planning.drain_completed(1);
        active_planning_submit_times.push(started.elapsed());
    }
    active_planning_submit_times.sort_unstable();
    let active_planning_submit_p95 = active_planning_submit_times[949];
    let active_planning_submit_max = active_planning_submit_times[999];
    assert!(
        active_planning_submit_p95 <= Duration::from_micros(500),
        "active asynchronous planning submission exceeded the 0.5 ms acceptance gate: {active_planning_submit_p95:?}"
    );
    assert!(
        active_planning_submit_max <= Duration::from_millis(25),
        "active asynchronous planning submission caused a frame spike: {active_planning_submit_max:?}"
    );
    let planning_counters = planning.counters();
    assert!(planning_counters.pending <= 1);
    assert!(planning_counters.completed <= 1);
    let mut residency = TerrainIncrementalResidencySession::new(
        planet.planet_id,
        TerrainRefinementConfig::default(),
    )
    .unwrap();
    let mut publisher =
        TerrainRenderDeltaPublisher::new(TerrainRenderDeltaConfig::default()).unwrap();
    residency
        .reconcile(&runtime, &mut publisher, &latest_authoritative_plan, 0)
        .unwrap();
    let mut active_reconcile_times = Vec::with_capacity(1_000);
    for tick in 1..=1_000 {
        let plan = if tick & 1 == 0 {
            &latest_authoritative_plan
        } else {
            &latest_plan
        };
        let started = Instant::now();
        residency
            .reconcile(&runtime, &mut publisher, plan, tick)
            .unwrap();
        active_reconcile_times.push(started.elapsed());
    }
    active_reconcile_times.sort_unstable();
    let active_reconcile_p95 = active_reconcile_times[949];
    assert!(
        active_reconcile_p95 <= Duration::from_micros(500),
        "active refinement orchestration exceeded the 0.5 ms acceptance gate: {active_reconcile_p95:?}"
    );

    const PERSISTENCE_EDIT_HISTORY: u64 = 10_000;
    let persistence_planet = PlanetDefinition {
        planet_id: PlanetId([12; 16]),
        root_lod: 24,
        ..planet.clone()
    };
    runtime.upsert_planet(persistence_planet.clone()).unwrap();
    for sequence in 1..=PERSISTENCE_EDIT_HISTORY {
        let mut stable_id = [0_u8; 16];
        stable_id[..8].copy_from_slice(&sequence.to_le_bytes());
        stable_id[8..].copy_from_slice(b"persist!");
        runtime
            .append_edit(
                persistence_planet.planet_id,
                EditOp {
                    sequence,
                    stable_id,
                    shape: EditShape::Sphere {
                        center_cell: [sequence as i64 * 3 - 15_000, 0, 0],
                        radius_cells: 1,
                    },
                    mode: EditMode::Subtract,
                    material: 0,
                },
            )
            .unwrap();
    }
    virtual_fs::reset_to_local();
    let persistence_directory = tempfile::tempdir().unwrap();
    let persistence_store = TerrainStore::new(persistence_directory.path().join("terrain"));
    let persistence = terrain_subsystem.persistence_handle();
    let mut save_submit_times = Vec::with_capacity(20);
    let mut saved_hash = None;
    let persistence_started = Instant::now();
    for _ in 0..20 {
        let submit_started = Instant::now();
        persistence
            .request_save(persistence_planet.planet_id, persistence_store.clone())
            .unwrap();
        save_submit_times.push(submit_started.elapsed());
        match wait_for_persistence_event(&persistence) {
            TerrainPersistenceEvent::Saved { snapshot_hash, .. } => {
                assert!(saved_hash.is_none_or(|previous| previous == snapshot_hash));
                saved_hash = Some(snapshot_hash);
            }
            event => panic!("unexpected persistence benchmark event: {event:?}"),
        }
    }
    let persistence_save_elapsed = persistence_started.elapsed();
    save_submit_times.sort_unstable();
    let save_submit_p95 = save_submit_times[18];
    assert!(
        save_submit_p95 <= Duration::from_millis(10),
        "10,000-edit persistence capture exceeded the 10 ms frame gate: {save_submit_p95:?}"
    );
    runtime
        .set_root(persistence_planet.planet_id, NodeState::Air)
        .unwrap();
    runtime.drain_events(64);
    let restore_started = Instant::now();
    persistence
        .request_restore(persistence_planet.planet_id, persistence_store)
        .unwrap();
    match wait_for_persistence_event(&persistence) {
        TerrainPersistenceEvent::Restored { snapshot_hash, .. } => {
            assert_eq!(Some(snapshot_hash), saved_hash)
        }
        event => panic!("unexpected persistence benchmark event: {event:?}"),
    }
    let persistence_restore_elapsed = restore_started.elapsed();
    terrain_subsystem.shutdown().unwrap();

    let ground_planet = PlanetDefinition {
        max_resident_pages: 8_192,
        ..planet.clone()
    };
    let ground_view = PlanetView::new(
        PlanetPosition::from_lod0_cell(
            [EARTH_RADIUS_CELLS as i64, 0, 0],
            ground_planet.lod0_cell_size_mm,
        )
        .unwrap(),
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
            .plan_planet(&ground_planet, ground_view)
            .unwrap();
        ground_times.push(started.elapsed());
        latest_ground_plan = Some(plan);
    }
    ground_times.sort_unstable();
    let ground_p95 = ground_times[8];
    let latest_ground_plan = latest_ground_plan.unwrap();
    let mut churn_subsystem = TerrainSubsystem::new(TerrainRuntimeConfig {
        worker_count: 1,
        ..TerrainRuntimeConfig::default()
    })
    .unwrap();
    churn_subsystem.init(&SubsystemContext::new()).unwrap();
    let churn_runtime = churn_subsystem.runtime_handle();
    churn_runtime.upsert_planet(planet.clone()).unwrap();
    let mut churn_residency = TerrainIncrementalResidencySession::new(
        planet.planet_id,
        TerrainRefinementConfig::default(),
    )
    .unwrap();
    let mut churn_publisher =
        TerrainRenderDeltaPublisher::new(TerrainRenderDeltaConfig::default()).unwrap();
    churn_residency
        .reconcile(
            &churn_runtime,
            &mut churn_publisher,
            &latest_authoritative_plan,
            0,
        )
        .unwrap();
    let mut churn_reconcile_times = Vec::with_capacity(1_000);
    for tick in 1..=1_000 {
        let plan = if tick & 1 == 0 {
            &latest_authoritative_plan
        } else {
            &latest_ground_plan
        };
        let started = Instant::now();
        churn_residency
            .reconcile(&churn_runtime, &mut churn_publisher, plan, tick)
            .unwrap();
        churn_reconcile_times.push(started.elapsed());
    }
    churn_reconcile_times.sort_unstable();
    let churn_reconcile_p95 = churn_reconcile_times[949];
    assert!(
        churn_reconcile_p95 <= Duration::from_micros(500),
        "superseded-plan refinement orchestration exceeded the 0.5 ms acceptance gate: {churn_reconcile_p95:?}, cancelled_stages={}",
        churn_residency.counters().stages_cancelled,
    );
    churn_subsystem.shutdown().unwrap();
    let mut switching_refinement = TerrainRefinementFrontier::new(
        planet.planet_id,
        TerrainRefinementConfig {
            max_active_pages: 8_192,
            max_transition_pages: 8_256,
            ..TerrainRefinementConfig::default()
        },
    )
    .unwrap();
    let mut plan_switch_times = Vec::with_capacity(1_000);
    for index in 0..1_000 {
        let plan = if index & 1 == 0 {
            &latest_authoritative_plan
        } else {
            &latest_ground_plan
        };
        let started = Instant::now();
        switching_refinement.set_target(plan).unwrap();
        plan_switch_times.push(started.elapsed());
    }
    plan_switch_times.sort_unstable();
    let plan_switch_p95 = plan_switch_times[949];
    assert!(
        plan_switch_p95 <= Duration::from_micros(500),
        "new-plan refinement orchestration exceeded the 0.5 ms acceptance gate: {plan_switch_p95:?}"
    );

    // A billion logical cells are represented by the root without allocation.
    let logical_dense_bytes = 1_000_000_000_u64 * 4;
    println!(
        "terrain_long_history edits={DELETE_EDIT_PREFIX} capture_p95_us={:.3}",
        long_history_capture_p95.as_secs_f64() * 1_000_000.0,
    );
    println!(
        "terrain_persistence edits={PERSISTENCE_EDIT_HISTORY} save_capture_p95_ms={:.3} save_e2e_average_ms={:.3} restore_e2e_ms={:.3}",
        save_submit_p95.as_secs_f64() * 1_000.0,
        persistence_save_elapsed.as_secs_f64() * 1_000.0 / 20.0,
        persistence_restore_elapsed.as_secs_f64() * 1_000.0,
    );
    println!(
        "terrain_core sparse_touches={TOUCHES} nodes={} sparse_ms={:.3} dense_sample_cells={} dense_sample_bytes={} dense_fill_ms={:.3} billion_dense_equivalent_bytes={logical_dense_bytes} edited_page_bytes={} resident_dense_bytes={} generated_cells={} edit_attachment_regions={} edit_attachment_refs={} edit_candidates_replayed={} edit_compact_ms={:.3} coarse_lod=12 coarse_generated_cells={} coarse_edit_candidates={} coarse_compact_ms={:.3} edit_radius_cells=[1,10,100,1000] edit_aabb_pages={edit_amplification:?} root_delete_prefix_ops={DELETE_EDIT_PREFIX} root_delete_us={:.3} orbit_plan_pages={} orbit_plan_nodes={} orbit_uniform_pruned={} orbit_plan_p95_ms={:.3} orbit_authoritative_p95_ms={:.3} orbit_plan_limits={:?} async_stationary_submit_p95_us={:.3} async_active_submit_p95_us={:.3} async_active_submit_max_us={:.3} async_submitted={} async_coalesced={} async_superseded={} async_stale={} async_capture_max_us={:.3} async_plan_max_ms={:.3} stationary_refinement_p95_us={:.3} active_refinement_p95_us={:.3} superseded_refinement_p95_us={:.3} new_plan_refinement_p95_us={:.3} ground_plan_pages={} ground_plan_nodes={} ground_uniform_pruned={} ground_plan_p95_ms={:.3} ground_plan_limits={:?}",
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
        latest_plan.counters().uniform_regions_pruned,
        plan_p95.as_secs_f64() * 1_000.0,
        authoritative_plan_p95.as_secs_f64() * 1_000.0,
        latest_plan.limits(),
        stationary_planning_submit_p95.as_secs_f64() * 1_000_000.0,
        active_planning_submit_p95.as_secs_f64() * 1_000_000.0,
        active_planning_submit_max.as_secs_f64() * 1_000_000.0,
        planning_counters.submitted,
        planning_counters.coalesced,
        planning_counters.superseded_pending,
        planning_counters.stale_results,
        planning_counters.longest_capture_nanoseconds as f64 / 1_000.0,
        planning_counters.longest_plan_nanoseconds as f64 / 1_000_000.0,
        stationary_reconcile_p95.as_secs_f64() * 1_000_000.0,
        active_reconcile_p95.as_secs_f64() * 1_000_000.0,
        churn_reconcile_p95.as_secs_f64() * 1_000_000.0,
        plan_switch_p95.as_secs_f64() * 1_000_000.0,
        latest_ground_plan.demands().len(),
        latest_ground_plan.counters().traversed_nodes,
        latest_ground_plan.counters().uniform_regions_pruned,
        ground_p95.as_secs_f64() * 1_000.0,
        latest_ground_plan.limits(),
    );
}
