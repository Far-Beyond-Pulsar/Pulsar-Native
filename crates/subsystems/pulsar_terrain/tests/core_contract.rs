use pulsar_terrain::{
    CellWord, DeterministicGenerator, EditLog, EditMode, EditOp, EditShape,
    FixedSphereGenerator, NodeState, PageKey, SparseBrickTree, VoxelPage,
};

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
    assert_eq!((word.density(), word.material(), word.flags()), (-123, 17, 9));
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
        let coordinate = |random: &mut u64| {
            (next_random(random) % (2 * half) as u64) as i64 - half
        };
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
    assert_eq!(tree.resolve(PageKey::new(0, [-16; 3])).unwrap(), NodeState::Solid(9));
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
