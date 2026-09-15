use engine_subsystems::{Subsystem, SubsystemContext};
use pulsar_terrain::{
    CellWord, DeterministicGenerator, EditLog, EditMode, EditOp, EditShape, FixedSphereGenerator,
    NodeState, PageKey, PlanetId, SparseBrickTree, TerrainCore, VoxelPage,
};
use pulsar_terrain::{
    PlanetDefinition, PlanetPosition, TerrainPersistenceConfig, TerrainPersistenceEvent,
    TerrainPlanningConfig, TerrainRuntimeConfig, TerrainStreamingConfig, TerrainSubsystem,
};
use std::time::{Duration, Instant};

fn sphere() -> FixedSphereGenerator {
    FixedSphereGenerator {
        center_cell: [0; 3],
        radius_cells: 64,
        material: 2,
    }
}

#[test]
fn edit_order_is_deterministic_and_changes_page_hash() {
    let generator = sphere();
    let key = PageKey::new(0, [-1, 0, 0]);
    let base = VoxelPage::generate(key, &generator, &EditLog::default()).unwrap();
    let operation = EditOp {
        sequence: 1,
        stable_id: [1; 16],
        shape: EditShape::Sphere {
            center_cell: [-8, 8, 8],
            radius_cells: 6,
        },
        mode: EditMode::Subtract,
        material: 0,
    };
    let mut edits = EditLog::default();
    edits.push(operation).unwrap();
    edits.push(operation).unwrap();
    let edited_a = VoxelPage::generate(key, &generator, &edits).unwrap();
    let edited_b = VoxelPage::generate(key, &generator, &edits).unwrap();
    assert_ne!(base.page_id(), edited_a.page_id());
    assert_eq!(edited_a.page_id(), edited_b.page_id());
}

#[test]
fn identical_core_inputs_produce_identical_hierarchy_page_and_snapshot_hashes() {
    let build = || {
        let mut core = TerrainCore::new(PlanetId([7; 16]), 12, sphere()).unwrap();
        for sequence in 1..=3_u64 {
            core.append_edit(EditOp {
                sequence,
                stable_id: [sequence as u8; 16],
                shape: EditShape::Sphere {
                    center_cell: [sequence as i64 * 3, 8, -4],
                    radius_cells: sequence as u32 + 1,
                },
                mode: EditMode::Subtract,
                material: 0,
            })
            .unwrap();
        }
        let page = core.compact_page(PageKey::new(0, [0, 0, -1])).unwrap();
        (
            page.page_id,
            core.hierarchy().content_hash(),
            core.snapshot().content_hash().unwrap(),
        )
    };

    assert_eq!(build(), build());
}

#[test]
fn billion_cell_logical_region_cost_depends_on_touched_paths() {
    let mut tree = SparseBrickTree::centered(24, NodeState::Procedural(sphere().hash())).unwrap();
    for index in 0..128 {
        tree.set(
            PageKey::new(0, [index * 1024 - 65_536, -index * 37, index * 11]),
            NodeState::Page(pulsar_terrain::ContentHash::of(&index.to_le_bytes())),
        )
        .unwrap();
    }
    assert!(tree.node_count() <= 1 + 128 * 8 * 24);
    tree.set_root(NodeState::Air).unwrap();
    assert_eq!(tree.node_count(), 1);
}

#[test]
fn cell_word_layout_is_exactly_four_bytes() {
    assert_eq!(std::mem::size_of::<CellWord>(), 4);
    let word = CellWord::new(-123, 17, 9);
    assert_eq!(
        (word.density(), word.material(), word.flags()),
        (-123, 17, 9)
    );
}

fn next_random(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *state
}

