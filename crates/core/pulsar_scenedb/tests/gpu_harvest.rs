//! `HarvestPipeline` verification (M2b-b T6, design Rev 2 S5, spec S8.3-8.5,
//! C4): per-view single-scan partition emitting global-row tokens with
//! scalar DEI dense compaction. Real surfaceless wgpu device (same headless
//! harness as `gpu_store.rs`); the test harness owns the `device.poll` pump.

use pulsar_scenedb::gpu::{
    EngineGpuContext, FrameDriver, HarvestPipeline, HarvestStaging, MeshClass, RegionClassConfig,
    SceneGpuConfig, SceneGpuStore, View,
};
use pulsar_scenedb::{Aabb, Scratchpad, SpatialCell};
use std::sync::Arc;

fn test_context() -> EngineGpuContext {
    // Fork rev fce5b80 (wgpu 28 API): `Instance::new` takes an owned
    // `InstanceDescriptor`, not a reference.
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .expect("no adapter — GPU tests need a local GPU");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("scenedb-harvest-test"),
        ..Default::default()
    }))
    .expect("device");
    EngineGpuContext::new(Arc::new(device), Arc::new(queue))
}

/// Kept verbatim from `gpu_store.rs`'s helper for parity with the rest of the
/// GPU test suite, though this file's assertions are all CPU-side (staging
/// arrays); no test currently reads back a GPU buffer.
#[allow(dead_code)]
fn readback(ctx: &EngineGpuContext, buf: &wgpu::Buffer, bytes: u64) -> Vec<u8> {
    let staging = ctx.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: bytes,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut enc = ctx.device().create_command_encoder(&Default::default());
    enc.copy_buffer_to_buffer(buf, 0, &staging, 0, bytes);
    ctx.queue().submit([enc.finish()]);
    let slice = staging.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("map"));
    // Fork rev fce5b80: `PollType::Wait` is a struct variant
    // (`{ submission_index, timeout }`), not a unit variant; use the
    // `wait_indefinitely()` convenience constructor instead.
    ctx.device()
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let data = slice.get_mapped_range().to_vec();
    staging.unmap();
    data
}

fn scene_cfg() -> SceneGpuConfig {
    SceneGpuConfig {
        classes: vec![RegionClassConfig { capacity: 64, max_resident_cells: 4 }],
        tombstone_headroom: 8,
        max_materials: 16,
        max_cells_metadata: 16,
    }
}

/// A `SpatialCell::with_transform` cell populated with `count` unit boxes:
/// box `i` spans `[x_offset + i, x_offset + i + 1)` on x (y/z pinned to
/// `[0,1]`) — a densely positional layout so a query's hit set is exactly
/// predictable from its x-range alone.
fn boxed_cell(capacity: u32, count: u32, x_offset: f32) -> SpatialCell {
    let mut cell = SpatialCell::with_transform(capacity).unwrap();
    for i in 0..count {
        let x = x_offset + i as f32;
        cell.alloc(Aabb { min: [x, 0.0, 0.0], max: [x + 1.0, 1.0, 1.0] }).unwrap();
    }
    cell
}

