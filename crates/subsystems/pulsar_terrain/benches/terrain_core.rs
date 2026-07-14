use pulsar_terrain::{
    CellWord, ContentHash, EditMode, EditOp, EditShape, FixedSphereGenerator, NodeState,
    PageKey, PlanetId, SparseBrickTree, TerrainCore,
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

    // A billion logical cells are represented by the root without allocation.
    let logical_dense_bytes = 1_000_000_000_u64 * 4;
    println!(
        "terrain_core sparse_touches={TOUCHES} nodes={} sparse_ms={:.3} dense_sample_cells={} dense_sample_bytes={} dense_fill_ms={:.3} billion_dense_equivalent_bytes={logical_dense_bytes} edited_page_bytes={} resident_dense_bytes={} generated_cells={} edit_compact_ms={:.3} edit_radius_cells=[1,10,100,1000] edit_aabb_pages={edit_amplification:?} root_delete_us={:.3}",
        sparse.node_count(),
        sparse_time.as_secs_f64() * 1_000.0,
        dense.len(),
        dense.len() * std::mem::size_of::<CellWord>(),
        dense_time.as_secs_f64() * 1_000.0,
        core.page(compacted.key).unwrap().encode().len(),
        memory.resident_dense_bytes,
        work.cells_generated,
        edit_time.as_secs_f64() * 1_000.0,
        delete_time.as_secs_f64() * 1_000_000.0,
    );
}