#[test]
fn randomized_sparse_hierarchy_matches_a_dense_reference_fixture() {
    const ROOT_LOD: u8 = 5;
    const ROOT_EDGE: usize = 1 << ROOT_LOD;
    const ROOT_MIN: i64 = -(ROOT_EDGE as i64 / 2);
    let mut tree = SparseBrickTree::centered(ROOT_LOD, NodeState::Air).unwrap();
    let mut dense = vec![NodeState::Air; ROOT_EDGE * ROOT_EDGE * ROOT_EDGE];
    let mut random = 0x4d59_5df4_d0f3_3173_u64;

    for operation in 0..512 {
        let lod = (next_random(&mut random) % 4) as u8;
        let half = 1_i64 << (ROOT_LOD - 1 - lod);
        let coordinate = |random: &mut u64| (next_random(random) % (2 * half) as u64) as i64 - half;
        let key = PageKey::new(
            lod,
            [
                coordinate(&mut random),
                coordinate(&mut random),
                coordinate(&mut random),
            ],
        );
        let state = if operation % 5 == 0 {
            NodeState::Air
        } else {
            NodeState::Solid((operation % 13 + 1) as u8)
        };
        tree.set(key, state.clone()).unwrap();

        let min = key.lod0_min().unwrap();
        let edge = 1_i64 << lod;
        for z in min[2]..min[2] + edge {
            for y in min[1]..min[1] + edge {
                for x in min[0]..min[0] + edge {
                    let local = [x - ROOT_MIN, y - ROOT_MIN, z - ROOT_MIN];
                    let index = local[0] as usize
                        + ROOT_EDGE * (local[1] as usize + ROOT_EDGE * local[2] as usize);
                    dense[index] = state.clone();
                }
            }
        }
    }

    for z in ROOT_MIN..ROOT_MIN + ROOT_EDGE as i64 {
        for y in ROOT_MIN..ROOT_MIN + ROOT_EDGE as i64 {
            for x in ROOT_MIN..ROOT_MIN + ROOT_EDGE as i64 {
                let local = [x - ROOT_MIN, y - ROOT_MIN, z - ROOT_MIN];
                let index = local[0] as usize
                    + ROOT_EDGE * (local[1] as usize + ROOT_EDGE * local[2] as usize);
                assert_eq!(
                    tree.resolve(PageKey::new(0, [x, y, z])).unwrap(),
                    dense[index],
                    "sparse mismatch at [{x}, {y}, {z}]"
                );
            }
        }
    }

    let decoded = SparseBrickTree::decode(&tree.encode()).unwrap();
    assert_eq!(decoded, tree);
    tree.set_root(NodeState::Solid(9)).unwrap();
    assert_eq!(tree.node_count(), 1);
    assert_eq!(
        tree.resolve(PageKey::new(0, [-16; 3])).unwrap(),
        NodeState::Solid(9)
    );
}

#[test]
fn randomized_page_codecs_are_canonical_and_deterministic() {
    let mut random = 0xa076_1d64_78bd_642f_u64;
    for fixture in 0..32 {
        let mut cells = Vec::with_capacity(pulsar_terrain::CELL_COUNT);
        let mut current = CellWord::AIR;
        for index in 0..pulsar_terrain::CELL_COUNT {
            if index == 0 || next_random(&mut random) % 19 == 0 {
                current = CellWord::new(
                    next_random(&mut random) as i16,
                    next_random(&mut random) as u8,
                    fixture,
                );
            }
            cells.push(current);
        }
        let page = VoxelPage::from_cells(cells).unwrap();
        let encoded = page.encode();
        let decoded = VoxelPage::decode(&encoded).unwrap();
        assert_eq!(decoded, page);
        assert_eq!(decoded.encode(), encoded);
        assert_eq!(decoded.page_id(), page.page_id());
    }
}

fn integration_definition(id: u8) -> PlanetDefinition {
    PlanetDefinition {
        planet_id: PlanetId([id; 16]),
        center_cell: [0; 3],
        radius_cells: 1_000,
        material: id.max(1),
        root_lod: 8,
        max_resident_pages: 64,
    }
}

fn integration_runtime_config() -> TerrainRuntimeConfig {
    TerrainRuntimeConfig {
        worker_count: 1,
        max_planets: 4,
        max_component_sources: 4,
        request_capacity: 32,
        critical_request_reserve: 4,
        completion_capacity: 32,
        event_capacity: 64,
        max_resident_pages: 64,
        max_resident_dense_bytes: 64 * pulsar_terrain::CELL_COUNT * 4,
        max_completions_per_frame: 16,
    }
}