#[test]
fn harvest_routes_global_tokens_by_class_and_never_offsets_sentinels() {
    let ctx = test_context();
    let mut store = SceneGpuStore::new(&ctx, scene_cfg());

    // Two cells, disjoint regions (both class 0 — the region POOL hands out
    // distinct bases, not the class index).
    let cell_a = boxed_cell(64, 4, 0.0);
    let cell_b = boxed_cell(64, 4, 100.0);
    let id_a = store.register_cell(cell_a.storage(), 0).unwrap();
    let id_b = store.register_cell(cell_b.storage(), 0).unwrap();
    let base_a = store.row_region_base(id_a);
    let base_b = store.row_region_base(id_b);
    assert_ne!(base_a, base_b, "disjoint regions");

    let mut frames = FrameDriver::new();
    let h = frames.begin().end().end();
    let pipeline = HarvestPipeline::new();
    let mut pad = Scratchpad::new();
    let mut staging = HarvestStaging::new();

    // Boxes are [i, i+1) for i in 0..4 (A) / 100..104 (B). Query [1.5, 2.5]
    // (offset +100 for B) overlaps only local rows 1 ([1,2)) and 2 ([2,3));
    // rows 0 ([0,1)) and 3 ([3,4)) fall entirely outside — exactly 2 hits.
    let view_a = View::Aabb(Aabb { min: [1.5, 0.0, 0.0], max: [2.5, 1.0, 1.0] });
    let n_a =
        pipeline.harvest_cell(&cell_a, base_a, MeshClass::Traditional, &view_a, &mut pad, &mut staging, &h);
    let view_b = View::Aabb(Aabb { min: [101.5, 0.0, 0.0], max: [102.5, 1.0, 1.0] });
    let n_b =
        pipeline.harvest_cell(&cell_b, base_b, MeshClass::VirtualGeometry, &view_b, &mut pad, &mut staging, &h);

    assert_eq!(n_a, 2, "A: 2 hits");
    assert_eq!(n_b, 2, "B: 2 hits");
    assert_eq!(
        staging.traditional,
        vec![base_a + 1, base_a + 2],
        "A routed to Traditional, every token offset by its own region base"
    );
    assert_eq!(
        staging.vg,
        vec![base_b + 1, base_b + 2],
        "B routed to VirtualGeometry, every token offset by its own region base"
    );
    assert!(staging.hlod.is_empty(), "nothing harvested as HlodProxy");

    // Sentinel never offset (S2): no value in ANY staging array is
    // NULL_ROW-derived. `region_base + NULL_ROW` would wrap to a value >=
    // 0xFFFF_0000 for any region base used in this test (both are tiny), so
    // this threshold catches an offset sentinel as reliably as an exact
    // 0xFFFF_FFFF check would.
    for arr in [&staging.traditional, &staging.vg, &staging.hlod, &staging.remap] {
        for &v in arr.iter() {
            assert!(v < 0xFFFF_0000, "sentinel-derived value leaked into staging: {v:#x}");
        }
    }

    assert_eq!(staging.stats.tokens_valid, 4, "2 + 2 valid tokens across both cells");
    assert_eq!(staging.stats.tokens_total, 8, "4 + 4 physical rows scanned across both cells");
    assert_eq!(staging.stats.cells, 2);
    assert_eq!(staging.stats.dei_compacted_runs, 0, "both runs are well above the 25% DEI threshold");
}

