//! `HarvestPipeline` verification (M2b-b T6, design Rev 2 S5, spec S8.3-8.5,
//! C4): per-view single-scan partition emitting global-row tokens with
//! scalar DEI dense compaction. Real surfaceless wgpu device (same headless
//! harness as `gpu_store.rs`); the test harness owns the `device.poll` pump.

use pulsar_scenedb::gpu::{
    revalidate_run, CellSlot, EngineGpuContext, FrameDriver, HarvestPipeline, HarvestStaging,
    MeshClass, RegionClassConfig, SceneGpuConfig, SceneGpuStore, View,
};
use pulsar_scenedb::{Aabb, Handle, LeaseMask, LivenessSnapshot, Scratchpad, SpatialCell, NULL_ROW};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

fn test_context() -> EngineGpuContext {
    // Upstream wgpu 30: `Instance::new` still takes an owned
    // `InstanceDescriptor`, but the type no longer derives `Default` — use
    // the `new_without_display_handle()` constructor (headless, no window
    // system connection), equivalent to the fork's bare `default()`.
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
        // Upstream wgpu 30 added this field (limit-bucketing/anti-fingerprint
        // knob); `false` preserves the fork's behavior of exposing the
        // adapter's real limits, unbucketed.
        apply_limit_buckets: false,
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
    // `PollType::Wait` is a struct variant (`{ submission_index, timeout }`),
    // not a unit variant, on both the fork and upstream 30; the
    // `wait_indefinitely()` convenience constructor is unchanged.
    ctx.device()
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    // Upstream wgpu 30: `get_mapped_range()` returns
    // `Result<BufferView, MapRangeError>` instead of a bare `BufferView`.
    let data = slice.get_mapped_range().expect("mapped range").to_vec();
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

/// M3-α T8 review defect 1 fix (and r2 fix — see below): builds a
/// `SpatialCell::with_transform` cell whose LIVE rows carry genuinely
/// DIFFERENT registry generations AND a genuinely NON-IDENTITY row→slot
/// mapping. Every earlier T8 test fixture allocated fresh handles only, and
/// `HandleRegistry::allocate` gives generation 1 to every fresh slot — so no
/// live harvested row anywhere in the suite ever carried a generation other
/// than 1 (mutant M1, `dest_gens.push(1)`, survived).
///
/// **r2 correction:** the ORIGINAL version of this fixture did two
/// free+compact+realloc round trips on one slot and one round trip on
/// another — empirically, that recipe's swaps happened to cancel out, so
/// every live row ended up back at `slot == row` (identity `col0`), which
/// let mutant M3 (`regs[local_row]`, skipping the `col0` indirection)
/// survive too: with `col0` identity, `regs[local_row] == regs[col0
/// [local_row]]` always, so the missing indirection is invisible. This
/// version uses exactly ONE free+compact+realloc round trip: free `h1`
/// (row 1), `compact()` (the LAST live row swaps INTO row 1 — this is what
/// breaks identity permanently, since nothing later moves it back), then
/// realloc a NEW handle appended at the new tail row (recycling `h1`'s
/// freed slot at generation 2). The swapped-in row and the newly-appended
/// row are BOTH now non-identity (`row != slot`), and — because the
/// recycled slot's generation (2) differs from the untouched slots'
/// generation (1) — at least one row's "generation read by naive row-index"
/// (`regs[row]`, what M3 computes) genuinely differs from its "generation
/// read by slot index" (`regs[col0[row]]`, the correct read), independent
/// of `col0`'s exact shape (which callers verify at runtime via
/// `SpatialCell::row_of`/`Handle::index`, not hand-derived here).
///
/// Box positions always land in `[x_offset, x_offset + 5)`, so a single
/// broad AABB query safely covers every live row regardless of which
/// physical row ends up holding which handle.
///
/// Returns the cell plus EVERY handle ever allocated (including the
/// now-dead intermediate from the churn) — callers filter to the
/// currently-live subset via `cell.row_of(h).is_some()` rather than this
/// fixture hard-coding which handles survive, so the churn recipe can change
/// without every call site needing to track it by hand.
fn gen_diverse_boxed_cell(x_offset: f32) -> (SpatialCell, Vec<Handle>) {
    let mut cell = SpatialCell::with_transform(64).unwrap();
    let box_at = |x: f32| Aabb { min: [x, 0.0, 0.0], max: [x + 1.0, 1.0, 1.0] };
    let mut all = Vec::new();

    let h0 = cell.alloc(box_at(x_offset)).unwrap();
    let h1 = cell.alloc(box_at(x_offset + 1.0)).unwrap();
    let h2 = cell.alloc(box_at(x_offset + 2.0)).unwrap();
    let h3 = cell.alloc(box_at(x_offset + 3.0)).unwrap();
    all.extend([h0, h1, h2, h3]);

    // ONE free+compact+realloc round trip on h1's slot: free row 1, compact
    // (the last live row — h3's — swaps INTO row 1, permanently breaking
    // identity for that row since nothing later undoes it), then realloc a
    // new handle appended at the new tail row, recycling the freed slot at
    // generation 2.
    cell.free(h1);
    cell.compact();
    let h1b = cell.alloc(box_at(x_offset + 4.0)).unwrap();
    all.push(h1b);

    (cell, all)
}

/// M3-α T8 review defect 2 fix: builds a small cell for the DEI
/// (dense-compaction) path whose single harvested hit row has a SLOT that
/// genuinely diverges from its ROW index, and a generation that genuinely
/// diverges from the fixture's gen-1 baseline — the exact double-blind spot
/// mutants M2 (DEI: `regs[ri]`, dropping the `col0` slot-column indirection)
/// and M1 (`dest_gens.push(1)`) exploit when every prior DEI fixture was
/// identity-mapped (`col0[row] == row`) and gen-1-uniform.
///
/// 6 "filler" boxes occupy `x_offset .. x_offset + 6` (rows 0..5, generation
/// 1, slot == row, never queried). One filler (originally row 2, slot 2) is
/// freed and compacted away — the LAST live row swaps into row 2 — and then
/// slot 2 is immediately recycled by a brand-new handle appended at the new
/// tail row; that handle's slot (2) no longer matches its row, and its
/// generation (2, from the recycle) no longer matches the gen-1 baseline.
/// Its box sits far outside the filler cluster (`x_offset + 100`) so a query
/// can select it alone: 6 live rows total, so a 1-hit query is 1/6 ≈ 16.7%
/// < 25% — DEI territory.
///
/// Returns the cell, the scrambled hit handle, and its box's x-position (so
/// callers can build a precise single-hit query without needing to know
/// which physical row the handle landed on).
fn scrambled_dei_cell(x_offset: f32) -> (SpatialCell, Handle, f32) {
    let mut cell = SpatialCell::with_transform(64).unwrap();
    let box_at = |x: f32| Aabb { min: [x, 0.0, 0.0], max: [x + 1.0, 1.0, 1.0] };

    let fillers: Vec<Handle> = (0..6u32).map(|i| cell.alloc(box_at(x_offset + i as f32)).unwrap()).collect();

    cell.free(fillers[2]);
    cell.compact();
    let hit_x = x_offset + 100.0;
    let hit = cell.alloc(box_at(hit_x)).unwrap();

    (cell, hit, hit_x)
}

/// Independently recompute the expected generation for every `token` in
/// `tokens` (each `base + row`): find, among `handles`, the one CURRENTLY
/// occupying that row (via `SpatialCell::row_of`, public API) and read its
/// generation. This is the test-side oracle for
/// `HarvestStaging::{traditional,vg,hlod}_gens` (M3-α T8, design §3.1) — it
/// deliberately does NOT reuse any of `harvest_cell`'s own machinery, so a
/// bug in the production alignment logic cannot cancel out against the same
/// bug here.
fn expected_gens(cell: &SpatialCell, handles: &[Handle], tokens: &[u32], base: u32) -> Vec<u32> {
    tokens
        .iter()
        .map(|&t| {
            let row = t - base;
            let h = handles
                .iter()
                .find(|h| cell.row_of(**h) == Some(row))
                .unwrap_or_else(|| panic!("no live handle in `handles` currently occupies row {row}"));
            h.generation()
        })
        .collect()
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

/// M3-α T8 review defect 1 fix: the ORIGINAL plain-path alignment test used
/// only fresh (never-freed) handles, so every live harvested row carried
/// registry generation 1 — the reviewer-verified mutant `dest_gens.push(1)`
/// (a hardcoded constant standing in for the real `regs[col0[local_row]]`
/// read) passed the entire 11-test suite under that fixture. This version
/// uses [`gen_diverse_boxed_cell`], whose live rows carry genuinely
/// different generations via free+realloc churn, so a constant-push mutant
/// is immediately distinguishable from the real per-row read. Values are
/// still verified via [`expected_gens`], which recomputes purely from
/// `SpatialCell::row_of`/`Handle::generation()` — public API, never
/// `harvest_cell`'s own crate-private `slot_column` binding.
///
/// **r2 fix (M3 regression):** the first rework's `gen_diverse_boxed_cell`
/// did enough churn that its swaps canceled out, leaving every live row at
/// `slot == row` (identity `col0`) — under an identity mapping, mutant M3
/// (`regs[local_row]`, dropping the `col0` indirection) is INDISTINGUISHABLE
/// from the correct `regs[col0[local_row]]` read, so it silently survived.
/// This version's guard block below is SELF-VERIFYING: it asserts, via
/// public API only (`SpatialCell::row_of`, `Handle::index`,
/// `registry().generations()`), that the fixture's `col0` is genuinely
/// non-identity AND that at least one row's naive (`regs[row]`, what M3
/// computes) and correct (`regs[col0[row]]`, via the live handle's own
/// generation) reads genuinely differ — if a future change to
/// `gen_diverse_boxed_cell`'s churn recipe ever re-cancels this property,
/// THIS test fails loudly here instead of silently losing M3 coverage again.
#[test]
fn harvest_gens_column_matches_expected_generation_plain_path() {
    let ctx = test_context();
    let mut store = SceneGpuStore::new(&ctx, scene_cfg());

    let (cell_a, all_a) = gen_diverse_boxed_cell(0.0);
    let (cell_b, all_b) = gen_diverse_boxed_cell(1000.0);
    let live_a: Vec<Handle> = all_a.iter().copied().filter(|h| cell_a.row_of(*h).is_some()).collect();
    let live_b: Vec<Handle> = all_b.iter().copied().filter(|h| cell_b.row_of(*h).is_some()).collect();
    assert_eq!(live_a.len(), 4, "gen_diverse_boxed_cell always nets out to 4 live rows");
    assert_eq!(live_b.len(), 4);

    // Guard the guard: the fixture must have genuine gen diversity, not
    // coincidentally regress to a uniform gen-1 baseline (review defect 1) —
    // a fixture that silently lost its diversity would make every assertion
    // below pass just as vacuously as the original.
    let distinct_a: HashSet<u32> = live_a.iter().map(|h| h.generation()).collect();
    let distinct_b: HashSet<u32> = live_b.iter().map(|h| h.generation()).collect();
    assert!(distinct_a.len() > 1, "cell A fixture must carry genuinely different generations across live rows");
    assert!(distinct_b.len() > 1, "cell B fixture must carry genuinely different generations across live rows");

    // r2 self-verifying guard: non-identity col0 (via public row_of/index)
    // AND a genuine naive-vs-correct gen mismatch on at least one row, for
    // BOTH cells. `naive_gen(row) = regs[row]` is exactly what mutant M3
    // computes (treating the row index AS a slot index); `correct_gen(row)
    // = live_handle_at(row).generation()` is what the real `col0` indirection
    // reads. If these never differ, M3 is invisible to this test.
    for (cell, live, label) in [(&cell_a, &live_a, "A"), (&cell_b, &live_b, "B")] {
        let regs = cell.storage().registry().generations();
        let non_identity = live.iter().any(|h| cell.row_of(*h) != Some(h.index()));
        assert!(non_identity, "cell {label}: fixture must have at least one row whose slot != its row index");
        let naive_mismatch = live.iter().any(|h| {
            let row = cell.row_of(*h).unwrap();
            regs[row as usize] != h.generation()
        });
        assert!(
            naive_mismatch,
            "cell {label}: fixture must have at least one row where regs[row] (M3's naive read) != \
             regs[col0[row]] (the correct read, == the live handle's own generation) — otherwise M3 \
             is invisible to this test"
        );
    }

    let id_a = store.register_cell(cell_a.storage(), 0).unwrap();
    let id_b = store.register_cell(cell_b.storage(), 0).unwrap();
    let base_a = store.row_region_base(id_a);
    let base_b = store.row_region_base(id_b);

    let mut frames = FrameDriver::new();
    let h = frames.begin().end().end();
    let pipeline = HarvestPipeline::new();
    let mut pad = Scratchpad::new();
    let mut staging = HarvestStaging::new();

    // Broad views: every live box in both cells hits (100% >= 25% -> plain
    // path both times). Box positions always land in `x_offset .. x_offset+6`.
    let view_a = View::Aabb(Aabb { min: [-1.0, 0.0, 0.0], max: [7.0, 1.0, 1.0] });
    let n_a =
        pipeline.harvest_cell(&cell_a, base_a, MeshClass::Traditional, &view_a, &mut pad, &mut staging, &h);
    let view_b = View::Aabb(Aabb { min: [999.0, 0.0, 0.0], max: [1007.0, 1.0, 1.0] });
    let n_b =
        pipeline.harvest_cell(&cell_b, base_b, MeshClass::VirtualGeometry, &view_b, &mut pad, &mut staging, &h);
    assert_eq!(n_a, 4);
    assert_eq!(n_b, 4);
    assert_eq!(staging.stats.dei_compacted_runs, 0, "both runs are well above the 25% DEI threshold");

    assert_eq!(staging.traditional.len(), staging.traditional_gens.len(), "traditional/gens stay aligned");
    assert_eq!(staging.vg.len(), staging.vg_gens.len(), "vg/gens stay aligned");
    assert_eq!(staging.hlod.len(), staging.hlod_gens.len(), "hlod/gens stay aligned (both empty)");
    assert!(staging.hlod.is_empty(), "nothing harvested as HlodProxy");

    assert_eq!(
        expected_gens(&cell_a, &live_a, &staging.traditional, base_a),
        staging.traditional_gens,
        "traditional_gens matches the independently recomputed expected generation, per token, \
         with genuine per-row diversity — kills a constant-push mutant (review M1)"
    );
    assert_eq!(
        expected_gens(&cell_b, &live_b, &staging.vg, base_b),
        staging.vg_gens,
        "vg_gens matches the independently recomputed expected generation, per token, \
         with genuine per-row diversity — kills a constant-push mutant (review M1)"
    );
}

/// M3-α T8 review defect 2 fix: the ORIGINAL DEI alignment test only ever
/// exercised a fresh, identity-mapped cell (`col0[row] == row`, every gen 1)
/// with exactly ONE DEI-compacted cell per staging buffer. Under that
/// fixture, reviewer-verified mutants M2 (DEI: `regs[ri as usize]`, dropping
/// the `col0` slot-column indirection) and M4 (DEI: iterating the FULL
/// `staging.remap` instead of only the `remap_start..` segment THIS cell's
/// `compress_tokens` call appended) both survived — M2 because `ri == col0
/// [ri]` when identity-mapped, so skipping the indirection changes nothing;
/// M4 because a single DEI cell's `remap_start` is always 0, so ignoring the
/// segmentation changes nothing either.
///
/// This version harvests TWO DEI-triggering cells into ONE shared
/// `HarvestStaging` (pins `remap_start` segmentation — M4's exact blind
/// spot: the SECOND cell's `remap_start` is nonzero), each built via
/// [`scrambled_dei_cell`] so its single hit row's slot genuinely diverges
/// from its row and its generation genuinely diverges from the gen-1
/// baseline (M2/M1's exact blind spot). Expected values are recomputed via
/// `SpatialCell::row_of`/`Handle::generation()` ALONE — never through
/// `staging.remap` — the exact same-buggy-path trap the task brief flagged
/// (the review's minor defect: deriving an expectation through the very
/// structure under test cannot distinguish a bug in that structure from a
/// bug-free one).
#[test]
fn harvest_gens_column_dei_path_scrambled_slots_and_two_cell_segmentation() {
    let ctx = test_context();
    let mut store = SceneGpuStore::new(&ctx, scene_cfg());

    let (cell_a, hit_a, hit_x_a) = scrambled_dei_cell(0.0);
    let (cell_b, hit_b, hit_x_b) = scrambled_dei_cell(10_000.0);
    assert_ne!(
        hit_a.index(),
        cell_a.row_of(hit_a).unwrap(),
        "cell A's hit row must NOT equal its slot (non-identity col0) — the M2 blind spot"
    );
    assert_ne!(
        hit_b.index(),
        cell_b.row_of(hit_b).unwrap(),
        "cell B's hit row must NOT equal its slot (non-identity col0) — the M2 blind spot"
    );
    assert_ne!(hit_a.generation(), 1, "cell A's hit gen must diverge from the gen-1 baseline — the M1 blind spot");
    assert_ne!(hit_b.generation(), 1, "cell B's hit gen must diverge from the gen-1 baseline — the M1 blind spot");

    let id_a = store.register_cell(cell_a.storage(), 0).unwrap();
    let id_b = store.register_cell(cell_b.storage(), 0).unwrap();
    let base_a = store.row_region_base(id_a);
    let base_b = store.row_region_base(id_b);

    let mut frames = FrameDriver::new();
    let h = frames.begin().end().end();
    let pipeline = HarvestPipeline::new();
    let mut pad = Scratchpad::new();
    let mut staging = HarvestStaging::new();

    let view_a = View::Aabb(Aabb { min: [hit_x_a + 0.25, 0.0, 0.0], max: [hit_x_a + 0.75, 1.0, 1.0] });
    let n_a =
        pipeline.harvest_cell(&cell_a, base_a, MeshClass::Traditional, &view_a, &mut pad, &mut staging, &h);
    assert_eq!(n_a, 1, "cell A: exactly the scrambled hit row (1/6 rows)");
    assert_eq!(staging.stats.dei_compacted_runs, 1, "cell A took the DEI path — remap_start was 0 for this call");

    let view_b = View::Aabb(Aabb { min: [hit_x_b + 0.25, 0.0, 0.0], max: [hit_x_b + 0.75, 1.0, 1.0] });
    let n_b =
        pipeline.harvest_cell(&cell_b, base_b, MeshClass::Traditional, &view_b, &mut pad, &mut staging, &h);
    assert_eq!(n_b, 1, "cell B: exactly the scrambled hit row (1/6 rows)");
    assert_eq!(
        staging.stats.dei_compacted_runs, 2,
        "cell B ALSO took the DEI path — its remap_start was 1, not 0 (the M4 blind spot)"
    );

    // Segmentation: exactly one remap entry contributed per cell.
    assert_eq!(staging.remap.len(), 2, "two DEI cells -> two remap entries total, no cross-contamination");
    assert_eq!(staging.traditional.len(), 2);
    assert_eq!(
        staging.traditional_gens.len(),
        2,
        "dest/gens length parity across BOTH DEI cells — kills M4 (iterating the full remap on cell B's \
         call would re-walk cell A's already-counted entry too, growing gens past the token count)"
    );

    // Expected values, independently recomputed via `row_of`/`generation()`
    // ALONE — NEVER via `staging.remap` (the flagged same-path trap).
    let row_a = cell_a.row_of(hit_a).unwrap();
    let row_b = cell_b.row_of(hit_b).unwrap();
    assert_eq!(staging.traditional[0], base_a + row_a, "token 0 is cell A's hit row");
    assert_eq!(staging.traditional[1], base_b + row_b, "token 1 is cell B's hit row");
    assert_eq!(
        staging.traditional_gens[0],
        hit_a.generation(),
        "gens[0] must be cell A's hit handle's generation, read via col0[{row_a}] == slot {} (NOT regs[{row_a}], which would read slot {row_a}'s OWN generation — a different, still-live handle)",
        hit_a.index()
    );
    assert_eq!(
        staging.traditional_gens[1],
        hit_b.generation(),
        "gens[1] must be cell B's hit handle's generation, read via col0[{row_b}] == slot {} (NOT regs[{row_b}])",
        hit_b.index()
    );
}

/// Test 2's CPU-side data path (design §3.1): a harvested run's gens column
/// is a point-in-time snapshot — it goes stale the moment its row's occupant
/// is freed and a boundary bumps that slot's generation. A consumer holding
/// an OLD harvest result can detect this by comparing its snapshot gen
/// against the LIVE `registry().generations()` for that slot (the M3-β cull
/// shader's job, GPU-side; this proves the CPU-side data it will read is
/// correct). A fresh re-harvest after the boundary reflects live state and
/// matches.
///
/// M3-α T8 review defect 3 fix: uses [`gen_diverse_boxed_cell`] (genuinely
/// different generations per live row, not a uniform gen-1 baseline) so a
/// position mix-up is actually detectable, and additionally asserts every
/// NON-freed OLD entry still matches the LIVE registry (not merely skipping
/// past them) — a regression that marked the WHOLE old run stale, not only
/// the freed slot, would have passed the original version of this test.
#[test]
fn harvest_gens_go_stale_after_free_deferred_and_boundary() {
    let ctx = test_context();
    let mut store = SceneGpuStore::new(&ctx, scene_cfg());

    let (mut cell, all) = gen_diverse_boxed_cell(0.0);
    let live: Vec<Handle> = all.iter().copied().filter(|h| cell.row_of(*h).is_some()).collect();
    assert_eq!(live.len(), 4);
    let distinct: HashSet<u32> = live.iter().map(|h| h.generation()).collect();
    assert!(distinct.len() > 1, "fixture must carry genuine gen diversity so position is pinned");

    // Every live handle's row AT OLD-HARVEST TIME, captured now (before the
    // free below) — `row_of` only resolves LIVE handles, and this mapping is
    // what lets the OLD-run checks below address a specific handle's entry
    // regardless of how compaction reshuffles rows afterwards.
    let pre_free_rows: HashMap<Handle, u32> = live.iter().map(|&h| (h, cell.row_of(h).unwrap())).collect();

    let id = store.register_cell(cell.storage(), 0).unwrap();
    let base = store.row_region_base(id);

    let mut frames = FrameDriver::new();
    let pipeline = HarvestPipeline::new();
    let mut pad = Scratchpad::new();
    let mut staging = HarvestStaging::new();
    let view = View::Aabb(Aabb { min: [-1.0, 0.0, 0.0], max: [7.0, 1.0, 1.0] });

    // Frame 1: harvest the OLD run (all 4 live rows, genuinely mixed gens),
    // then close the boundary with nothing pending — required to legally
    // reach a fresh SimulateA for frame 2 below (the phase machine's chain).
    let h1 = frames.begin().end().end();
    let n1 = pipeline.harvest_cell(&cell, base, MeshClass::Traditional, &view, &mut pad, &mut staging, &h1);
    assert_eq!(n1, 4);
    let old_tokens = staging.traditional.clone();
    let old_gens = staging.traditional_gens.clone();
    assert!(
        old_gens.iter().collect::<HashSet<_>>().len() > 1,
        "the OLD run itself must be non-uniform, or the freed-vs-surviving comparisons below can't pin position"
    );
    {
        let mut slots = [CellSlot { id, cell: cell.storage_mut() }];
        h1.end().run(&mut store, &mut slots);
    }

    // Free the handle with the STRICTLY-HIGHEST live generation, via the
    // deferred path, force its serial complete, and drive the boundary —
    // retirement commits CPU-side and bumps the registry generation for that
    // slot (§5 flow step 3). r2 fix (de-coincidence): freeing the max-gen
    // handle is provably collision-free — its bumped value (max+1) exceeds
    // every OTHER live handle's generation (all <= max by construction), so
    // it can never coincide with a survivor's gen. The original version
    // freed `live[0]` arbitrarily, and the reviewer found an empirical
    // coincidence (the freed slot's bumped gen happened to equal another
    // live slot's gen), which masked mutant M3 in this test.
    let freed = *live.iter().max_by_key(|h| h.generation()).unwrap();
    let survivors: Vec<Handle> = live.iter().copied().filter(|&h| h != freed).collect();
    let sim2 = frames.begin();
    let serial = store.tracker().next_serial();
    assert!(store.free_deferred(id, cell.storage_mut(), freed, serial, &sim2));
    store.tracker().force_complete(serial);
    let h2 = sim2.end().end();
    {
        let mut slots = [CellSlot { id, cell: cell.storage_mut() }];
        h2.end().run(&mut store, &mut slots);
    }

    let live_gens = cell.storage().registry().generations().to_vec();
    let bumped = live_gens[freed.index() as usize];
    assert_ne!(bumped, freed.generation(), "free_deferred + boundary bumped the freed slot's generation");

    // r2 self-verifying de-coincidence guard: the bumped generation must be
    // unique among the surviving live handles' CURRENT generations — proven
    // by construction (freed had the max pre-free generation, so bumped =
    // max+1 exceeds every survivor's unchanged generation), but asserted
    // here so a future change to the "pick the freed handle" strategy fails
    // loudly instead of silently reintroducing the coincidence.
    assert!(
        survivors.iter().all(|h| h.generation() != bumped),
        "the freed slot's post-bump generation ({bumped}) must be unique among surviving live generations \
         — otherwise M3 (regs[local_row] skipping col0) can produce a value-identical wrong answer"
    );

    // Walk EVERY live handle's OLD-run entry (found via the pre-free row
    // map, never via any post-boundary row): the freed one must no longer
    // match LIVE (the staleness the M3-β cull shader is meant to catch);
    // every SURVIVING one must still match LIVE (review defect 3 — the
    // original test only ever checked the freed slot, so a regression that
    // marked the WHOLE old run stale would have passed).
    for &handle in &live {
        let row = pre_free_rows[&handle];
        let old_idx = old_tokens
            .iter()
            .position(|&t| t == base + row)
            .unwrap_or_else(|| panic!("row {row} (handle {handle:?}) was not in the OLD run"));
        assert_eq!(
            old_gens[old_idx], handle.generation(),
            "OLD run's gen for {handle:?} must equal its own generation at harvest time"
        );
        if handle == freed {
            assert_ne!(
                old_gens[old_idx],
                live_gens[handle.index() as usize],
                "OLD run's gen for the FREED row must no longer match LIVE after the free+boundary"
            );
        } else {
            assert_eq!(
                old_gens[old_idx],
                live_gens[handle.index() as usize],
                "OLD run's gen for a NON-freed row must STILL match LIVE (untouched slot) — \
                 review defect 3: a regression that staled the whole run, not just the freed slot"
            );
        }
    }

    // NEW run: re-harvest after the boundary. Only 3 rows remain (one live
    // row swaps into the freed row by compaction); every gen in this fresh
    // run must match the live registry, independently recomputed via
    // `row_of` over the surviving handles.
    let h3 = frames.begin().end().end();
    staging.clear();
    let n3 = pipeline.harvest_cell(&cell, base, MeshClass::Traditional, &view, &mut pad, &mut staging, &h3);
    assert_eq!(n3, 3, "one row fewer after the free");
    assert_eq!(staging.traditional.len(), staging.traditional_gens.len());
    assert_eq!(
        expected_gens(&cell, &survivors, &staging.traditional, base),
        staging.traditional_gens,
        "NEW run's gens match the live registry for every surviving row"
    );
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
    // M3-α T8: the gens columns are persistent staging arrays too — their
    // capacity must survive warm-up exactly like their token counterparts.
    let cap_trad_gens = staging.traditional_gens.capacity();
    let cap_vg_gens = staging.vg_gens.capacity();
    let cap_hlod_gens = staging.hlod_gens.capacity();

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
    assert_eq!(staging.traditional_gens.capacity(), cap_trad_gens, "traditional_gens capacity unchanged");
    assert_eq!(staging.vg_gens.capacity(), cap_vg_gens, "vg_gens capacity unchanged");
    assert_eq!(staging.hlod_gens.capacity(), cap_hlod_gens, "hlod_gens capacity unchanged");
}

#[test]
fn harvest_dei_branch_makes_zero_new_allocations_after_warmup() {
    // Review fold-in (T6): the zero-alloc warm-up test above only exercises
    // the plain (>= 25% hit) path — the DEI dense-compaction branch's
    // no-realloc invariant was unasserted. Mirror it with a 12.5%-hit cell
    // so `harvest_cell` takes the `compress_tokens` branch both runs.
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

    // Same cell + same view both runs: box i = [i, i+1); query [10.5, 17.5]
    // hits i in {10..=17} -> 8/64 = 12.5% < 25% -> DEI path both times.
    let view = View::Aabb(Aabb { min: [10.5, 0.0, 0.0], max: [17.5, 1.0, 1.0] });
    let n1 = pipeline.harvest_cell(&cell, base, MeshClass::Traditional, &view, &mut pad, &mut staging, &h);
    assert_eq!(n1, 8, "12.5% hit ratio");
    assert_eq!(staging.stats.dei_compacted_runs, 1, "run 1 takes the DEI branch");

    let pad_u32_after1 = pad.buf_len_u32();
    let pad_u64_after1 = pad.buf_len_u64();
    let cap_trad = staging.traditional.capacity();
    let cap_vg = staging.vg.capacity();
    let cap_hlod = staging.hlod.capacity();
    let cap_remap = staging.remap.capacity();
    // M3-α T8: the gens columns are persistent staging arrays too — their
    // capacity must survive warm-up exactly like their token counterparts,
    // including on the DEI branch (the remap-segment loop grows
    // `traditional_gens` by exactly `dest`'s growth, so it warms up
    // identically).
    let cap_trad_gens = staging.traditional_gens.capacity();
    let cap_vg_gens = staging.vg_gens.capacity();
    let cap_hlod_gens = staging.hlod_gens.capacity();

    // Clear WITHOUT freeing (S8.1) — a fresh `HarvestStaging::new()` here
    // would defeat the entire point of this test.
    staging.clear();

    let n2 = pipeline.harvest_cell(&cell, base, MeshClass::Traditional, &view, &mut pad, &mut staging, &h);
    assert_eq!(n2, 8, "same cell + view -> identical hit count on the second run");
    assert_eq!(staging.stats.dei_compacted_runs, 1, "run 2 also takes the DEI branch");

    assert_eq!(pad.buf_len_u32(), pad_u32_after1, "scratch u32 buffer size unchanged after warmup");
    assert_eq!(pad.buf_len_u64(), pad_u64_after1, "scratch u64 buffer size unchanged after warmup");
    assert_eq!(staging.traditional.capacity(), cap_trad, "traditional (dense) capacity unchanged across DEI runs");
    assert_eq!(staging.vg.capacity(), cap_vg, "vg capacity unchanged");
    assert_eq!(staging.hlod.capacity(), cap_hlod, "hlod capacity unchanged");
    assert_eq!(staging.remap.capacity(), cap_remap, "remap capacity unchanged across DEI runs (no realloc)");
    assert_eq!(
        staging.traditional_gens.capacity(),
        cap_trad_gens,
        "traditional_gens capacity unchanged across DEI runs"
    );
    assert_eq!(staging.vg_gens.capacity(), cap_vg_gens, "vg_gens capacity unchanged");
    assert_eq!(staging.hlod_gens.capacity(), cap_hlod_gens, "hlod_gens capacity unchanged");
}

/// Test 10 (spec §9.2/§9.2.1, C4 2.0 ms revocation budget): lease timeout,
/// revocation, and the stale-validation lane. Pure CPU-side — no GPU context
/// needed, this exercises `LeaseMask`/`HarvestLease`/`LivenessSnapshot`
/// against a plain `SpatialCell`.
#[test]
fn lease_timeout_revocation_and_stale_lane_revalidation() {
    // 4 live rows, densely positional: box i = [i, i+1).
    let mut cell = SpatialCell::new(64).unwrap();
    let handles: Vec<_> = (0..4u32)
        .map(|i| {
            let x = i as f32;
            cell.alloc(Aabb { min: [x, 0.0, 0.0], max: [x + 1.0, 1.0, 1.0] }).unwrap()
        })
        .collect();
    let len = cell.rows_in_use() as usize;
    assert_eq!(len, 4);

    // Capture-time snapshot (pinned) — the lease holder's view (§9.2.1
    // double-buffered state mask).
    let snap = LivenessSnapshot::capture(cell.storage().liveness(), len as u32);

    let mask = LeaseMask::new();
    let pipeline = HarvestPipeline::new();

    // Acquire a lease at t=0.0 against that snapshot, attributed to
    // "test10-holder" (Test 10's revocation-attribution client tag).
    let lease = pipeline
        .acquire_lease(&mask, 0.0, "test10-holder")
        .expect("lease pool has room");
    assert!(mask.any_held(), "the acquired slot is held");
    assert!(!lease.revocation.is_revoked(), "fresh lease is not revoked");

    // Query via query_aabb_in against the pinned snapshot: all 4 rows hit.
    let query = Aabb { min: [-1.0, 0.0, 0.0], max: [10.0, 1.0, 1.0] };
    let mut run = vec![0u32; len];
    let n = cell.query_aabb_in(&query, snap.words(), &mut run);
    assert_eq!(n, 4, "snapshot query: all 4 live rows hit");
    assert_eq!(run, vec![0, 1, 2, 3]);

    // Free row 2 AFTER capture. The live mask moves on immediately, but the
    // pinned snapshot must not.
    cell.free(handles[2]);
    assert!(!cell.storage().liveness().is_live(2), "live mask advanced past the free");
    assert!(snap.is_live(2), "pinned snapshot unaffected by the free (LivenessSnapshot semantics)");

    // Re-running the SAME query against the SAME snapshot after the
    // mutation must reproduce the identical run — the snapshot, not the
    // live mask, backs the query.
    let mut run_after_free = vec![0u32; len];
    let n_after_free = cell.query_aabb_in(&query, snap.words(), &mut run_after_free);
    assert_eq!(n_after_free, 4, "pinned-snapshot query result unchanged by the free");
    assert_eq!(run_after_free, run, "identical run before/after the mutation, via the pinned snapshot");

    // §9.2.1 isolation check: at t=2.5ms with a 2.0ms budget, the lease
    // (held since t=0.0) is overdue -> revoked.
    let revoked = pipeline.revoke_overdue(&[&lease], 2.5, 2.0);
    assert_eq!(revoked, 1, "one overdue lease revoked");
    assert!(lease.revocation.is_revoked(), "revocation flag observably set");

    // The stale-validation lane then reconciles the (still snapshot-derived)
    // run against LIVE liveness: row 2 died, so it's stripped to NULL_ROW and
    // the surviving count drops to 3.
    let mut reconciled = run_after_free.clone();
    let surviving = revalidate_run(&cell, &mut reconciled);
    assert_eq!(surviving, 3, "row 2 died since capture -> 3 survivors");
    assert_eq!(reconciled, vec![0, 1, NULL_ROW, 3], "freed row's slot is now NULL_ROW");

    // Dropping the RAII guard releases the slot -> compaction may proceed.
    drop(lease);
    assert!(!mask.any_held(), "no leases held after the guard drops; compaction may proceed");

    // Second sweep: a freshly acquired, NOT-overdue lease -> 0 revocations.
    let lease2 = pipeline
        .acquire_lease(&mask, 10.0, "test10-holder-2")
        .expect("slot available again");
    let revoked2 = pipeline.revoke_overdue(&[&lease2], 10.5, 2.0);
    assert_eq!(revoked2, 0, "0.5ms held < 2.0ms budget -> nothing overdue");
    assert!(!lease2.revocation.is_revoked());
}

/// Test 10 (pool exhaustion half): the 65th concurrent lease acquire on a
/// 64-slot pool returns `None`. Spec §9.2's blocking-retry loop around
/// exhaustion is the World driver's scope — `HarvestPipeline::acquire_lease`
/// itself never blocks, it just reports exhaustion immediately.
#[test]
fn lease_pool_exhaustion_returns_none_then_recovers() {
    let mask = LeaseMask::new();
    let pipeline = HarvestPipeline::new();

    let mut held = Vec::new();
    for _ in 0..64 {
        held.push(
            pipeline
                .acquire_lease(&mask, 0.0, "exhaustion-test")
                .expect("64 slots available"),
        );
    }
    assert!(
        pipeline.acquire_lease(&mask, 0.0, "exhaustion-test").is_none(),
        "65th acquire fails on a full 64-slot pool"
    );

    drop(held);
    assert!(
        pipeline.acquire_lease(&mask, 0.0, "exhaustion-test").is_some(),
        "a slot frees up after all leases release"
    );
}

/// M2b-b T9 (spec §8.4): `harvest_views` scans `views × cells`, routing each
/// view's hits into its OWN `(Scratchpad, HarvestStaging)` pair. This test
/// checks that the batched multi-view entry point produces byte-identical
/// results to manually driving the same `views × cells` loop through
/// `harvest_cell` one view at a time into fresh buffers.
#[test]
fn multi_view_harvest_matches_per_view_sequential() {
    let ctx = test_context();
    let mut store = SceneGpuStore::new(&ctx, scene_cfg());

    // Two cells, disjoint regions, both registered under region class 0 (the
    // only class `scene_cfg` configures).
    let cell_a = boxed_cell(64, 8, 0.0);
    let cell_b = boxed_cell(64, 8, 100.0);
    let id_a = store.register_cell(cell_a.storage(), 0).unwrap();
    let id_b = store.register_cell(cell_b.storage(), 0).unwrap();
    let base_a = store.row_region_base(id_a);
    let base_b = store.row_region_base(id_b);
    assert_ne!(base_a, base_b, "disjoint regions");

    let mut frames = FrameDriver::new();
    let h = frames.begin().end().end();
    let pipeline = HarvestPipeline::new();

    let cells: Vec<(&SpatialCell, u32, MeshClass)> = vec![
        (&cell_a, base_a, MeshClass::Traditional),
        (&cell_b, base_b, MeshClass::Traditional),
    ];

    // box i = [x_offset + i, x_offset + i + 1). View 0 ([1.5, 4.5]) hits
    // cell_a's local rows 1..=4 (4/8 = 50%) and nothing in cell_b (offset
    // 100 — no overlap). View 1 ([101.5, 104.5]) is the mirror: hits cell_b's
    // local rows 1..=4 and nothing in cell_a. Two views, disjoint hit
    // subsets, exactly as the brief calls for.
    let views = vec![
        View::Aabb(Aabb { min: [1.5, 0.0, 0.0], max: [4.5, 1.0, 1.0] }),
        View::Aabb(Aabb { min: [101.5, 0.0, 0.0], max: [104.5, 1.0, 1.0] }),
    ];

    let mut pads: Vec<Scratchpad> = (0..views.len()).map(|_| Scratchpad::new()).collect();
    let mut stagings: Vec<HarvestStaging> = (0..views.len()).map(|_| HarvestStaging::new()).collect();
    pipeline.harvest_views(&cells, &views, &mut pads, &mut stagings, &h);

    // Manually reproduce the same `views × cells` scan sequentially, into
    // fresh per-view buffers, and compare.
    let mut expected_pads: Vec<Scratchpad> = (0..views.len()).map(|_| Scratchpad::new()).collect();
    let mut expected_stagings: Vec<HarvestStaging> =
        (0..views.len()).map(|_| HarvestStaging::new()).collect();
    for v in 0..views.len() {
        for &(cell, base, class) in &cells {
            pipeline.harvest_cell(
                cell,
                base,
                class,
                &views[v],
                &mut expected_pads[v],
                &mut expected_stagings[v],
                &h,
            );
        }
    }

    for v in 0..views.len() {
        assert_eq!(
            stagings[v].traditional, expected_stagings[v].traditional,
            "view {v}: traditional mismatch"
        );
        assert_eq!(stagings[v].vg, expected_stagings[v].vg, "view {v}: vg mismatch");
        assert_eq!(stagings[v].hlod, expected_stagings[v].hlod, "view {v}: hlod mismatch");
        assert_eq!(stagings[v].remap, expected_stagings[v].remap, "view {v}: remap mismatch");
        assert_eq!(
            stagings[v].stats.cells, expected_stagings[v].stats.cells,
            "view {v}: stats.cells mismatch"
        );
        assert_eq!(
            stagings[v].stats.tokens_valid, expected_stagings[v].stats.tokens_valid,
            "view {v}: stats.tokens_valid mismatch"
        );
        assert_eq!(
            stagings[v].stats.tokens_total, expected_stagings[v].stats.tokens_total,
            "view {v}: stats.tokens_total mismatch"
        );
        assert_eq!(
            stagings[v].stats.dei_compacted_runs, expected_stagings[v].stats.dei_compacted_runs,
            "view {v}: stats.dei_compacted_runs mismatch"
        );
    }

    // Confirm the two views actually hit disjoint subsets (view 0 -> cell_a
    // only, view 1 -> cell_b only) rather than both happening to see nothing.
    assert_eq!(stagings[0].traditional, vec![base_a + 1, base_a + 2, base_a + 3, base_a + 4]);
    assert_eq!(stagings[1].traditional, vec![base_b + 1, base_b + 2, base_b + 3, base_b + 4]);
}

/// M2b-b T9 concurrency smoke (spec §8.4's safety claim): `harvest_cell`
/// takes `&self` (the pipeline carries no state) and only `&SpatialCell`
/// (read-only — every cell mutation path requires `&mut SpatialCell`, not
/// reachable from here), so queries over different views may run on separate
/// threads, each with its own scratch/staging pair, over the SAME cell
/// references without synchronization. This drives exactly that: two
/// `std::thread::scope` threads, each owning a private `Scratchpad` +
/// `HarvestStaging`, both reading the same `&SpatialCell` refs — and checks
/// the result is identical to running the two views sequentially.
#[test]
fn concurrent_views_match_sequential() {
    let ctx = test_context();
    let mut store = SceneGpuStore::new(&ctx, scene_cfg());

    let cell_a = boxed_cell(64, 8, 0.0);
    let cell_b = boxed_cell(64, 8, 100.0);
    let id_a = store.register_cell(cell_a.storage(), 0).unwrap();
    let id_b = store.register_cell(cell_b.storage(), 0).unwrap();
    let base_a = store.row_region_base(id_a);
    let base_b = store.row_region_base(id_b);

    let mut frames = FrameDriver::new();
    let h = frames.begin().end().end();
    let pipeline = HarvestPipeline::new();

    let cells: Vec<(&SpatialCell, u32, MeshClass)> = vec![
        (&cell_a, base_a, MeshClass::Traditional),
        (&cell_b, base_b, MeshClass::Traditional),
    ];
    let view0 = View::Aabb(Aabb { min: [1.5, 0.0, 0.0], max: [4.5, 1.0, 1.0] });
    let view1 = View::Aabb(Aabb { min: [101.5, 0.0, 0.0], max: [104.5, 1.0, 1.0] });

    // Sequential baseline: same two views, driven one after another into
    // their own fresh buffers.
    let mut seq_pad0 = Scratchpad::new();
    let mut seq_staging0 = HarvestStaging::new();
    for &(cell, base, class) in &cells {
        pipeline.harvest_cell(cell, base, class, &view0, &mut seq_pad0, &mut seq_staging0, &h);
    }
    let mut seq_pad1 = Scratchpad::new();
    let mut seq_staging1 = HarvestStaging::new();
    for &(cell, base, class) in &cells {
        pipeline.harvest_cell(cell, base, class, &view1, &mut seq_pad1, &mut seq_staging1, &h);
    }

    // Concurrent run. `HarvestPhase` is `pub struct HarvestPhase(());` — a
    // ZST with no interior state and no explicit `Send`/`Sync` opt-outs, so
    // it is auto-`Sync` (and `&HarvestPhase` is therefore `Send`); likewise
    // `HarvestPipeline(())` and, per `Page`'s `unsafe impl Send`/`Sync`
    // (page.rs), `SpatialCell`/`CellStorage` are `Sync` all the way down. That
    // means a single witness/pipeline/cell set can simply be shared by
    // reference across the two scope threads below — no per-thread
    // `FrameDriver` needed (that fallback would only be required if
    // `HarvestPhase` were NOT `Sync`).
    let (par_staging0, par_staging1) = std::thread::scope(|scope| {
        let cells_ref = &cells;
        let pipeline_ref = &pipeline;
        let h_ref = &h;
        let view0_ref = &view0;
        let view1_ref = &view1;

        let t0 = scope.spawn(move || {
            let mut pad = Scratchpad::new();
            let mut staging = HarvestStaging::new();
            for &(cell, base, class) in cells_ref {
                pipeline_ref.harvest_cell(cell, base, class, view0_ref, &mut pad, &mut staging, h_ref);
            }
            staging
        });
        let t1 = scope.spawn(move || {
            let mut pad = Scratchpad::new();
            let mut staging = HarvestStaging::new();
            for &(cell, base, class) in cells_ref {
                pipeline_ref.harvest_cell(cell, base, class, view1_ref, &mut pad, &mut staging, h_ref);
            }
            staging
        });
        (t0.join().expect("thread 0 panicked"), t1.join().expect("thread 1 panicked"))
    });

    assert_eq!(par_staging0.traditional, seq_staging0.traditional, "view 0 traditional mismatch");
    assert_eq!(par_staging0.vg, seq_staging0.vg, "view 0 vg mismatch");
    assert_eq!(par_staging0.hlod, seq_staging0.hlod, "view 0 hlod mismatch");
    assert_eq!(par_staging0.remap, seq_staging0.remap, "view 0 remap mismatch");
    assert_eq!(par_staging0.stats.cells, seq_staging0.stats.cells, "view 0 stats.cells mismatch");
    assert_eq!(
        par_staging0.stats.tokens_valid, seq_staging0.stats.tokens_valid,
        "view 0 stats.tokens_valid mismatch"
    );
    assert_eq!(
        par_staging0.stats.tokens_total, seq_staging0.stats.tokens_total,
        "view 0 stats.tokens_total mismatch"
    );
    assert_eq!(
        par_staging0.stats.dei_compacted_runs, seq_staging0.stats.dei_compacted_runs,
        "view 0 stats.dei_compacted_runs mismatch"
    );

    assert_eq!(par_staging1.traditional, seq_staging1.traditional, "view 1 traditional mismatch");
    assert_eq!(par_staging1.vg, seq_staging1.vg, "view 1 vg mismatch");
    assert_eq!(par_staging1.hlod, seq_staging1.hlod, "view 1 hlod mismatch");
    assert_eq!(par_staging1.remap, seq_staging1.remap, "view 1 remap mismatch");
    assert_eq!(par_staging1.stats.cells, seq_staging1.stats.cells, "view 1 stats.cells mismatch");
    assert_eq!(
        par_staging1.stats.tokens_valid, seq_staging1.stats.tokens_valid,
        "view 1 stats.tokens_valid mismatch"
    );
    assert_eq!(
        par_staging1.stats.tokens_total, seq_staging1.stats.tokens_total,
        "view 1 stats.tokens_total mismatch"
    );
    assert_eq!(
        par_staging1.stats.dei_compacted_runs, seq_staging1.stats.dei_compacted_runs,
        "view 1 stats.dei_compacted_runs mismatch"
    );

    // Sanity: the two views really did hit disjoint subsets.
    assert_eq!(seq_staging0.traditional, vec![base_a + 1, base_a + 2, base_a + 3, base_a + 4]);
    assert_eq!(seq_staging1.traditional, vec![base_b + 1, base_b + 2, base_b + 3, base_b + 4]);
}