#[test]
fn removed_planet_cancels_background_plan_without_publishing_a_miss() {
    let mut subsystem = TerrainSubsystem::new(integration_runtime_config()).unwrap();
    subsystem.init(&SubsystemContext::new()).unwrap();
    let runtime = subsystem.runtime_handle();
    let planning = subsystem.planning_handle();
    let definition = integration_definition(20);
    runtime.upsert_planet(definition.clone()).unwrap();

    let view = PlanetPosition::from_lod0_cell([1_000, 0, 0]);
    let view = pulsar_terrain::PlanetView::new(
        view,
        [-1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        60_f64.to_radians(),
        [1280, 720],
        0.1,
        100_000.0,
        [0.0; 3],
    )
    .unwrap();
    planning
        .submit(
            definition.planet_id,
            view,
            TerrainPlanningConfig {
                streaming: TerrainStreamingConfig {
                    max_pages: 64,
                    max_traversal_nodes: 4_096,
                    ..TerrainStreamingConfig::default()
                },
                ..TerrainPlanningConfig::default()
            },
        )
        .unwrap();
    assert!(runtime.remove_planet(definition.planet_id).unwrap());

    let deadline = Instant::now() + Duration::from_secs(5);
    while planning.counters().pending != 0 && Instant::now() < deadline {
        std::thread::yield_now();
    }
    let counters = planning.counters();
    assert_eq!(counters.pending, 0);
    assert_eq!(counters.completed, 0);
    assert!(counters.cancelled >= 1);
    assert!(planning.drain_completed(1).is_empty());
    subsystem.shutdown().unwrap();
}

#[test]
fn persistence_save_completion_is_delivered_and_durable() {
    engine_fs::virtual_fs::reset_to_local();
    let temporary = tempfile::tempdir().unwrap();
    let store = pulsar_terrain::TerrainStore::new(temporary.path().join("terrain"));
    let mut subsystem = TerrainSubsystem::new_with_persistence(
        integration_runtime_config(),
        TerrainPersistenceConfig::default(),
    )
    .unwrap();
    subsystem.init(&SubsystemContext::new()).unwrap();
    let runtime = subsystem.runtime_handle();
    let persistence = subsystem.persistence_handle();
    let definition = integration_definition(21);
    runtime.upsert_planet(definition.clone()).unwrap();
    persistence
        .request_save(definition.planet_id, store.clone())
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut events = Vec::new();
    while events.is_empty() && Instant::now() < deadline {
        persistence.pump(8);
        events.extend(persistence.drain_events(8));
        std::thread::yield_now();
    }
    assert!(matches!(
        events.as_slice(),
        [TerrainPersistenceEvent::Saved { .. }]
    ));
    assert_eq!(persistence.counters().outstanding, 0);
    let (_, snapshot) = store.load_latest_snapshot().unwrap().unwrap();
    let stored_hash = snapshot.content_hash().unwrap();
    assert!(matches!(
        events.as_slice(),
        [TerrainPersistenceEvent::Saved { snapshot_hash, .. }] if *snapshot_hash == stored_hash
    ));
    subsystem.shutdown().unwrap();
}

#[test]
fn host_component_registration_rebinds_source_and_retires_old_planet() {
    let mut subsystem = TerrainSubsystem::new(integration_runtime_config()).unwrap();
    subsystem.init(&SubsystemContext::new()).unwrap();
    let runtime = subsystem.runtime_handle();
    let first = integration_definition(22);
    let second = integration_definition(23);
    runtime
        .upsert_component("host-format:terrain".to_owned(), first.clone())
        .unwrap();
    assert_eq!(runtime.counters().planets, 1);
    runtime
        .upsert_component("host-format:terrain".to_owned(), second.clone())
        .unwrap();
    assert_eq!(runtime.counters().planets, 1);
    assert!(matches!(
        runtime.request_page(
            first.planet_id,
            PageKey::new(0, [0; 3]),
            pulsar_terrain::TerrainRequestClass::Visible,
            0,
        ),
        Err(pulsar_terrain::TerrainRuntimeError::PlanetMissing(id)) if id == first.planet_id
    ));
    assert!(runtime
        .request_page(
            second.planet_id,
            PageKey::new(0, [0; 3]),
            pulsar_terrain::TerrainRequestClass::Visible,
            0,
        )
        .is_ok());
    subsystem.shutdown().unwrap();
}