#[test]
fn dei_below_quarter_compacts_with_roundtrip_remap() {
    let ctx = test_context();
    let mut store = SceneGpuStore::new(&ctx, scene_cfg());

    // Cell 1: 64 rows, exactly 8 hits (12.5% < 25%) -> DEI dense compaction.
    let cell1 = boxed_cell(64, 64, 0.0);
    let id1 = store.register_cell(cell1.storage(), 0).unwrap();
    let base1 = store.row_region_base(id1);

    let mut frames = FrameDriver::new();
    let h = frames.begin().end().end();
    let pipeline = HarvestPipeline::new();
    let mut pad = Scratchpad::new();
    let mut staging = HarvestStaging::new();

    // box i = [i, i+1); query [10.5, 17.5] hits i in {10..=17} -> 8 hits.
    let view1 = View::Aabb(Aabb { min: [10.5, 0.0, 0.0], max: [17.5, 1.0, 1.0] });
    let n1 =
        pipeline.harvest_cell(&cell1, base1, MeshClass::Traditional, &view1, &mut pad, &mut staging, &h);
    assert_eq!(n1, 8, "12.5% hit ratio");
    assert_eq!(staging.traditional.len(), 8, "dense array holds exactly the 8 hits");
    assert_eq!(staging.remap.len(), 8, "remap grew by exactly 8 entries");
    assert_eq!(staging.stats.dei_compacted_runs, 1);

    for i in 0..8usize {
        // remap[dense_i] = original_run_index (C4 M3-frozen layout). The
        // query's positional-token contract writes `tokens[row] == row` on a
        // hit, so the original run index IS the local row that hit.
        let run_index = staging.remap[i];
        assert!((10..=17).contains(&run_index), "remap[{i}]={run_index} must be a real hit row");
        assert_eq!(
            staging.traditional[i],
            base1 + run_index,
            "dense[{i}] == region_base + remap[{i}] (roundtrip)"
        );
    }
    // Every hit row appears in remap exactly once.
    let mut sorted = staging.remap.clone();
    sorted.sort_unstable();
    assert_eq!(sorted, (10u32..=17).collect::<Vec<_>>(), "remap covers exactly the hit set, once each");

    // Cell 2: a fresh 64-row cell, exactly 32 hits (50% >= 25%) -> plain
    // path; the DEI counter must not move.
    let cell2 = boxed_cell(64, 64, 1000.0);
    let id2 = store.register_cell(cell2.storage(), 0).unwrap();
    let base2 = store.row_region_base(id2);
    // box i = [1000+i, 1000+i+1); query [999.5, 1031.5] hits i in {0..=31}.
    let view2 = View::Aabb(Aabb { min: [999.5, 0.0, 0.0], max: [1031.5, 1.0, 1.0] });
    let n2 =
        pipeline.harvest_cell(&cell2, base2, MeshClass::Traditional, &view2, &mut pad, &mut staging, &h);
    assert_eq!(n2, 32, "50% hit ratio");
    assert_eq!(
        staging.stats.dei_compacted_runs, 1,
        "a 50%-hit run takes the plain path — the DEI counter from cell 1 is untouched"
    );
    assert_eq!(staging.traditional.len(), 8 + 32, "plain path appended to the same dest array");
    assert_eq!(staging.remap.len(), 8, "plain path never touches remap");
}

#[test]
fn harvest_makes_zero_new_allocations_after_warmup() {
    let ctx = test_context();
    let mut store = SceneGpuStore::new(&ctx, scene_cfg());
    let cell = boxed_cell(64, 64, 0.0);
    let id = store.register_cell(cell.storage(), 0).unwrap();
    let base = store.row_region_base(id);

    let mut frames = FrameDriver::new();
    let h = frames.begin().end().end();
    let pipeline = HarvestPipeline::new();
    let mut pad = Scratchpad::new();
    let mut staging = HarvestStaging::new();

    // Same cell + same view both runs: box i = [i, i+1); query [-0.5, 31.5]
    // hits i in {0..=31} -> 32/64 = 50%, plain path both times.
    let view = View::Aabb(Aabb { min: [-0.5, 0.0, 0.0], max: [31.5, 1.0, 1.0] });
    let n1 = pipeline.harvest_cell(&cell, base, MeshClass::Traditional, &view, &mut pad, &mut staging, &h);
    assert_eq!(n1, 32);

    let pad_u32_after1 = pad.buf_len_u32();
    let pad_u64_after1 = pad.buf_len_u64();
    let cap_trad = staging.traditional.capacity();
    let cap_vg = staging.vg.capacity();
    let cap_hlod = staging.hlod.capacity();
    let cap_remap = staging.remap.capacity();

    // Clear WITHOUT freeing (S8.1) — a fresh `HarvestStaging::new()` here
    // would defeat the entire point of this test.
    staging.clear();

    let n2 = pipeline.harvest_cell(&cell, base, MeshClass::Traditional, &view, &mut pad, &mut staging, &h);
    assert_eq!(n2, 32, "same cell + view -> identical hit count on the second run");

    assert_eq!(pad.buf_len_u32(), pad_u32_after1, "scratch u32 buffer size unchanged after warmup");
    assert_eq!(pad.buf_len_u64(), pad_u64_after1, "scratch u64 buffer size unchanged after warmup");
    assert_eq!(staging.traditional.capacity(), cap_trad, "traditional capacity unchanged");
    assert_eq!(staging.vg.capacity(), cap_vg, "vg capacity unchanged");
    assert_eq!(staging.hlod.capacity(), cap_hlod, "hlod capacity unchanged");
    assert_eq!(staging.remap.capacity(), cap_remap, "remap capacity unchanged (plain path never touches it)");
}
